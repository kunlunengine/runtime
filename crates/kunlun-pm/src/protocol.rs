//! P0 process boundary. No engine, project execution, network or installer dependency.

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "kunlun.package-manager-provider/v1";
pub const PLAN_SCHEMA: &str = "kunlun.package-manager-plan/v1";
pub const POLICY_SCHEMA: &str = "kunlun.package-manager-policy/v1";
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GRAPH_NODES: usize = 10_000;
pub const MAX_WHY_PATHS: usize = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Version,
    Detect,
    Plan,
    Why,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArguments,
    ProjectUnreadable,
    InvalidManifest,
    InvalidLockfile,
    UnsupportedSchema,
    UnsupportedConfiguration,
    UnsupportedFeature,
    AmbiguousAuthority,
    FrozenDrift,
    PolicyDenied,
    GraphInvalid,
    LimitExceeded,
    Internal,
}

/// Diagnostics are fixed public text: parser errors and user input never enter the envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: ErrorCode,
    pub message: String,
    pub remediation: String,
}

impl Diagnostic {
    pub fn new(code: ErrorCode) -> Self {
        let (message, remediation) = match code {
            ErrorCode::InvalidArguments => (
                "invalid provider arguments",
                "use version, detect, plan or why with the documented options",
            ),
            ErrorCode::ProjectUnreadable => (
                "static project input is missing or unreadable",
                "provide a readable project root and contained regular input files",
            ),
            ErrorCode::InvalidManifest => (
                "invalid static package manifest",
                "use unique JSON keys, valid package identities and an exact pnpm packageManager pin",
            ),
            ErrorCode::InvalidLockfile => (
                "invalid pnpm lockfile",
                "provide a well-formed lockfile with unique keys and supported static fields",
            ),
            ErrorCode::UnsupportedSchema => (
                "unsupported lockfile or policy schema",
                "use pnpm lockfile 9.0 and the documented package-manager policy v1",
            ),
            ErrorCode::UnsupportedConfiguration => (
                "configuration is outside the P0 static subset",
                "remove executable or unsupported graph configuration; no fallback is performed",
            ),
            ErrorCode::UnsupportedFeature => (
                "dependency semantics are outside the P0 subset",
                "use supported registry ranges and workspace links; wait for a qualified provider for other sources",
            ),
            ErrorCode::AmbiguousAuthority => (
                "multiple dependency graph authorities are present",
                "retain pnpm-lock.yaml as the sole graph authority; migrate explicitly",
            ),
            ErrorCode::FrozenDrift => (
                "manifest, workspace or configuration differs from the frozen graph",
                "regenerate and review the authoritative pnpm lockfile using the pinned package manager",
            ),
            ErrorCode::PolicyDenied => (
                "project policy attempts to expand execution authority",
                "P0 accepts deny-only static policy; project files cannot approve lifecycle execution",
            ),
            ErrorCode::GraphInvalid => (
                "locked graph is incomplete or inconsistent",
                "review package identities, workspace links, snapshot edges and locked integrity",
            ),
            ErrorCode::LimitExceeded => (
                "provider resource limit exceeded",
                "reduce the project or query size; do not treat a truncated plan as success",
            ),
            ErrorCode::Internal => (
                "provider response could not be encoded",
                "check the provider installation and output transport",
            ),
        };
        Self {
            code,
            message: message.into(),
            remediation: remediation.into(),
        }
    }

    pub fn exit_status(&self) -> i32 {
        if self.code == ErrorCode::InvalidArguments {
            2
        } else {
            1
        }
    }
}

pub type Result<T> = std::result::Result<T, Diagnostic>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub schema: &'static str,
    pub provider: &'static str,
    pub requires_node: bool,
    pub operation: Operation,
    pub exit_status: i32,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Outcome {
    Ok { result: Box<ProviderResult> },
    Error { diagnostics: Vec<Diagnostic> },
}

