//! Public resolver contract tests. These exercise module identities and policy;
//! JavaScriptCore linking, evaluation, and import callbacks are separate work.

#![cfg(unix)]

use kunlun_runtime::{
    BUILTIN_MODULES, ModuleKind, ModuleResolutionError, ModuleResolutionErrorKind, ModuleResolver,
    ModuleUrl,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "kunlun-resolution-contract-{}-{timestamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("project 空间");
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/entry.mjs"), "import './dep.mjs';").unwrap();
        fs::write(root.join("src/dep.mjs"), "import './entry.mjs';").unwrap();
        Self { base, root }
    }

    fn resolver(&self) -> ModuleResolver {
        ModuleResolver::new(&self.root).unwrap()
    }

    fn entry(&self, resolver: &ModuleResolver) -> ModuleUrl {
        resolver.resolve_entry("src/entry.mjs").unwrap()
    }

    fn write(&self, path: &str) -> PathBuf {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "export {};").unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Preserve the original assertion if a test is already unwinding.
        let result = fs::remove_dir_all(&self.base);
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

fn file_url(path: impl AsRef<Path>) -> Url {
    Url::from_file_path(path.as_ref().canonicalize().unwrap()).unwrap()
}

fn resolution_error(
    resolver: &ModuleResolver,
    specifier: &str,
    referrer: &ModuleUrl,
) -> ModuleResolutionError {
    let error = resolver.resolve(specifier, referrer).unwrap_err();
    assert_eq!(error.specifier, specifier);
    assert_eq!(error.referrer.as_deref(), Some(referrer.as_str()));
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains(&format!("{specifier:?}")),
        "{diagnostic}"
    );
    assert!(diagnostic.contains(referrer.as_str()), "{diagnostic}");
    error
}

#[test]
fn relative_absolute_and_localhost_aliases_have_one_file_identity() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    let expected = resolver.resolve_entry("src/dep.mjs").unwrap();
    let absolute = fixture.root.join("src/dep.mjs");
    let absolute_url = file_url(&absolute);
    let localhost_url = absolute_url
        .as_str()
        .replacen("file:///", "file://localhost/", 1);
    let cases = [
        "./dep.mjs".to_owned(),
        "./nested/../dep.mjs".to_owned(),
        "./nested/%2e%2e/dep.mjs".to_owned(),
        "./%64ep.mjs".to_owned(),
        absolute.to_str().unwrap().to_owned(),
        absolute_url.to_string(),
        localhost_url,
    ];

    for specifier in cases {
        let resolved = resolver.resolve(&specifier, &entry).unwrap();
        assert_eq!(resolved, expected, "{specifier}");
        assert_eq!(resolved.cache_key(), expected.cache_key(), "{specifier}");
        assert_eq!(resolved.kind(), ModuleKind::File);
        assert_eq!(
            resolved.to_file_path(),
            Some(absolute.canonicalize().unwrap())
        );
    }
}

#[test]
fn unix_path_layouts_and_unicode_round_trip_without_changing_identity() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    // Both supported host families use absolute POSIX paths. Keep the fixtures
    // under the test root instead of depending on a real user's home directory.
    for relative in [
        "Users/alice/My App/模块 ☃.mjs",
        "home/alice/my-app/模块 ☃.mjs",
        "src/100%.mjs",
        "src/%2f.mjs",
        "src/what?#.mjs",
    ] {
        let path = fixture.write(relative);
        let expected = resolver.resolve_entry(relative).unwrap();
        let encoded = file_url(&path);
        assert_eq!(expected.to_file_path(), Some(path.canonicalize().unwrap()));
        assert_eq!(expected.cache_key(), encoded.as_str());
        assert_eq!(
            resolver.resolve(encoded.as_str(), &entry).unwrap(),
            expected
        );
        // URL percent-escape hex case is not a new file identity.
        let lowercase_escapes = encoded
            .as_str()
            .replace("%E", "%e")
            .replace("%A", "%a")
            .replace("%B", "%b")
            .replace("%C", "%c")
            .replace("%F", "%f");
        assert_eq!(
            resolver.resolve(&lowercase_escapes, &entry).unwrap(),
            expected
        );
    }

    fixture.write("src/模块 ☃.mjs");
    let unicode = resolver.resolve("./模块 ☃.mjs", &entry).unwrap();
    let encoded = resolver
        .resolve("./%E6%A8%A1%E5%9D%97%20%E2%98%83.mjs", &entry)
        .unwrap();
    assert_eq!(unicode, encoded);
    assert_eq!(unicode.cache_key(), encoded.cache_key());
}

