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
    /// SHA-256 of the canonical approval content. Empty only for records written before binding.
    #[serde(default)]
    pub content_hash: String,
    /// Label of the client that decided, or `daemon` for a timeout.
    #[serde(default)]
    pub client_label: String,
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
    pub session_create_request: SessionCreateRequest,
    pub session_create_audit: SessionCreateAudit,
    pub approval_binding: ApprovalBinding,
    pub session_check_result: SessionCheckResult,
}

/// First frame on a client connection; authentication completes before streaming.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientHello {
    pub min_version: u32,
    pub max_version: u32,
    pub client_id: Uuid,
    pub public_key: String,
    pub nonce: String,
    pub resume_after: i64,
    pub journal_epoch: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerHello {
    pub protocol_version: u32,
    pub host_id: Uuid,
    pub journal_epoch: Uuid,
    pub public_key: String,
    pub nonce: String,
    pub signature: String,
    pub scope: ClientScope,
    pub cursor: i64,
    pub resync_required: bool,
}

/// Echo of the pending approval. The daemon rejects any field that does not match.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ApprovalBinding {
    pub approval_id: Uuid,
    pub session_id: Uuid,
    pub turn_id: String,
    pub provider_request_id: String,
    pub content_hash: String,
    pub journal_epoch: Uuid,
}

/// One per-session checklist row journaled by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionCheckStatus {
    Verified,
    Failed,
    NotVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionCheckResult {
    pub item_id: String,
    pub status: SessionCheckStatus,
    pub source: String,
    pub checked_at_ms: i64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResolutionRequest {
    pub approval_id: Uuid,
    pub option_id: String,
    pub content_hash: String,
    pub session_id: Uuid,
    pub turn_id: String,
    pub provider_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CommandRequest {
    pub command_id: Uuid,
    pub kind: String,
    pub expected_state: Option<String>,
    pub expected_turn_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

/// Optional `session.create` payload. A missing `workdir` uses the daemon default.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionCreateRequest {
    pub workdir: Option<String>,
}

/// Durable record of one `session.create` attempt. Denied attempts use a nil session id.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SessionCreateAudit {
    pub client_id: Uuid,
    pub client_label: String,
    pub scope: ClientScope,
    pub capability: String,
    pub workdir: Option<String>,
    pub outcome: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientFrame {
    Authenticate { signature: String },
    Request { request: CommandRequest },
    ApprovalResolution { request: ApprovalResolutionRequest },
    Resume { resume_after: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonFrame {
    Authenticated,
    CommandAccepted {
        command_id: Uuid,
        cursor: i64,
        receipt_monotonic_ns: Option<String>,
        receipt_wall_clock_ms: Option<i64>,
    },
    CommandRejected {
        command_id: Uuid,
        error: String,
    },
    ApprovalResolved {
        approval_id: Uuid,
    },
    DurableEvent {
        event: DurableEvent,
        cursor: i64,
        receipt_monotonic_ns: Option<String>,
        receipt_wall_clock_ms: Option<i64>,
    },
    Error {
        error: String,
    },
    ResyncRequired {
        cursor: i64,
    },
    /// Allowlisted session workdirs. Sent only to a limited client that holds `session.create`.
    SessionWorkdirs {
        workdirs: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_create_audit_and_workdir_frame_round_trip() {
        let audit = SessionCreateAudit {
            client_id: Uuid::nil(),
            client_label: "browser".into(),
            scope: ClientScope::Limited,
            capability: "session.create".into(),
            workdir: Some("/tmp/work".into()),
            outcome: "denied".into(),
            reason: Some("workdir".into()),
        };
        let text = serde_json::to_string(&audit).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionCreateAudit>(&text).unwrap(),
            audit
        );

        let request = SessionCreateRequest {
            workdir: Some("/tmp/work".into()),
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionCreateRequest>(&text).unwrap(),
            request
        );

        let frame = DaemonFrame::SessionWorkdirs {
            workdirs: vec!["/tmp/work".into()],
        };
        let text = serde_json::to_string(&frame).unwrap();
        assert!(text.contains("\"kind\":\"session_workdirs\""));
        assert_eq!(serde_json::from_str::<DaemonFrame>(&text).unwrap(), frame);
    }
}
