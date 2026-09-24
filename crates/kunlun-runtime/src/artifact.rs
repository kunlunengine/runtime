//! Versioned, data-only server artifact admission. No application code runs here.
use crate::module_sources::ModuleSources;
use crate::modules::{ModuleKind, ModuleResolver};
use crate::source_maps::{MAX_MAP_BYTES, SourceMaps};
use crate::{ApplicationAuthority, HostPermissions};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::path::Path;
use url::Url;

pub const RUNTIME_MANIFEST_SCHEMA: &str = "kunlun.runtime-manifest/v1";
pub const FETCH_ENTRY_CONTRACT: &str = "kunlun.fetch-entry/v1";
pub const RUNTIME_ENGINE_ABI: u32 = 1;
pub const RUNTIME_PROFILE: &str = "kunlun-m2-web/1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOURCE_TOTAL: usize = 64 * 1024 * 1024;
const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
const MAX_FILES: usize = 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema: String,
    pub engine: EngineRequirement,
    pub entry_contract: String,
    pub entry: String,
    pub files: Vec<ManifestFile>,
    pub required_features: Vec<String>,
    pub compatibility_flags: Vec<String>,
    pub capabilities: CapabilityRequirements,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRequirement {
    pub abi: u32,
    pub runtime_profile: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    pub url: String,
    pub kind: ManifestFileKind,
    pub sha256: String,
    #[serde(default, rename = "for")]
    pub source_map_for: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestFileKind {
    Module,
    Asset,
    SourceMap,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequirements {
    pub required: Vec<Capability>,
    pub optional: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub resource: String,
}

/// Trusted deployment inputs. The grant set is derived from actual host
/// permissions, not from arbitrary manifest-matching strings.
pub struct AdmissionPolicy {
    pub expected_manifest_sha256: String,
    deployment: HostPermissions,
}

impl AdmissionPolicy {
    pub fn new(expected_manifest_sha256: impl Into<String>, deployment: HostPermissions) -> Self {
        Self {
            expected_manifest_sha256: expected_manifest_sha256.into(),
            deployment,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionErrorKind {
    Manifest,
    Schema,
    Compatibility,
    Capability,
    Path,
    Identity,
    Integrity,
    MissingFile,
    InvalidFile,
    Limit,
    SourceMap,
}

impl Display for AdmissionErrorKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Manifest => "manifest",
                Self::Schema => "schema",
                Self::Compatibility => "compatibility",
                Self::Capability => "capability",
                Self::Path => "path",
                Self::Identity => "identity",
                Self::Integrity => "integrity",
                Self::MissingFile => "missing_file",
                Self::InvalidFile => "invalid_file",
                Self::Limit => "limit",
                Self::SourceMap => "source_map",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionError {
    pub kind: AdmissionErrorKind,
    pub location: String,
    pub detail: String,
}

impl AdmissionError {
    fn new(
        kind: AdmissionErrorKind,
        location: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            location: location.into(),
            detail: detail.into(),
        }
    }
}

impl Display for AdmissionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "artifact {} at {}: {}",
            self.kind, self.location, self.detail
        )
    }
}

impl std::error::Error for AdmissionError {}

/// Owns the exact checked bytes. Assets are served from this snapshot; module
/// sources are transferred to the loader without reopening disk files.
pub struct AdmittedArtifact {
    manifest: RuntimeManifest,
    entry_url: String,
    sources: ModuleSources,
    assets: BTreeMap<String, Vec<u8>>,
    authority: ApplicationAuthority,
}

impl AdmittedArtifact {
    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }
    pub fn entry_url(&self) -> &str {
        &self.entry_url
    }
    /// The key is the canonical manifest URL, including the leading `./`.
    pub fn asset_bytes(&self, url: &str) -> Option<&[u8]> {
        self.assets.get(url).map(Vec::as_slice)
    }
    pub fn authority(&self) -> &ApplicationAuthority {
        &self.authority
    }
    /// Keep the authority alongside checked bytes when creating an M3 server.
    pub fn into_parts(
        self,
    ) -> (
        String,
        ModuleSources,
        BTreeMap<String, Vec<u8>>,
        ApplicationAuthority,
    ) {
        (self.entry_url, self.sources, self.assets, self.authority)
    }
}

