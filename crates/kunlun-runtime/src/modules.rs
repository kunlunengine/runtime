//! URL identity and resolution for native ECMAScript modules.
//!
//! This module deliberately stops before JavaScriptCore linking and evaluation.
//! It gives static and dynamic loader callbacks one canonical, policy-enforcing
//! resolver and cache-key contract without requiring an engine to test either.

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

    /// The shared identity key for static and dynamic module requests.
    ///
    /// A loader must resolve a request before consulting its per-graph module
    /// cache. Equal keys identify one module, including during a cycle; query
    /// strings and fragments are significant. This API does not create a cache
    /// or confer permission to fetch or execute the identified module.
    pub fn cache_key(&self) -> &str {
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

/// A resolution failure retaining the original request and its referring URL.
///
/// `resolve` always supplies a referrer. Root construction, entry paths, and
/// generated registration have no referring module and use `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleResolutionError {
    pub specifier: String,
    pub referrer: Option<String>,
    pub kind: ModuleResolutionErrorKind,
}

impl ModuleResolutionError {
    fn new(specifier: &str, referrer: Option<&ModuleUrl>, kind: ModuleResolutionErrorKind) -> Self {
        Self {
            specifier: specifier.to_owned(),
            referrer: referrer.map(|url| url.as_str().to_owned()),
            kind,
        }
    }
}

impl Display for ModuleResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "cannot resolve module {:?}", self.specifier)?;
        if let Some(referrer) = &self.referrer {
            write!(formatter, " from {referrer:?}")?;
        }
        write!(formatter, ": {}", self.kind)
    }
}

impl Error for ModuleResolutionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleResolutionErrorKind {
    InvalidModuleRoot { path: PathBuf, detail: String },
    InvalidSpecifier { detail: String },
    UnsupportedBareSpecifier(String),
    UnsupportedScheme(String),
    UnknownBuiltin(String),
    UnknownGeneratedModule(String),
    InvalidFileModule { url: String, detail: String },
    OutsideModuleRoot { path: PathBuf, root: PathBuf },
    InvalidReferrer { detail: String },
}

impl Display for ModuleResolutionErrorKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModuleRoot { path, detail } => {
                write!(
                    formatter,
                    "invalid module root {}: {detail}",
                    path.display()
                )
            }
            Self::InvalidSpecifier { detail } => {
                write!(formatter, "invalid module specifier: {detail}")
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
            Self::InvalidReferrer { detail } => {
                write!(formatter, "invalid module referrer: {detail}")
            }
        }
    }
}

/// Resolves canonical module URLs within one application artifact root.
///
/// Files must exist, be regular files, and remain below the canonical root
/// after symbolic links are resolved. Bare package specifiers are intentionally
/// rejected: production entrypoints are bundles, while development-time
/// package resolution belongs to the selected build/package provider.
/// Resolution performs no fetching, execution, or host capability grants.
pub struct ModuleResolver {
    root: PathBuf,
    generated: BTreeSet<Url>,
}

