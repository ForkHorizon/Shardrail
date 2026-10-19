use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub session_id: Uuid,
    pub schema_version: u32,
    pub kind: String,
    pub occurred_at_ms: i64,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ApprovalResponding {
    pub approval_id: Uuid,
    pub decision: ApprovalDecision,
    pub option_id: Option<String>,
    pub responder: String,
    pub responded_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ContractSchema {
    pub event: DurableEvent,
    pub approval_response: ApprovalResponding,
    pub approval_state: ApprovalState,
    pub session_lifecycle: SessionLifecycle,
}
