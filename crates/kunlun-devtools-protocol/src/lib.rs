//! Draft v1 plain-data runtime boundary. No transport, broker, engine, or authorization store.
//! JSON Schema is derived from these same serde types, never maintained independently.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    Attach,
    Detach,
    ReadSource,
    ReadScopes,
    SetBreakpoint,
    Pause,
    Continue,
    Step,
    Evaluate,
    Mutate,
    ReadLogs,
    CaptureCpu,
    CaptureHeap,
}

impl Operation {
    pub fn permission(self) -> Permission {
        match self {
            Self::Attach | Self::Detach | Self::ReadSource | Self::ReadScopes | Self::ReadLogs => {
                Permission::Inspect
            }
            Self::SetBreakpoint | Self::Pause | Self::Continue | Self::Step => Permission::Control,
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
    ReadSource {
        source: SourceIdentity,
        offset: u32,
        max_bytes: u32,
    },
    ReadScopes {
        handle: Handle,
    },
    SetBreakpoint {
        source: SourceIdentity,
        line: u32,
        column: u32,
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
}

impl Command {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Attach => Operation::Attach,
            Self::Detach => Operation::Detach,
            Self::ReadSource { .. } => Operation::ReadSource,
            Self::ReadScopes { .. } => Operation::ReadScopes,
            Self::SetBreakpoint { .. } => Operation::SetBreakpoint,
            Self::ReadLogs { .. } => Operation::ReadLogs,
            Self::Mutate { .. } => Operation::Mutate,
            Self::CaptureCpu { .. } => Operation::CaptureCpu,
            Self::CaptureHeap { .. } => Operation::CaptureHeap,
            Self::Pause => Operation::Pause,
            Self::Continue => Operation::Continue,
            Self::Step => Operation::Step,
            Self::Evaluate { .. } => Operation::Evaluate,
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

/// Allowlisted metadata only: never expression, result, header, environment, or grant values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub identity: Identity,
    pub request_id: String,
    pub operation: Operation,
    pub permission: Permission,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventData {
    Paused { pause: u64 },
    Resumed,
    Replaced { previous_epoch: u64 },
    Audit { audit: Audit },
    Terminal { error: Error },
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
    Request {
        request: Request,
    },
    Event {
        event: Event,
    },
    Source {
        chunk: SourceChunk,
    },
    Failure {
        #[schemars(required)]
        request_id: Option<String>,
        error: Error,
    },
}

pub fn schema() -> schemars::Schema {
    schemars::schema_for!(Wire)
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

/// Enforce byte limits before allocating a JSON tree. Decode failures never dispatch commands.
pub fn decode(bytes: &[u8]) -> Result<Wire, Error> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::MessageTooLarge);
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
    let version = match value["kind"].as_str() {
        Some("request") => &value["request"]["version"],
        Some("event") => &value["event"]["version"],
        Some("welcome") => &value["version"],
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
            if serde_json::from_value::<Operation>(request["command"]["op"].clone()).is_err() {
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
                Command::ReadSource {
                    source, max_bytes, ..
                } => {
                    validate_source(source)?;
                    if source.target != request.identity.target
                        || source.epoch != request.identity.epoch
                    {
                        return Err(Error::StaleEpoch);
                    }
                    if *max_bytes == 0 || *max_bytes > MAX_SOURCE_BYTES {
                        return Err(Error::SourceRange);
                    }
                }
                Command::SetBreakpoint { source, .. } => {
                    validate_source(source)?;
                    if source.target != request.identity.target
                        || source.epoch != request.identity.epoch
                    {
                        return Err(Error::StaleEpoch);
                    }
                }
                Command::ReadScopes { handle } | Command::Mutate { handle, .. } => {
                    validate_identity(&handle.identity)?;
                    if handle.identity != request.identity
                        || !identifier(&handle.id)
                        || !(1..=MAX_SAFE_INTEGER).contains(&handle.pause)
                    {
                        return Err(Error::StaleHandle);
                    }
                }
                _ => {}
            }
        }
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
                    if audit.identity != event.identity
                        || !identifier(&audit.request_id)
                        || audit.permission != audit.operation.permission()
                    {
                        return Err(Error::Malformed);
                    }
                }
                EventData::Paused { pause } if !(1..=MAX_SAFE_INTEGER).contains(pause) => {
                    return Err(Error::StaleHandle);
                }
                EventData::Replaced { previous_epoch }
                    if *previous_epoch == 0 || *previous_epoch >= event.identity.epoch =>
                {
                    return Err(Error::StaleEpoch);
                }
                _ => {}
            }
        }
        Wire::Source { chunk } => {
            validate_source(&chunk.source)?;
            let end = u64::from(chunk.offset) + chunk.text.len() as u64;
            if chunk.text.len() > MAX_SOURCE_BYTES as usize
                || end > u64::from(chunk.total_bytes)
                || chunk.eof != (end == u64::from(chunk.total_bytes))
                || (chunk.text.is_empty() && !chunk.eof)
            {
                return Err(Error::SourceRange);
            }
        }
        _ => {}
    }
    Ok(wire)
}

/// The same size/semantic checks apply to outbound data.
pub fn encode(wire: &Wire) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(wire).map_err(|_| Error::Malformed)?;
    decode(&bytes)?;
    Ok(bytes)
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
