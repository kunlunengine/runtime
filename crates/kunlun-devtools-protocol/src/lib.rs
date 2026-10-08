//! Draft v1 plain-data runtime boundary. No transport, broker, engine, or authorization store.
//! JSON Schema is derived from these same serde types, never maintained independently.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod semantic;
mod source_model;

pub use semantic::*;
pub use source_model::{SourceDescriptor, SourceLocation, SourceMapping};

pub const MAX_MESSAGE_BYTES: usize = 65_536;
pub const MAX_SOURCE_BYTES: u32 = 16_384;
pub const MAX_EVENTS: usize = 64;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Version {
    #[serde(rename = "1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub target: String,
    pub session: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Inspect,
    Control,
    Evaluate,
    Mutate,
    CaptureSensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Discover,
    Attach,
    Detach,
    ListSources,
    ReadSource,
    LookupSourceMap,
    ReadScopes,
    SetBreakpoint,
    RemoveBreakpoint,
    Pause,
    Continue,
    Step,
    Evaluate,
    Mutate,
    ReadLogs,
    CaptureCpu,
    CaptureHeap,
    Cancel,
}

impl Operation {
    pub fn permission(self) -> Permission {
        match self {
            Self::Discover
            | Self::Attach
            | Self::Detach
            | Self::ListSources
            | Self::ReadSource
            | Self::LookupSourceMap
            | Self::ReadScopes
            | Self::ReadLogs
            | Self::Cancel => Permission::Inspect,
            Self::SetBreakpoint
            | Self::RemoveBreakpoint
            | Self::Pause
            | Self::Continue
            | Self::Step => Permission::Control,
            Self::Evaluate => Permission::Evaluate,
            Self::Mutate => Permission::Mutate,
            Self::CaptureCpu | Self::CaptureHeap => Permission::CaptureSensitive,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Support {
    Supported,
    Unsupported { reason: UnsupportedReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    BackendUnavailable,
    ProtocolSpecific,
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub operation: Operation,
    pub support: Support,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum Error {
    Malformed,
    MessageTooLarge,
    UnknownVersion,
    UnknownOperation,
    InvalidIdentity,
    StaleEpoch,
    StaleHandle,
    PermissionDenied {
        permission: Permission,
    },
    Unsupported {
        operation: Operation,
        reason: UnsupportedReason,
    },
    SourceNotFound,
    SourceRange,
    EventOverflow,
    EventOrder,
    Disconnected,
    DuplicateRequest,
    InvalidState,
    Cancelled,
    RequestNotFound,
    OutcomeUnknown,
    AuditUnavailable,
    LimitExceeded {
        resource: Resource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub target: String,
    pub epoch: u64,
    /// Opaque adapter-assigned identifier, not a filesystem path or network request.
    pub id: String,
    /// Display identity only; preserve the backend's source naming.
    pub url: String,
    /// Opaque immutable revision, changed whenever source bytes change.
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceChunk {
    pub source: SourceIdentity,
    pub offset: u32,
    pub total_bytes: u32,
    pub text: String,
    pub eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Handle {
    pub identity: Identity,
    pub pause: u64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Attach,
    Detach,
    ListSources {
        max_entries: u32,
    },
    ReadSource {
        source: SourceIdentity,
        offset: u32,
        max_bytes: u32,
    },
    LookupSourceMap {
        location: SourceLocation,
    },
    ReadScopes {
        handle: Handle,
    },
    SetBreakpoint {
        source: SourceIdentity,
        line: u32,
        column: u32,
    },
    RemoveBreakpoint {
        breakpoint_id: String,
    },
    ReadLogs {
        max_entries: u32,
    },
    Mutate {
        handle: Handle,
        value: String,
    },
    CaptureCpu {
        max_bytes: u32,
    },
    CaptureHeap {
        max_bytes: u32,
    },
    Pause,
    Continue,
    Step,
    Evaluate {
        expression: String,
    },
    Cancel {
        request_id: String,
    },
}

impl Command {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Attach => Operation::Attach,
            Self::Detach => Operation::Detach,
            Self::ListSources { .. } => Operation::ListSources,
            Self::ReadSource { .. } => Operation::ReadSource,
            Self::LookupSourceMap { .. } => Operation::LookupSourceMap,
            Self::ReadScopes { .. } => Operation::ReadScopes,
            Self::SetBreakpoint { .. } => Operation::SetBreakpoint,
            Self::RemoveBreakpoint { .. } => Operation::RemoveBreakpoint,
            Self::ReadLogs { .. } => Operation::ReadLogs,
            Self::Mutate { .. } => Operation::Mutate,
            Self::CaptureCpu { .. } => Operation::CaptureCpu,
            Self::CaptureHeap { .. } => Operation::CaptureHeap,
            Self::Pause => Operation::Pause,
            Self::Continue => Operation::Continue,
            Self::Step => Operation::Step,
            Self::Evaluate { .. } => Operation::Evaluate,
            Self::Cancel { .. } => Operation::Cancel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: Version,
    pub request_id: String,
    pub identity: Identity,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allowed,
    Denied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuditCheck {
    Operation,
    /// A cancel checks both Inspect and the original operation's permission; record both honestly.
    Cancellation {
        request_id: String,
        operation: Operation,
    },
}

/// Allowlisted metadata only: never expression, result, header, environment, or grant values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub identity: Identity,
    pub request_id: String,
    pub operation: Operation,
    pub permission: Permission,
    pub check: AuditCheck,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventData {
    Paused {
        pause: u64,
        reason: PauseReason,
        frames: Vec<StackFrame>,
        truncated: bool,
    },
    Resumed,
    Replaced {
        previous_epoch: u64,
    },
    SourceAdded {
        source: SourceDescriptor,
    },
    Log {
        entry: LogEntry,
    },
    Audit {
        audit: Audit,
    },
    Terminal {
        error: Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: Version,
    pub identity: Identity,
    pub sequence: u64,
    pub data: EventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Wire {
    Hello {
        versions: Vec<String>,
    },
    Welcome {
        version: Version,
        capabilities: Vec<Capability>,
    },
    Discover {
        version: Version,
        request_id: String,
        max_targets: u32,
    },
    Discovered {
        version: Version,
        request_id: String,
        targets: Vec<Target>,
        truncated: bool,
    },
    Request {
        request: Request,
    },
    Response {
        response: Response,
    },
    Event {
        event: Event,
    },
    Failure {
        request_id: Option<String>,
        error: Error,
    },
}

pub fn schema() -> schemars::Schema {
    // decode accepts the canonical serialized shape: nullable fields are present, even as null.
    // `schemars(required)` would incorrectly turn Option<T> into non-nullable T.
    schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<Wire>()
}

/// Select an exact known version before decoding any session traffic.
pub fn negotiate(versions: &[String]) -> Result<Version, Error> {
    if versions.iter().any(|v| v == "1") {
        Ok(Version::V1)
    } else {
        Err(Error::UnknownVersion)
    }
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

pub fn validate_identity(identity: &Identity) -> Result<(), Error> {
    if identifier(&identity.target)
        && identifier(&identity.session)
        && (1..=MAX_SAFE_INTEGER).contains(&identity.epoch)
    {
        Ok(())
    } else {
        Err(Error::InvalidIdentity)
    }
}

fn validate_source(source: &SourceIdentity) -> Result<(), Error> {
    if !identifier(&source.target)
        || !identifier(&source.id)
        || !identifier(&source.revision)
        || !(1..=MAX_SAFE_INTEGER).contains(&source.epoch)
        || source.url.is_empty()
        || source.url.len() > 2048
        || source.url.chars().any(char::is_control)
    {
        return Err(Error::InvalidIdentity);
    }
    Ok(())
}

fn source_for_identity(source: &SourceIdentity, identity: &Identity) -> Result<(), Error> {
    validate_source(source)?;
    if source.target != identity.target || source.epoch != identity.epoch {
        return Err(Error::StaleEpoch);
    }
    Ok(())
}

fn validate_handle(handle: &Handle, identity: &Identity) -> Result<(), Error> {
    validate_identity(&handle.identity)?;
    if &handle.identity != identity
        || !identifier(&handle.id)
        || !(1..=MAX_SAFE_INTEGER).contains(&handle.pause)
    {
        return Err(Error::StaleHandle);
    }
    Ok(())
}

fn validate_count(count: u32) -> Result<(), Error> {
    if count == 0 || count > MAX_ENTRIES {
        return Err(Error::LimitExceeded {
            resource: Resource::Collection,
        });
    }
    Ok(())
}

fn validate_collection<T>(items: &[T]) -> Result<(), Error> {
    if items.len() > MAX_ENTRIES as usize {
        return Err(Error::LimitExceeded {
            resource: Resource::Collection,
        });
    }
    Ok(())
}

fn validate_text(text: &str) -> Result<(), Error> {
    if text.len() > MAX_VALUE_BYTES {
        return Err(Error::LimitExceeded {
            resource: Resource::Collection,
        });
    }
    Ok(())
}

fn validate_capabilities(capabilities: &[Capability]) -> Result<(), Error> {
    validate_collection(capabilities)?;
    let mut operations = Vec::new();
    for capability in capabilities {
        if operations.contains(&capability.operation) {
            return Err(Error::Malformed);
        }
        operations.push(capability.operation);
    }
    Ok(())
}

/// Enforce byte limits before allocating a JSON tree. Decode failures never dispatch commands.
pub fn decode(bytes: &[u8]) -> Result<Wire, Error> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::MessageTooLarge);
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
    let version = match value["kind"].as_str() {
        Some("request") => &value["request"]["version"],
        Some("response") => &value["response"]["version"],
        Some("event") => &value["event"]["version"],
        Some("welcome" | "discover" | "discovered") => &value["version"],
        _ => &serde_json::Value::Null,
    };
    if let Some(version) = version.as_str() {
        if version != "1" {
            return Err(Error::UnknownVersion);
        }
    }
    if value["kind"] == "request" {
        let request = &value["request"];
        if request["command"]["op"].is_string() {
            // Operation's serde vocabulary is the source of truth, not another name table.
            // Discover is connection-scoped, never a session command.
            if matches!(
                serde_json::from_value::<Operation>(request["command"]["op"].clone()),
                Err(_) | Ok(Operation::Discover)
            ) {
                return Err(Error::UnknownOperation);
            }
        }
    }
    // Deserialize from original bytes so duplicate fields remain visible to serde.
    let wire: Wire = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
    // Serde's internally tagged unit variants otherwise ignore extra fields even with
    // deny_unknown_fields. Canonical value equality closes that gap with derived JSON Schema.
    if serde_json::to_value(&wire).map_err(|_| Error::Malformed)? != value {
        return Err(Error::Malformed);
    }
    match &wire {
        Wire::Request { request } => {
            validate_identity(&request.identity)?;
            if !identifier(&request.request_id) {
                return Err(Error::InvalidIdentity);
            }
            match &request.command {
                Command::ListSources { max_entries } | Command::ReadLogs { max_entries } => {
                    validate_count(*max_entries)?;
                }
                Command::ReadSource {
                    source, max_bytes, ..
                } => {
                    source_for_identity(source, &request.identity)?;
                    if *max_bytes == 0 || *max_bytes > MAX_SOURCE_BYTES {
                        return Err(Error::SourceRange);
                    }
                }
                Command::SetBreakpoint { source, .. } => {
                    source_for_identity(source, &request.identity)?;
                }
                Command::LookupSourceMap { location } => {
                    source_model::validate_location(location)?;
                    source_for_identity(&location.source, &request.identity)?;
                }
                Command::ReadScopes { handle } | Command::Mutate { handle, .. } => {
                    validate_handle(handle, &request.identity)?;
                }
                Command::CaptureCpu { max_bytes } | Command::CaptureHeap { max_bytes } => {
                    if *max_bytes == 0 || *max_bytes > MAX_CAPTURE_BYTES {
                        return Err(Error::LimitExceeded {
                            resource: Resource::Snapshot,
                        });
                    }
                }
                Command::RemoveBreakpoint { breakpoint_id } => {
                    if !identifier(breakpoint_id) {
                        return Err(Error::InvalidIdentity);
                    }
                }
                Command::Cancel { request_id }
                    if !identifier(request_id) || request_id == &request.request_id =>
                {
                    return Err(Error::InvalidIdentity);
                }
                _ => {}
            }
        }
        Wire::Response { response } => {
            validate_identity(&response.identity)?;
            if !identifier(&response.request_id) {
                return Err(Error::InvalidIdentity);
            }
            if let ResponseResult::Success { result } = &response.result {
                validate_result(result, &response.identity)?;
            }
        }
        Wire::Discover {
            request_id,
            max_targets,
            ..
        } => {
            if !identifier(request_id) {
                return Err(Error::InvalidIdentity);
            }
            validate_count(*max_targets)?;
        }
        Wire::Discovered {
            request_id,
            targets,
            ..
        } => {
            if !identifier(request_id) {
                return Err(Error::InvalidIdentity);
            }
            validate_collection(targets)?;
            let mut ids = std::collections::BTreeSet::new();
            for target in targets {
                validate_target(target)?;
                if !ids.insert(&target.id) {
                    return Err(Error::InvalidIdentity);
                }
            }
        }
        Wire::Welcome { capabilities, .. } => validate_capabilities(capabilities)?,
        Wire::Failure {
            request_id: Some(request_id),
            ..
        } if !identifier(request_id) => {
            return Err(Error::InvalidIdentity);
        }
        Wire::Event { event } => {
            validate_identity(&event.identity)?;
            if !(1..=MAX_SAFE_INTEGER).contains(&event.sequence) {
                return Err(Error::EventOrder);
            }
            match &event.data {
                EventData::Audit { audit } => {
                    if audit.identity != event.identity || !identifier(&audit.request_id) {
                        return Err(Error::Malformed);
                    }
                    let operation = match &audit.check {
                        AuditCheck::Operation => audit.operation,
                        AuditCheck::Cancellation {
                            request_id,
                            operation,
                        } if audit.operation == Operation::Cancel
                            && identifier(request_id)
                            && request_id != &audit.request_id
                            && !matches!(operation, Operation::Discover | Operation::Cancel) =>
                        {
                            *operation
                        }
                        _ => return Err(Error::Malformed),
                    };
                    if audit.permission != operation.permission() {
                        return Err(Error::Malformed);
                    }
                }
                EventData::Paused { pause, frames, .. } => {
                    if !(1..=MAX_SAFE_INTEGER).contains(pause) {
                        return Err(Error::StaleHandle);
                    }
                    validate_collection(frames)?;
                    for frame in frames {
                        validate_text(&frame.name)?;
                        validate_handle(&frame.handle, &event.identity)?;
                        if frame.handle.pause != *pause {
                            return Err(Error::StaleHandle);
                        }
                        if let Some(location) = &frame.location {
                            source_model::validate_location(location)?;
                            source_for_identity(&location.source, &event.identity)?;
                        }
                    }
                }
                EventData::Replaced { previous_epoch }
                    if *previous_epoch == 0 || *previous_epoch >= event.identity.epoch =>
                {
                    return Err(Error::StaleEpoch);
                }
                EventData::SourceAdded { source } => {
                    source_model::validate_descriptor(source)?;
                    source_for_identity(&source.source, &event.identity)?;
                }
                EventData::Log { entry } => validate_log(entry, &event.identity)?,
                _ => {}
            }
        }
        _ => {}
    }
    Ok(wire)
}

/// The same size/semantic checks apply to outbound data.
pub fn encode(wire: &Wire) -> Result<Vec<u8>, Error> {
    // Stop serialization at the byte limit instead of allocating an arbitrarily large
    // adapter-provided snapshot and rejecting it only after serialization completes.
    struct BoundedWriter {
        bytes: Vec<u8>,
        exceeded: bool,
    }
    impl std::io::Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_MESSAGE_BYTES - self.bytes.len() {
                self.exceeded = true;
                return Err(std::io::Error::other("DevTools frame limit exceeded"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, wire).is_err() {
        return Err(if writer.exceeded {
            Error::MessageTooLarge
        } else {
            Error::Malformed
        });
    }
    decode(&writer.bytes)?;
    Ok(writer.bytes)
}

/// Pure policy check, not an authority source. Callers supply already authenticated grants.
pub fn authorize(operation: Operation, grants: &[Permission]) -> Result<(), Error> {
    let permission = operation.permission();
    if grants.contains(&permission) {
        Ok(())
    } else {
        Err(Error::PermissionDenied { permission })
    }
}

/// Serve immutable adapter-provided UTF-8 bytes, never fetch a caller-provided URL.
pub fn source_chunk(
    source: SourceIdentity,
    text: &str,
    offset: u32,
    max_bytes: u32,
) -> Result<SourceChunk, Error> {
    validate_source(&source)?;
    if max_bytes == 0 || max_bytes > MAX_SOURCE_BYTES || text.len() > u32::MAX as usize {
        return Err(Error::SourceRange);
    }
    let start = offset as usize;
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(Error::SourceRange);
    }
    let mut end = start.saturating_add(max_bytes as usize).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if start < text.len() && end == start {
        return Err(Error::SourceRange);
    }
    Ok(SourceChunk {
        source,
        offset,
        total_bytes: text.len() as u32,
        text: text[start..end].to_owned(),
        eof: end == text.len(),
    })
}
