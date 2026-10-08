use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    input::{self, Inputs},
    protocol::{Diagnostic, ErrorCode, POLICY_SCHEMA, Result},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub package_manager: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

fn default_version() -> String {
    "0.0.0".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Workspace {
    packages: Vec<String>,
}

pub fn package_name(name: &str) -> bool {
    fn part(value: &str) -> bool {
        !value.is_empty()
            && !value.starts_with('.')
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    }
    if name.len() > 214 {
        return false;
    }
    if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, name)| part(scope) && part(name))
    } else {
        part(name)
    }
}

pub fn exact_pnpm(pin: &str) -> bool {
    let Some(version) = pin.strip_prefix("pnpm@") else {
        return false;
    };
    // Corepack integrity suffixes remain part of the exact executable identity.
    let (version, integrity) = version.split_once('+').unwrap_or((version, ""));
    let valid_integrity = !pin.contains('+')
        || integrity
            .strip_prefix("sha512.")
            .is_some_and(|hash| hash.len() == 128 && hash.bytes().all(|c| c.is_ascii_hexdigit()));
    Version::parse(version).is_ok_and(|v| v.to_string() == version && v.pre.is_empty())
        && valid_integrity
}

pub fn discover(inputs: &mut Inputs) -> Result<(String, BTreeMap<String, Manifest>)> {
    for authority in [
        "kunlun.lock",
        "kunlun.lockb",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
    ] {
        if inputs.exists(authority)? {
            return Err(Diagnostic::new(ErrorCode::AmbiguousAuthority));
        }
    }
    let mut paths = BTreeSet::from([".".to_string()]);
    if inputs.exists("pnpm-workspace.yaml")? {
        let value = input::yaml(
            &inputs.read("pnpm-workspace.yaml")?,
            ErrorCode::UnsupportedConfiguration,
        )?;
        let workspace: Workspace = serde_json::from_value(value)
            .map_err(|_| Diagnostic::new(ErrorCode::UnsupportedConfiguration))?;
        let patterns: BTreeSet<_> = workspace.packages.into_iter().collect();
        if patterns.len() > 512 {
            return Err(Diagnostic::new(ErrorCode::LimitExceeded));
        }
        let mut exclusions = Vec::new();
        for pattern in patterns {
            if let Some(exclusion) = pattern.strip_prefix('!') {
                validate_pattern(exclusion)?;
                exclusions.push(exclusion.to_string());
            } else {
                validate_pattern(&pattern)?;
                for path in expand(inputs, &pattern)? {
                    if inputs.exists(&format!("{path}/package.json"))? {
                        paths.insert(path);
                    }
                }
            }
        }
        paths.retain(|path| {
            path == "."
                || !exclusions
                    .iter()
                    .any(|pattern| matches_pattern(path, pattern))
        });
    }
    if paths.len() > 512 {
        return Err(Diagnostic::new(ErrorCode::LimitExceeded));
    }
    let mut manifests = BTreeMap::new();
    let mut names = BTreeSet::new();
    for path in paths {
        let prefix = if path == "." {
            String::new()
        } else {
            format!("{path}/")
        };
        for executable in [
            ".pnpmfile.cjs",
            ".pnpmfile.mjs",
            ".npmrc",
            "kunlun.config.ts",
            "kunlun.config.js",
            "kunlun.config.mjs",
        ] {
            if inputs.exists(&format!("{prefix}{executable}"))? {
                return Err(Diagnostic::new(ErrorCode::UnsupportedConfiguration));
            }
        }
        if inputs.exists(&format!("{prefix}kunlun-pm-policy.json"))? {
            policy(input::json(
                &inputs.read(&format!("{prefix}kunlun-pm-policy.json"))?,
                ErrorCode::PolicyDenied,
            )?)?;
        }
        let value = input::json(
            &inputs.read(&format!("{prefix}package.json"))?,
            ErrorCode::InvalidManifest,
        )?;
        let object = value
            .as_object()
            .ok_or_else(|| Diagnostic::new(ErrorCode::InvalidManifest))?;
        // Metadata may be ignored; these fields change graph or execution semantics.
        for field in [
            "pnpm",
            "overrides",
            "resolutions",
            "workspaces",
            "dependenciesMeta",
            "peerDependencies",
            "peerDependenciesMeta",
            "bundledDependencies",
            "bundleDependencies",
            "devEngines",
            "config",
        ] {
            if object.contains_key(field) {
                return Err(Diagnostic::new(ErrorCode::UnsupportedConfiguration));
            }
        }
        let manifest: Manifest = serde_json::from_value(value)
            .map_err(|_| Diagnostic::new(ErrorCode::InvalidManifest))?;
        if !package_name(&manifest.name)
            || Version::parse(&manifest.version).is_err()
            || !names.insert(manifest.name.clone())
            || manifest
                .dependencies
                .keys()
                .chain(manifest.dev_dependencies.keys())
                .chain(manifest.optional_dependencies.keys())
                .any(|name| !package_name(name))
        {
            return Err(Diagnostic::new(ErrorCode::InvalidManifest));
        }
        manifests.insert(path, manifest);
    }
    let pin = manifests["."]
        .package_manager
        .as_deref()
        .filter(|pin| exact_pnpm(pin))
        .ok_or_else(|| Diagnostic::new(ErrorCode::InvalidManifest))?
        .to_string();
    if manifests.values().any(|manifest| {
        manifest
            .package_manager
            .as_ref()
            .is_some_and(|other| other != &pin)
    }) {
        return Err(Diagnostic::new(ErrorCode::FrozenDrift));
    }
    Ok((pin, manifests))
}

