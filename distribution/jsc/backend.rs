//! Offline Cargo backend policy, shared with the engine-free xtask test harness.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const RECEIPT: &str = ".kunlun-jsc-verification.json";
pub const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
];

#[derive(Debug, PartialEq, Eq)]
pub enum Backend {
    Bundled,
    System,
}

pub fn select(bundled: bool, system: bool, target: &str) -> Result<Backend, String> {
    match (bundled, system) {
        (true, true) => Err("bundled-jsc and system-jsc are mutually exclusive; use --no-default-features --features system-jsc for macOS development (do not use --all-features)".into()),
        (false, false) => Err("no JSC backend selected; enable bundled-jsc, or system-jsc for macOS development".into()),
        (true, false) if TARGETS.contains(&target) => Ok(Backend::Bundled),
        (false, true) if TARGETS[..2].contains(&target) => Ok(Backend::System),
        (true, false) => Err(format!("bundled-jsc does not support target {target}; supported targets: {}", TARGETS.join(", "))),
        (false, true) => Err(format!("system-jsc is development-only and supports only macOS arm64/x64, not {target}; use bundled-jsc on supported Linux glibc targets")),
    }
}

pub struct VerifiedDistribution {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub mode: String,
    pub revision: String,
}

pub fn sha256(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn digest_matches(path: &Path, expected: &str) -> Result<(), String> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || sha256(path)? != expected
    {
        return Err(format!("SHA-256 mismatch for {}", path.display()));
    }
    Ok(())
}

fn inventory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(format!(
                "distribution contains a symlink or special file: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            inventory(root, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("distribution path is not UTF-8")?
                .to_owned();
            if relative != RECEIPT {
                files.insert(relative, sha256(&path)?);
            }
        }
    }
    Ok(())
}

