//! Actual process evidence for Core's provider boundary, not native server qualification.
use kunlun_runtime_protocol::{MAX_RESPONSE_BYTES, PROVIDER_SCHEMA};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

// Reviewed fixture metadata, not a production auto-pin computed from untrusted input.
const FIXTURE_PIN: &str = "8426a39ef4bb82e80863c0ff7f3ba0c648c238888ddbc9c0a37131921a356a95";

fn artifact() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1")
}

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args(args)
        .env("KUNLUN_TEST_PROVIDER_PRIVATE", "PRIVATE_SENTINEL")
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    assert!(output.stdout.len() <= MAX_RESPONSE_BYTES);
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(!stdout.contains("PRIVATE_SENTINEL"));
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let value: Value = serde_json::from_str(stdout).unwrap();
    assert_eq!(value["schema"], PROVIDER_SCHEMA);
    value
}

#[test]
fn version_is_an_honest_provider_handshake() {
    let output = invoke(&["version", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "version");
    assert_eq!(value["status"], "ok");
    let report = &value["result"];
    assert_eq!(report["executable"], "kunlun-runtime");
    assert_eq!(report["runtime_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["engine_abi"], kunlun_runtime::RUNTIME_ENGINE_ABI);
    assert_eq!(
        report["artifact_manifest_schema"],
        kunlun_runtime::RUNTIME_MANIFEST_SCHEMA
    );
    assert_eq!(report["capabilities"]["artifactAdmission"], true);
    for capability in ["applicationExecution", "httpServe", "inspectorTransport"] {
        assert_eq!(report["capabilities"][capability], false);
    }
    assert_eq!(
        report["capabilities"]["nativeModules"],
        kunlun_jsc::JscVm::backend_info().supports_native_modules
    );
}

#[test]
fn doctor_json_preserves_smoke_tests_without_human_stdout() {
    let output = invoke(&["doctor", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "doctor");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["result"]["smoke_tests"]["synchronous"], true);
    assert_eq!(value["result"]["smoke_tests"]["async_timer"], true);
    assert!(value["result"]["smoke_tests"]["temporal"].is_boolean());
}

#[derive(Deserialize)]
struct ErrorFixture {
    arguments: Vec<String>,
    exit_code: i32,
    operation: String,
    code: String,
    admission_kind: Option<String>,
}

#[test]
fn portable_error_fixtures_match_actual_exit_and_json_diagnostics() {
    let fixtures: Vec<ErrorFixture> = serde_json::from_str(include_str!(
        "../../../fixtures/runtime-provider-v0.2/errors.json"
    ))
    .unwrap();
    let root = artifact();
    for fixture in fixtures {
        let args: Vec<_> = fixture
            .arguments
            .iter()
            .map(|arg| {
                if arg == "$artifact" {
                    root.to_str().unwrap()
                } else {
                    arg
                }
            })
            .collect();
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(fixture.exit_code));
        let value = json(&output);
        assert_eq!(value["operation"], fixture.operation);
        assert_eq!(value["status"], "error");
        assert!(value.get("result").is_none());
        let diagnostics = value["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], fixture.code);
        assert!(diagnostics[0]["message"].as_str().unwrap().len() > 5);
        assert!(diagnostics[0]["remediation"].as_str().unwrap().len() > 5);
        assert_eq!(
            diagnostics[0]["admission_kind"].as_str(),
            fixture.admission_kind.as_deref()
        );
        let wire = String::from_utf8(output.stdout).unwrap();
        assert!(!wire.contains("api.example.test"));
        assert!(!wire.contains(root.to_str().unwrap()));
    }
}

#[test]
fn check_artifact_reuses_admission_without_running_application() {
    let root = artifact();
    let output = invoke(&[
        "check-artifact",
        root.to_str().unwrap(),
        "--manifest-sha256",
        FIXTURE_PIN,
        "--allow-net",
        "api.example.test",
        "--json",
    ]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "check-artifact");
    assert_eq!(value["result"]["application_evaluated"], false);
    assert_eq!(value["result"]["indexed_files"], 4);
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("api.example.test")
    );
}

#[test]
fn named_read_binding_grants_the_matching_artifact_capability_only() {
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "kunlun-provider-binding-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&fixture.0).unwrap();
    let original = artifact();
    for relative in [
        "server.mjs",
        "chunks/问候% space.mjs",
        "maps/server.mjs.map",
        "assets/help/欢迎 组件.svg",
    ] {
        let destination = fixture.0.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(original.join(relative), destination).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(original.join("manifest.json")).unwrap()).unwrap();
    manifest["capabilities"]["required"] = serde_json::json!([
        {"name": "fs.binding", "resource": "public-data"}
    ]);
    manifest["capabilities"]["optional"] = serde_json::json!([]);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    // Test caller supplies trusted metadata for its own newly authored fixture.
    let pin: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::write(fixture.0.join("manifest.json"), bytes).unwrap();
    let directory = fixture.0.to_str().unwrap();
    for (binding, expected_exit) in [("other-data", 1), ("public-data", 0)] {
        let output = invoke(&[
            "check-artifact",
            directory,
            "--manifest-sha256",
            &pin,
            "--bind-read",
            binding,
            original.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(expected_exit));
        let value = json(&output);
        if expected_exit == 0 {
            assert_eq!(value["result"]["required_capabilities"], 1);
            assert_eq!(value["result"]["application_evaluated"], false);
        } else {
            assert_eq!(value["diagnostics"][0]["admission_kind"], "capability");
        }
        let wire = String::from_utf8(output.stdout).unwrap();
        assert!(!wire.contains(directory));
        assert!(!wire.contains(binding));
        assert!(!wire.contains(original.to_str().unwrap()));
    }
}

#[test]
fn developer_commands_do_not_silently_accept_extra_arguments() {
    for args in [
        vec!["eval", "'ok'", "--inspect"],
        vec!["run", "does-not-exist.js", "--inspect"],
        vec!["types", "--json"],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let output = invoke(&["version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("kunlun-runtime {}\n", env!("CARGO_PKG_VERSION"))
    );
}