impl ModuleResolver {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ModuleResolutionError> {
        let supplied = root.as_ref();
        let error = |kind| ModuleResolutionError::new(&supplied.to_string_lossy(), None, kind);
        let canonical = supplied.canonicalize().map_err(|cause| {
            error(ModuleResolutionErrorKind::InvalidModuleRoot {
                path: supplied.to_path_buf(),
                detail: cause.to_string(),
            })
        })?;
        if !canonical.is_dir() {
            return Err(error(ModuleResolutionErrorKind::InvalidModuleRoot {
                path: canonical,
                detail: "root is not a directory".to_owned(),
            }));
        }
        Ok(Self {
            root: canonical,
            generated: BTreeSet::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an OS path, not a URL string; literal `%`, `?`, `#`, and spaces
    /// in file names are encoded before URL validation.
    pub fn resolve_entry(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        let supplied = path.as_ref();
        let error = |kind| ModuleResolutionError::new(&supplied.to_string_lossy(), None, kind);
        let absolute = if supplied.is_absolute() {
            supplied.to_path_buf()
        } else {
            self.root.join(supplied)
        };
        let url = Url::from_file_path(&absolute).map_err(|()| {
            error(ModuleResolutionErrorKind::InvalidFileModule {
                url: absolute.display().to_string(),
                detail: "path cannot be represented as a file URL".to_owned(),
            })
        })?;
        self.resolve_file_url(url).map_err(error)
    }

    /// Registers an in-memory module identity before any graph resolves it.
    ///
    /// Generated modules use authority-free hierarchical URLs. Canonically
    /// equivalent registrations are idempotent; this registry stores identities
    /// only, so registration never creates or overwrites module source.
    pub fn register_generated(
        &mut self,
        specifier: &str,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        let error = |kind| ModuleResolutionError::new(specifier, None, kind);
        validate_specifier(specifier).map_err(error)?;
        let url =
            Url::parse(specifier).map_err(|cause| error(invalid_specifier(cause.to_string())))?;
        let url = canonical_generated_url(url).map_err(error)?;
        self.generated.insert(url.clone());
        Ok(ModuleUrl {
            url,
            kind: ModuleKind::Generated,
        })
    }

    /// Resolves either a static or dynamic request under the same policy.
    /// Referrers are revalidated in this resolver, including when their identity
    /// was obtained from another resolver with a different root or registry.
    pub fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleUrl,
    ) -> Result<ModuleUrl, ModuleResolutionError> {
        let error = |kind| ModuleResolutionError::new(specifier, Some(referrer), kind);
        validate_specifier(specifier).map_err(error)?;
        let checked_referrer = self.resolve_url(referrer.url.clone()).map_err(|cause| {
            error(ModuleResolutionErrorKind::InvalidReferrer {
                detail: cause.to_string(),
            })
        })?;
        if checked_referrer != *referrer {
            return Err(error(ModuleResolutionErrorKind::InvalidReferrer {
                detail: "referrer no longer has its original canonical identity".to_owned(),
            }));
        }

        let url = match Url::parse(specifier) {
            Ok(url) => url,
            Err(_) if is_url_reference(specifier) => referrer
                .url
                .join(specifier)
                .map_err(|cause| error(invalid_specifier(cause.to_string())))?,
            Err(cause) if looks_like_absolute_url(specifier) => {
                return Err(error(invalid_specifier(cause.to_string())));
            }
            Err(_) => {
                return Err(error(ModuleResolutionErrorKind::UnsupportedBareSpecifier(
                    specifier.to_owned(),
                )));
            }
        };
        self.resolve_url(url).map_err(error)
    }

    fn resolve_url(&self, url: Url) -> Result<ModuleUrl, ModuleResolutionErrorKind> {
        match url.scheme() {
            "file" => self.resolve_file_url(url),
            "kunlun" if is_builtin_specifier(url.as_str()) => Ok(ModuleUrl {
                url,
                kind: ModuleKind::Builtin,
            }),
            "kunlun" => Err(ModuleResolutionErrorKind::UnknownBuiltin(url.into())),
            GENERATED_MODULE_SCHEME => {
                let url = canonical_generated_url(url)?;
                if !self.generated.contains(&url) {
                    return Err(ModuleResolutionErrorKind::UnknownGeneratedModule(
                        url.into(),
                    ));
                }
                Ok(ModuleUrl {
                    url,
                    kind: ModuleKind::Generated,
                })
            }
            scheme => Err(ModuleResolutionErrorKind::UnsupportedScheme(
                scheme.to_owned(),
            )),
        }
    }

    fn resolve_file_url(&self, mut url: Url) -> Result<ModuleUrl, ModuleResolutionErrorKind> {
        validate_specifier(url.as_str())?;
        validate_encoded_path(url.path())?;
        let supplied = url.to_string();
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ModuleResolutionErrorKind::InvalidFileModule {
                url: supplied,
                detail: "credentials are not allowed in file URLs".to_owned(),
            });
        }
        if let Some(host) = url.host_str() {
            if host != "localhost" {
                return Err(ModuleResolutionErrorKind::InvalidFileModule {
                    url: supplied,
                    detail: "remote file URL authorities are not supported".to_owned(),
                });
            }
            url.set_host(None)
                .map_err(|cause| ModuleResolutionErrorKind::InvalidFileModule {
                    url: supplied,
                    detail: format!("localhost authority cannot be normalized: {cause}"),
                })?;
        }