impl Envelope {
    pub fn new(operation: Operation, result: Result<ProviderResult>) -> Self {
        let (exit_status, outcome) = match result {
            Ok(result) => (
                0,
                Outcome::Ok {
                    result: Box::new(result),
                },
            ),
            Err(error) => (
                error.exit_status(),
                Outcome::Error {
                    diagnostics: vec![error],
                },
            ),
        };
        Self {
            schema: SCHEMA,
            provider: "kunlun-pm",
            requires_node: false,
            operation,
            exit_status,
            outcome,
        }
    }

    /// Exactly one bounded UTF-8 JSON line. An oversized result becomes an explicit failure.
    pub fn encode(self) -> (i32, Vec<u8>) {
        let mut bytes = serde_json::to_vec(&self).unwrap_or_default();
        if bytes.is_empty() || bytes.len() >= MAX_RESPONSE_BYTES {
            let code = if bytes.is_empty() {
                ErrorCode::Internal
            } else {
                ErrorCode::LimitExceeded
            };
            return Self::new(self.operation, Err(Diagnostic::new(code))).encode();
        }
        bytes.push(b'\n');
        (self.exit_status, bytes)
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ProviderResult {
    Version(VersionResult),
    Detect(DetectResult),
    Plan(PlanResult),
    Why(WhyResult),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub detect: bool,
    pub plan: bool,
    pub why: bool,
    pub resolve: bool,
    pub fetch: bool,
    pub install: bool,
    pub mutate: bool,
    pub prune: bool,
    pub exec: bool,
    pub lifecycle_scripts: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub max_response_bytes: usize,
    pub max_input_bytes: usize,
    pub max_graph_nodes: usize,
    pub max_why_paths: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResult {
    pub version: &'static str,
    pub stage: &'static str,
    pub lockfile_versions: Vec<&'static str>,
    pub capabilities: Capabilities,
    pub limits: Limits,
}

impl Default for VersionResult {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            stage: "P0",
            lockfile_versions: vec!["9.0"],
            capabilities: Capabilities {
                detect: true,
                plan: true,
                why: true,
                resolve: false,
                fetch: false,
                install: false,
                mutate: false,
                prune: false,
                exec: false,
                lifecycle_scripts: false,
            },
            limits: Limits {
                max_response_bytes: MAX_RESPONSE_BYTES,
                max_input_bytes: MAX_INPUT_BYTES,
                max_graph_nodes: MAX_GRAPH_NODES,
                max_why_paths: MAX_WHY_PATHS,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub package_manager: String,
    pub lockfile_version: &'static str,
    pub authority: &'static str,
    pub read_only: bool,
    pub workspace_importers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub name: String,
    pub target: String,
    pub kind: DependencyKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specifier: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    Production,
    Development,
    Optional,
}

#[derive(Debug, Serialize)]
pub struct Importer {
    pub path: String,
    pub name: String,
    pub version: String,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conditions {
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub libc: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    pub conditions: Conditions,
    pub peer_dependencies: std::collections::BTreeMap<String, String>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptDecision {
    pub importer: String,
    pub hook: String,
    pub digest: String,
    pub decision: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanResult {
    pub plan_schema: &'static str,
    pub package_manager: String,
    pub lockfile_version: &'static str,
    pub frozen: bool,
    pub read_only: bool,
    pub ignore_scripts: bool,
    pub default_script_policy: &'static str,
    pub evidence: &'static str,
    pub fingerprint: String,
    pub importers: Vec<Importer>,
    pub nodes: Vec<Node>,
    pub script_decisions: Vec<ScriptDecision>,
    pub unbuilt_packages: Vec<String>,
    pub readiness: &'static str,
    pub changed_manifest_paths: Vec<String>,
    pub policy_decisions: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct WhyPath {
    pub importer: String,
    pub nodes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WhyResult {
    pub package: String,
    pub paths: Vec<WhyPath>,
}