/// Admit a `.kunlun/runtime` directory containing `manifest.json`.
/// `expected_manifest_sha256` must come from the deployment's trusted metadata.
pub fn admit_artifact(
    root: impl AsRef<Path>,
    policy: &AdmissionPolicy,
) -> Result<AdmittedArtifact, AdmissionError> {
    let resolver = ModuleResolver::new(root.as_ref()).map_err(|_| {
        AdmissionError::new(
            AdmissionErrorKind::Path,
            "artifact root",
            "invalid artifact root",
        )
    })?;
    let directory = Dir::open_ambient_dir(resolver.root(), ambient_authority()).map_err(|e| {
        AdmissionError::new(
            AdmissionErrorKind::Path,
            "artifact root",
            format!("cannot open artifact root: {:?}", e.kind()),
        )
    })?;
    let raw = read_bounded(
        &directory,
        Path::new("manifest.json"),
        "manifest.json",
        MAX_MANIFEST_BYTES,
    )?;
    check_digest(&raw, &policy.expected_manifest_sha256, "manifest.json")?;
    let manifest: RuntimeManifest = serde_json::from_slice(&raw).map_err(|e| {
        AdmissionError::new(AdmissionErrorKind::Manifest, "manifest.json", e.to_string())
    })?;
    validate_contract(&manifest, policy)?;
    let root_url = Url::from_directory_path(resolver.root()).map_err(|()| {
        AdmissionError::new(
            AdmissionErrorKind::Path,
            "artifact root",
            "cannot form file URL",
        )
    })?;
    let entry_url = resolve_manifest_url(&resolver, &root_url, &manifest.entry)?;
    let mut indexed = BTreeMap::new();
    for file in &manifest.files {
        let url = resolve_manifest_url(&resolver, &root_url, &file.url)?;
        if file.url == "./manifest.json" {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Identity,
                &file.url,
                "manifest cannot index itself",
            ));
        }
        if indexed.insert(url, file).is_some() {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Identity,
                &file.url,
                "duplicate canonical file identity",
            ));
        }
    }
    if indexed
        .get(&entry_url)
        .is_none_or(|file| file.kind != ManifestFileKind::Module)
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Identity,
            &manifest.entry,
            "entry must be an indexed module",
        ));
    }

    let mut sources = HashMap::new();
    let mut assets = BTreeMap::new();
    let mut maps = Vec::new();
    let mut source_total = 0usize;
    let mut snapshot_total = 0usize;
    for (url, file) in &indexed {
        let max = match file.kind {
            ManifestFileKind::Module => MAX_SOURCE_BYTES,
            ManifestFileKind::Asset => MAX_ASSET_BYTES,
            ManifestFileKind::SourceMap => MAX_MAP_BYTES,
        };
        let path = Url::parse(url)
            .expect("resolved URL")
            .to_file_path()
            .expect("file URL");
        let relative_path = path.strip_prefix(resolver.root()).expect("contained path");
        let bytes = read_bounded(&directory, relative_path, &file.url, max)?;
        check_digest(&bytes, &file.sha256, &file.url)?;
        snapshot_total += bytes.len();
        if snapshot_total > MAX_SNAPSHOT_BYTES {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Limit,
                &file.url,
                "snapshot exceeds 128 MiB",
            ));
        }
        match file.kind {
            ManifestFileKind::Module => {
                if file.source_map_for.is_some() {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Manifest,
                        &file.url,
                        "only source maps may have a for field",
                    ));
                }
                source_total += bytes.len();
                if source_total > MAX_SOURCE_TOTAL {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Limit,
                        &file.url,
                        "module sources exceed 64 MiB",
                    ));
                }
                let source = String::from_utf8(bytes).map_err(|_| {
                    AdmissionError::new(
                        AdmissionErrorKind::InvalidFile,
                        &file.url,
                        "module source is not UTF-8",
                    )
                })?;
                sources.insert(url.clone(), source);
            }
            ManifestFileKind::Asset => {
                if file.source_map_for.is_some() {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::Manifest,
                        &file.url,
                        "only source maps may have a for field",
                    ));
                }
                assets.insert(file.url.clone(), bytes);
            }
            ManifestFileKind::SourceMap => {
                let for_url = file.source_map_for.as_deref().ok_or_else(|| {
                    AdmissionError::new(
                        AdmissionErrorKind::Manifest,
                        &file.url,
                        "source map requires a for field",
                    )
                })?;
                let module_url = resolve_manifest_url(&resolver, &root_url, for_url)?;
                if indexed
                    .get(&module_url)
                    .is_none_or(|record| record.kind != ManifestFileKind::Module)
                {
                    return Err(AdmissionError::new(
                        AdmissionErrorKind::SourceMap,
                        &file.url,
                        "for must name an indexed module",
                    ));
                }
                maps.push((url.clone(), module_url, bytes));
            }
        }
    }
    let mut decoded_maps = SourceMaps::default();
    let map_by_module: BTreeMap<String, String> = maps
        .iter()
        .map(|(url, module, _)| (module.clone(), url.clone()))
        .collect();
    if map_by_module.len() != maps.len() {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Identity,
            "files",
            "multiple maps target one module",
        ));
    }
    for (url, module, bytes) in maps {
        decoded_maps
            .insert(&module, Url::parse(&url).expect("resolved URL"), &bytes)
            .map_err(|e| AdmissionError::new(AdmissionErrorKind::SourceMap, url, e))?;
    }
    for (url, source) in &sources {
        if let Some(reference) = source_map_trailer(source) {
            let expected = map_by_module.get(url.as_str()).ok_or_else(|| {
                AdmissionError::new(
                    AdmissionErrorKind::SourceMap,
                    url,
                    "sourceMappingURL has no indexed map",
                )
            })?;
            let actual = Url::parse(url)
                .expect("resolved URL")
                .join(reference)
                .map_err(|e| {
                    AdmissionError::new(AdmissionErrorKind::SourceMap, url, e.to_string())
                })?;
            if actual.as_str() != *expected {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::SourceMap,
                    url,
                    "sourceMappingURL disagrees with indexed map",
                ));
            }
        }
    }
    let sources = ModuleSources::from_admitted(resolver.root(), sources, decoded_maps)
        .map_err(|e| AdmissionError::new(AdmissionErrorKind::Path, "artifact root", e))?;
    let authority = ApplicationAuthority::new(&policy.deployment, &manifest.capabilities);
    Ok(AdmittedArtifact {
        manifest,
        entry_url,
        sources,
        assets,
        authority,
    })
}