        let query = url.query().map(str::to_owned);
        let fragment = url.fragment().map(str::to_owned);
        url.set_query(None);
        url.set_fragment(None);
        let path =
            url.to_file_path()
                .map_err(|()| ModuleResolutionErrorKind::InvalidFileModule {
                    url: url.to_string(),
                    detail: "URL cannot be converted to a local path".to_owned(),
                })?;
        let canonical =
            path.canonicalize()
                .map_err(|cause| ModuleResolutionErrorKind::InvalidFileModule {
                    url: url.to_string(),
                    detail: cause.to_string(),
                })?;
        if !canonical.starts_with(&self.root) {
            return Err(ModuleResolutionErrorKind::OutsideModuleRoot {
                path: canonical,
                root: self.root.clone(),
            });
        }
        if !fs::metadata(&canonical)
            .map_err(|cause| ModuleResolutionErrorKind::InvalidFileModule {
                url: url.to_string(),
                detail: cause.to_string(),
            })?
            .is_file()
        {
            return Err(ModuleResolutionErrorKind::InvalidFileModule {
                url: url.into(),
                detail: "module is not a regular file".to_owned(),
            });
        }

        let mut canonical_url = Url::from_file_path(&canonical).map_err(|()| {
            ModuleResolutionErrorKind::InvalidFileModule {
                url: canonical.display().to_string(),
                detail: "canonical path cannot be represented as a file URL".to_owned(),
            }
        })?;
        // A symlink may select a filename which was not present in the request.
        validate_specifier(canonical_url.as_str())?;
        validate_encoded_path(canonical_url.path())?;
        canonical_url.set_query(query.as_deref());
        canonical_url.set_fragment(fragment.as_deref());
        normalize_suffix(&mut canonical_url)?;
        Ok(ModuleUrl {
            url: canonical_url,
            kind: ModuleKind::File,
        })
    }
}

fn invalid_specifier(detail: impl Into<String>) -> ModuleResolutionErrorKind {
    ModuleResolutionErrorKind::InvalidSpecifier {
        detail: detail.into(),
    }
}

/// Validate before URL parsing, which otherwise repairs some invalid inputs.
fn validate_specifier(specifier: &str) -> Result<(), ModuleResolutionErrorKind> {
    if specifier.is_empty() {
        return Err(invalid_specifier("specifier is empty"));
    }
    if specifier.trim() != specifier {
        return Err(invalid_specifier(
            "leading or trailing whitespace is not allowed",
        ));
    }
    if specifier.chars().any(char::is_control) {
        return Err(invalid_specifier("control characters are not allowed"));
    }
    if specifier.contains('\\') {
        return Err(invalid_specifier(
            "backslashes are not allowed in module URLs",
        ));
    }
    let mut characters = specifier.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            let value = percent_byte(&mut characters)?;
            if value.is_ascii_control() {
                return Err(invalid_specifier(
                    "percent-encoded control characters are not allowed",
                ));
            }
        }
    }
    // Check the request path before URL joining can remove a dot-segment and
    // hide a forbidden escape. Query and fragment separators are not paths.
    validate_encoded_path(specifier.split(['?', '#']).next().unwrap_or(specifier))?;
    Ok(())
}

fn percent_byte(characters: &mut std::str::Chars<'_>) -> Result<u8, ModuleResolutionErrorKind> {
    let high = characters
        .next()
        .and_then(|character| character.to_digit(16));
    let low = characters
        .next()
        .and_then(|character| character.to_digit(16));
    match (high, low) {
        (Some(high), Some(low)) => Ok((high * 16 + low) as u8),
        _ => Err(invalid_specifier(
            "percent escapes must contain exactly two hexadecimal digits",
        )),
    }
}

fn validate_encoded_path(path: &str) -> Result<(), ModuleResolutionErrorKind> {
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character == '%' && matches!(percent_byte(&mut characters)?, b'/' | b'\\') {
            return Err(invalid_specifier(
                "percent-encoded path separators are not allowed",
            ));
        }
    }
    Ok(())
}

