use kunlun_jsc::ModuleLoader;
use kunlun_runtime::{
    AdmissionErrorKind, AdmissionPolicy, Capability, HostPermissions, RUNTIME_MANIFEST_SCHEMA,
    RequestContext, TokioIsolate, admit_artifact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
            "chunks/问候% space.mjs",
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
        AdmissionPolicy::new(
            sha(&fs::read(self.0.join("manifest.json")).unwrap()),
            HostPermissions::none().allow_net_host("api.example.test"),
        )
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

fn capability(name: &str, resource: &str) -> Capability {
    Capability {
        name: name.to_owned(),
        resource: resource.to_owned(),
    }
}

#[test]
fn admitted_authority_intersects_grants_and_request_handles_expire() {
    let fixture = Fixture::new();
    let private = Fixture::new();
    let deployment = HostPermissions::none()
        .allow_net_host("api.example.test")
        .allow_net_host("undeclared.example.test")
        .allow_read_root(&private.0)
        .unwrap()
        .bind_read_root("public-data", &fixture.0)
        .unwrap();
    let policy = AdmissionPolicy::new(
        sha(&fs::read(fixture.0.join("manifest.json")).unwrap()),
        deployment,
    );
    let artifact = admit_artifact(&fixture.0, &policy).unwrap();
    let authority = artifact.authority();
    let _isolate = TokioIsolate::new_with_authority("request-scopes", authority).unwrap();
    let api = capability("http.host", "api.example.test");
    let undeclared = capability("http.host", "undeclared.example.test");
    let public = capability("fs.binding", "public-data");
    assert!(authority.contains(&api));
    assert!(authority.contains(&public));
    assert!(!authority.contains(&undeclared));

    let mut a_context = RequestContext::new();
    a_context.insert("auth", "private-A");
    let a = authority.begin_request(a_context).unwrap();
    let mut b_context = RequestContext::new();
    b_context.insert("auth", "private-B");
    let b = authority.begin_request(b_context).unwrap();
    assert_eq!(a.context_value("auth"), Some("private-A"));
    assert_eq!(b.context_value("auth"), Some("private-B"));
    assert!(a.handle(&undeclared).is_none());
    let http = a.handle(&api).unwrap();
    http.authorize_http(&a, "https://api.example.test/v1")
        .unwrap();
    assert!(
        http.authorize_http(&a, "https://other.example.test/")
            .is_err()
    );
    assert!(http.authorize_http(&a, "file:///etc/hosts").is_err());
    assert!(
        http.authorize_http(&b, "https://api.example.test/")
            .is_err()
    );

    let fs_handle = a.handle(&public).unwrap();
    fs_handle
        .authorize_read(&a, Path::new("server.mjs"))
        .unwrap();
    assert!(
        fs_handle
            .authorize_read(&a, &private.0.join("server.mjs"))
            .is_err()
    );
    assert!(
        fs_handle
            .authorize_read(&a, Path::new("../escaped"))
            .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = fixture.0.join("escaped-link");
        symlink(private.0.join("server.mjs"), &link).unwrap();
        assert!(
            fs_handle
                .authorize_read(&a, Path::new("escaped-link"))
                .is_err()
        );
    }
    a.revoke();
    assert!(
        http.authorize_http(&a, "https://api.example.test/")
            .is_err()
    );
    assert_eq!(a.context_value("auth"), None);
    assert!(b.handle(&api).is_some());
    authority.revoke();
    assert!(b.handle(&api).is_none());
}

#[test]
fn optional_binding_is_omitted_and_legacy_read_root_is_not_a_manifest_grant() {
    let fixture = Fixture::new();
    let deployment = HostPermissions::none()
        .allow_net_host("api.example.test")
        .allow_read_root(&fixture.0)
        .unwrap();
    let policy = AdmissionPolicy::new(
        sha(&fs::read(fixture.0.join("manifest.json")).unwrap()),
        deployment,
    );
    let artifact = admit_artifact(&fixture.0, &policy).unwrap();
    let authority = artifact.authority();
    let _isolate = TokioIsolate::new_with_authority("optional-binding", authority).unwrap();
    let optional = capability("fs.binding", "public-data");
    assert!(!authority.contains(&optional));
    let request = authority.begin_request(RequestContext::new()).unwrap();
    assert!(request.handle(&optional).is_none());

    let mut manifest = fixture.manifest();
    manifest["capabilities"]["required"] = json!([
        {"name":"http.host", "resource":"api.example.test"},
        {"name":"fs.binding", "resource":"public-data"}
    ]);
    manifest["capabilities"]["optional"] = json!([]);
    fixture.write_manifest(&manifest);
    assert_eq!(
        fixture.reject(AdmissionErrorKind::Capability),
        "artifact capability at capabilities.required: missing grant for fs.binding:public-data"
    );
}

#[test]
fn admitted_isolate_denies_undeclared_builtin_authority() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let deployment = HostPermissions::none()
        .allow_net_host("api.example.test")
        .allow_net_host("undeclared.example.test")
        .allow_read_root(&outside.0)
        .unwrap();
    let policy = AdmissionPolicy::new(
        sha(&fs::read(fixture.0.join("manifest.json")).unwrap()),
        deployment,
    );
    let artifact = admit_artifact(&fixture.0, &policy).unwrap();
    let mut isolate =
        TokioIsolate::new_with_authority("scoped-artifact", artifact.authority()).unwrap();
    assert!(TokioIsolate::new_with_authority("second-isolate", artifact.authority()).is_err());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(isolate.evaluate_async_body(
            "const http = await kunlun.import('kunlun:http'); return await http.request('http://undeclared.example.test/');",
            "test:///undeclared-http.js",
        ))
        .unwrap_err();
    assert!(
        error.to_string().contains("network access denied"),
        "{error}"
    );

    let path = serde_json::to_string(&outside.0.join("server.mjs").display().to_string()).unwrap();
    let error = runtime
        .block_on(isolate.evaluate_async_body(
            &format!(
                "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile({path});"
            ),
            "test:///undeclared-fs.js",
        ))
        .unwrap_err();
    assert!(error.to_string().contains("read access denied"), "{error}");
    assert!(!error.to_string().contains(&outside.0.display().to_string()));

    let api = capability("http.host", "api.example.test");
    let request = artifact
        .authority()
        .begin_request(RequestContext::new())
        .unwrap();
    let handle = request.handle(&api).unwrap();
    isolate.shutdown_handle().request();
    assert!(
        handle
            .authorize_http(&request, "https://api.example.test/")
            .is_err()
    );
}

