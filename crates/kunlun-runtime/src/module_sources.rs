//! Root-scoped source fetching for the canonical native module resolver.
use crate::source_maps::{MAX_MAP_BYTES, SourceMaps};
use crate::{ModuleKind, ModuleResolver, ModuleUrl};
use base64::Engine;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use kunlun_jsc::ModuleLoader;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
const MAX_MODULES: usize = 1024;

/// One immutable artifact-root policy for a VM's module cache. Generated
/// sources are registered before installation; runtime built-ins keep their
/// existing host capability checks. Fetching never accesses the network.
pub struct ModuleSources {
    resolver: ModuleResolver,
    directory: Dir,
    generated: HashMap<String, String>,
    fetched: RefCell<HashMap<String, usize>>,
    maps: RefCell<SourceMaps>,
}

impl ModuleSources {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let resolver = ModuleResolver::new(root).map_err(|e| e.to_string())?;
        let directory = Dir::open_ambient_dir(resolver.root(), ambient_authority())
            .map_err(|e| format!("cannot open module root: {e}"))?;
        Ok(Self {
            resolver,
            directory,
            generated: HashMap::new(),
            fetched: RefCell::new(HashMap::new()),
            maps: RefCell::new(SourceMaps::default()),
        })
    }

    pub fn register_generated(
        &mut self,
        url: &str,
        source: impl Into<String>,
    ) -> Result<ModuleUrl, String> {
        let source = source.into();
        if self.generated.len() >= MAX_MODULES
            || self.generated.values().map(String::len).sum::<usize>() + source.len()
                > MAX_GRAPH_BYTES
        {
            return Err("generated sources exceed the per-VM module budget".to_owned());
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err("generated module exceeds the 8 MiB source limit".to_owned());
        }
        let url = self
            .resolver
            .register_generated(url)
            .map_err(|e| e.to_string())?;
        if self.generated.contains_key(url.as_str()) {
            return Err(format!("generated module {} is already registered", url));
        }
        self.generated.insert(url.as_str().to_owned(), source);
        Ok(url)
    }

    /// Registers a v3 map explicitly, including for generated modules. Relative
    /// original-source names are resolved against the module URL, without I/O.
    pub fn register_source_map(&mut self, module_url: &str, json: &str) -> Result<(), String> {
        let module = self
            .resolver
            .resolve_absolute_url(module_url)
            .map_err(|e| e.to_string())?;
        self.maps
            .get_mut()
            .insert(module.as_str(), module.as_url().clone(), json.as_bytes())
    }

    fn record_source_map(&self, module: &ModuleUrl, source: &str) {
        if self.maps.borrow().contains(module.as_str()) {
            return;
        }
        // Bundlers emit a trailing sourceMappingURL line. Only that trailer is
        // read, and map metadata never grants authority to fetch original code.
        let Some(last) = source.lines().rfind(|line| !line.trim().is_empty()) else {
            return;
        };
        let Some(reference) = last.trim().strip_prefix("//# sourceMappingURL=") else {
            return;
        };
        let reference = reference.trim();
        let loaded = (|| -> Result<(url::Url, Vec<u8>), String> {
            if let Some(data) = reference
                .strip_prefix("data:application/json;base64,")
                .or_else(|| reference.strip_prefix("data:application/json;charset=utf-8;base64,"))
            {
                if data.len() > MAX_MAP_BYTES * 4 / 3 + 4 {
                    return Err("inline source map too large".to_owned());
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| e.to_string())?;
                return Ok((module.as_url().clone(), bytes));
            }
            let absolute = module.as_url().join(reference).map_err(|e| e.to_string())?;
            let map_url = self
                .resolver
                .resolve_absolute_url(absolute.as_str())
                .map_err(|e| e.to_string())?;
            if map_url.kind() != ModuleKind::File {
                return Err("external source maps must be artifact files".to_owned());
            }
            Ok((
                map_url.as_url().clone(),
                self.read_file(&map_url, MAX_MAP_BYTES)?.into_bytes(),
            ))
        })();
        // Missing or malformed debug maps never mask the actual module result.
        // The original generated URL and engine stack are always retained.
        if let Ok((base, json)) = loaded {
            let _ = self.maps.borrow_mut().insert(module.as_str(), base, &json);
        }
    }

    fn read_file(&self, url: &ModuleUrl, limit: usize) -> Result<String, String> {
        let path = url.to_file_path().ok_or("module has no file path")?;
        let relative = path
            .strip_prefix(self.resolver.root())
            .map_err(|e| e.to_string())?;
        // Directory-relative open preserves containment even if a component is
        // swapped for a symlink after resolution. O_NONBLOCK avoids hanging if
        // the file is raced into a FIFO; metadata rejects all non-regular files.
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(libc::O_NONBLOCK);
        let file = self
            .directory
            .open_with(relative, &options)
            .map_err(|e| format!("cannot fetch {url}: {e}"))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err(format!("module {url} is not a regular file"));
        }
        let mut source = String::new();
        file.take(limit as u64 + 1)
            .read_to_string(&mut source)
            .map_err(|e| format!("cannot read UTF-8 module {url}: {e}"))?;
        if source.len() > limit {
            return Err(format!("source {url} exceeds the {limit} byte limit"));
        }
        Ok(source)
    }
}

