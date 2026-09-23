use kunlun_jsc::ModuleLoader;
use kunlun_runtime::{
    AdmissionErrorKind, AdmissionPolicy, Capability, RUNTIME_MANIFEST_SCHEMA, admit_artifact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
type Mutation = (fn(&mut Value), AdmissionErrorKind);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-artifact-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1");
        for path in [
            "manifest.json",
            "server.mjs",
            "chunks/问候% space?.mjs",
            "maps/server.mjs.map",
            "assets/help/欢迎 组件.svg",
        ] {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(source.join(path), destination).unwrap();
        }
        Self(root)
    }

    fn policy(&self) -> AdmissionPolicy {
        AdmissionPolicy {
            expected_manifest_sha256: sha(&fs::read(self.0.join("manifest.json")).unwrap()),
            supported_capabilities: ["http.host".to_owned(), "fs.binding".to_owned()].into(),
            granted_capabilities: [Capability {
                name: "http.host".to_owned(),
                resource: "api.example.test".to_owned(),
            }]
            .into(),
        }
    }

    fn manifest(&self) -> Value {
        serde_json::from_slice(&fs::read(self.0.join("manifest.json")).unwrap()).unwrap()
    }

    fn write_manifest(&self, value: &Value) {
        fs::write(
            self.0.join("manifest.json"),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }

    fn reject(&self, kind: AdmissionErrorKind) -> String {
        let error = match admit_artifact(&self.0, &self.policy()) {
            Ok(_) => panic!("expected rejection"),
            Err(error) => error,
        };
        assert_eq!(error.kind, kind, "{error}");
        error.to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn portable_fixture_admits_unicode_escaped_paths_subpath_assets_and_snapshots() {
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy()).unwrap();
    assert_eq!(artifact.manifest().schema, RUNTIME_MANIFEST_SCHEMA);
    assert!(artifact.entry_url().ends_with("/server.mjs"));
    let asset_url = "./assets/help/%E6%AC%A2%E8%BF%8E%20%E7%BB%84%E4%BB%B6.svg";
    let expected_asset = artifact.asset_bytes(asset_url).unwrap().to_vec();
    assert!(
        std::str::from_utf8(&expected_asset)
            .unwrap()
            .contains("欢迎")
    );
    let (entry, mut sources, assets) = artifact.into_parts();
    assert!(
        sources
            .register_generated("kunlun-generated:///extra.mjs", "export default 1")
            .is_err()
    );
    assert!(sources.register_source_map(&entry, "{}").is_err());
    let chunk = sources
        .resolve(
            "./chunks/%E9%97%AE%E5%80%99%25%20space%3F.mjs",
            Some(&entry),
        )
        .unwrap();
    let expected_source = sources.fetch(&chunk).unwrap();
    fs::write(
        fixture.0.join("chunks/问候% space?.mjs"),
        "export const greeting = 'changed';",
    )
    .unwrap();
    fs::write(fixture.0.join("assets/help/欢迎 组件.svg"), "changed").unwrap();
    assert_eq!(sources.fetch(&chunk).unwrap(), expected_source);
    assert_eq!(assets[asset_url], expected_asset);
    fs::write(
        fixture.0.join("chunks/unlisted.mjs"),
        "export default 'unlisted';",
    )
    .unwrap();
    let unlisted = sources.resolve("./unlisted.mjs", Some(&chunk)).unwrap();
    assert!(
        sources
            .fetch(&unlisted)
            .unwrap_err()
            .contains("undeclared module source")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = Fixture::new();
        fs::remove_file(fixture.0.join("chunks/问候% space?.mjs")).unwrap();
        symlink(
            outside.0.join("server.mjs"),
            fixture.0.join("chunks/问候% space?.mjs"),
        )
        .unwrap();
        assert!(sources.fetch(&chunk).is_err());
    }
}

#[test]
fn tampering_missing_files_and_manifest_digest_fail_before_admission() {
    let fixture = Fixture::new();
    let policy = fixture.policy();
    fs::write(fixture.0.join("server.mjs"), "tampered").unwrap();
    let error = match admit_artifact(&fixture.0, &policy) {
        Ok(_) => panic!(),
        Err(error) => error,
    };
    assert_eq!(error.kind, AdmissionErrorKind::Integrity);
    assert_eq!(error.location, "./server.mjs");

    let fixture = Fixture::new();
    fs::remove_file(fixture.0.join("maps/server.mjs.map")).unwrap();
    assert!(
        fixture
            .reject(AdmissionErrorKind::MissingFile)
            .contains("./maps/server.mjs.map")
    );

    let fixture = Fixture::new();
    let mut policy = fixture.policy();
    policy.expected_manifest_sha256 = "0".repeat(64);
    let error = match admit_artifact(&fixture.0, &policy) {
        Ok(_) => panic!(),
        Err(error) => error,
    };
    assert_eq!(error.kind, AdmissionErrorKind::Integrity);
    assert_eq!(error.location, "manifest.json");
}

#[test]
fn schema_engine_unknown_fields_features_and_grants_fail_closed() {
    let mutations: [Mutation; 7] = [
        (
            |m: &mut Value| m["schema"] = json!("kunlun.runtime-manifest/v2"),
            AdmissionErrorKind::Schema,
        ),
        (
            |m: &mut Value| m["engine"]["abi"] = json!(2),
            AdmissionErrorKind::Compatibility,
        ),
        (
            |m: &mut Value| m["engine"]["runtime_profile"] = json!("other/1"),
            AdmissionErrorKind::Compatibility,
        ),
        (
            |m: &mut Value| m["entry_contract"] = json!("kunlun.fetch-entry/v2"),
            AdmissionErrorKind::Compatibility,
        ),
        (
            |m: &mut Value| m["required_features"] = json!(["future-api"]),
            AdmissionErrorKind::Compatibility,
        ),
        (
            |m: &mut Value| m["compatibility_flags"] = json!(["unknown"]),
            AdmissionErrorKind::Compatibility,
        ),
        (
            |m: &mut Value| m["handler"] = json!("source text"),
            AdmissionErrorKind::Manifest,
        ),
    ];
    for (mutator, kind) in mutations {
        let fixture = Fixture::new();
        let mut manifest = fixture.manifest();
        mutator(&mut manifest);
        fixture.write_manifest(&manifest);
        fixture.reject(kind);
    }
    let fixture = Fixture::new();
    let mut policy = fixture.policy();
    policy.granted_capabilities = BTreeSet::new();
    let error = match admit_artifact(&fixture.0, &policy) {
        Ok(_) => panic!(),
        Err(error) => error,
    };
    assert_eq!(error.kind, AdmissionErrorKind::Capability);
    assert_eq!(error.location, "capabilities.required");

    let fixture = Fixture::new();
    fs::write(fixture.0.join("manifest.json"), b"{").unwrap();
    let first = fixture.reject(AdmissionErrorKind::Manifest);
    let second = fixture.reject(AdmissionErrorKind::Manifest);
    assert_eq!(first, second);
    assert!(!first.contains(&fixture.0.to_string_lossy().to_string()));
}

#[test]
fn aliases_traversal_invalid_maps_and_conflicting_identities_fail_closed() {
    let mutations: [Mutation; 6] = [
        (
            |m| m["entry"] = json!("./%73erver.mjs"),
            AdmissionErrorKind::Path,
        ),
        (
            |m| m["entry"] = json!("../server.mjs"),
            AdmissionErrorKind::Path,
        ),
        (
            |m| m["entry"] = json!("./%2e%2e/server.mjs"),
            AdmissionErrorKind::Path,
        ),
        (
            |m| {
                let duplicate = m["files"][0].clone();
                m["files"].as_array_mut().unwrap().push(duplicate);
            },
            AdmissionErrorKind::Identity,
        ),
        (
            |m| m["files"][2]["for"] = json!("./chunks/unlisted.mjs"),
            AdmissionErrorKind::MissingFile,
        ),
        (
            |m| m["files"][2]["for"] = json!("./server.mjs?x=1"),
            AdmissionErrorKind::Path,
        ),
    ];
    for (mutator, kind) in mutations {
        let fixture = Fixture::new();
        let mut manifest = fixture.manifest();
        mutator(&mut manifest);
        fixture.write_manifest(&manifest);
        fixture.reject(kind);
    }
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    fs::write(fixture.0.join("maps/server.mjs.map"), "not json").unwrap();
    manifest["files"][2]["sha256"] = json!(sha(b"not json"));
    fixture.write_manifest(&manifest);
    fixture.reject(AdmissionErrorKind::SourceMap);
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    fs::remove_file(fixture.0.join("server.mjs")).unwrap();
    symlink(outside.0.join("server.mjs"), fixture.0.join("server.mjs")).unwrap();
    fixture.reject(AdmissionErrorKind::Path);
}
