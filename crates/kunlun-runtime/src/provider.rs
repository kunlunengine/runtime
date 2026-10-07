//! Binary adapter for Core's runtime provider protocol. No workflow commands live here.
use kunlun_jsc::JscVm;
use kunlun_runtime::{
    AdmissionPolicy, FETCH_ENTRY_CONTRACT, HostPermissions, RUNTIME_ENGINE_ABI,
    RUNTIME_MANIFEST_SCHEMA, RUNTIME_PROFILE, admit_artifact,
};
use kunlun_runtime_protocol::{
    ArtifactOptions, Command, ErrorCode, ProviderError, Response, encode, parse,
};
use serde::Serialize;
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Serialize)]
pub(crate) struct VersionReport {
    executable: &'static str,
    runtime_version: &'static str,
    provider_schema: &'static str,
    artifact_manifest_schema: &'static str,
    entry_contract: &'static str,
    engine_abi: u32,
    runtime_profile: &'static str,
    operations: [Command; 3],
    engine: EngineReport,
    capabilities: Capabilities,
}

#[derive(Serialize)]
struct EngineReport {
    name: &'static str,
    backend: &'static str,
    revision: &'static str,
    target: &'static str,
    distribution_mode: &'static str,
    hermetic: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    artifact_admission: bool,
    application_execution: bool,
    http_serve: bool,
    inspector_transport: bool,
    inspection_primitive: bool,
    native_modules: bool,
    deferred_promises: bool,
    explicit_microtask_checkpoint: bool,
    execution_watchdog: bool,
    heap_telemetry: bool,
}

pub(crate) fn version_report() -> VersionReport {
    let backend = JscVm::backend_info();
    VersionReport {
        executable: "kunlun-runtime",
        runtime_version: env!("CARGO_PKG_VERSION"),
        provider_schema: kunlun_runtime_protocol::PROVIDER_SCHEMA,
        artifact_manifest_schema: RUNTIME_MANIFEST_SCHEMA,
        entry_contract: FETCH_ENTRY_CONTRACT,
        engine_abi: RUNTIME_ENGINE_ABI,
        runtime_profile: RUNTIME_PROFILE,
        operations: [Command::Version, Command::Doctor, Command::CheckArtifact],
        engine: EngineReport {
            name: backend.name,
            backend: backend.backend,
            revision: backend.engine_revision,
            target: backend.target,
            distribution_mode: backend.distribution_mode,
            hermetic: backend.hermetic,
        },
        capabilities: Capabilities {
            artifact_admission: true,
            // This flag means the provider can launch an admitted Fetch application.
            // Bare run-module and the in-process dispatcher are not that process API.
            application_execution: false,
            http_serve: false,
            inspector_transport: false,
            inspection_primitive: backend.supports_inspection,
            native_modules: backend.supports_native_modules,
            deferred_promises: backend.supports_deferred_promises,
            explicit_microtask_checkpoint: backend.supports_explicit_microtask_checkpoint,
            execution_watchdog: backend.supports_execution_watchdog,
            heap_telemetry: backend.supports_heap_telemetry,
        },
    }
}

#[derive(Serialize)]
pub(crate) struct DoctorReport {
    #[serde(flatten)]
    pub version: VersionReport,
    pub smoke_tests: SmokeTests,
}

#[derive(Serialize)]
pub(crate) struct SmokeTests {
    pub synchronous: bool,
    pub temporal: bool,
    pub async_timer: bool,
}

#[derive(Serialize)]
struct ArtifactReport {
    artifact_manifest_schema: &'static str,
    entry_contract: &'static str,
    engine_abi: u32,
    runtime_profile: &'static str,
    indexed_files: usize,
    required_capabilities: usize,
    optional_capabilities: usize,
    application_evaluated: bool,
}

