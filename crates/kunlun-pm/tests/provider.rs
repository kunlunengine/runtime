use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(unix)]
use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Project(PathBuf);

impl Project {
    fn fixture() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kunlun-pm-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/package-manager-v1/workspace"),
            &path,
        );
        Self(path)
    }

    fn set(&self, path: &str, value: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn replace(&self, path: &str, from: &str, to: &str) {
        let bytes = fs::read_to_string(self.0.join(path)).unwrap();
        assert!(bytes.contains(from));
        self.set(path, &bytes.replace(from, to));
    }

    fn manifest(&self, field: &str, value: Value) {
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(self.0.join("package.json")).unwrap()).unwrap();
        manifest[field] = value;
        self.set("package.json", &manifest.to_string());
    }

    fn lockfile(&self, edit: impl FnOnce(&mut Value)) {
        let mut value: Value =
            serde_yaml_ng::from_slice(&fs::read(self.0.join("pnpm-lock.yaml")).unwrap()).unwrap();
        edit(&mut value);
        self.set("pnpm-lock.yaml", &serde_yaml_ng::to_string(&value).unwrap());
    }

    fn run(&self, operation: &str, flags: &[&str]) -> (Output, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kunlun-pm"));
        command.arg(operation);
        for flag in flags {
            command.arg(flag);
        }
        command.arg("--project").arg(&self.0).arg("--json");
        command
            .env_clear()
            .env("PATH", "")
            .env("HOME", &self.0)
            .current_dir(&self.0);
        let output = command.output().unwrap();
        assert!(output.stderr.is_empty());
        assert_eq!(
            output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
            1
        );
        assert_eq!(output.stdout.last(), Some(&b'\n'));
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["schema"], "kunlun.package-manager-provider/v1");
        assert_eq!(reply["provider"], "kunlun-pm");
        assert_eq!(reply["requiresNode"], false);
        assert_eq!(reply["exitStatus"], output.status.code().unwrap());
        (output, reply)
    }

    fn error(&self, code: &str) {
        let before = inventory(&self.0);
        let (output, reply) = self.run("plan", &["--frozen", "--ignore-scripts"]);
        assert_eq!(output.status.code(), Some(1), "{reply}");
        assert_eq!(reply["status"], "error");
        assert_eq!(reply["diagnostics"][0]["code"], code, "{reply}");
        assert!(reply.get("result").is_none());
        assert_eq!(before, inventory(&self.0));
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("TOP_SECRET")
        );
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        // Only the uniquely owned test directory created above is removed.
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn copy(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir(&dest).unwrap();
            copy(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn inventory(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, result: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let name = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                result.push((name, Vec::new()));
                walk(root, &entry.path(), result);
            } else if kind.is_symlink() {
                result.push((name, b"symlink".to_vec()));
            } else {
                result.push((name, fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut result = Vec::new();
    walk(root, root, &mut result);
    result.sort();
    result
}

fn conformance() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/package-manager-v1/conformance.json"
    ))
    .unwrap()
}

#[test]
fn stable_node_free_plan_is_read_only_and_honest_about_scripts_and_evidence() {
    let project = Project::fixture();
    project.set("node_modules/PRIOR_TREE", "must remain unchanged");
    let before = inventory(&project.0);
    let (first, reply) = project.run("plan", &["--frozen", "--ignore-scripts"]);
    assert!(first.status.success(), "{reply}");
    let (second, _) = project.run("plan", &["--ignore-scripts"]);
    assert_eq!(first.stdout, second.stdout);
    let copied = Project::fixture();
    let (other, _) = copied.run("plan", &["--frozen", "--ignore-scripts"]);
    assert_eq!(first.stdout, other.stdout); // No machine paths, timestamps or installed-tree state.
    let result = &reply["result"];
    assert_eq!(result["frozen"], true);
    assert_eq!(result["readOnly"], true);
    assert_eq!(result["defaultScriptPolicy"], "deny");
    assert_eq!(result["readiness"], "not-assessed");
    assert_eq!(result["evidence"], "not-verified");
    assert_eq!(result["changedManifestPaths"], json!([]));
    assert_eq!(
        result["unbuiltPackages"],
        conformance()["knownUnbuiltImporters"]
    );
    assert!(
        result["scriptDecisions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|decision| decision["decision"] == "denied")
    );
    assert!(
        result["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "ui@1.0.0(react@18.0.0)")
    );
    let platform = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "platform")
        .unwrap();
    assert_eq!(platform["conditions"]["os"], json!(["darwin", "linux"]));
    assert_eq!(before, inventory(&project.0));
    let (_, defaults) = project.run("plan", &[]);
    assert_eq!(defaults["result"]["ignoreScripts"], false);
    assert!(
        defaults["result"]["scriptDecisions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|decision| decision["reason"] == "default-deny")
    );
}

#[test]
fn why_covers_transitive_workspace_peer_and_cyclic_graphs() {
    let project = Project::fixture();
    let (_, why) = project.run("why", &["leaf"]);
    let paths = why["result"]["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 4);
    assert_eq!(why["result"]["paths"], conformance()["whyLeafPaths"]);
    assert!(
        paths
            .iter()
            .any(|path| path["nodes"] == json!(["workspace:packages/shared", "leaf@1.1.0"]))
    );
    let (_, peer) = project.run("why", &["react"]);
    assert_eq!(
        peer["result"]["paths"][0]["nodes"],
        json!(["ui@1.0.0(react@18.0.0)", "react@18.0.0"])
    );
    let (_, cycle) = project.run("why", &["loop-b"]);
    assert_eq!(
        cycle["result"]["paths"][0]["nodes"],
        json!([
            "tool@1.0.0(extra@1.0.0(inner@1.0.0))",
            "loop-a@1.0.0",
            "loop-b@1.0.0"
        ])
    );
    let (output, absent) = project.run("why", &["not-installed"]);
    assert!(output.status.success());
    assert_eq!(absent["result"]["paths"], json!([]));
}

#[test]
fn detect_and_version_do_not_claim_installation_capabilities() {
    let project = Project::fixture();
    let (_, detect) = project.run("detect", &[]);
    assert_eq!(
        detect["result"]["workspaceImporters"],
        conformance()["workspaceImporters"]
    );
    let output = Command::new(env!("CARGO_BIN_EXE_kunlun-pm"))
        .args(["version", "--json"])
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    let version: Value = serde_json::from_slice(&output.stdout).unwrap();
    for capability in [
        "resolve",
        "fetch",
        "install",
        "mutate",
        "prune",
        "exec",
        "lifecycleScripts",
    ] {
        assert_eq!(version["result"]["capabilities"][capability], false);
    }
}

#[test]
fn invalid_grammar_is_redacted_and_never_starts_an_operation() {
    let project = Project::fixture();
    for args in [
        vec!["plan", "--json", "--json"],
        vec!["install", "--json", "TOP_SECRET"],
        vec!["plan", "--json", "--allow-scripts", "TOP_SECRET"],
        vec!["plan", "--json", "--project"],
        vec!["why", "--json"],
        vec!["version", "--json", "TOP_SECRET"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kunlun-pm"))
            .args(args)
            .current_dir(&project.0)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["diagnostics"][0]["code"], "invalid_arguments");
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("TOP_SECRET")
        );
    }
}

#[test]
fn unsupported_schema_missing_and_ambiguous_authority_fail_closed() {
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "'9.0'", "'6.0'");
    project.error("unsupported_schema");
    let project = Project::fixture();
    fs::remove_file(project.0.join("pnpm-lock.yaml")).unwrap();
    project.error("frozen_drift");
    for path in [
        "kunlun.lock",
        "kunlun.lockb",
        "yarn.lock",
        "package-lock.json",
        "bun.lock",
    ] {
        let project = Project::fixture();
        project.set(path, "TOP_SECRET");
        project.error("ambiguous_authority");
    }
}

#[test]
fn manifest_workspace_and_source_drift_are_rejected() {
    let project = Project::fixture();
    project.manifest(
        "dependencies",
        json!({"ui":"^2.0.0","@fixture/shared":"workspace:*"}),
    );
    project.error("frozen_drift");
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "specifier: ^1.0.0", "specifier: ^2.0.0");
    project.error("frozen_drift");
    let project = Project::fixture();
    project.set(
        "packages/new/package.json",
        r#"{"name":"new","version":"1.0.0"}"#,
    );
    project.error("frozen_drift");
    let project = Project::fixture();
    project.replace(
        "pnpm-lock.yaml",
        "version: link:../shared",
        "version: link:../../TOP_SECRET",
    );
    project.error("graph_invalid");
    let project = Project::fixture();
    project.replace(
        "pnpm-lock.yaml",
        "version: 1.0.0(react@18.0.0)",
        "version: https://user:TOP_SECRET@example.test/pkg.tgz",
    );
    project.error("unsupported_feature");
}

#[test]
fn executable_configuration_and_self_authorized_grants_are_rejected() {
    for path in [
        ".pnpmfile.cjs",
        ".pnpmfile.mjs",
        ".npmrc",
        "kunlun.config.ts",
        "packages/app/.npmrc",
    ] {
        let project = Project::fixture();
        project.set(path, "TOP_SECRET; writeFile('PROJECT_CODE_EXECUTED')");
        project.error("unsupported_configuration");
    }
    for field in [
        "pnpm",
        "overrides",
        "workspaces",
        "dependenciesMeta",
        "devEngines",
    ] {
        let project = Project::fixture();
        project.manifest(field, json!({"TOP_SECRET":"arbitrary"}));
        project.error("unsupported_configuration");
    }
    let project = Project::fixture();
    project.set("kunlun-pm-policy.json", r#"{"schema":"kunlun.package-manager-policy/v1","lifecycleScripts":"allow","trusted":true,"secret":"TOP_SECRET"}"#);
    project.error("policy_denied");
    let project = Project::fixture();
    project.set("packages/app/kunlun-pm-policy.json", r#"{"schema":"kunlun.package-manager-policy/v1","lifecycleScripts":"deny","approvedPackages":["TOP_SECRET"]}"#);
    project.error("policy_denied");
}

#[test]
fn incomplete_integrity_unknown_semantics_and_duplicate_keys_are_rejected() {
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "leaf: 1.1.0", "leaf: 1.9.0");
    project.error("graph_invalid");
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "sha512-AAAA", "sha1-TOP_SECRET");
    project.error("graph_invalid");
    let project = Project::fixture();
    project.replace(
        "pnpm-lock.yaml",
        "excludeLinksFromLockfile: false",
        "excludeLinksFromLockfile: false\n  injectedSetting: TOP_SECRET",
    );
    project.error("unsupported_feature");
    let project = Project::fixture();
    project.replace(
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'",
        "lockfileVersion: '9.0'\nlockfileVersion: '6.0'",
    );
    project.error("invalid_lockfile");
    let project = Project::fixture();
    project.set(
        "package.json",
        r#"{"name":"test","packageManager":"pnpm@10.15.0","packageManager":"TOP_SECRET"}"#,
    );
    project.error("invalid_manifest");
}

#[test]
fn peer_identity_must_agree_with_resolved_edges_and_have_a_snapshot() {
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "(react@18.0.0)", "(react@17.0.0)");
    project.error("graph_invalid");
    let project = Project::fixture();
    project.replace("pnpm-lock.yaml", "(react@18.0.0)", "");
    project.error("graph_invalid");
    let project = Project::fixture();
    project.replace(
        "pnpm-lock.yaml",
        "(react@18.0.0)",
        "(react@18.0.0)(react@18.0.0)",
    );
    project.error("graph_invalid");
    let project = Project::fixture();
    project.lockfile(|lock| {
        let leaf = lock["snapshots"]
            .as_object_mut()
            .unwrap()
            .remove("leaf@1.1.0")
            .unwrap();
        lock["snapshots"]["leaf@1.1.0(react@18.0.0)"] = leaf;
        lock["importers"]["packages/shared"]["dependencies"]["leaf"]["version"] =
            json!("1.1.0(react@18.0.0)");
        lock["snapshots"]["ui@1.0.0(react@18.0.0)"]["dependencies"]["leaf"] =
            json!("1.1.0(react@18.0.0)");
    });
    project.error("graph_invalid"); // A syntactically valid but extraneous context is not a peer.
}