/// The caller supplies a digest obtained from its trusted offline verification
/// step, never a digest discovered in the untrusted distribution itself.
/// This detects corruption; it does not authenticate a caller-controlled digest.
pub fn verify(
    directory: &Path,
    receipt_sha256: &str,
    manifest_path: &Path,
    header_path: &Path,
    target: &str,
) -> Result<VerifiedDistribution, String> {
    select(true, false, target)?;
    let root = directory
        .canonicalize()
        .map_err(|e| format!("KUNLUN_JSC_DIST_DIR {}: {e}", directory.display()))?;
    let receipt_path = root.join(RECEIPT);
    if !fs::symlink_metadata(&receipt_path)
        .map_err(|e| {
            format!("verified receipt missing: {e}; run jsc_artifact.py verify --install-dir first")
        })?
        .file_type()
        .is_file()
    {
        return Err("verification receipt must be a regular file".into());
    }
    digest_matches(&receipt_path, receipt_sha256)?;
    let receipt = json(&receipt_path)?;
    if receipt["schema_version"] != 1
        || receipt["target"] != target
        || receipt["native_verified"] != true
    {
        return Err("verification receipt schema, target, or native verification mismatch".into());
    }
    digest_matches(
        manifest_path,
        receipt["manifest_sha256"]
            .as_str()
            .ok_or("missing manifest SHA-256")?,
    )?;
    let mode = receipt["mode"]
        .as_str()
        .ok_or("missing verification mode")?;
    if !["published", "source-build"].contains(&mode) {
        return Err("unsupported verification mode".into());
    }
    let mut actual = BTreeMap::new();
    inventory(&root, &root, &mut actual)?;
    let expected: BTreeMap<String, String> = serde_json::from_value(receipt["files"].clone())
        .map_err(|e| format!("invalid verification file inventory: {e}"))?;
    if actual != expected {
        return Err(
            "distribution file inventory/SHA-256 mismatch; re-run trusted artifact verification"
                .into(),
        );
    }

    let manifest = json(manifest_path)?;
    let entry = manifest["targets"]
        .as_array()
        .ok_or("manifest targets missing")?
        .iter()
        .find(|entry| entry["triple"] == target)
        .ok_or("target absent from pinned manifest")?;
    if (mode == "published" && entry["artifact"]["status"] != "published")
        || (mode == "source-build" && entry["artifact"]["status"] != "planned")
    {
        return Err("verification mode does not match pinned artifact status".into());
    }
    let build = json(&root.join("metadata/build.json"))?;
    for field in ["distribution", "source", "build", "abi"] {
        if build[field] != manifest[field] {
            return Err(format!(
                "distribution {field} metadata does not match pinned manifest"
            ));
        }
    }
    for field in ["triple", "os", "arch", "libc", "deployment_target"] {
        if build["target"][field] != entry[field] {
            return Err(format!("distribution target {field} mismatch for {target}"));
        }
    }
    if build["toolchain_id"] != entry["toolchain"] {
        return Err("distribution toolchain mismatch".into());
    }
    for library in entry["artifact"]["library_paths"]
        .as_array()
        .ok_or("missing library paths")?
    {
        let relative = library.as_str().ok_or("invalid library path")?;
        if !actual.contains_key(relative) {
            return Err(format!("required distribution library missing: {relative}"));
        }
    }
    digest_matches(&root.join("include/kunlun_jsc.h"), &sha256(header_path)?)?;
    if target.ends_with("linux-gnu") {
        let runtime = json(&root.join("metadata/runtime-dependencies.json"))?;
        if runtime["target"] != target {
            return Err("Linux runtime dependency target mismatch".into());
        }
        digest_matches(
            &root.join("metadata/runtime-dependencies.json"),
            build["runtime_dependencies"]["sha256"]
                .as_str()
                .ok_or("missing runtime dependency digest")?,
        )?;
    }
    let revision = manifest["source"]["revision"]
        .as_str()
        .ok_or("missing pinned engine revision")?
        .to_owned();
    let mut files: Vec<_> = actual.keys().map(|path| root.join(path)).collect();
    files.push(receipt_path);
    Ok(VerifiedDistribution {
        root,
        files,
        mode: mode.to_owned(),
        revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture {
        root: PathBuf,
        manifest: PathBuf,
        header: PathBuf,
        target: String,
    }

    impl Fixture {
        fn new(target: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "kunlun-backend-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let manifest = root.join("manifest.json");
            let header = root.join("header.h");
            let value: Value = serde_json::from_str(include_str!("manifest.json")).unwrap();
            let entry = value["targets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["triple"] == target)
                .unwrap();
            let mut metadata = json!({
                "distribution": value["distribution"], "source": value["source"],
                "build": value["build"], "abi": value["abi"],
                "toolchain_id": entry["toolchain"],
                "target": {
                    "triple": target, "os": entry["os"], "arch": entry["arch"],
                    "libc": entry["libc"], "deployment_target": entry["deployment_target"]
                }
            });
            let staging = root.join("staging");
            for dir in ["metadata", "include", "lib"] {
                fs::create_dir_all(staging.join(dir)).unwrap();
            }
            fs::write(&header, b"fixture header").unwrap();
            fs::write(staging.join("include/kunlun_jsc.h"), b"fixture header").unwrap();
            for library in entry["artifact"]["library_paths"].as_array().unwrap() {
                fs::write(staging.join(library.as_str().unwrap()), b"fixture library").unwrap();
            }
            if target.ends_with("linux-gnu") {
                let runtime = staging.join("metadata/runtime-dependencies.json");
                fs::write(&runtime, json!({"target": target}).to_string()).unwrap();
                metadata["runtime_dependencies"] = json!({"sha256": sha256(&runtime).unwrap()});
            }
            fs::write(&manifest, value.to_string()).unwrap();
            fs::write(staging.join("metadata/build.json"), metadata.to_string()).unwrap();
            let fixture = Self {
                root,
                manifest,
                header,
                target: target.into(),
            };
            fixture.receipt();
            fixture
        }

        fn staging(&self) -> PathBuf {
            self.root.join("staging")
        }

        fn receipt(&self) -> String {
            let mut files = BTreeMap::new();
            inventory(&self.staging(), &self.staging(), &mut files).unwrap();
            let receipt = json!({
                "schema_version": 1, "target": self.target, "mode": "source-build",
                "native_verified": true, "manifest_sha256": sha256(&self.manifest).unwrap(),
                "files": files
            });
            let path = self.staging().join(RECEIPT);
            fs::write(&path, receipt.to_string()).unwrap();
            sha256(&path).unwrap()
        }

        fn verify(&self, digest: &str) -> Result<VerifiedDistribution, String> {
            verify(
                &self.staging(),
                digest,
                &self.manifest,
                &self.header,
                &self.target,
            )
        }

        fn mutate_build(&self, pointer: &str, value: Value) {
            let path = self.staging().join("metadata/build.json");
            let mut metadata = json(&path).unwrap();
            *metadata.pointer_mut(pointer).unwrap() = value;
            fs::write(path, metadata.to_string()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn feature_selection_is_exclusive_and_target_specific() {
        for target in TARGETS {
            assert_eq!(select(true, false, target).unwrap(), Backend::Bundled);
            assert_eq!(
                select(false, true, target).is_ok(),
                target.ends_with("apple-darwin")
            );
            assert!(
                select(false, false, target)
                    .unwrap_err()
                    .contains("no JSC backend")
            );
            assert!(
                select(true, true, target)
                    .unwrap_err()
                    .contains("mutually exclusive")
            );
        }
        for target in [
            "x86_64-unknown-linux-musl",
            "i686-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
            "aarch64-apple-ios",
        ] {
            assert!(select(true, false, target).is_err());
            assert!(select(false, true, target).is_err());
        }
    }

    #[test]
    fn accepts_verified_files_for_all_four_targets() {
        for target in TARGETS {
            let fixture = Fixture::new(target);
            let verified = fixture.verify(&fixture.receipt()).unwrap();
            assert_eq!(verified.root, fixture.staging().canonicalize().unwrap());
            assert_eq!(verified.mode, "source-build");
            assert_eq!(verified.revision.len(), 40);
            assert!(
                verified
                    .files
                    .contains(&fixture.staging().join(RECEIPT).canonicalize().unwrap())
            );
        }
    }

    #[test]
    fn rejects_corruption_deletions_additions_and_stale_receipts() {
        for operation in ["modify", "delete", "add", "receipt"] {
            let fixture = Fixture::new(TARGETS[0]);
            let digest = fixture.receipt();
            let library = fixture.staging().join("lib/libJavaScriptCore.dylib");
            match operation {
                "modify" => fs::write(library, b"corruption").unwrap(),
                "delete" => fs::remove_file(library).unwrap(),
                "add" => fs::write(fixture.staging().join("lib/extra.dylib"), b"extra").unwrap(),
                _ => fs::write(fixture.staging().join(RECEIPT), b"{}").unwrap(),
            }
            assert!(fixture.verify(&digest).is_err(), "accepted {operation}");
        }
    }

    #[test]
    fn rejects_metadata_mismatch_even_with_refreshed_inventory() {
        for (pointer, value) in [
            ("/source/revision", json!("0".repeat(40))),
            ("/abi/shim_version", json!(999)),
            ("/build/configuration", json!("Debug")),
            ("/target/triple", json!(TARGETS[1])),
            ("/target/os", json!("linux")),
            ("/target/deployment_target/minimum", json!("99.0")),
            ("/toolchain_id", json!("unknown")),
        ] {
            let fixture = Fixture::new(TARGETS[0]);
            fixture.mutate_build(pointer, value);
            assert!(
                fixture.verify(&fixture.receipt()).is_err(),
                "accepted {pointer}"
            );
        }
    }

    #[test]
    fn rejects_missing_trust_and_changed_manifest_or_header() {
        let fixture = Fixture::new(TARGETS[0]);
        let digest = fixture.receipt();
        assert!(fixture.verify("").is_err());
        assert!(fixture.verify(&"0".repeat(64)).is_err());
        fs::write(&fixture.header, b"new ABI header").unwrap();
        assert!(fixture.verify(&digest).is_err());
        fs::write(&fixture.header, b"fixture header").unwrap();
        fs::write(&fixture.manifest, b"{}").unwrap();
        assert!(fixture.verify(&digest).is_err());
    }

    #[test]
    fn rejects_linux_runtime_dependency_mismatch() {
        let fixture = Fixture::new(TARGETS[2]);
        fs::write(
            fixture.staging().join("metadata/runtime-dependencies.json"),
            json!({"target": TARGETS[3]}).to_string(),
        )
        .unwrap();
        assert!(fixture.verify(&fixture.receipt()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_including_receipt() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new(TARGETS[0]);
        let digest = fixture.receipt();
        let library = fixture.staging().join("lib/libJavaScriptCore.dylib");
        fs::remove_file(&library).unwrap();
        symlink(&fixture.header, &library).unwrap();
        assert!(fixture.verify(&digest).is_err());
        fs::remove_file(library).unwrap();
        let path = fixture.staging().join(RECEIPT);
        let moved = fixture.root.join("receipt.json");
        fs::rename(&path, &moved).unwrap();
        symlink(moved, path).unwrap();
        assert!(fixture.verify(&digest).is_err());
    }
}