pub(crate) fn run(command: Command, args: &[String]) -> ExitCode {
    // Preserve structured errors even when the rest of a JSON request is malformed.
    let json = args
        .iter()
        .take_while(|value| value.as_str() != "--")
        .any(|value| value == "--json");
    let request = match parse(command, args) {
        Ok(request) => request,
        Err(message) => {
            return fail(
                command,
                json,
                ProviderError {
                    code: ErrorCode::InvalidArguments,
                    message: message.to_owned(),
                    remediation: "consult kunlun-runtime help for the provider command syntax"
                        .into(),
                    admission_kind: None,
                },
            );
        }
    };
    match command {
        Command::Version if request.json => success(command, version_report()),
        Command::Version => {
            println!("kunlun-runtime {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Doctor => match super::doctor_report(!request.json) {
            Ok(report) if request.json => success(command, report),
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => fail(command, request.json, ProviderError {
                code: ErrorCode::DiagnosticFailed,
                message: if request.json {
                    "runtime smoke test failed".into()
                } else {
                    format!("runtime smoke test failed: {error}")
                },
                remediation: "run kunlun-runtime doctor without --json and verify the selected JSC backend".into(),
                admission_kind: None,
            }),
        },
        Command::CheckArtifact => match check_artifact(request.artifact.expect("parsed artifact options")) {
            Ok(report) if request.json => success(command, report),
            Ok(report) => {
                println!("artifact admission: ok ({} indexed files; application not evaluated)",
                    report.indexed_files);
                ExitCode::SUCCESS
            }
            Err(error) => fail(command, request.json, error),
        },
    }
}

fn check_artifact(options: ArtifactOptions) -> Result<ArtifactReport, ProviderError> {
    let mut permissions = HostPermissions::none();
    for (binding, root) in options.read_bindings {
        permissions = permissions
            .bind_read_root(binding, root)
            .map_err(|_| ProviderError {
                code: ErrorCode::InvalidArguments,
                message: "cannot open a named deployment read binding".into(),
                remediation:
                    "provide a valid unique binding name and an existing readable deployment directory".into(),
                admission_kind: None,
            })?;
    }
    for host in options.net_hosts {
        permissions = permissions.allow_net_host(host);
    }
    let policy = AdmissionPolicy::new(options.manifest_sha256, permissions);
    let artifact = admit_artifact(options.directory, &policy).map_err(|error| ProviderError {
        code: ErrorCode::AdmissionRejected,
        // Never echo manifest fragments, file paths, or capability resources into JSON.
        message: "artifact admission rejected".into(),
        remediation: "verify the trusted manifest digest, artifact integrity, ABI/profile and deployment grants".into(),
        admission_kind: Some(error.kind.to_string()),
    })?;
    let manifest = artifact.manifest();
    Ok(ArtifactReport {
        artifact_manifest_schema: RUNTIME_MANIFEST_SCHEMA,
        entry_contract: FETCH_ENTRY_CONTRACT,
        engine_abi: RUNTIME_ENGINE_ABI,
        runtime_profile: RUNTIME_PROFILE,
        indexed_files: manifest.files.len(),
        required_capabilities: manifest.capabilities.required.len(),
        optional_capabilities: manifest.capabilities.optional.len(),
        application_evaluated: false,
    })
}

fn success<T: Serialize>(command: Command, result: T) -> ExitCode {
    match write_response(&Response::success(command, result)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(
            command,
            true,
            ProviderError {
                code: ErrorCode::Internal,
                message: message.into(),
                remediation: "verify provider compatibility and stdout availability".into(),
                admission_kind: None,
            },
        ),
    }
}

fn fail(command: Command, json: bool, error: ProviderError) -> ExitCode {
    let exit_code = error.code.exit_code();
    if json {
        if let Err(message) = write_response(&Response::<()>::failure(command, error)) {
            eprintln!("provider output failed: {message}");
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!(
            "error: {}\nremediation: {}",
            error.message, error.remediation
        );
    }
    ExitCode::from(exit_code)
}

fn write_response<T: Serialize>(response: &Response<T>) -> Result<(), &'static str> {
    let wire = encode(response)?;
    io::stdout()
        .lock()
        .write_all(wire.as_bytes())
        .map_err(|_| "cannot write provider response")
}