fn validate_contract(
    manifest: &RuntimeManifest,
    policy: &AdmissionPolicy,
) -> Result<(), AdmissionError> {
    if manifest.schema != RUNTIME_MANIFEST_SCHEMA {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Schema,
            "schema",
            format!("unsupported schema {:?}", manifest.schema),
        ));
    }
    if manifest.engine.abi != RUNTIME_ENGINE_ABI
        || manifest.engine.runtime_profile != RUNTIME_PROFILE
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Compatibility,
            "engine",
            "unsupported ABI or runtime profile",
        ));
    }
    if manifest.entry_contract != FETCH_ENTRY_CONTRACT {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Compatibility,
            "entry_contract",
            format!("unsupported entry contract {:?}", manifest.entry_contract),
        ));
    }
    let mut seen_features = BTreeSet::new();
    for feature in &manifest.required_features {
        if !seen_features.insert(feature) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Compatibility,
                "required_features",
                "duplicate feature",
            ));
        }
        if feature != "closed-module-graph" {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Compatibility,
                "required_features",
                format!("unsupported feature {feature:?}"),
            ));
        }
    }
    let mut seen_flags = BTreeSet::new();
    for flag in &manifest.compatibility_flags {
        if !seen_flags.insert(flag) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Compatibility,
                "compatibility_flags",
                "duplicate flag",
            ));
        }
        if flag != "source-map-v3" {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Compatibility,
                "compatibility_flags",
                format!("unsupported flag {flag:?}"),
            ));
        }
    }
    if manifest.files.is_empty() || manifest.files.len() > MAX_FILES {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Limit,
            "files",
            "must contain 1 to 1024 files",
        ));
    }
    let mut declared = BTreeSet::new();
    for capability in manifest
        .capabilities
        .required
        .iter()
        .chain(&manifest.capabilities.optional)
    {
        if capability.name.is_empty()
            || capability.resource.is_empty()
            || capability.name.len() > 128
            || capability.resource.len() > 256
            || capability.name.chars().any(char::is_control)
            || capability.resource.chars().any(char::is_control)
        {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Capability,
                "capabilities",
                "invalid capability identifier",
            ));
        }
        let valid_resource = match capability.name.as_str() {
            "http.host" => Url::parse(&format!("https://{}/", capability.resource))
                .ok()
                .is_some_and(|url| {
                    url.host_str() == Some(capability.resource.as_str())
                        && url.port().is_none()
                        && url.username().is_empty()
                        && url.password().is_none()
                }),
            "fs.binding" => capability
                .resource
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
            _ => true,
        };
        if !valid_resource {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Capability,
                "capabilities",
                "capability resource is not canonical",
            ));
        }
        if !declared.insert(capability) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Capability,
                "capabilities",
                "duplicate capability declaration",
            ));
        }
        if !HostPermissions::supports_capability(&capability.name) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Capability,
                "capabilities",
                format!("unsupported capability {:?}", capability.name),
            ));
        }
    }
    for capability in &manifest.capabilities.required {
        if !policy.deployment.has_capability(capability) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Capability,
                "capabilities.required",
                format!(
                    "missing grant for {}:{}",
                    capability.name, capability.resource
                ),
            ));
        }
    }
    Ok(())
}

