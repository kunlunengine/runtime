//! Independent C1-P0 native package-manager shell.
//!
//! Only static discovery, frozen graph validation and read-only explanations are available.
//! No subprocess, network, installation, writer, lifecycle runner or JSC dependency exists here.

mod discovery;
mod input;
mod lockfile;
pub mod protocol;

use std::{collections::BTreeMap, path::Path};

use sha2::{Digest, Sha256};

use input::Inputs;
use protocol::{
    Diagnostic, ErrorCode, MAX_WHY_PATHS, PLAN_SCHEMA, PlanResult, Result, ScriptDecision, WhyPath,
    WhyResult,
};

pub fn plan(root: &Path, ignore_scripts: bool) -> Result<PlanResult> {
    let mut inputs = Inputs::new(root)?;
    let (package_manager, manifests) = discovery::discover(&mut inputs)?;
    let (importers, nodes) = lockfile::read_graph(&mut inputs, &manifests)?;
    let mut script_decisions = Vec::new();
    let mut unbuilt_packages = Vec::new();
    for (path, manifest) in &manifests {
        let before = script_decisions.len();
        for hook in ["preinstall", "install", "postinstall", "prepare"] {
            if let Some(command) = manifest.scripts.get(hook) {
                script_decisions.push(script_decision(path, hook, command, ignore_scripts));
            }
        }
        if path == "." {
            if let Some(command) = manifest.scripts.get("pnpm:devPreinstall") {
                script_decisions.push(script_decision(
                    path,
                    "pnpm:devPreinstall",
                    command,
                    ignore_scripts,
                ));
            }
        }
        let gyp = if path == "." {
            "binding.gyp".into()
        } else {
            format!("{path}/binding.gyp")
        };
        if inputs.exists(&gyp)? {
            inputs.read(&gyp)?;
            script_decisions.push(script_decision(
                path,
                "implicit-native",
                "node-gyp rebuild",
                ignore_scripts,
            ));
        }
        if before != script_decisions.len() {
            unbuilt_packages.push(path.clone());
        }
    }
    script_decisions.sort_by(|a, b| (&a.importer, &a.hook).cmp(&(&b.importer, &b.hook)));
    Ok(PlanResult {
        plan_schema: PLAN_SCHEMA,
        package_manager,
        lockfile_version: "9.0",
        frozen: true,
        read_only: true,
        ignore_scripts,
        default_script_policy: "deny",
        evidence: "not-verified",
        fingerprint: inputs.fingerprint(),
        importers,
        nodes,
        script_decisions,
        unbuilt_packages,
        readiness: "not-assessed",
        changed_manifest_paths: Vec::new(),
        policy_decisions: vec![
            "read-only",
            "frozen-graph-validated",
            "scripts-denied",
            "content-evidence-not-verified",
        ],
    })
}

fn script_decision(importer: &str, hook: &str, command: &str, ignore: bool) -> ScriptDecision {
    ScriptDecision {
        importer: importer.into(),
        hook: hook.into(),
        digest: input::hex(&Sha256::digest(command.as_bytes())),
        decision: "denied",
        reason: if ignore {
            "ignore-scripts"
        } else {
            "default-deny"
        },
    }
}

/// Enumerate simple dependency paths, retaining workspace and exact peer identities.
/// A resource limit rejects the entire explanation rather than returning truncated success.
pub fn why(plan: &PlanResult, package: &str) -> Result<WhyResult> {
    if !discovery::package_name(package) {
        return Err(Diagnostic::new(ErrorCode::InvalidArguments));
    }
    let nodes: BTreeMap<_, _> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut result = WhyResult {
        package: package.into(),
        paths: Vec::new(),
    };
    let mut visits = 0usize;
    for importer in &plan.importers {
        for dependency in &importer.dependencies {
            visit(
                &nodes,
                &importer.path,
                &dependency.target,
                package,
                &mut Vec::new(),
                &mut result.paths,
                &mut visits,
            )?;
        }
    }
    result
        .paths
        .sort_by(|a, b| (&a.importer, &a.nodes).cmp(&(&b.importer, &b.nodes)));
    result
        .paths
        .dedup_by(|a, b| a.importer == b.importer && a.nodes == b.nodes);
    Ok(result)
}

fn visit(
    nodes: &BTreeMap<&str, &protocol::Node>,
    importer: &str,
    id: &str,
    package: &str,
    chain: &mut Vec<String>,
    paths: &mut Vec<WhyPath>,
    visits: &mut usize,
) -> Result<()> {
    if chain.iter().any(|node| node == id) {
        return Ok(());
    }
    *visits += 1;
    if chain.len() >= 64 || *visits > 100_000 {
        return Err(Diagnostic::new(ErrorCode::LimitExceeded));
    }
    let node = nodes
        .get(id)
        .ok_or_else(|| Diagnostic::new(ErrorCode::GraphInvalid))?;
    chain.push(id.into());
    if node.name == package {
        if paths.len() >= MAX_WHY_PATHS {
            return Err(Diagnostic::new(ErrorCode::LimitExceeded));
        }
        paths.push(WhyPath {
            importer: importer.into(),
            nodes: chain.clone(),
        });
    }
    for dependency in &node.dependencies {
        visit(
            nodes,
            importer,
            &dependency.target,
            package,
            chain,
            paths,
            visits,
        )?;
    }
    chain.pop();
    Ok(())
}
