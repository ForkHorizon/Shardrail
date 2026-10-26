//! Per-session checklist rows. A `verified` row is a check this daemon ran.

use crate::Daemon;
use crate::daemon_log;
use crate::wall_clock_ms;
use shardrail_protocol::{DurableEvent, SessionCheckResult, SessionCheckStatus};
use uuid::Uuid;

pub(crate) struct CheckDraft {
    pub(crate) item_id: &'static str,
    pub(crate) status: SessionCheckStatus,
    pub(crate) source: String,
    pub(crate) detail: String,
}

fn row(
    item_id: &'static str,
    status: SessionCheckStatus,
    source: &str,
    detail: &str,
) -> CheckDraft {
    CheckDraft {
        item_id,
        status,
        source: source.to_owned(),
        detail: detail.to_owned(),
    }
}

const CONTAINMENT: &str = "The child is started in its own process group. A macOS sandbox helper and a descendant snapshot are not checked.";
const CREDENTIALS: &str = "Codex spawn removes SSH_AUTH_SOCK, SSH_AGENT_PID, and GPG_AGENT_INFO from the child. Whether those sockets stay reachable by path is not checked.";
const WRITES: &str = "Codex workspace-write is the provider sandbox. Writes to Shardrail data, binaries, and provider install trees are not checked.";
const IDENTITY: &str = "Reachability of the Shardrail socket, host identity, ControlMaster sockets, and CLI token helpers is not checked.";
const BOUNDARY: &str = "The journal is not scanned for a boundary-breaking allow since protection was last established.";
const GAPS: &str = "Codex can run a built-in read-only command without an approval card. That gap is still open. A file change that skips approval is detected only after it completes.";

fn unverified_rest() -> Vec<CheckDraft> {
    vec![
        row(
            "containment",
            SessionCheckStatus::NotVerified,
            "not checked",
            CONTAINMENT,
        ),
        row(
            "credentials",
            SessionCheckStatus::NotVerified,
            "not checked",
            CREDENTIALS,
        ),
        row(
            "writes",
            SessionCheckStatus::NotVerified,
            "not checked",
            WRITES,
        ),
        row(
            "identity",
            SessionCheckStatus::NotVerified,
            "not checked",
            IDENTITY,
        ),
        row(
            "boundary",
            SessionCheckStatus::NotVerified,
            "not checked",
            BOUNDARY,
        ),
        row(
            "gaps",
            SessionCheckStatus::NotVerified,
            "static provider gap",
            GAPS,
        ),
    ]
}

pub(crate) fn fake_acp_checks() -> Vec<CheckDraft> {
    let mut checks = vec![
        row(
            "sandbox",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP does not apply or echo a sandbox profile.",
        ),
        row(
            "bypass",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP does not echo an approval policy, and this path does not scan for a bypass flag.",
        ),
        row(
            "routing",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP does not echo approvalsReviewer.",
        ),
        row(
            "config",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP has no provider approval config to pin.",
        ),
        row(
            "widening",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP mode changes are not checked.",
        ),
        row(
            "mcp",
            SessionCheckStatus::NotVerified,
            "fake ACP",
            "fake ACP does not start MCP servers and does not prove they are disabled.",
        ),
    ];
    checks.extend(unverified_rest());
    checks
}

pub(crate) fn codex_ready_checks(sandbox_label: &str) -> Vec<CheckDraft> {
    let mut checks = vec![
        row(
            "sandbox",
            SessionCheckStatus::Verified,
            "thread/start sandbox echo",
            &format!("thread/start echoed {sandbox_label} with network access closed"),
        ),
        row(
            "bypass",
            SessionCheckStatus::Verified,
            "thread/start approvalPolicy echo",
            "approvalPolicy echoed untrusted; the spawn argv has no --dangerously flag; acceptForSession is not a decision",
        ),
        row(
            "routing",
            SessionCheckStatus::Verified,
            "thread/start approvalsReviewer echo",
            "approvalsReviewer echoed user; an unknown server request is refused",
        ),
        row(
            "config",
            SessionCheckStatus::NotVerified,
            "not checked",
            "Per-run -c overrides are used and config writes are not issued. A hash of the provider approval config at session start, before every turn, and at session end is not taken.",
        ),
        row(
            "widening",
            SessionCheckStatus::Verified,
            "thread/settings/updated refusal",
            "a thread/settings/updated notification stops the session; none has been accepted",
        ),
        row(
            "mcp",
            SessionCheckStatus::Verified,
            "codex_extensions_verified mcp_servers 0",
            "zero MCP servers were enabled at start; any mcpServer activity stops the session",
        ),
    ];
    checks.extend(unverified_rest());
    checks
}