impl ModuleLoader for ModuleSources {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        let url = match referrer {
            Some(referrer) => {
                let from = self
                    .resolver
                    .resolve_absolute_url(referrer)
                    .map_err(|e| e.to_string())?;
                if from.as_str() != referrer {
                    return Err(format!(
                        "module referrer {referrer:?} is no longer canonical"
                    ));
                }
                self.resolver.resolve(specifier, &from)
            }
            None if url::Url::parse(specifier).is_ok() => {
                self.resolver.resolve_absolute_url(specifier)
            }
            None => self.resolver.resolve_entry(specifier),
        }
        .map_err(|e| e.to_string())?;
        Ok(url.as_str().to_owned())
    }

    fn fetch(&self, canonical_url: &str) -> Result<String, String> {
        let url = self
            .resolver
            .resolve_absolute_url(canonical_url)
            .map_err(|e| e.to_string())?;
        if url.as_str() != canonical_url {
            return Err(format!(
                "module URL {canonical_url:?} changed identity before fetch"
            ));
        }
        // Bound the source work independently of the native graph cache.
        if self.fetched.borrow().len() >= MAX_MODULES
            && !self.fetched.borrow().contains_key(canonical_url)
        {
            return Err("module graph exceeds the 1024 module limit".to_owned());
        }
        let source = match url.kind() {
            ModuleKind::File => self.read_file(&url, MAX_SOURCE_BYTES)?,
            ModuleKind::Generated => self
                .generated
                .get(canonical_url)
                .ok_or_else(|| format!("no source registered for {url}"))?
                .clone(),
            ModuleKind::Builtin => {
                let descriptor = crate::BUILTIN_MODULES
                    .iter()
                    .find(|module| module.specifier == canonical_url)
                    .ok_or_else(|| format!("unknown built-in {url}"))?;
                // Capture the capability-gated bootstrap exports once, then
                // expose them through a genuine JSC module namespace.
                format!(
                    "const module = await kunlun.import({});\n{}",
                    serde_json::to_string(canonical_url).map_err(|e| e.to_string())?,
                    descriptor
                        .exports
                        .iter()
                        .map(|name| format!("export const {name} = module.{name};\n"))
                        .collect::<String>()
                )
            }
        };
        let mut fetched = self.fetched.borrow_mut();
        let total: usize = fetched.values().sum();
        let previous = fetched.get(canonical_url).copied().unwrap_or(0);
        if total - previous + source.len() > MAX_GRAPH_BYTES {
            return Err("module graph exceeds the 64 MiB source limit".to_owned());
        }
        fetched.insert(canonical_url.to_owned(), source.len());
        drop(fetched);
        self.record_source_map(&url, &source);
        Ok(source)
    }

    fn map_error(&self, description: &str) -> String {
        self.maps.borrow().describe(description)
    }
}
