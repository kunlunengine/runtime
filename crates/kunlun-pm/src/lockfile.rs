//! Read-only pnpm v9 subset. Unknown semantics are refused, never discarded by a writer.

use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use semver::{Version, VersionReq};
use serde::Deserialize;

use crate::{
    discovery::{Manifest, package_name},
    input::{self, Inputs},
    protocol::{
        Conditions, Dependency, DependencyKind, Diagnostic, ErrorCode, Importer, MAX_GRAPH_NODES,
        Node, Result,
    },
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lockfile {
    lockfile_version: String,
    settings: Settings,
    importers: BTreeMap<String, LockedImporter>,
    #[serde(default)]
    packages: BTreeMap<String, Package>,
    #[serde(default)]
    snapshots: BTreeMap<String, Snapshot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    auto_install_peers: bool,
    exclude_links_from_lockfile: bool,
    #[serde(default)]
    inject_workspace_packages: bool,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockedImporter {
    #[serde(default)]
    dependencies: BTreeMap<String, LockedDependency>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, LockedDependency>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, LockedDependency>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedDependency {
    specifier: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Package {
    resolution: Resolution,
    #[serde(default)]
    engines: BTreeMap<String, String>,
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    cpu: Vec<String>,
    #[serde(default)]
    libc: Vec<String>,
    #[serde(default)]
    peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    #[serde(default)]
    has_bin: bool,
    deprecated: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerMeta {
    optional: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolution {
    integrity: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    transitive_peer_dependencies: Vec<String>,
    #[serde(default)]
    optional: bool,
}

pub fn read_graph(
    inputs: &mut Inputs,
    manifests: &BTreeMap<String, Manifest>,
) -> Result<(Vec<Importer>, Vec<Node>)> {
    if !inputs.exists("pnpm-lock.yaml")? {
        return Err(Diagnostic::new(ErrorCode::FrozenDrift));
    }
    let raw = input::yaml(&inputs.read("pnpm-lock.yaml")?, ErrorCode::InvalidLockfile)?;
    if raw
        .get("lockfileVersion")
        .and_then(serde_json::Value::as_str)
        != Some("9.0")
    {
        return Err(Diagnostic::new(ErrorCode::UnsupportedSchema));
    }
    let lock: Lockfile =
        serde_json::from_value(raw).map_err(|_| Diagnostic::new(ErrorCode::UnsupportedFeature))?;
    if lock.lockfile_version != "9.0"
        || !lock.settings.auto_install_peers
        || lock.settings.exclude_links_from_lockfile
        || lock.settings.inject_workspace_packages
    {
        return Err(Diagnostic::new(ErrorCode::UnsupportedConfiguration));
    }
    if lock.snapshots.len() + manifests.len() > MAX_GRAPH_NODES {
        return Err(Diagnostic::new(ErrorCode::LimitExceeded));
    }
    if lock.importers.keys().ne(manifests.keys()) {
        return Err(Diagnostic::new(ErrorCode::FrozenDrift));
    }

    let mut importers = Vec::new();
    for (path, manifest) in manifests {
        let locked = &lock.importers[path];
        let mut dependencies = Vec::new();
        let mut names = BTreeSet::new();
        for (kind, declared, resolved) in [
            (
                DependencyKind::Production,
                &manifest.dependencies,
                &locked.dependencies,
            ),
            (
                DependencyKind::Development,
                &manifest.dev_dependencies,
                &locked.dev_dependencies,
            ),
            (
                DependencyKind::Optional,
                &manifest.optional_dependencies,
                &locked.optional_dependencies,
            ),
        ] {
            if declared.keys().ne(resolved.keys()) {
                return Err(Diagnostic::new(ErrorCode::FrozenDrift));
            }
            for (name, specifier) in declared {
                if !names.insert(name) {
                    return Err(Diagnostic::new(ErrorCode::UnsupportedFeature));
                }
                let dependency = &resolved[name];
                if &dependency.specifier != specifier {
                    return Err(Diagnostic::new(ErrorCode::FrozenDrift));
                }
                let target = if let Some(workspace_spec) = specifier.strip_prefix("workspace:") {
                    let link = dependency
                        .version
                        .strip_prefix("link:")
                        .ok_or_else(|| Diagnostic::new(ErrorCode::FrozenDrift))?;
                    let target_path = normalize_link(path, link)?;
                    let workspace = manifests
                        .get(&target_path)
                        .ok_or_else(|| Diagnostic::new(ErrorCode::GraphInvalid))?;
                    if workspace.name != *name
                        || !workspace_matches(workspace_spec, &workspace.version)?
                    {
                        return Err(Diagnostic::new(ErrorCode::FrozenDrift));
                    }
                    format!("workspace:{target_path}")
                } else {
                    let target = registry_target(name, &dependency.version)?;
                    let (_, version) = identity(&target)?;
                    if !matches_spec(specifier, &version)? {
                        return Err(Diagnostic::new(ErrorCode::FrozenDrift));
                    }
                    target
                };
                dependencies.push(Dependency {
                    name: name.clone(),
                    target,
                    kind,
                    specifier: Some(specifier.clone()),
                });
            }
        }
        dependencies.sort_by(|a, b| a.name.cmp(&b.name));
        importers.push(Importer {
            path: path.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            dependencies,
        });
    }

    let mut nodes = BTreeMap::new();
    let mut used_packages = BTreeSet::new();
    for (id, snapshot) in &lock.snapshots {
        let (name, version) = identity(id)?;
        let base = id.split('(').next().unwrap_or(id);
        let package = lock
            .packages
            .get(base)
            .ok_or_else(|| Diagnostic::new(ErrorCode::GraphInvalid))?;
        used_packages.insert(base.to_string());
        if !valid_integrity(&package.resolution.integrity) {
            return Err(Diagnostic::new(ErrorCode::GraphInvalid));
        }
        for condition in package.os.iter().chain(&package.cpu).chain(&package.libc) {
            if condition.is_empty()
                || condition.len() > 64
                || !condition
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!_-".contains(&c))
            {
                return Err(Diagnostic::new(ErrorCode::UnsupportedFeature));
            }
        }
        let mut dependencies = Vec::new();
        let mut names = BTreeSet::new();
        for (kind, resolved) in [
            (DependencyKind::Production, &snapshot.dependencies),
            (DependencyKind::Optional, &snapshot.optional_dependencies),
        ] {
            for (name, reference) in resolved {
                if !names.insert(name) {
                    return Err(Diagnostic::new(ErrorCode::UnsupportedFeature));
                }
                dependencies.push(Dependency {
                    name: name.clone(),
                    target: registry_target(name, reference)?,
                    kind,
                    specifier: None,
                });
            }
        }
        dependencies.sort_by(|a, b| a.name.cmp(&b.name));
        let context = peer_context(id)?;
        if context
            .values()
            .any(|peer_id| !lock.snapshots.contains_key(peer_id))
        {
            return Err(Diagnostic::new(ErrorCode::GraphInvalid));
        }
        for (peer, specifier) in &package.peer_dependencies {
            if !package_name(peer) {
                return Err(Diagnostic::new(ErrorCode::GraphInvalid));
            }
            let resolved = dependencies
                .iter()
                .find(|dependency| &dependency.name == peer);
            match resolved {
                Some(dependency)
                    if matches_spec(specifier, &identity(&dependency.target)?.1)?
                        && context.get(peer) == Some(&dependency.target) => {}
                None if package
                    .peer_dependencies_meta
                    .get(peer)
                    .is_some_and(|meta| meta.optional) =>
                {
                    // Even absent optional peers must have supported range semantics.
                    matches_spec(specifier, &Version::new(0, 0, 0))?;
                    if context.contains_key(peer) {
                        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
                    }
                }
                _ => return Err(Diagnostic::new(ErrorCode::GraphInvalid)),
            }
        }
        for (name, target) in &context {
            if !package.peer_dependencies.contains_key(name)
                && !snapshot.transitive_peer_dependencies.contains(name)
            {
                return Err(Diagnostic::new(ErrorCode::GraphInvalid));
            }
            if let Some(dependency) = dependencies
                .iter()
                .find(|dependency| &dependency.name == name)
            {
                if &dependency.target != target {
                    return Err(Diagnostic::new(ErrorCode::GraphInvalid));
                }
            }
        }
        if snapshot
            .transitive_peer_dependencies
            .iter()
            .any(|name| !package_name(name))
            || package
                .peer_dependencies_meta
                .keys()
                .any(|name| !package.peer_dependencies.contains_key(name))
        {
            return Err(Diagnostic::new(ErrorCode::GraphInvalid));
        }
        // These are read metadata, not proof that a hook or executable is ready.
        let _metadata = (
            &package.engines,
            package.has_bin,
            &package.deprecated,
            snapshot.optional,
        );
        nodes.insert(
            id.clone(),
            Node {
                id: id.clone(),
                kind: "registry",
                name,
                version: version.to_string(),
                integrity: Some(package.resolution.integrity.clone()),
                conditions: Conditions {
                    os: package.os.clone(),
                    cpu: package.cpu.clone(),
                    libc: package.libc.clone(),
                },
                peer_dependencies: package.peer_dependencies.clone(),
                dependencies,
            },
        );
    }
    if used_packages.iter().ne(lock.packages.keys()) {
        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
    }
    for importer in &importers {
        let id = format!("workspace:{}", importer.path);
        nodes.insert(
            id.clone(),
            Node {
                id,
                kind: "workspace",
                name: importer.name.clone(),
                version: importer.version.clone(),
                integrity: None,
                conditions: Conditions {
                    os: Vec::new(),
                    cpu: Vec::new(),
                    libc: Vec::new(),
                },
                peer_dependencies: BTreeMap::new(),
                dependencies: importer
                    .dependencies
                    .iter()
                    .cloned()
                    .map(|mut dependency| {
                        dependency.specifier = None;
                        dependency
                    })
                    .collect(),
            },
        );
    }
    for dependencies in importers
        .iter()
        .map(|importer| &importer.dependencies)
        .chain(nodes.values().map(|node| &node.dependencies))
    {
        if dependencies
            .iter()
            .any(|dependency| !nodes.contains_key(&dependency.target))
        {
            return Err(Diagnostic::new(ErrorCode::GraphInvalid));
        }
    }
    let mut reachable = BTreeSet::new();
    let mut pending: Vec<_> = importers
        .iter()
        .map(|importer| format!("workspace:{}", importer.path))
        .collect();
    while let Some(id) = pending.pop() {
        if reachable.insert(id.clone()) {
            pending.extend(
                nodes[&id]
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.target.clone()),
            );
        }
    }
    if reachable.len() != nodes.len() {
        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
    }
    Ok((importers, nodes.into_values().collect()))
}

fn valid_integrity(integrity: &str) -> bool {
    integrity.strip_prefix("sha512-").is_some_and(|encoded| {
        STANDARD
            .decode(encoded)
            .is_ok_and(|bytes| bytes.len() == 64 && STANDARD.encode(bytes) == encoded)
    })
}

fn registry_target(name: &str, reference: &str) -> Result<String> {
    if !package_name(name) {
        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
    }
    let target = format!("{name}@{reference}");
    identity(&target)?;
    Ok(target)
}

/// Retain exact peer-context identity. Aliases, patched IDs, Git and local sources are unsupported.
fn identity(id: &str) -> Result<(String, Version)> {
    if id.len() > 1024 {
        return Err(Diagnostic::new(ErrorCode::LimitExceeded));
    }
    let base = id.split('(').next().unwrap_or(id);
    let (name, version) = base
        .rsplit_once('@')
        .ok_or_else(|| Diagnostic::new(ErrorCode::UnsupportedFeature))?;
    if !package_name(name) {
        return Err(Diagnostic::new(ErrorCode::UnsupportedFeature));
    }
    let version =
        Version::parse(version).map_err(|_| Diagnostic::new(ErrorCode::UnsupportedFeature))?;
    let suffix = &id[base.len()..];
    let mut start = 0;
    let mut depth = 0usize;
    for (index, byte) in suffix.bytes().enumerate() {
        match byte {
            b'(' => {
                if depth == 0 {
                    start = index + 1;
                }
                depth += 1;
                if depth > 32 {
                    return Err(Diagnostic::new(ErrorCode::LimitExceeded));
                }
            }
            b')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    identity(&suffix[start..index])?;
                }
            }
            _ if depth == 0 => return Err(Diagnostic::new(ErrorCode::UnsupportedFeature)),
            _ => {}
        }
    }
    if depth != 0 {
        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
    }
    Ok((name.into(), version))
}

fn peer_context(id: &str) -> Result<BTreeMap<String, String>> {
    // identity() has already checked balanced groups and recursive version syntax.
    let suffix = &id[id.find('(').unwrap_or(id.len())..];
    let mut peers = BTreeMap::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, byte) in suffix.bytes().enumerate() {
        if byte == b'(' {
            if depth == 0 {
                start = index + 1;
            }
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                let peer = &suffix[start..index];
                let name = identity(peer)?.0;
                if peers.insert(name, peer.to_string()).is_some() {
                    return Err(Diagnostic::new(ErrorCode::GraphInvalid));
                }
            }
        }
    }
    Ok(peers)
}

fn matches_spec(specifier: &str, version: &Version) -> Result<bool> {
    if specifier == "*" {
        return Ok(version.pre.is_empty());
    }
    if let Some(operand) = specifier
        .strip_prefix('^')
        .or_else(|| specifier.strip_prefix('~'))
    {
        Version::parse(operand).map_err(|_| Diagnostic::new(ErrorCode::UnsupportedFeature))?;
        return VersionReq::parse(specifier)
            .map(|range| range.matches(version))
            .map_err(|_| Diagnostic::new(ErrorCode::UnsupportedFeature));
    }
    Version::parse(specifier)
        .map(|exact| exact == *version)
        .map_err(|_| Diagnostic::new(ErrorCode::UnsupportedFeature))
}

fn workspace_matches(specifier: &str, version: &str) -> Result<bool> {
    if ["*", "^", "~"].contains(&specifier) {
        return Ok(true);
    }
    matches_spec(
        specifier,
        &Version::parse(version).map_err(|_| Diagnostic::new(ErrorCode::InvalidManifest))?,
    )
}

fn normalize_link(importer: &str, link: &str) -> Result<String> {
    if link.is_empty() || link.starts_with('/') || link.contains('\\') {
        return Err(Diagnostic::new(ErrorCode::GraphInvalid));
    }
    let mut parts: Vec<&str> = if importer == "." {
        Vec::new()
    } else {
        importer.split('/').collect()
    };
    for part in link.split('/') {
        match part {
            "." => {}
            ".." => {
                parts
                    .pop()
                    .ok_or_else(|| Diagnostic::new(ErrorCode::GraphInvalid))?;
            }
            "" => return Err(Diagnostic::new(ErrorCode::GraphInvalid)),
            segment
                if segment
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) =>
            {
                parts.push(segment)
            }
            _ => return Err(Diagnostic::new(ErrorCode::GraphInvalid)),
        }
    }
    Ok(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_exact_is_not_a_caret_and_only_declared_ranges_are_supported() {
        assert!(!matches_spec("1.0.0", &Version::new(1, 1, 0)).unwrap());
        assert!(matches_spec("^1.0.0", &Version::new(1, 1, 0)).unwrap());
        assert!(!matches_spec("~1.0.0", &Version::new(1, 1, 0)).unwrap());
        assert!(matches_spec("1.2", &Version::new(1, 2, 0)).is_err());
        assert!(matches_spec("latest", &Version::new(1, 2, 0)).is_err());
    }

    #[test]
    fn peer_context_and_workspace_containment() {
        assert_eq!(
            identity("@scope/pkg@1.0.0(react@18.0.0)").unwrap().0,
            "@scope/pkg"
        );
        assert!(identity("pkg@1.0.0(peer@1.0.0)SECRET").is_err());
        assert_eq!(normalize_link("packages/a", "../b").unwrap(), "packages/b");
        assert!(normalize_link(".", "../outside").is_err());
        assert!(normalize_link(".", "https://secret").is_err());
    }
}