#[test]
fn query_and_fragment_are_identity_parts_but_not_filesystem_names() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    let physical_path = fixture.root.join("src/dep.mjs").canonicalize().unwrap();
    let mut identities = HashSet::new();
    for suffix in ["", "?", "#", "?v=1", "?v=2", "#one", "#two", "?v=1#one"] {
        let resolved = resolver
            .resolve(&format!("./dep.mjs{suffix}"), &entry)
            .unwrap();
        assert_eq!(resolved.to_file_path(), Some(physical_path.clone()));
        assert!(
            identities.insert(resolved.cache_key().to_owned()),
            "{suffix:?}"
        );
    }

    let decorated = resolver.resolve("./dep.mjs?v=1#old", &entry).unwrap();
    for (specifier, suffix) in [("?v=2", "?v=2"), ("#new", "?v=1#new"), ("./dep.mjs", "")] {
        let resolved = resolver.resolve(specifier, &decorated).unwrap();
        assert_eq!(
            resolved.cache_key(),
            format!("{}{suffix}", file_url(&physical_path))
        );
    }

    let lower = resolver.resolve("./dep.mjs?q=%2f#%7e", &entry).unwrap();
    let upper = resolver.resolve("./dep.mjs?q=%2F#%7E", &entry).unwrap();
    assert_eq!(lower, upper);
    assert_ne!(
        upper,
        resolver.resolve("./dep.mjs?q=%2F#~", &entry).unwrap()
    );
}

#[test]
fn aliases_and_cyclic_edges_converge_on_the_same_cache_entries() {
    let fixture = Fixture::new();
    symlink("dep.mjs", fixture.root.join("src/dep-alias.mjs")).unwrap();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    let mut cache = HashMap::new();
    cache.insert(entry.cache_key().to_owned(), entry.clone());
    let mut current = entry.clone();

    // Follow resolver edges A -> B -> A repeatedly. This is a cache identity
    // contract, not a claim that JavaScriptCore has linked or evaluated a cycle.
    for _ in 0..16 {
        for specifier in ["./dep-alias.mjs", "./nested/../entry.mjs"] {
            current = resolver.resolve(specifier, &current).unwrap();
            cache
                .entry(current.cache_key().to_owned())
                .or_insert_with(|| current.clone());
        }
        assert_eq!(current, entry);
    }
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.values().cloned().collect::<HashSet<_>>().len(), 2);

    let another_instance = resolver.resolve("./dep-alias.mjs?v=2", &entry).unwrap();
    assert!(
        cache
            .insert(another_instance.cache_key().to_owned(), another_instance)
            .is_none()
    );
    assert_eq!(cache.len(), 3);
}

#[test]
fn builtins_require_registered_names_and_have_stable_non_file_identities() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    for descriptor in BUILTIN_MODULES {
        let module = resolver.resolve(descriptor.specifier, &entry).unwrap();
        assert_eq!(module.kind(), ModuleKind::Builtin);
        assert_eq!(module.cache_key(), descriptor.specifier);
        assert_eq!(module.to_file_path(), None);
        assert_eq!(
            resolver.resolve(descriptor.specifier, &module).unwrap(),
            module
        );
    }
    assert_eq!(
        resolver.resolve("KUNLUN:fs", &entry).unwrap(),
        resolver.resolve("kunlun:fs", &entry).unwrap()
    );
    for specifier in [
        "kunlun:not-registered",
        "kunlun:%66s",
        "kunlun:fs?mode=read",
        "kunlun:fs#alias",
    ] {
        assert!(matches!(
            resolution_error(&resolver, specifier, &entry).kind,
            ModuleResolutionErrorKind::UnknownBuiltin(_)
        ));
    }
}

#[test]
fn generated_registration_canonicalizes_aliases_and_resolves_relative_cycles() {
    let fixture = Fixture::new();
    let mut resolver = fixture.resolver();
    let file_entry = fixture.entry(&resolver);
    let first = resolver
        .register_generated("kunlun-generated:///graph/entry.mjs")
        .unwrap();
    let alias = resolver
        .register_generated("kunlun-generated:///graph/nested/../%65ntry.mjs")
        .unwrap();
    assert_eq!(first, alias);
    assert_eq!(first.kind(), ModuleKind::Generated);
    assert_eq!(first.to_file_path(), None);
    let second = resolver
        .register_generated("kunlun-generated:///graph/模块.mjs")
        .unwrap();
    assert_eq!(
        resolver.resolve(first.as_str(), &file_entry).unwrap(),
        first
    );
    assert_eq!(
        resolver
            .resolve("./%E6%A8%A1%E5%9D%97.mjs", &first)
            .unwrap(),
        second
    );
    assert_eq!(resolver.resolve("./%65ntry.mjs", &second).unwrap(), first);
    let lowercase = resolver
        .register_generated("kunlun-generated:///graph/%e6%a8%a1%e5%9d%97.mjs")
        .unwrap();
    assert_eq!(second, lowercase);
    let identities = [first, alias, second, lowercase]
        .into_iter()
        .map(|module| module.cache_key().to_owned())
        .collect::<HashSet<_>>();
    assert_eq!(identities.len(), 2);
}

