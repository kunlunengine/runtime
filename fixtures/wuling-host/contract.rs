//! Preview Kunlun normalization, not frozen Wuling/Qingting wire conformance.
//! No credential, filesystem path, executable, or transport identity is renderer supplied.
//! Qingting Rust FFI/IPC selection and upstream wire ownership remain open; no ACP
//! or JavaScript SDK wrapper is assumed by this local serialized consumer fixture.
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE: usize = 4096;
pub const MAX_PAYLOAD: usize = 1024;
pub const MAX_FRAMES: usize = 4;
pub const MAX_STREAM_BYTES: usize = MAX_PAYLOAD * MAX_FRAMES;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    KunlunHostPreviewV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Desktop,
    MobileRemote,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub app: String,
    pub window: String,
    pub session: String,
    pub epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: Protocol,
    pub profile: Profile,
    pub request_id: String,
    pub binding: Binding,
    pub grant: String,
    pub resource: String,
    pub lease: u64,
    pub operation: Operation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    PtyOpen {},
    PtyInput { data: String },
    PtyResize { columns: u16, rows: u16 },
    ProcessStart {},
    Cancel { target: String },
    FsRead {},
    FsWrite { data: String },
    ProviderConnect {},
    ProviderDisconnect {},
    WindowOpen {},
    DeepLinkOpen {},
    SessionRead {},
    SessionCommand { command: String },
    ApprovalRespond { approval: String, allow: bool },
}

impl Operation {
    pub fn read_only(&self) -> bool {
        matches!(self, Self::FsRead {} | Self::SessionRead {})
    }

    pub fn requires_input_lease(&self) -> bool {
        self.local_exec() || matches!(self, Self::SessionCommand { .. })
    }

    pub fn local_exec(&self) -> bool {
        matches!(
            self,
            Self::PtyOpen {}
                | Self::PtyInput { .. }
                | Self::PtyResize { .. }
                | Self::ProcessStart {}
        )
    }

    pub fn valid(&self) -> bool {
        match self {
            Self::PtyResize { columns, rows } => {
                (1..=500).contains(columns) && (1..=300).contains(rows)
            }
            Self::PtyInput { data } | Self::FsWrite { data } => data.len() <= MAX_PAYLOAD,
            Self::SessionCommand { command } => command.len() <= MAX_PAYLOAD,
            _ => true,
        }
    }
}

pub fn decode(bytes: &[u8]) -> Result<Request, &'static str> {
    if bytes.len() > MAX_MESSAGE {
        return Err("message bound");
    }
    let request: Request = serde_json::from_slice(bytes).map_err(|_| "schema")?;
    if !request.operation.valid() {
        return Err("payload");
    }
    Ok(request)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionEvent {
    Output { data: String },
    CommandDone { command: String },
    ApprovalRequired { approval: String },
    AuthStatus { connection: String, state: Auth },
    Terminal {},
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    Pending,
    Authorized,
    Denied,
    Expired,
    Cancelled,
    ReauthorizationRequired,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub session: String,
    pub history: u64,
    pub sequence: u64,
    pub event: SessionEvent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub request: Request,
    pub frames: Vec<Frame>,
}

pub fn conversation(bytes: &[u8]) -> Result<Conversation, &'static str> {
    if bytes.len() > MAX_MESSAGE {
        return Err("message bound");
    }
    let value: Conversation = serde_json::from_slice(bytes).map_err(|_| "schema")?;
    if value.frames.len() > MAX_FRAMES
        || !value.request.operation.valid()
        || value.frames.iter().any(|frame| {
            serde_json::to_vec(&frame.event).map_or(true, |bytes| bytes.len() > MAX_PAYLOAD)
        })
    {
        return Err("payload");
    }
    Ok(value)
}
