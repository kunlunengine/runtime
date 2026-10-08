//! Plain-data semantic results, not a backend implementation or lossless protocol translator.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Capability, Error, Handle, Identity, Operation, SourceChunk, SourceDescriptor, SourceLocation,
    SourceMapping, Version,
};

pub const MAX_ENTRIES: u32 = 64;
pub const MAX_VALUE_BYTES: usize = 1_024;
pub const MAX_CAPTURE_BYTES: u32 = 16_384;
pub const MAX_PENDING_REQUESTS: usize = 16;
pub const MAX_REQUEST_HISTORY: usize = 256;
pub const MAX_RECORDING_BYTES: usize = crate::MAX_MESSAGE_BYTES * crate::MAX_EVENTS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Mock,
    Wip,
    Cdp,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Available,
    Terminated,
}

/// Display metadata is allowlisted. It is not an environment, header, or host-grant bag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub epoch: u64,
    pub name: String,
    pub backend: Backend,
    pub state: TargetState,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionState {
    Running,
    Paused { pause: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Requested,
    Breakpoint,
    Step,
    Exception,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackFrame {
    pub name: String,
    pub handle: Handle,
    pub location: Option<SourceLocation>,
}

/// Redacted values carry no preview or object handle that could recover the hidden value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteValue {
    Redacted,
    Undefined,
    Null,
    Boolean {
        value: bool,
    },
    /// JSON cannot represent NaN/Infinity; these use exact string spellings.
    Number {
        value: String,
    },
    String {
        text: String,
        truncated: bool,
    },
    Object {
        handle: Handle,
        preview: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    pub value: RemoteValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub name: String,
    pub handle: Handle,
    pub variables: Vec<Variable>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Breakpoint {
    pub id: String,
    pub requested: SourceLocation,
    /// An empty list means pending resolution, not an invented executable location.
    pub resolved: Vec<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    pub sequence: u64,
    pub level: LogLevel,
    pub message: RemoteValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSnapshot {
    pub format: DiagnosticFormat,
    /// Already redacted adapter output, never an implicit raw heap/object dump.
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancellationStatus {
    /// Work was stopped before any side effect; the original request fails with Cancelled.
    Cancelled,
    /// Work may already have executed. The original outcome must not be suppressed or retried.
    TooLate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    PendingRequests,
    RequestHistory,
    Collection,
    Snapshot,
    Recording,
}

/// Every success is operation-specific. Control acknowledgements do not imply a pause event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandResult {
    Attach {
        target: Target,
        state: ExecutionState,
        /// First sequence expected after attach; reconnect does not replay previous events.
        next_sequence: u64,
    },
    Detach,
    ListSources {
        sources: Vec<SourceDescriptor>,
        truncated: bool,
    },
    ReadSource {
        chunk: SourceChunk,
    },
    LookupSourceMap {
        mapping: Box<SourceMapping>,
    },
    ReadScopes {
        scopes: Vec<Scope>,
        truncated: bool,
    },
    SetBreakpoint {
        breakpoint: Breakpoint,
    },
    RemoveBreakpoint {
        breakpoint_id: String,
    },
    Pause,
    Continue,
    Step,
    Evaluate {
        value: RemoteValue,
    },
    Mutate {
        value: RemoteValue,
    },
    ReadLogs {
        entries: Vec<LogEntry>,
        truncated: bool,
    },
    CaptureCpu {
        snapshot: DiagnosticSnapshot,
    },
    CaptureHeap {
        snapshot: DiagnosticSnapshot,
    },
    Cancel {
        request_id: String,
        status: CancellationStatus,
    },
}

impl CommandResult {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Attach { .. } => Operation::Attach,
            Self::Detach => Operation::Detach,
            Self::ListSources { .. } => Operation::ListSources,
            Self::ReadSource { .. } => Operation::ReadSource,
            Self::LookupSourceMap { .. } => Operation::LookupSourceMap,
            Self::ReadScopes { .. } => Operation::ReadScopes,
            Self::SetBreakpoint { .. } => Operation::SetBreakpoint,
            Self::RemoveBreakpoint { .. } => Operation::RemoveBreakpoint,
            Self::Pause => Operation::Pause,
            Self::Continue => Operation::Continue,
            Self::Step => Operation::Step,
            Self::Evaluate { .. } => Operation::Evaluate,
            Self::Mutate { .. } => Operation::Mutate,
            Self::ReadLogs { .. } => Operation::ReadLogs,
            Self::CaptureCpu { .. } => Operation::CaptureCpu,
            Self::CaptureHeap { .. } => Operation::CaptureHeap,
            Self::Cancel { .. } => Operation::Cancel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseResult {
    Success { result: CommandResult },
    Failure { error: Error },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: Version,
    pub request_id: String,
    pub identity: Identity,
    pub result: ResponseResult,
}

pub(crate) fn validate_target(target: &Target) -> Result<(), Error> {
    if !crate::identifier(&target.id)
        || !(1..=crate::MAX_SAFE_INTEGER).contains(&target.epoch)
        || target.name.is_empty()
        || target.name.chars().any(char::is_control)
    {
        return Err(Error::InvalidIdentity);
    }
    crate::validate_text(&target.name)?;
    crate::validate_capabilities(&target.capabilities)
}

fn validate_value(value: &RemoteValue, identity: &Identity) -> Result<(), Error> {
    match value {
        RemoteValue::Number { value } => {
            if value.len() > 64
                || !(matches!(value.as_str(), "NaN" | "Infinity" | "-Infinity")
                    || value.parse::<f64>().is_ok_and(f64::is_finite))
            {
                return Err(Error::Malformed);
            }
        }
        RemoteValue::String { text, .. } => crate::validate_text(text)?,
        RemoteValue::Object { handle, preview } => {
            crate::validate_handle(handle, identity)?;
            crate::validate_text(preview)?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_log(entry: &LogEntry, identity: &Identity) -> Result<(), Error> {
    if !(1..=crate::MAX_SAFE_INTEGER).contains(&entry.sequence) {
        return Err(Error::EventOrder);
    }
    validate_value(&entry.message, identity)?;
    // Logs are durable; never persist pause-scoped object handles.
    if matches!(entry.message, RemoteValue::Object { .. }) {
        return Err(Error::StaleHandle);
    }
    Ok(())
}

pub(crate) fn validate_result(result: &CommandResult, identity: &Identity) -> Result<(), Error> {
    use crate::{source_for_identity, source_model, validate_collection, validate_handle};
    match result {
        CommandResult::Attach {
            target,
            state,
            next_sequence,
        } => {
            validate_target(target)?;
            if target.id != identity.target || target.epoch != identity.epoch {
                return Err(Error::StaleEpoch);
            }
            if !(1..=crate::MAX_SAFE_INTEGER).contains(next_sequence) {
                return Err(Error::EventOrder);
            }
            if target.state != TargetState::Available {
                return Err(Error::InvalidState);
            }
            if let ExecutionState::Paused { pause } = state {
                if !(1..=crate::MAX_SAFE_INTEGER).contains(pause) {
                    return Err(Error::StaleHandle);
                }
            }
        }
        CommandResult::ListSources { sources, .. } => {
            validate_collection(sources)?;
            let mut ids = std::collections::BTreeSet::new();
            for source in sources {
                source_model::validate_descriptor(source)?;
                source_for_identity(&source.source, identity)?;
                if !ids.insert(&source.source.id) {
                    return Err(Error::InvalidIdentity);
                }
            }
        }
        CommandResult::ReadSource { chunk } => {
            source_for_identity(&chunk.source, identity)?;
            let end = u64::from(chunk.offset) + chunk.text.len() as u64;
            if chunk.text.len() > crate::MAX_SOURCE_BYTES as usize
                || end > u64::from(chunk.total_bytes)
                || chunk.eof != (end == u64::from(chunk.total_bytes))
                || (chunk.text.is_empty() && !chunk.eof)
            {
                return Err(Error::SourceRange);
            }
        }
        CommandResult::LookupSourceMap { mapping } => {
            source_model::validate_mapping(mapping)?;
            source_for_identity(&mapping.generated.source, identity)?;
        }
        CommandResult::ReadScopes { scopes, .. } => {
            validate_collection(scopes)?;
            let mut pause = None;
            for scope in scopes {
                crate::validate_text(&scope.name)?;
                validate_handle(&scope.handle, identity)?;
                if pause.is_some_and(|pause| pause != scope.handle.pause) {
                    return Err(Error::StaleHandle);
                }
                pause = Some(scope.handle.pause);
                validate_collection(&scope.variables)?;
                for variable in &scope.variables {
                    crate::validate_text(&variable.name)?;
                    validate_value(&variable.value, identity)?;
                    if let RemoteValue::Object { handle, .. } = &variable.value {
                        if handle.pause != scope.handle.pause {
                            return Err(Error::StaleHandle);
                        }
                    }
                }
            }
        }
        CommandResult::SetBreakpoint { breakpoint } => {
            if !crate::identifier(&breakpoint.id) {
                return Err(Error::InvalidIdentity);
            }
            source_for_identity(&breakpoint.requested.source, identity)?;
            validate_collection(&breakpoint.resolved)?;
            for location in &breakpoint.resolved {
                source_for_identity(&location.source, identity)?;
            }
        }
        CommandResult::RemoveBreakpoint { breakpoint_id }
        | CommandResult::Cancel {
            request_id: breakpoint_id,
            ..
        } => {
            if !crate::identifier(breakpoint_id) {
                return Err(Error::InvalidIdentity);
            }
        }
        CommandResult::Evaluate { value } | CommandResult::Mutate { value } => {
            validate_value(value, identity)?;
        }
        CommandResult::ReadLogs { entries, .. } => {
            validate_collection(entries)?;
            let mut previous = 0;
            for entry in entries {
                validate_log(entry, identity)?;
                if entry.sequence <= previous {
                    return Err(Error::EventOrder);
                }
                previous = entry.sequence;
            }
        }
        CommandResult::CaptureCpu { snapshot } | CommandResult::CaptureHeap { snapshot } => {
            if snapshot.text.len() > MAX_CAPTURE_BYTES as usize {
                return Err(Error::LimitExceeded {
                    resource: Resource::Snapshot,
                });
            }
            if snapshot.format == DiagnosticFormat::Json
                && serde_json::from_str::<serde_json::Value>(&snapshot.text).is_err()
            {
                return Err(Error::Malformed);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Validate correlation and requested bounds before a client accepts a reply as success.
pub fn correlate(request: &crate::Request, response: &Response) -> Result<(), Error> {
    crate::encode(&crate::Wire::Request {
        request: request.clone(),
    })?;
    crate::encode(&crate::Wire::Response {
        response: response.clone(),
    })?;
    if request.version != response.version
        || request.request_id != response.request_id
        || request.identity != response.identity
    {
        return Err(Error::InvalidIdentity);
    }
    let result = match &response.result {
        ResponseResult::Success { result } => result,
        ResponseResult::Failure {
            error: Error::Unsupported { operation, .. },
        } if *operation != request.command.operation() => {
            return Err(Error::Malformed);
        }
        ResponseResult::Failure {
            error: Error::PermissionDenied { permission },
        } if request.command.operation() != Operation::Cancel
            && *permission != request.command.operation().permission() =>
        {
            return Err(Error::Malformed);
        }
        ResponseResult::Failure { .. } => return Ok(()),
    };
    if request.command.operation() != result.operation() {
        return Err(Error::Malformed);
    }
    use crate::Command;
    match (&request.command, result) {
        (
            Command::ReadSource {
                source,
                offset,
                max_bytes,
            },
            CommandResult::ReadSource { chunk },
        ) if &chunk.source != source
            || chunk.offset != *offset
            || chunk.text.len() > *max_bytes as usize =>
        {
            Err(Error::SourceRange)
        }
        (Command::LookupSourceMap { location }, CommandResult::LookupSourceMap { mapping })
            if &mapping.generated != location =>
        {
            Err(Error::SourceNotFound)
        }
        (Command::ReadScopes { handle }, CommandResult::ReadScopes { scopes, .. })
            if scopes
                .iter()
                .any(|scope| scope.handle.pause != handle.pause) =>
        {
            Err(Error::StaleHandle)
        }
        (
            Command::Mutate { handle, .. },
            CommandResult::Mutate {
                value:
                    RemoteValue::Object {
                        handle: returned, ..
                    },
            },
        ) if returned.pause != handle.pause => Err(Error::StaleHandle),
        (Command::ListSources { max_entries }, CommandResult::ListSources { sources, .. })
            if sources.len() > *max_entries as usize =>
        {
            Err(Error::LimitExceeded {
                resource: Resource::Collection,
            })
        }
        (Command::ReadLogs { max_entries }, CommandResult::ReadLogs { entries, .. })
            if entries.len() > *max_entries as usize =>
        {
            Err(Error::LimitExceeded {
                resource: Resource::Collection,
            })
        }
        (Command::CaptureCpu { max_bytes }, CommandResult::CaptureCpu { snapshot })
        | (Command::CaptureHeap { max_bytes }, CommandResult::CaptureHeap { snapshot })
            if snapshot.text.len() > *max_bytes as usize =>
        {
            Err(Error::LimitExceeded {
                resource: Resource::Snapshot,
            })
        }
        (
            Command::SetBreakpoint {
                source,
                line,
                column,
            },
            CommandResult::SetBreakpoint { breakpoint },
        ) if &breakpoint.requested.source != source
            || breakpoint.requested.line != *line
            || breakpoint.requested.column != *column =>
        {
            Err(Error::SourceNotFound)
        }
        (
            Command::RemoveBreakpoint {
                breakpoint_id: requested,
            },
            CommandResult::RemoveBreakpoint { breakpoint_id },
        ) if breakpoint_id != requested => Err(Error::InvalidIdentity),
        (
            Command::Cancel {
                request_id: requested,
            },
            CommandResult::Cancel { request_id, .. },
        ) if request_id != requested => Err(Error::InvalidIdentity),
        _ => Ok(()),
    }
}

/// Discovery is connection-scoped because a client does not know a target/epoch yet.
pub fn correlate_discovery(request: &crate::Wire, response: &crate::Wire) -> Result<(), Error> {
    crate::encode(request)?;
    crate::encode(response)?;
    let crate::Wire::Discover {
        version,
        request_id,
        max_targets,
    } = request
    else {
        return Err(Error::Malformed);
    };
    match response {
        crate::Wire::Discovered {
            version: returned_version,
            request_id: returned_id,
            targets,
            ..
        } => {
            if returned_version != version || returned_id != request_id {
                return Err(Error::InvalidIdentity);
            }
            if targets.len() > *max_targets as usize {
                return Err(Error::LimitExceeded {
                    resource: Resource::Collection,
                });
            }
            Ok(())
        }
        crate::Wire::Failure {
            request_id: Some(returned_id),
            ..
        } if returned_id == request_id => Ok(()),
        _ => Err(Error::InvalidIdentity),
    }
}