#[test]
fn generated_identities_cannot_be_created_by_importing_unregistered_variants() {
    let fixture = Fixture::new();
    let mut resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    let registered = resolver
        .register_generated("kunlun-generated:///graph/entry.mjs")
        .unwrap();
    for specifier in [
        "kunlun-generated:///graph/missing.mjs",
        "kunlun-generated:///graph/%2565ntry.mjs",
        "kunlun-generated:///graph/entry.mjs?mode=other",
        "kunlun-generated:///graph/entry.mjs#other",
        "./missing.mjs",
    ] {
        assert!(matches!(
            resolution_error(&resolver, specifier, &registered).kind,
            ModuleResolutionErrorKind::UnknownGeneratedModule(_)
        ));
    }
    let encoded_query = resolver
        .register_generated("kunlun-generated:///graph/entry.mjs?tag=%61#%7e")
        .unwrap();
    assert_eq!(
        resolver
            .resolve("kunlun-generated:///graph/entry.mjs?tag=%61#%7E", &entry)
            .unwrap(),
        encoded_query
    );
    assert!(matches!(
        resolution_error(
            &resolver,
            "kunlun-generated:///graph/entry.mjs?tag=a#~",
            &entry
        )
        .kind,
        ModuleResolutionErrorKind::UnknownGeneratedModule(_)
    ));
}

#[test]
fn rejects_bare_unsupported_and_malformed_specifiers_with_original_context() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    for specifier in ["package", "@scope/package", "dep.mjs"] {
        assert!(matches!(
            resolution_error(&resolver, specifier, &entry).kind,
            ModuleResolutionErrorKind::UnsupportedBareSpecifier(_)
        ));
    }
    for specifier in [
        "https://example.com/x.mjs",
        "http://example.com/x.mjs",
        "node:fs",
        "data:text/javascript,export{}",
    ] {
        assert!(matches!(
            resolution_error(&resolver, specifier, &entry).kind,
            ModuleResolutionErrorKind::UnsupportedScheme(_)
        ));
    }
    for specifier in [
        "",
        " ./dep.mjs",
        "./dep.mjs ",
        "./dep\n.mjs",
        "./dep\t.mjs",
        "./dep\0.mjs",
        "./dep\u{7f}.mjs",
        "./nested\\dep.mjs",
        "./dep%.mjs",
        "./dep%0.mjs",
        "./dep%GG.mjs",
        "./dep%00.mjs",
        "./dep%0a.mjs",
        "./dep%7F.mjs",
        "./dep.mjs?q=%",
        "./dep.mjs#%00",
        "./nested%2fdep.mjs",
        "./nested%5Cdep.mjs",
        "https://[",
        "kunlun-generated:entry.mjs",
        "kunlun-generated:/entry.mjs",
        "kunlun-generated://host/entry.mjs",
        "kunlun-generated://user:pass@host/entry.mjs",
    ] {
        assert!(
            matches!(
                resolution_error(&resolver, specifier, &entry).kind,
                ModuleResolutionErrorKind::InvalidSpecifier { .. }
            ),
            "{specifier:?}"
        );
    }
}

#[test]
fn refuses_remote_authorities_directories_and_missing_files() {
    let fixture = Fixture::new();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    for specifier in [
        "file://remote.example/dep.mjs",
        "//remote.example/dep.mjs",
        "./nested",
        "./missing.mjs",
    ] {
        assert!(
            matches!(
                resolution_error(&resolver, specifier, &entry).kind,
                ModuleResolutionErrorKind::InvalidFileModule { .. }
            ),
            "{specifier}"
        );
    }
    let builtin = resolver.resolve("kunlun:fs", &entry).unwrap();
    assert!(matches!(
        resolution_error(&resolver, "./dep.mjs", &builtin).kind,
        ModuleResolutionErrorKind::InvalidSpecifier { .. }
    ));
}

