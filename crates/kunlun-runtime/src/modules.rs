//! URL identity and resolution for native ECMAScript modules.
//!
//! This module deliberately stops before JavaScriptCore linking and evaluation.
//! It gives the future JSC callbacks one canonical, policy-enforcing resolver
//! instead of duplicating URL and filesystem rules at the C ABI boundary.

use crate::is_builtin_specifier;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

pub const GENERATED_MODULE_SCHEME: &str = "kunlun-generated";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    File,
    Builtin,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleUrl {
    url: Url,
    kind: ModuleKind,
}

impl ModuleUrl {
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn kind(&self) -> ModuleKind {
        self.kind
    }

    pub fn as_url(&self) -> &Url {
        &self.url
    }

    pub fn to_file_path(&self) -> Option<PathBuf> {
        (self.kind == ModuleKind::File)
            .then(|| self.url.to_file_path().ok())
            .flatten()
    }
}

impl Display for ModuleUrl {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.url, formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleResolutionError {
    InvalidModuleRoot { path: PathBuf, detail: String },
    InvalidSpecifier { specifier: String, detail: String },
    UnsupportedBareSpecifier(String),
    UnsupportedScheme(String),
    UnknownBuiltin(String),
    UnknownGeneratedModule(String),
    InvalidFileModule { url: String, detail: String },
    OutsideModuleRoot { path: PathBuf, root: PathBuf },
}

impl Display for ModuleResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModuleRoot { path, detail } => {
                write!(
                    formatter,
                    "invalid module root {}: {detail}",
                    path.display()
                )
            }
            Self::InvalidSpecifier { specifier, detail } => {
                write!(
                    formatter,
                    "invalid module specifier {specifier:?}: {detail}"
                )
            }
            Self::UnsupportedBareSpecifier(specifier) => write!(
                formatter,
                "bare module specifier {specifier:?} is not supported by the native runtime; bundle dependencies before execution"
            ),
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "unsupported module URL scheme {scheme:?}")
            }
            Self::UnknownBuiltin(specifier) => {
                write!(formatter, "unknown Kunlun built-in module {specifier:?}")
            }
            Self::UnknownGeneratedModule(specifier) => {
                write!(formatter, "unknown generated module {specifier:?}")
            }
            Self::InvalidFileModule { url, detail } => {
                write!(formatter, "invalid file module {url:?}: {detail}")
            }
            Self::OutsideModuleRoot { path, root } => write!(
                formatter,
                "module {} resolves outside the module root {}",
                path.display(),
                root.display()
            ),
        }
    }
}

impl Error for ModuleResolutionError {}

/// Resolves canonical module URLs within one application artifact root.
///
/// Files must exist, be regular files, and remain below the canonical root
/// after symbolic links are resolved. Bare package specifiers are intentionally
/// rejected: production entrypoints are bundles, while development-time
/// package resolution belongs to the selected build/package provider.
pub struct ModuleResolver {
    root: PathBuf,
    generated: BTreeSet<Url>,
}