#[test]
fn unreachable_snapshots_are_not_a_successful_plan() {
    let project = Project::fixture();
    project.lockfile(|lock| {
        lock["packages"]["orphan@1.0.0"] = lock["packages"]["leaf@1.1.0"].clone();
        lock["snapshots"]["orphan@1.0.0"] = json!({});
    });
    project.error("graph_invalid");
}

#[test]
fn why_retains_deeper_occurrences_of_the_same_package_name() {
    let project = Project::fixture();
    project.lockfile(|lock| {
        lock["packages"]["tool@2.0.0"] = lock["packages"]["leaf@1.1.0"].clone();
        lock["snapshots"]["tool@2.0.0"] = json!({});
        lock["snapshots"]["tool@1.0.0(extra@1.0.0(inner@1.0.0))"]["dependencies"]["tool"] =
            json!("2.0.0");
    });
    let (_, response) = project.run("why", &["tool"]);
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["result"]["paths"],
        json!([
            {"importer": ".", "nodes": ["tool@1.0.0(extra@1.0.0(inner@1.0.0))"]},
            {"importer": ".", "nodes": ["tool@1.0.0(extra@1.0.0(inner@1.0.0))", "tool@2.0.0"]}
        ])
    );
}

#[test]
fn root_dev_preinstall_hook_and_implicit_native_builds_are_denied() {
    let project = Project::fixture();
    project.manifest("scripts", json!({"pnpm:devPreinstall":"TOP_SECRET"}));
    project.set("packages/shared/binding.gyp", "{}");
    let (_, response) = project.run("plan", &[]);
    assert_eq!(response["status"], "ok");
    assert!(
        response["result"]["scriptDecisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| {
                decision["importer"] == "."
                    && decision["hook"] == "pnpm:devPreinstall"
                    && decision["decision"] == "denied"
            })
    );
    assert!(
        response["result"]["scriptDecisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| {
                decision["importer"] == "packages/shared" && decision["hook"] == "implicit-native"
            })
    );
    assert_eq!(
        response["result"]["unbuiltPackages"],
        json!([".", "packages/app", "packages/shared"])
    );
    assert!(!response.to_string().contains("TOP_SECRET"));
}