fn normalize_percent_encoding(
    component: &str,
    decode_unreserved: bool,
) -> Result<String, ModuleResolutionErrorKind> {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut normalized = String::with_capacity(component.len());
    let mut characters = component.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            normalized.push(character);
            continue;
        }
        let value = percent_byte(&mut characters)?;
        if decode_unreserved && (value.is_ascii_alphanumeric() || b"-._~".contains(&value)) {
            normalized.push(char::from(value));
        } else {
            normalized.push('%');
            normalized.push(char::from(HEX[usize::from(value / 16)]));
            normalized.push(char::from(HEX[usize::from(value % 16)]));
        }
    }
    Ok(normalized)
}

fn normalize_suffix(url: &mut Url) -> Result<(), ModuleResolutionErrorKind> {
    if let Some(query) = url.query() {
        let query = normalize_percent_encoding(query, false)?;
        url.set_query(Some(&query));
    }
    if let Some(fragment) = url.fragment() {
        let fragment = normalize_percent_encoding(fragment, false)?;
        url.set_fragment(Some(&fragment));
    }
    Ok(())
}

fn canonical_generated_url(mut url: Url) -> Result<Url, ModuleResolutionErrorKind> {
    if url.scheme() != GENERATED_MODULE_SCHEME {
        return Err(ModuleResolutionErrorKind::UnsupportedScheme(
            url.scheme().to_owned(),
        ));
    }
    if url.cannot_be_a_base()
        || url.host().is_some()
        || !url.path().starts_with('/')
        || !url.as_str().starts_with("kunlun-generated:///")
    {
        return Err(invalid_specifier(format!(
            "generated modules require an authority-free hierarchical {GENERATED_MODULE_SCHEME}:/// URL"
        )));
    }
    validate_specifier(url.as_str())?;
    validate_encoded_path(url.path())?;
    let path = normalize_percent_encoding(url.path(), true)?;
    url.set_path(&path);
    // Parsing again applies dot-segment rules after unreserved normalization.
    let mut url = Url::parse(url.as_str()).map_err(|cause| invalid_specifier(cause.to_string()))?;
    normalize_suffix(&mut url)?;
    Ok(url)
}

fn is_url_reference(specifier: &str) -> bool {
    ["./", "../", "/", "?", "#"]
        .iter()
        .any(|prefix| specifier.starts_with(prefix))
}

fn looks_like_absolute_url(specifier: &str) -> bool {
    specifier
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|prefix| prefix.contains(':'))
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
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::UnknownBuiltin(_),
                ..
            })
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
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::UnknownGeneratedModule(_),
                ..
            })
        ));
    }

    #[test]
    fn rejects_bare_and_unsupported_specifiers() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert_eq!(
            resolver.resolve("some-package", &entry).unwrap_err().kind,
            ModuleResolutionErrorKind::UnsupportedBareSpecifier("some-package".to_owned())
        );
        assert_eq!(
            resolver
                .resolve("https://example.com/mod.mjs", &entry)
                .unwrap_err()
                .kind,
            ModuleResolutionErrorKind::UnsupportedScheme("https".to_owned())
        );
        assert!(matches!(
            resolver.resolve("file://example.com/mod.mjs", &entry),
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::InvalidFileModule { .. },
                ..
            })
        ));
        assert!(matches!(
            resolver.resolve("", &entry),
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::InvalidSpecifier { .. },
                ..
            })
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
                Err(ModuleResolutionError {
                    kind: ModuleResolutionErrorKind::InvalidSpecifier { .. },
                    ..
                })
            ));
            assert!(matches!(
                resolver.resolve(invalid, &entry),
                Err(ModuleResolutionError {
                    kind: ModuleResolutionErrorKind::InvalidSpecifier { .. },
                    ..
                })
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
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::OutsideModuleRoot { .. },
                ..
            })
        ));
    }

    #[test]
    fn rejects_directories_and_missing_files() {
        let fixture = Fixture::new();
        let resolver = fixture.resolver();
        let entry = fixture.entry(&resolver);

        assert!(matches!(
            resolver.resolve("./nested", &entry),
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::InvalidFileModule { .. },
                ..
            })
        ));
        assert!(matches!(
            resolver.resolve("./missing.mjs", &entry),
            Err(ModuleResolutionError {
                kind: ModuleResolutionErrorKind::InvalidFileModule { .. },
                ..
            })
        ));
    }
}