impl ModuleResolver {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ModuleResolutionError> {
        let supplied = root.as_ref();
        let canonical =
            supplied
                .canonicalize()
                .map_err(|error| ModuleResolutionError::InvalidModuleRoot {
                    path: supplied.to_path_buf(),
                    detail: error.to_string(),
                })?;
        if !canonical.is_dir() {
            return Err(ModuleResolutionError::InvalidModuleRoot {
                path: canonical,
                detail: "root is not a directory".to_owned(),
            });
        }
        Ok(Self {
            root: canonical,
            generated: BTreeSet::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_entry(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        let supplied = path.as_ref();
        let absolute = if supplied.is_absolute() {
            supplied.to_path_buf()
        } else {
            self.root.join(supplied)
        };
        let url = Url::from_file_path(&absolute).map_err(|()| {
            ModuleResolutionError::InvalidFileModule {
                url: absolute.display().to_string(),
                detail: "path cannot be represented as a file URL".to_owned(),
            }
        })?;
        self.resolve_file_url(url)
    }

    /// Registers an in-memory module identity before any graph resolves it.
    /// Generated modules use hierarchical URLs such as
    /// `kunlun-generated:///bootstrap/runtime.mjs`, so their relative imports
    /// retain ordinary URL semantics.
    pub fn register_generated(
        &mut self,
        specifier: &str,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        let url =
            Url::parse(specifier).map_err(|error| ModuleResolutionError::InvalidSpecifier {
                specifier: specifier.to_owned(),
                detail: error.to_string(),
            })?;
        self.validate_generated_url(specifier, &url)?;
        self.generated.insert(url.clone());
        Ok(ModuleUrl {
            url,
            kind: ModuleKind::Generated,
        })
    }

    pub fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleUrl,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        if specifier.is_empty() {
            return Err(ModuleResolutionError::InvalidSpecifier {
                specifier: specifier.to_owned(),
                detail: "specifier is empty".to_owned(),
            });
        }

        let url = match Url::parse(specifier) {
            Ok(url) => url,
            Err(_) if is_url_reference(specifier) => {
                referrer.url.join(specifier).map_err(|join_error| {
                    ModuleResolutionError::InvalidSpecifier {
                        specifier: specifier.to_owned(),
                        detail: format!(
                            "cannot resolve against {}: {join_error}",
                            referrer.as_str()
                        ),
                    }
                })?
            }
            Err(_) => {
                return Err(ModuleResolutionError::UnsupportedBareSpecifier(
                    specifier.to_owned(),
                ));
            }
        };

        match url.scheme() {
            "file" => self.resolve_file_url(url),
            "kunlun" if is_builtin_specifier(url.as_str()) => Ok(ModuleUrl {
                url,
                kind: ModuleKind::Builtin,
            }),
            "kunlun" => Err(ModuleResolutionError::UnknownBuiltin(url.into())),
            GENERATED_MODULE_SCHEME => {
                self.validate_generated_url(specifier, &url)?;
                if !self.generated.contains(&url) {
                    return Err(ModuleResolutionError::UnknownGeneratedModule(url.into()));
                }
                Ok(ModuleUrl {
                    url,
                    kind: ModuleKind::Generated,
                })
            }
            scheme => Err(ModuleResolutionError::UnsupportedScheme(scheme.to_owned())),
        }
    }

    fn resolve_file_url(&self, mut url: Url) -> Result<ModuleUrl, ModuleResolutionError> {
        let supplied = url.to_string();
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ModuleResolutionError::InvalidFileModule {
                url: supplied,
                detail: "credentials are not allowed in file URLs".to_owned(),
            });
        }
        if let Some(host) = url.host_str() {
            if host != "localhost" {
                return Err(ModuleResolutionError::InvalidFileModule {
                    url: supplied,
                    detail: "remote file URL authorities are not supported".to_owned(),
                });
            }
            url.set_host(None)
                .map_err(|error| ModuleResolutionError::InvalidFileModule {
                    url: supplied,
                    detail: format!("localhost authority cannot be normalized: {error}"),
                })?;
        }

        let query = url.query().map(str::to_owned);
        let fragment = url.fragment().map(str::to_owned);
        url.set_query(None);
        url.set_fragment(None);
        let path = url
            .to_file_path()
            .map_err(|()| ModuleResolutionError::InvalidFileModule {
                url: url.to_string(),
                detail: "URL cannot be converted to a local path".to_owned(),
            })?;
        let canonical =
            path.canonicalize()
                .map_err(|error| ModuleResolutionError::InvalidFileModule {
                    url: url.to_string(),
                    detail: error.to_string(),
                })?;
        if !canonical.starts_with(&self.root) {
            return Err(ModuleResolutionError::OutsideModuleRoot {
                path: canonical,
                root: self.root.clone(),
            });
        }
        if !fs::metadata(&canonical)
            .map_err(|error| ModuleResolutionError::InvalidFileModule {
                url: url.to_string(),
                detail: error.to_string(),
            })?
            .is_file()
        {
            return Err(ModuleResolutionError::InvalidFileModule {
                url: url.into(),
                detail: "module is not a regular file".to_owned(),
            });
        }

        let mut canonical_url = Url::from_file_path(&canonical).map_err(|()| {
            ModuleResolutionError::InvalidFileModule {
                url: canonical.display().to_string(),
                detail: "canonical path cannot be represented as a file URL".to_owned(),
            }
        })?;
        canonical_url.set_query(query.as_deref());
        canonical_url.set_fragment(fragment.as_deref());
        Ok(ModuleUrl {
            url: canonical_url,
            kind: ModuleKind::File,
        })
    }

    fn validate_generated_url(
        &self,
        supplied: &str,
        url: &Url,
    ) -> Result<(), ModuleResolutionError> {
        if url.scheme() != GENERATED_MODULE_SCHEME {
            return Err(ModuleResolutionError::UnsupportedScheme(
                url.scheme().to_owned(),
            ));
        }
        if url.cannot_be_a_base()
            || url.host().is_some()
            || !url.path().starts_with('/')
            || !url.as_str().starts_with("kunlun-generated:///")
        {
            return Err(ModuleResolutionError::InvalidSpecifier {
                specifier: supplied.to_owned(),
                detail: format!(
                    "generated modules require an authority-free hierarchical {GENERATED_MODULE_SCHEME}:/// URL"
                ),
            });
        }
        Ok(())
    }
}