#[test]
fn malformed_capability_resource_fails_before_evaluation() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    manifest["capabilities"]["required"][0]["resource"] = json!("api.example.test:8443");
    fixture.write_manifest(&manifest);
    let error = fixture.reject(AdmissionErrorKind::Capability);
    assert!(error.contains("not canonical"));
    assert!(!error.contains("8443"));
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
    let (entry, mut sources, assets, _authority) = artifact.into_parts();
    assert!(
        sources
            .register_generated("kunlun-generated:///extra.mjs", "export default 1")
            .is_err()
    );
    assert!(sources.register_source_map(&entry, "{}").is_err());
    let chunk = sources
        .resolve("./chunks/%E9%97%AE%E5%80%99%25%20space.mjs", Some(&entry))
        .unwrap();
    let expected_source = sources.fetch(&chunk).unwrap();
    fs::write(
        fixture.0.join("chunks/问候% space.mjs"),
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
        fs::remove_file(fixture.0.join("chunks/问候% space.mjs")).unwrap();
        symlink(
            outside.0.join("server.mjs"),
            fixture.0.join("chunks/问候% space.mjs"),
        )
        .unwrap();
        assert!(sources.fetch(&chunk).is_err());
    }
}

#[cfg(unix)]
#[test]
fn question_mark_filename_admits_when_created_at_runtime() {
    let fixture = Fixture::new();
    let question_path = fixture.0.join("chunks/问候% space?.mjs");
    fs::rename(fixture.0.join("chunks/问候% space.mjs"), &question_path).unwrap();

    let entry_path = fixture.0.join("server.mjs");
    let entry = fs::read_to_string(&entry_path).unwrap().replace(
        "%E9%97%AE%E5%80%99%25%20space.mjs",
        "%E9%97%AE%E5%80%99%25%20space%3F.mjs",
    );
    assert!(entry.contains("%E9%97%AE%E5%80%99%25%20space%3F.mjs"));
    fs::write(&entry_path, entry).unwrap();

    let mut manifest = fixture.manifest();
    manifest["files"][0]["sha256"] = json!(sha(&fs::read(&entry_path).unwrap()));
    manifest["files"][1]["url"] = json!("./chunks/%E9%97%AE%E5%80%99%25%20space%3F.mjs");
    manifest["files"][1]["sha256"] = json!(sha(&fs::read(&question_path).unwrap()));
    fixture.write_manifest(&manifest);

    let artifact = admit_artifact(&fixture.0, &fixture.policy()).unwrap();
    let (entry, sources, _, _authority) = artifact.into_parts();
    let chunk = sources
        .resolve(
            "./chunks/%E9%97%AE%E5%80%99%25%20space%3F.mjs",
            Some(&entry),
        )
        .unwrap();
    assert!(sources.fetch(&chunk).unwrap().contains("你好"));
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
    policy = AdmissionPolicy::new(policy.expected_manifest_sha256, HostPermissions::none());
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