fn resolve_manifest_url(
    resolver: &ModuleResolver,
    root_url: &Url,
    relative: &str,
) -> Result<String, AdmissionError> {
    let fail = |detail: String| AdmissionError::new(AdmissionErrorKind::Path, relative, detail);
    if !relative.starts_with("./") || relative.contains(['?', '#', '\\']) {
        return Err(fail(
            "expected canonical root-relative file URL without query or fragment".into(),
        ));
    }
    let absolute = root_url.join(relative).map_err(|e| fail(e.to_string()))?;
    if !absolute.as_str().starts_with(root_url.as_str()) {
        return Err(fail("file URL is outside artifact root".into()));
    }
    if let Ok(path) = absolute.to_file_path() {
        if let Err(error) = std::fs::symlink_metadata(path) {
            if error.kind() == std::io::ErrorKind::NotFound {
                return Err(AdmissionError::new(
                    AdmissionErrorKind::MissingFile,
                    relative,
                    "file does not exist",
                ));
            }
        }
    }
    let resolved = resolver
        .resolve_absolute_url(absolute.as_str())
        .map_err(|_| fail("invalid or escaping artifact file URL".into()))?;
    if resolved.kind() != ModuleKind::File {
        return Err(fail("expected artifact file".into()));
    }
    let canonical_relative = resolved
        .as_str()
        .strip_prefix(root_url.as_str())
        .ok_or_else(|| fail("file URL is outside artifact root".into()))?;
    if relative != format!("./{canonical_relative}") {
        return Err(fail("noncanonical file URL or symlink alias".into()));
    }
    Ok(resolved.as_str().to_owned())
}

fn read_bounded(
    directory: &Dir,
    relative: &Path,
    location: &str,
    limit: usize,
) -> Result<Vec<u8>, AdmissionError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    let file = directory.open_with(relative, &options).map_err(|e| {
        AdmissionError::new(
            if e.kind() == std::io::ErrorKind::NotFound {
                AdmissionErrorKind::MissingFile
            } else {
                AdmissionErrorKind::InvalidFile
            },
            location,
            format!("cannot open file: {:?}", e.kind()),
        )
    })?;
    if !file
        .metadata()
        .map_err(|e| {
            AdmissionError::new(
                AdmissionErrorKind::InvalidFile,
                location,
                format!("cannot inspect file: {:?}", e.kind()),
            )
        })?
        .is_file()
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::InvalidFile,
            location,
            "not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            AdmissionError::new(
                AdmissionErrorKind::InvalidFile,
                location,
                format!("cannot read file: {:?}", e.kind()),
            )
        })?;
    if bytes.len() > limit {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Limit,
            location,
            format!("exceeds {limit} byte limit"),
        ));
    }
    Ok(bytes)
}

fn check_digest(bytes: &[u8], expected: &str, location: &str) -> Result<(), AdmissionError> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Manifest,
            location,
            "SHA-256 must be 64 lowercase hexadecimal digits",
        ));
    }
    let mut actual = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write;
        write!(actual, "{byte:02x}").expect("writing to String");
    }
    if actual != expected {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Integrity,
            location,
            "SHA-256 mismatch",
        ));
    }
    Ok(())
}

fn source_map_trailer(source: &str) -> Option<&str> {
    source
        .lines()
        .rfind(|line| !line.trim().is_empty())?
        .trim()
        .strip_prefix("//# sourceMappingURL=")
        .map(str::trim)
}