fn policy(value: Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| Diagnostic::new(ErrorCode::PolicyDenied))?;
    if object.get("schema").and_then(Value::as_str) != Some(POLICY_SCHEMA) {
        return Err(Diagnostic::new(ErrorCode::UnsupportedSchema));
    }
    if object.len() != 2 || object.get("lifecycleScripts").and_then(Value::as_str) != Some("deny") {
        return Err(Diagnostic::new(ErrorCode::PolicyDenied));
    }
    Ok(())
}

fn validate_pattern(pattern: &str) -> Result<()> {
    if pattern == "." {
        return Ok(());
    }
    let parts: Vec<_> = pattern.split('/').collect();
    if parts.is_empty()
        || parts.iter().enumerate().any(|(index, part)| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || (*part == "*" && index + 1 != parts.len())
                || (*part != "*"
                    && !part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)))
        })
    {
        return Err(Diagnostic::new(ErrorCode::UnsupportedConfiguration));
    }
    Ok(())
}

fn matches_pattern(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        path.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
    } else {
        path == pattern
    }
}

fn expand(inputs: &Inputs, pattern: &str) -> Result<Vec<String>> {
    let Some(prefix) = pattern.strip_suffix('*') else {
        return Ok(if pattern == "." {
            Vec::new()
        } else {
            vec![pattern.into()]
        });
    };
    let parent = prefix.trim_end_matches('/');
    let parent = if parent.is_empty() { "." } else { parent };
    if parent != "." && !inputs.exists(parent)? {
        return Ok(Vec::new());
    }
    let entries = inputs
        .root
        .read_dir(parent)
        .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
    let mut paths = Vec::new();
    for (count, entry) in entries.enumerate() {
        if count >= 10_000 {
            return Err(Diagnostic::new(ErrorCode::LimitExceeded));
        }
        let entry = entry.map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
        let kind = entry
            .file_type()
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
        if kind.is_symlink() {
            return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
        }
        if !kind.is_dir() || entry.file_name() == "node_modules" || entry.file_name() == ".git" {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Diagnostic::new(ErrorCode::UnsupportedConfiguration))?;
        let path = format!("{prefix}{name}");
        validate_pattern(&path)?;
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_pin_is_exact_and_suffix_is_not_optional_after_plus() {
        assert!(exact_pnpm("pnpm@10.15.0"));
        assert!(exact_pnpm(&format!(
            "pnpm@10.15.0+sha512.{}",
            "a".repeat(128)
        )));
        for pin in [
            "pnpm@10.15.0+",
            "pnpm@10",
            "pnpm@^10.15.0",
            "pnpm@10.15.0-beta",
            "pnpm@10.15.0+arbitrary",
        ] {
            assert!(!exact_pnpm(pin));
        }
    }
}
