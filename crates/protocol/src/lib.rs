use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OptionClass {
    Once,
    Widening,
    BoundaryBreaking,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientScope {
    Full,
    Limited,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Responding,
    RespondedUnacked,
    Allowed,
    Denied,
    Expired,
    Withdrawn,
    DeliveredUnconfirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    Open,
    Closing,
    Closed,
    Purging,
    Purged,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DurableEvent {
    pub event_id: i64,
    pub journal_epoch: Uuid,
    pub host_id: Uuid,
    pub session_id: Uuid,
    pub schema_version: u32,
    pub kind: String,
    pub occurred_at_ms: i64,
    pub provider_ref: Option<String>,
    pub causation: Option<String>,
    pub redaction_flags: Vec<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ApprovalResponding {
    pub approval_id: Uuid,
    pub decision: ApprovalDecision,
    pub option_id: Option<String>,
    pub responder: String,
    pub responded_at_ms: i64,
    pub option_class: OptionClass,
    pub client_scope: ClientScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ContractSchema {
    pub event: DurableEvent,
    pub approval_response: ApprovalResponding,
    pub approval_state: ApprovalState,
    pub session_lifecycle: SessionLifecycle,
    pub option_class: OptionClass,
    pub client_scope: ClientScope,
    pub client_hello: ClientHello,
    pub server_hello: ServerHello,
    pub command_request: CommandRequest,
    pub client_frame: ClientFrame,
    pub daemon_frame: DaemonFrame,
}

/// First frame on a local client connection. Public-key proof verification is
/// intentionally represented explicitly even while dev admission is UID based.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientHello {
    pub min_version: u32,
    pub max_version: u32,
    pub client_id: Uuid,
    pub public_key: Option<String>,
    pub proof: Option<String>,
    pub resume_after: i64,
    pub journal_epoch: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerHello {
    pub protocol_version: u32,
    pub host_id: Uuid,
    pub journal_epoch: Uuid,
    pub scope: ClientScope,
    pub cursor: i64,
    pub resync_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CommandRequest {
    pub command_id: Uuid,
    pub kind: String,
    pub expected_state: Option<String>,
    pub expected_turn_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientFrame {
    pub request: Option<CommandRequest>,
    pub resume_after: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DaemonFrame {
    pub kind: String,
    pub command_id: Option<Uuid>,
    pub event: Option<DurableEvent>,
    pub receipt_monotonic_ns: Option<String>,
    pub receipt_wall_clock_ms: Option<i64>,
    pub cursor: Option<i64>,
    pub error: Option<String>,
}