pub(crate) fn failed_check(item_id: &'static str, source: &str, detail: &str) -> CheckDraft {
    row(item_id, SessionCheckStatus::Failed, source, detail)
}

pub(crate) fn config_verified(point: &str, hash: &str) -> CheckDraft {
    row(
        "config",
        SessionCheckStatus::Verified,
        &format!("config snapshot {point} sha256:{hash}"),
        &format!("SHA-256 {hash}. Snapshot point {point}."),
    )
}

/// Echo checks run in order: routing, then bypass, then sandbox.
/// A failure leaves the later echo items not verified.
pub(crate) fn codex_echo_failure(item_id: &'static str, detail: &str) -> Vec<CheckDraft> {
    const ORDER: [&str; 3] = ["routing", "bypass", "sandbox"];
    let failed_at = ORDER.iter().position(|id| *id == item_id);
    let mut checks = codex_ready_checks("not echoed");
    for check in &mut checks {
        if check.item_id == item_id {
            *check = failed_check(item_id, "thread/start echo", detail);
            continue;
        }
        let Some(at) = failed_at else {
            continue;
        };
        let Some(position) = ORDER.iter().position(|id| *id == check.item_id) else {
            continue;
        };
        if position > at {
            let item_id = check.item_id;
            *check = row(
                item_id,
                SessionCheckStatus::NotVerified,
                "not checked",
                "blocked before this echo was accepted",
            );
        }
    }
    if let Some(widening) = checks.iter_mut().find(|check| check.item_id == "widening") {
        *widening = row(
            "widening",
            SessionCheckStatus::NotVerified,
            "not checked",
            "session was rejected on startup before settings monitoring began",
        );
    }
    checks
}

pub(crate) fn routing_failure_item(reason: &str) -> &'static str {
    if reason.contains("sandbox") {
        "sandbox"
    } else if reason.contains("policy") || reason.contains("untrusted") {
        "bypass"
    } else {
        "routing"
    }
}

pub(crate) async fn journal_checks(
    daemon: &Daemon,
    session_id: Uuid,
    checks: &[CheckDraft],
) -> Result<(), String> {
    let now = wall_clock_ms();
    {
        let mut journal = daemon.journal.lock().await;
        let journal_epoch = journal.journal_epoch().map_err(|error| error.to_string())?;
        let host_id = journal.host_id().map_err(|error| error.to_string())?;
        for check in checks {
            let payload = serde_json::to_value(SessionCheckResult {
                item_id: check.item_id.to_owned(),
                status: check.status.clone(),
                source: check.source.clone(),
                checked_at_ms: now,
                detail: check.detail.clone(),
            })
            .map_err(|error| error.to_string())?;
            journal
                .append_event(&DurableEvent {
                    event_id: 0,
                    journal_epoch,
                    host_id,
                    session_id,
                    schema_version: 1,
                    kind: "session_check_result".into(),
                    occurred_at_ms: now,
                    provider_ref: None,
                    causation: None,
                    redaction_flags: Vec::new(),
                    payload,
                })
                .map_err(|error| error.to_string())?;
        }
    }
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
    for check in checks {
        let status = serde_json::to_value(&check.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        daemon_log::log(
            "INFO",
            "session_check",
            &format!(
                "session_check session_id={session_id} item={} status={status} source={}",
                check.item_id,
                check
                    .source
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(120)
                    .collect::<String>()
            ),
        );
    }
    Ok(())
}