fn is_url_reference(specifier: &str) -> bool {
    ["./", "../", "/", "?", "#"]
        .iter()
        .any(|prefix| specifier.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        base: PathBuf,
        project: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().join(format!(
                "kunlun-module-resolver-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let project = base.join("project");
            fs::create_dir_all(project.join("src/nested")).unwrap();
            fs::write(project.join("src/entry.mjs"), "").unwrap();
            fs::write(project.join("src/dep.mjs"), "").unwrap();
            Self { base, project }
        }

        fn resolver(&self) -> ModuleResolver {
            ModuleResolver::new(&self.project).unwrap()
        }

        fn entry(&self, resolver: &ModuleResolver) -> ModuleUrl {
            resolver.resolve_entry("src/entry.mjs").unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    #[test]
    fn resolves_and_canonicalizes_relative_file_urls() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);
        let resolved = resolver
            .resolve("./nested/../dep.mjs?loader=test#source", &entry)
            .unwrap();
        let expected =
            Url::from_file_path(fixture.project.join("src/dep.mjs").canonicalize().unwrap())
                .unwrap();

        assert_eq!(resolved.kind(), ModuleKind::File);
        assert_eq!(
            resolved.to_file_path(),
            Some(fixture.project.join("src/dep.mjs").canonicalize().unwrap())
        );
        assert_eq!(resolved.as_str(), format!("{expected}?loader=test#source"));
    }

    #[test]
    fn resolves_only_registered_builtins() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert_eq!(
            resolver.resolve("kunlun:fs", &entry).unwrap().kind(),
            ModuleKind::Builtin
        );
        assert!(matches!(
            resolver.resolve("kunlun:not-registered", &entry),
            Err(ModuleResolutionError::UnknownBuiltin(_))
        ));
    }

    #[test]
    fn resolves_registered_generated_module_graphs() {
        let fixture = Fixture::new();
        let mut resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);
        let generated_entry = resolver
            .register_generated("kunlun-generated:///bootstrap/entry.mjs")
            .unwrap();
        resolver
            .register_generated("kunlun-generated:///bootstrap/dep.mjs")
            .unwrap();

        assert_eq!(
            resolver
                .resolve("kunlun-generated:///bootstrap/entry.mjs", &entry)
                .unwrap(),
            generated_entry
        );
        assert_eq!(
            resolver
                .resolve("./dep.mjs", &generated_entry)
                .unwrap()
                .as_str(),
            "kunlun-generated:///bootstrap/dep.mjs"
        );
        assert!(matches!(
            resolver.resolve("./missing.mjs", &generated_entry),
            Err(ModuleResolutionError::UnknownGeneratedModule(_))
        ));
    }

    #[test]
    fn rejects_bare_and_unsupported_specifiers() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert_eq!(
            resolver.resolve("some-package", &entry),
            Err(ModuleResolutionError::UnsupportedBareSpecifier(
                "some-package".to_owned()
            ))
        );
        assert_eq!(
            resolver.resolve("https://example.com/mod.mjs", &entry),
            Err(ModuleResolutionError::UnsupportedScheme("https".to_owned()))
        );
        assert!(matches!(
            resolver.resolve("file://example.com/mod.mjs", &entry),
            Err(ModuleResolutionError::InvalidFileModule { .. })
        ));
        assert!(matches!(
            resolver.resolve("", &entry),
            Err(ModuleResolutionError::InvalidSpecifier { .. })
        ));
    }

    #[test]
    fn generated_module_ids_must_be_hierarchical_and_authority_free() {
        let fixture = Fixture::new();
        let mut resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        for invalid in [
            "kunlun-generated:bootstrap/entry.mjs",
            "kunlun-generated:/bootstrap/entry.mjs",
            "kunlun-generated://runtime/bootstrap/entry.mjs",
        ] {
            assert!(matches!(
                resolver.register_generated(invalid),
                Err(ModuleResolutionError::InvalidSpecifier { .. })
            ));
            assert!(matches!(
                resolver.resolve(invalid, &entry),
                Err(ModuleResolutionError::InvalidSpecifier { .. })
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_module_root() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let outside = fixture.base.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.mjs"), "").unwrap();
        symlink(&outside, fixture.project.join("src/outside")).unwrap();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert!(matches!(
            resolver.resolve("./outside/secret.mjs", &entry),
            Err(ModuleResolutionError::OutsideModuleRoot { .. })
        ));
    }

    #[test]
    fn rejects_directories_and_missing_files() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert!(matches!(
            resolver.resolve("./nested", &entry),
            Err(ModuleResolutionError::InvalidFileModule { .. })
        ));
        assert!(matches!(
            resolver.resolve("./missing.mjs", &entry),
            Err(ModuleResolutionError::InvalidFileModule { .. })
        ));
    }
}
