use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path};

const SCHEMA_LOCATION: &str = "./manifest.schema.json";
const CANONICAL_REPOSITORY: &str = "https://github.com/WebKit/WebKit.git";
const SBOM_FORMAT: &str = "SPDX-2.3-json";
const PROVENANCE_FORMAT: &str = "SLSA-provenance-v1";
const REQUIRED_TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];
const REQUIRED_FEATURE_FLAGS: [&str; 12] = [
    "ENABLE_DFG_JIT",
    "ENABLE_FTL_JIT",
    "ENABLE_JSC_GLIB_API",
    "ENABLE_JIT",
    "ENABLE_REMOTE_INSPECTOR",
    "ENABLE_SAMPLING_PROFILER",
    "ENABLE_STATIC_JSC",
    "ENABLE_WEBASSEMBLY",
    "ENABLE_WEBASSEMBLY_BBQJIT",
    "ENABLE_WEBASSEMBLY_OMGJIT",
    "EVENT_LOOP_TYPE",
    "USE_LIBBACKTRACE",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(rename = "$schema")]
    schema: String,
    schema_version: u64,
    distribution: String,
    source: Source,
    build: Build,
    toolchains: Vec<Toolchain>,
    targets: Vec<Target>,
    patches: Vec<Patch>,
    licenses: Vec<License>,
    abi: Abi,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    repository: String,
    revision: String,
    commit_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    configuration: String,
    driver: String,
    arguments: BTreeMap<String, Vec<String>>,
    environment: BTreeMap<String, String>,
    feature_flags: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Toolchain {
    id: String,
    host: String,
    container_image: Nullable<String>,
    package_snapshot: Nullable<String>,
    tools: Vec<Tool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    triple: String,
    os: String,
    arch: String,
    libc: Nullable<String>,
    toolchain: String,
    deployment_target: DeploymentTarget,
    artifact: Artifact,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeploymentTarget {
    kind: String,
    minimum: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    status: ArtifactStatus,
    archive_path: String,
    sha256: Nullable<String>,
    library_paths: Vec<String>,
    sbom: Evidence,
    provenance: Evidence,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ArtifactStatus {
    Planned,
    Published,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    format: String,
    path: String,
    sha256: Nullable<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    path: String,
    purpose: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct License {
    component: String,
    spdx_expression: String,
    source: InputSource,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputSource {
    kind: InputKind,
    path: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum InputKind {
    Local,
    Upstream,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Abi {
    shim_version: u64,
    public_headers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Nullable<T>(Option<T>);

pub fn validate_file(manifest_path: &Path, repository_root: &Path) -> Result<(), Vec<String>> {
    let source = std::fs::read_to_string(manifest_path).map_err(|error| {
        vec![format!(
            "could not read {}: {error}",
            manifest_path.display()
        )]
    })?;
    validate_json(&source, repository_root)
}

fn validate_json(source: &str, repository_root: &Path) -> Result<(), Vec<String>> {
    let manifest: Manifest = serde_json::from_str(source)
        .map_err(|error| vec![format!("manifest does not match the v1 structure: {error}")])?;
    let mut errors = Vec::new();

    require_equal("$schema", &manifest.schema, SCHEMA_LOCATION, &mut errors);
    if manifest.schema_version != 1 {
        errors.push(format!(
            "schema_version must be 1, found {}",
            manifest.schema_version
        ));
    }
    require_nonempty("distribution", &manifest.distribution, &mut errors);
    validate_source(&manifest.source, &mut errors);
    validate_build(&manifest.build, &mut errors);
    let toolchains = validate_toolchains(&manifest.toolchains, &mut errors);
    validate_targets(&manifest.targets, &toolchains, &mut errors);
    validate_patches(&manifest.patches, repository_root, &mut errors);
    validate_licenses(&manifest.licenses, repository_root, &mut errors);
    validate_abi(&manifest.abi, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_source(source: &Source, errors: &mut Vec<String>) {
    require_equal(
        "source.repository",
        &source.repository,
        CANONICAL_REPOSITORY,
        errors,
    );
    if !is_full_revision(&source.revision) {
        errors.push(
            "source.revision must be a full 40-character lowercase hexadecimal commit SHA".into(),
        );
    }
    let expected_url = format!(
        "https://github.com/WebKit/WebKit/commit/{}",
        source.revision
    );
    require_equal(
        "source.commit_url",
        &source.commit_url,
        &expected_url,
        errors,
    );
}

fn validate_build(build: &Build, errors: &mut Vec<String>) {
    require_nonempty("build.configuration", &build.configuration, errors);
    validate_relative_path("build.driver", &build.driver, errors);
    for host in ["macos", "linux"] {
        match build.arguments.get(host) {
            Some(arguments) if arguments.is_empty() => {
                errors.push(format!("build.arguments.{host} must not be empty"));
            }
            Some(arguments) => {
                for (index, argument) in arguments.iter().enumerate() {
                    require_nonempty(
                        &format!("build.arguments.{host}[{index}]"),
                        argument,
                        errors,
                    );
                }
            }
            None => errors.push(format!("build.arguments is missing {host}")),
        }
    }
    for host in build.arguments.keys() {
        if !["macos", "linux"].contains(&host.as_str()) {
            errors.push(format!("build.arguments has unsupported host {host:?}"));
        }
    }
    if build.environment.is_empty() {
        errors.push("build.environment must not be empty".into());
    }
    for (name, value) in &build.environment {
        require_nonempty("build.environment key", name, errors);
        require_nonempty(&format!("build.environment.{name}"), value, errors);
    }
    for flag in REQUIRED_FEATURE_FLAGS {
        if !build.feature_flags.contains_key(flag) {
            errors.push(format!("build.feature_flags is missing {flag}"));
        }
    }
    for (name, value) in &build.feature_flags {
        require_nonempty("build.feature_flags key", name, errors);
        if !value.is_boolean() && !value.is_string() && !value.is_number() {
            errors.push(format!(
                "build.feature_flags.{name} must be a boolean, string, or number"
            ));
        }
    }
}

fn validate_toolchains<'a>(
    toolchains: &'a [Toolchain],
    errors: &mut Vec<String>,
) -> HashMap<&'a str, &'a str> {
    let mut result = HashMap::new();
    for (index, toolchain) in toolchains.iter().enumerate() {
        require_nonempty(&format!("toolchains[{index}].id"), &toolchain.id, errors);
        require_nonempty(
            &format!("toolchains[{index}].host"),
            &toolchain.host,
            errors,
        );
        match (
            toolchain.host.as_str(),
            &toolchain.container_image.0,
            &toolchain.package_snapshot.0,
        ) {
            ("macos", None, None) => {}
            ("macos", _, _) => errors.push(format!(
                "macOS toolchain {} must use pinned Xcode without a container or package snapshot",
                toolchain.id
            )),
            ("linux", Some(image), Some(snapshot)) => {
                validate_oci_image(&toolchain.id, image, errors);
                validate_package_snapshot(&toolchain.id, snapshot, errors);
            }
            ("linux", None, _) => errors.push(format!(
                "Linux toolchain {} must pin container_image by OCI digest",
                toolchain.id
            )),
            ("linux", _, None) => errors.push(format!(
                "Linux toolchain {} must pin package_snapshot",
                toolchain.id
            )),
            (host, _, _) => errors.push(format!(
                "toolchain {} has unsupported host {host:?}",
                toolchain.id
            )),
        }
        if result
            .insert(toolchain.id.as_str(), toolchain.host.as_str())
            .is_some()
        {
            errors.push(format!("duplicate toolchain id: {}", toolchain.id));
        }
        if toolchain.tools.is_empty() {
            errors.push(format!("toolchain {} has no pinned tools", toolchain.id));
        }
        let mut names = HashSet::new();
        for tool in &toolchain.tools {
            require_nonempty("tool name", &tool.name, errors);
            require_nonempty(
                &format!("toolchain {} tool {} version", toolchain.id, tool.name),
                &tool.version,
                errors,
            );
            if ["latest", "stable", "system", "tbd", "unpinned"]
                .contains(&tool.version.to_ascii_lowercase().as_str())
            {
                errors.push(format!(
                    "toolchain {} tool {} must use an exact version, found {:?}",
                    toolchain.id, tool.name, tool.version
                ));
            }
            if !names.insert(tool.name.as_str()) {
                errors.push(format!(
                    "toolchain {} contains duplicate tool {}",
                    toolchain.id, tool.name
                ));
            }
        }
    }
    result
}

fn validate_targets(
    targets: &[Target],
    toolchains: &HashMap<&str, &str>,
    errors: &mut Vec<String>,
) {
    let mut triples = BTreeSet::new();
    let mut archive_paths = HashSet::new();
    let mut sbom_paths = HashSet::new();
    let mut provenance_paths = HashSet::new();
    for target in targets {
        if !triples.insert(target.triple.as_str()) {
            errors.push(format!("duplicate target triple: {}", target.triple));
        }
        validate_target_metadata(target, errors);
        match toolchains.get(target.toolchain.as_str()) {
            Some(host) if *host != target.os => errors.push(format!(
                "target {} uses {} toolchain {}",
                target.triple, host, target.toolchain
            )),
            Some(_) => {}
            None => errors.push(format!(
                "target {} references unknown toolchain {}",
                target.triple, target.toolchain
            )),
        }
        if !archive_paths.insert(target.artifact.archive_path.as_str()) {
            errors.push(format!(
                "duplicate artifact archive path: {}",
                target.artifact.archive_path
            ));
        }
        if !sbom_paths.insert(target.artifact.sbom.path.as_str()) {
            errors.push(format!(
                "duplicate artifact SBOM path: {}",
                target.artifact.sbom.path
            ));
        }
        if !provenance_paths.insert(target.artifact.provenance.path.as_str()) {
            errors.push(format!(
                "duplicate artifact provenance path: {}",
                target.artifact.provenance.path
            ));
        }
        validate_artifact(&target.triple, &target.artifact, errors);
    }

    let required = REQUIRED_TARGETS.into_iter().collect::<BTreeSet<_>>();
    for missing in required.difference(&triples) {
        errors.push(format!("missing required target triple: {missing}"));
    }
    for unexpected in triples.difference(&required) {
        errors.push(format!(
            "unsupported target triple in v1 manifest: {unexpected}"
        ));
    }
}

fn validate_target_metadata(target: &Target, errors: &mut Vec<String>) {
    let (os, arch, libc, deployment_kind) = match target.triple.as_str() {
        "aarch64-apple-darwin" => ("macos", "arm64", None, "macos"),
        "x86_64-apple-darwin" => ("macos", "x64", None, "macos"),
        "aarch64-unknown-linux-gnu" => ("linux", "arm64", Some("glibc"), "glibc"),
        "x86_64-unknown-linux-gnu" => ("linux", "x64", Some("glibc"), "glibc"),
        _ => return,
    };
    require_equal("target.os", &target.os, os, errors);
    require_equal("target.arch", &target.arch, arch, errors);
    if target.libc.0.as_deref() != libc {
        errors.push(format!("target {} libc must be {:?}", target.triple, libc));
    }
    require_equal(
        "target.deployment_target.kind",
        &target.deployment_target.kind,
        deployment_kind,
        errors,
    );
    require_nonempty(
        "target.deployment_target.minimum",
        &target.deployment_target.minimum,
        errors,
    );
}

fn validate_artifact(target: &str, artifact: &Artifact, errors: &mut Vec<String>) {
    validate_relative_path("artifact.archive_path", &artifact.archive_path, errors);
    if artifact.library_paths.is_empty() {
        errors.push(format!("target {target} artifact has no library paths"));
    }
    let mut libraries = HashSet::new();
    for path in &artifact.library_paths {
        validate_relative_path("artifact.library_paths entry", path, errors);
        if !libraries.insert(path) {
            errors.push(format!(
                "target {target} contains duplicate library path {path}"
            ));
        }
    }
    validate_evidence("sbom", SBOM_FORMAT, &artifact.sbom, errors);
    validate_evidence(
        "provenance",
        PROVENANCE_FORMAT,
        &artifact.provenance,
        errors,
    );

    match artifact.status {
        ArtifactStatus::Planned => {
            require_absent_digest(target, "artifact.sha256", &artifact.sha256.0, errors);
            require_absent_digest(
                target,
                "artifact.sbom.sha256",
                &artifact.sbom.sha256.0,
                errors,
            );
            require_absent_digest(
                target,
                "artifact.provenance.sha256",
                &artifact.provenance.sha256.0,
                errors,
            );
        }
        ArtifactStatus::Published => {
            require_digest(target, "artifact.sha256", &artifact.sha256.0, errors);
            require_digest(
                target,
                "artifact.sbom.sha256",
                &artifact.sbom.sha256.0,
                errors,
            );
            require_digest(
                target,
                "artifact.provenance.sha256",
                &artifact.provenance.sha256.0,
                errors,
            );
        }
    }
}

fn validate_evidence(
    label: &str,
    expected_format: &str,
    evidence: &Evidence,
    errors: &mut Vec<String>,
) {
    require_equal(
        &format!("artifact.{label}.format"),
        &evidence.format,
        expected_format,
        errors,
    );
    validate_relative_path(&format!("artifact.{label}.path"), &evidence.path, errors);
}

fn validate_patches(patches: &[Patch], repository_root: &Path, errors: &mut Vec<String>) {
    let mut paths = HashSet::new();
    let mut digests = HashSet::new();
    for (index, patch) in patches.iter().enumerate() {
        let label = format!("patches[{index}]");
        validate_relative_path(&format!("{label}.path"), &patch.path, errors);
        require_nonempty(&format!("{label}.purpose"), &patch.purpose, errors);
        validate_digest(&format!("{label}.sha256"), &patch.sha256, errors);
        if !paths.insert(patch.path.as_str()) {
            errors.push(format!("duplicate patch path: {}", patch.path));
        }
        if !digests.insert(patch.sha256.as_str()) {
            errors.push(format!("duplicate patch digest: {}", patch.sha256));
        }
        verify_local_digest(&label, &patch.path, &patch.sha256, repository_root, errors);
    }
}

fn validate_licenses(licenses: &[License], repository_root: &Path, errors: &mut Vec<String>) {
    if licenses.is_empty() {
        errors.push("licenses must not be empty".into());
    }
    let mut components = HashSet::new();
    let mut sources = HashSet::new();
    for (index, license) in licenses.iter().enumerate() {
        let label = format!("licenses[{index}]");
        require_nonempty(&format!("{label}.component"), &license.component, errors);
        require_nonempty(
            &format!("{label}.spdx_expression"),
            &license.spdx_expression,
            errors,
        );
        validate_relative_path(
            &format!("{label}.source.path"),
            &license.source.path,
            errors,
        );
        validate_digest(&format!("{label}.sha256"), &license.sha256, errors);
        if !components.insert(license.component.as_str()) {
            errors.push(format!(
                "duplicate license component: {}",
                license.component
            ));
        }
        let source_key = (
            format!("{:?}", license.source.kind),
            license.source.path.as_str(),
        );
        if !sources.insert(source_key) {
            errors.push(format!("duplicate license source: {}", license.source.path));
        }
        if license.source.kind == InputKind::Local {
            verify_local_digest(
                &label,
                &license.source.path,
                &license.sha256,
                repository_root,
                errors,
            );
        }
    }
}

fn validate_abi(abi: &Abi, errors: &mut Vec<String>) {
    if abi.shim_version == 0 {
        errors.push("abi.shim_version must be greater than zero".into());
    }
    if abi.public_headers.is_empty() {
        errors.push("abi.public_headers must not be empty".into());
    }
    let mut headers = HashSet::new();
    for header in &abi.public_headers {
        validate_relative_path("abi.public_headers entry", header, errors);
        if !headers.insert(header) {
            errors.push(format!("duplicate public header: {header}"));
        }
    }
}

fn verify_local_digest(
    label: &str,
    relative_path: &str,
    expected: &str,
    repository_root: &Path,
    errors: &mut Vec<String>,
) {
    let path = repository_root.join(relative_path);
    match sha256_file(&path) {
        Ok(actual) if actual != expected => errors.push(format!(
            "{label} digest mismatch for {relative_path}: expected {expected}, computed {actual}"
        )),
        Ok(_) => {}
        Err(error) => errors.push(format!(
            "{label} could not read local input {relative_path}: {error}"
        )),
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn is_full_revision(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_digest(label: &str, value: &str, errors: &mut Vec<String>) {
    if !is_digest(value) {
        errors.push(format!(
            "{label} must be a 64-character lowercase hexadecimal SHA-256 digest"
        ));
    }
}

fn require_digest(target: &str, label: &str, value: &Option<String>, errors: &mut Vec<String>) {
    match value {
        Some(value) => validate_digest(&format!("target {target} {label}"), value, errors),
        None => errors.push(format!("target {target} is published but {label} is null")),
    }
}

fn require_absent_digest(
    target: &str,
    label: &str,
    value: &Option<String>,
    errors: &mut Vec<String>,
) {
    if value.is_some() {
        errors.push(format!(
            "target {target} is planned but {label} is populated"
        ));
    }
}

fn validate_relative_path(label: &str, value: &str, errors: &mut Vec<String>) {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        errors.push(format!(
            "{label} must be a non-empty repository-relative path without '..': {value:?}"
        ));
    }
}

fn validate_oci_image(toolchain: &str, image: &str, errors: &mut Vec<String>) {
    match image.rsplit_once("@sha256:") {
        Some((name, digest)) if !name.is_empty() => {
            validate_digest(
                &format!("toolchain {toolchain} container_image"),
                digest,
                errors,
            );
        }
        _ => errors.push(format!(
            "toolchain {toolchain} container_image must be an OCI name pinned by sha256 digest"
        )),
    }
}

fn validate_package_snapshot(toolchain: &str, snapshot: &str, errors: &mut Vec<String>) {
    let bytes = snapshot.as_bytes();
    let valid = bytes.len() == 16
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z';
    if !valid {
        errors.push(format!(
            "toolchain {toolchain} package_snapshot must use YYYYMMDDTHHMMSSZ"
        ));
    }
}

fn require_nonempty(label: &str, value: &str, errors: &mut Vec<String>) {
    if value.trim().is_empty() {
        errors.push(format!("{label} must not be empty"));
    }
}

fn require_equal(label: &str, actual: &str, expected: &str, errors: &mut Vec<String>) {
    if actual != expected {
        errors.push(format!("{label} must be {expected:?}, found {actual:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repository_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask must be below the repository root")
    }

    fn manifest_value() -> Value {
        let path = repository_root().join("distribution/jsc/manifest.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("manifest should be readable"))
            .expect("manifest should be JSON")
    }

    fn errors_for(value: Value) -> Vec<String> {
        let source = serde_json::to_string(&value).expect("test manifest should serialize");
        validate_json(&source, repository_root()).expect_err("test manifest should be rejected")
    }

    #[test]
    fn checked_in_manifest_is_valid() {
        let source = serde_json::to_string(&manifest_value()).unwrap();
        assert_eq!(validate_json(&source, repository_root()), Ok(()));
    }

    #[test]
    fn rejects_missing_fields() {
        let mut value = manifest_value();
        value.as_object_mut().unwrap().remove("build");
        assert!(errors_for(value)[0].contains("missing field `build`"));
    }

    #[test]
    fn rejects_missing_nullable_fields() {
        let mut value = manifest_value();
        value["targets"][0]["artifact"]
            .as_object_mut()
            .unwrap()
            .remove("sha256");
        assert!(errors_for(value)[0].contains("missing field `sha256`"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut value = manifest_value();
        value["unexpected"] = json!(true);
        assert!(errors_for(value)[0].contains("unknown field `unexpected`"));
    }

    #[test]
    fn rejects_abbreviated_revisions() {
        let mut value = manifest_value();
        value["source"]["revision"] = json!("4b62d53e");
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("full 40-character"))
        );
    }

    #[test]
    fn rejects_duplicate_patches() {
        let mut value = manifest_value();
        let patch = json!({
            "path": "LICENSE",
            "purpose": "test fixture",
            "sha256": "25dd5c0c1fcdb9005335f5e9f538a73b9a3c82eaa8420c4e505fe821bfe5e14f"
        });
        value["patches"] = json!([patch.clone(), patch]);
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate patch path"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate patch digest"))
        );
    }

    #[test]
    fn rejects_duplicate_evidence_paths_across_targets() {
        let mut value = manifest_value();
        value["targets"][1]["artifact"]["sbom"]["path"] =
            value["targets"][0]["artifact"]["sbom"]["path"].clone();
        value["targets"][1]["artifact"]["provenance"]["path"] =
            value["targets"][0]["artifact"]["provenance"]["path"].clone();
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate artifact SBOM path"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate artifact provenance path"))
        );
    }

    #[test]
    fn rejects_unsupported_evidence_formats() {
        let mut value = manifest_value();
        value["targets"][0]["artifact"]["sbom"]["format"] = json!("CycloneDX");
        value["targets"][0]["artifact"]["provenance"]["format"] = json!("custom");
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("artifact.sbom.format must be \"SPDX-2.3-json\""))
        );
        assert!(errors.iter().any(|error| {
            error.contains("artifact.provenance.format must be \"SLSA-provenance-v1\"")
        }));
    }

    #[test]
    fn rejects_malformed_digests() {
        let mut value = manifest_value();
        value["licenses"][0]["sha256"] = json!("sha256:not-a-digest");
        let errors = errors_for(value);
        assert!(errors.iter().any(|error| error.contains("64-character")));
    }

    #[test]
    fn verifies_local_input_digests() {
        let mut value = manifest_value();
        value["licenses"][0]["sha256"] =
            json!("0000000000000000000000000000000000000000000000000000000000000000");
        let errors = errors_for(value);
        assert!(errors.iter().any(|error| error.contains("digest mismatch")));
    }

    #[test]
    fn published_artifacts_require_all_integrity_records() {
        let mut value = manifest_value();
        value["targets"][0]["artifact"]["status"] = json!("published");
        let errors = errors_for(value);
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.contains("is published") && error.contains("is null"))
                .count(),
            3
        );
    }

    #[test]
    fn requires_all_supported_target_triples() {
        let mut value = manifest_value();
        value["targets"].as_array_mut().unwrap().pop();
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("missing required target triple"))
        );
    }

    #[test]
    fn linux_toolchains_require_a_pinned_package_snapshot() {
        let mut value = manifest_value();
        value["toolchains"][1]["package_snapshot"] = json!("latest");
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("package_snapshot must use YYYYMMDDTHHMMSSZ"))
        );
    }
}