#[cfg(unix)]
#[test]
fn named_pipe_input_is_rejected_without_waiting_for_a_writer() {
    let project = Project::fixture();
    fs::remove_file(project.0.join("package.json")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(project.0.join("package.json"))
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_kunlun-pm"))
        .args(["plan", "--json"])
        .arg("--project")
        .arg(&project.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("special-file input blocked the provider");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(response["diagnostics"][0]["code"], "project_unreadable");
}

#[test]
fn static_input_limits_and_unsupported_workspace_patterns_fail_closed() {
    let project = Project::fixture();
    project.set("package.json", &" ".repeat(8 * 1024 * 1024 + 1));
    project.error("limit_exceeded");
    let project = Project::fixture();
    project.set("pnpm-workspace.yaml", "packages: ['packages/**']");
    project.error("unsupported_configuration");
}

#[cfg(unix)]
#[test]
fn static_inputs_may_not_escape_through_symlinks() {
    use std::os::unix::fs::symlink;
    let project = Project::fixture();
    fs::remove_file(project.0.join("package.json")).unwrap();
    symlink("/etc/passwd", project.0.join("package.json")).unwrap();
    project.error("project_unreadable");
}

#[test]
fn shared_negative_fixtures_agree_with_real_process_exits() {
    for case in conformance()["negativeCases"].as_array().unwrap() {
        let project = Project::fixture();
        let file = case["file"].as_str().unwrap();
        if let Some(write) = case["write"].as_str() {
            project.set(file, write);
        } else {
            let replace = case["replace"].as_array().unwrap();
            project.replace(
                file,
                replace[0].as_str().unwrap(),
                replace[1].as_str().unwrap(),
            );
        }
        project.error(case["code"].as_str().unwrap());
    }
}