#[test]
fn root_boundary_checks_cover_dot_segments_prefix_siblings_and_symlinks() {
    let fixture = Fixture::new();
    let outside = fixture.base.join("project 空间-copy");
    fs::create_dir(&outside).unwrap();
    let secret = outside.join("secret.mjs");
    fs::write(&secret, "export const secret = true;").unwrap();
    symlink(&outside, fixture.root.join("src/outside-directory")).unwrap();
    symlink(&secret, fixture.root.join("src/outside-file.mjs")).unwrap();
    let resolver = fixture.resolver();
    let entry = fixture.entry(&resolver);
    let cases = [
        "../../project 空间-copy/secret.mjs".to_owned(),
        "./%2e%2e/%2e%2e/project 空间-copy/secret.mjs".to_owned(),
        secret.to_str().unwrap().to_owned(),
        file_url(&secret).to_string(),
        "./outside-directory/secret.mjs".to_owned(),
        "./outside-file.mjs".to_owned(),
    ];
    for specifier in cases {
        let error = resolution_error(&resolver, &specifier, &entry);
        match error.kind {
            ModuleResolutionErrorKind::OutsideModuleRoot { path, root } => {
                assert_eq!(path, secret.canonicalize().unwrap());
                assert_eq!(root, fixture.root.canonicalize().unwrap());
            }
            kind => panic!("unexpected error for {specifier:?}: {kind:?}"),
        }
    }

    symlink("loop-b.mjs", fixture.root.join("src/loop-a.mjs")).unwrap();
    symlink("loop-a.mjs", fixture.root.join("src/loop-b.mjs")).unwrap();
    assert!(matches!(
        resolution_error(&resolver, "./loop-a.mjs", &entry).kind,
        ModuleResolutionErrorKind::InvalidFileModule { .. }
    ));
}

#[test]
fn canonical_root_allows_in_root_symlinks_and_symlinked_application_roots() {
    let fixture = Fixture::new();
    let root_alias = fixture.base.join("application-link");
    symlink(&fixture.root, &root_alias).unwrap();
    symlink("dep.mjs", fixture.root.join("src/alias.mjs")).unwrap();
    let resolver = ModuleResolver::new(&root_alias).unwrap();
    assert_eq!(resolver.root(), fixture.root.canonicalize().unwrap());
    let entry = fixture.entry(&resolver);
    let expected = resolver.resolve_entry("src/dep.mjs").unwrap();
    assert_eq!(resolver.resolve("./alias.mjs", &entry).unwrap(), expected);
    assert_eq!(
        resolver
            .resolve_entry(root_alias.join("src/alias.mjs"))
            .unwrap(),
        expected
    );
}

#[test]
fn referrers_must_belong_to_the_current_resolver_policy() {
    let first_fixture = Fixture::new();
    let second_fixture = Fixture::new();
    let mut first = first_fixture.resolver();
    let second = second_fixture.resolver();
    let foreign_file = first_fixture.entry(&first);
    let foreign_generated = first
        .register_generated("kunlun-generated:///private/entry.mjs")
        .unwrap();
    for referrer in [&foreign_file, &foreign_generated] {
        for specifier in ["kunlun:fs", "./dep.mjs"] {
            assert!(matches!(
                resolution_error(&second, specifier, referrer).kind,
                ModuleResolutionErrorKind::InvalidReferrer { .. }
            ));
        }
    }

    // A resolver with the same policy may reuse the canonical identity. The
    // policy boundary is the root and registry, not the Rust object address.
    let mut same_policy = first_fixture.resolver();
    let registered_here = same_policy
        .register_generated(foreign_generated.as_str())
        .unwrap();
    assert_eq!(
        same_policy.resolve("./entry.mjs", &foreign_file).unwrap(),
        foreign_file
    );
    assert_eq!(
        same_policy
            .resolve("./entry.mjs", &foreign_generated)
            .unwrap(),
        registered_here
    );
}

#[test]
fn constructor_entry_and_registration_errors_retain_input_without_a_referrer() {
    let fixture = Fixture::new();
    for path in [
        fixture.root.join("missing"),
        fixture.root.join("src/entry.mjs"),
    ] {
        let error = match ModuleResolver::new(&path) {
            Ok(_) => panic!("accepted invalid root {}", path.display()),
            Err(error) => error,
        };
        assert_eq!(error.specifier, path.to_string_lossy());
        assert_eq!(error.referrer, None);
        assert!(matches!(
            error.kind,
            ModuleResolutionErrorKind::InvalidModuleRoot { .. }
        ));
    }
    let mut resolver = fixture.resolver();
    for path in ["src/missing.mjs", "src/nested"] {
        let error = resolver.resolve_entry(path).unwrap_err();
        assert_eq!(error.specifier, path);
        assert_eq!(error.referrer, None);
        assert!(matches!(
            error.kind,
            ModuleResolutionErrorKind::InvalidFileModule { .. }
        ));
    }
    for specifier in [
        "kunlun-generated:entry.mjs",
        "kunlun-generated://host/entry.mjs",
        "kunlun-generated:///entry%.mjs",
        "https://example.com/entry.mjs",
    ] {
        let error = resolver.register_generated(specifier).unwrap_err();
        assert_eq!(error.specifier, specifier);
        assert_eq!(error.referrer, None);
        assert!(matches!(
            error.kind,
            ModuleResolutionErrorKind::InvalidSpecifier { .. }
                | ModuleResolutionErrorKind::UnsupportedScheme(_)
        ));
    }
}
