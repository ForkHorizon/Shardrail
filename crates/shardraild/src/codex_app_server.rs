//! Codex app-server adapter. One child process per session, JSON-RPC over stdio.
//!
//! `thread/start` must echo `approvalsReviewer: "user"`. `turn/start` in schema
//! 0.160.0 does not return that field; a present non-user value still fails closed.
//!
//! The approval policy is `untrusted`. With `on-request` under `workspace-write`,
//! Codex 0.160.0 applies in-workspace patches and sandboxed commands without
//! asking (observed in the S1-lite live run), so `on-request` is treated as
//! weaker than requested and fails closed. A file change that completes without
//! a prior approval request also fails the session closed.

use crate::{
    Daemon, DaemonApproval, ProviderCommand, ProviderOption, expire_approval, wall_clock_ms,
};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::{Value, json};
use shardrail_protocol::{ApprovalDecision, ApprovalState, ClientScope, DurableEvent, OptionClass};
use shardrail_session_core::{Approval, TurnOutcome, TurnState};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, Once};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

pub(crate) const SAFETY_CONFIG: &[&str] = &[
    "mcp_servers.node_repl.enabled=false",
    "mcp_servers.cua_repl.command=\"/usr/bin/false\"",
    "mcp_servers.cua_repl.enabled=false",
    "mcp_servers.computer-use.enabled=false",
    "mcp_servers.nexus-unity.enabled=false",
    "mcp_servers.unityMCP.enabled=false",
    "plugins.\"browser@openai-bundled\".enabled=false",
    "plugins.\"computer-use@openai-bundled\".enabled=false",
    "plugins.\"unified-computer-use@openai-bundled\".enabled=false",
    "plugins.\"codex-app-tools@openai-bundled\".enabled=false",
    "plugins.\"visualize@openai-bundled\".enabled=false",
    "features.js_repl=false",
    "web_search=\"disabled\"",
];

/// Per-run overrides that turn plugins, hooks, apps, and bundled skills off.
/// User skills are disabled by name in a later `skills.config` override.
pub(crate) const EXTENSION_CONFIG: &[&str] = &[
    "features.plugins=false",
    "features.hooks=false",
    "features.apps=false",
    "skills.bundled.enabled=false",
    "skills.include_instructions=false",
];

const MAX_SKILL_NAMES: usize = 64;
const MAX_SKILL_NAME_CHARS: usize = 256;
const MAX_SKILL_CONFIG_BYTES: usize = 4096;

const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
const FILE_APPROVAL: &str = "item/fileChange/requestApproval";
/// Provider events held before `session_created` is committed.
const MAX_HELD_EVENTS: usize = 64;
/// The only approval policy the daemon requests and accepts in an echo.
pub(crate) const APPROVAL_POLICY: &str = "untrusted";

static GROUPS: StdMutex<Vec<u32>> = StdMutex::new(Vec::new());
static SHUTDOWN_HOOK: Once = Once::new();

#[derive(Clone)]
pub(crate) struct CodexConfig {
    pub(crate) program: PathBuf,
    pub(crate) workdir: PathBuf,
    pub(crate) extra: Vec<String>,
    /// Directory object admitted for this session. Spawn uses it instead of the path.
    pub(crate) directory: Option<Arc<File>>,
}

#[derive(Clone, Debug)]
pub(crate) enum CodexStatus {
    Starting,
    Ready,
    RoutingRejected(String),
    Failed(String),
}

pub(crate) enum SessionCommand {
    StartTurn {
        command_id: Uuid,
        prompt: String,
        done: oneshot::Sender<Result<i64, String>>,
    },
    InterruptTurn {
        command_id: Uuid,
        done: oneshot::Sender<Result<i64, String>>,
    },
}

pub(crate) fn config_override_allowed(value: &str) -> bool {
    if value.is_empty() || value.contains(['\n', '\r', '\0']) {
        return false;
    }
    let Some((key, raw_value)) = value.split_once('=') else {
        return false;
    };
    !raw_value.is_empty() && matches!(key, "model" | "model_reasoning_effort")
}

pub(crate) fn command_arguments(extra: &[String], skills_config: Option<&str>) -> Vec<String> {
    let mut args = Vec::with_capacity(
        1 + (extra.len() + SAFETY_CONFIG.len() + EXTENSION_CONFIG.len() + 1) * 2,
    );
    args.push("app-server".to_owned());
    for item in extra {
        args.push("-c".to_owned());
        args.push(item.clone());
    }
    for item in SAFETY_CONFIG.iter().chain(EXTENSION_CONFIG.iter()) {
        args.push("-c".to_owned());
        args.push((*item).to_owned());
    }
    if let Some(skills_config) = skills_config {
        args.push("-c".to_owned());
        args.push(skills_config.to_owned());
    }
    args
}

/// `skills.config=[{name="...",enabled=false},...]` for every enabled name.
/// Control characters and an oversized list fail closed. A `name` selector
/// disables every copy of that name; `*` and path globs do not.
pub(crate) fn skills_config_override(names: &[String]) -> Result<String, String> {
    if names.len() > MAX_SKILL_NAMES {
        return Err("skill disable list is too long".into());
    }
    let mut parts = Vec::with_capacity(names.len());
    for name in names {
        if name.is_empty()
            || name.chars().count() > MAX_SKILL_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err("skill name is not safe to disable".into());
        }
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        parts.push(format!("{{name=\"{escaped}\",enabled=false}}"));
    }
    let value = format!("skills.config=[{}]", parts.join(","));
    if value.len() > MAX_SKILL_CONFIG_BYTES {
        return Err("skill disable list is too long".into());
    }
    Ok(value)
}

pub(crate) fn initialize_params() -> Value {
    json!({
        "clientInfo": {"name": "shardrail", "version": "0.1.0"},
        "capabilities": {}
    })
}

pub(crate) fn codex_decision(option_id: Option<&str>) -> &'static str {
    match option_id {
        Some("accept") => "accept",
        Some("decline") => "decline",
        Some("cancel") => "cancel",
        None => "cancel",
        Some(_) => "decline",
    }
}

pub(crate) fn thread_routing_accepted(
    result: &Value,
    workdir: &str,
) -> Result<ConfirmedSandbox, String> {
    if result.get("approvalsReviewer").and_then(Value::as_str) != Some("user") {
        return Err("approvals are not routed to the user".into());
    }
    if result.get("cwd").and_then(Value::as_str) != Some(workdir) {
        return Err("codex thread cwd does not match the configured workdir".into());
    }
    if !approval_policy_asks(result.get("approvalPolicy")) {
        return Err("codex approval policy is weaker than untrusted".into());
    }
    confirmed_sandbox(result.get("sandbox"))
        .ok_or_else(|| "codex sandbox is not workspace-write or read-only".into())
}

fn approval_policy_asks(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str) == Some(APPROVAL_POLICY)
}

/// A completed file change that never went through an approval request.
pub(crate) fn unapproved_file_change<'a>(
    item: &'a Value,
    requested: &HashMap<String, Uuid>,
) -> Option<&'a str> {
    if item.get("type").and_then(Value::as_str) != Some("fileChange") {
        return None;
    }
    let id = item.get("id").and_then(Value::as_str)?;
    match item.get("status").and_then(Value::as_str) {
        Some("declined" | "failed" | "inProgress") => None,
        _ if requested.contains_key(id) => None,
        _ => Some(id),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmedSandbox {
    WorkspaceWrite,
    ReadOnly,
}

impl ConfirmedSandbox {
    fn turn_type(self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspaceWrite",
            Self::ReadOnly => "readOnly",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::ReadOnly => "read-only",
        }
    }
}

fn sandbox_network_closed(object: &serde_json::Map<String, Value>) -> bool {
    matches!(object.get("networkAccess"), None | Some(Value::Bool(false)))
}

pub(crate) fn confirmed_sandbox(value: Option<&Value>) -> Option<ConfirmedSandbox> {
    match value {
        Some(Value::String(text)) if text == "workspace-write" => {
            Some(ConfirmedSandbox::WorkspaceWrite)
        }
        Some(Value::String(text)) if text == "read-only" => Some(ConfirmedSandbox::ReadOnly),
        Some(Value::Object(object)) => match object.get("type").and_then(Value::as_str) {
            Some("readOnly") if sandbox_network_closed(object) => Some(ConfirmedSandbox::ReadOnly),
            Some("workspaceWrite")
                if sandbox_network_closed(object)
                    && object
                        .get("writableRoots")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty) =>
            {
                Some(ConfirmedSandbox::WorkspaceWrite)
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn install_shutdown_kill() {
    SHUTDOWN_HOOK.call_once(|| {
        tokio::spawn(async {
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                return;
            };
            let Ok(mut interrupt) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            else {
                return;
            };
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
            kill_tracked_groups();
            std::process::exit(0);
        });
    });
}

fn kill_tracked_groups() {
    let Ok(groups) = GROUPS.lock() else {
        return;
    };
    for pid in groups.iter().copied() {
        kill_group(pid);
    }
}

fn kill_group(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
}

fn track_group(pid: u32) {
    if pid == 0 {
        return;
    }
    if let Ok(mut groups) = GROUPS.lock() {
        groups.push(pid);
    }
}

struct GroupGuard(u32);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        kill_group(self.0);
        if let Ok(mut groups) = GROUPS.lock() {
            groups.retain(|pid| *pid != self.0);
        }
    }
}

struct Outbound {
    line: String,
    done: Option<std::sync::mpsc::SyncSender<io::Result<()>>>,
}

enum PendingRpc {
    Initialize,
    SkillsList,
    HooksList,
    PluginsInstalled,
    AppsList,
    ThreadStart,
    TurnStart {
        done: oneshot::Sender<Result<i64, String>>,
        event_id: i64,
    },
    Interrupt {
        done: oneshot::Sender<Result<i64, String>>,
        event_id: i64,
    },
}

/// Runs extension checks before `commit`. `commit` records the session.
/// A failure before those checks returns [`CodexStatus::Failed`] and does not call `commit`.
pub(crate) async fn start_session<F, Fut>(
    daemon: Arc<Daemon>,
    config: CodexConfig,
    session_id: Uuid,
    commit: F,
) -> CodexStatus
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    let (approval_tx, approval_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (verified_tx, verified_rx) = oneshot::channel();
    let (commit_tx, commit_rx) = oneshot::channel();
    let session_tx = command_tx.clone();
    daemon
        .turn_commands
        .lock()
        .await
        .insert(session_id, command_tx.clone());
    daemon
        .codex_status
        .lock()
        .await
        .insert(session_id, CodexStatus::Starting);
    let daemon_task = Arc::clone(&daemon);
    let handle = tokio::spawn(async move {
        run_codex_session(
            daemon_task,
            config,
            session_id,
            approval_tx,
            approval_rx,
            command_tx,
            command_rx,
            ready_tx,
            verified_tx,
            commit_rx,
        )
        .await;
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    tokio::pin!(ready_rx);
    let admitted = tokio::select! {
        biased;
        status = &mut ready_rx => {
            return observe_ready(&daemon, session_id, &session_tx, status).await;
        }
        admitted = verified_rx => admitted,
        _ = tokio::time::sleep_until(deadline) => {
            handle.abort();
            return failed_status(
                &daemon,
                session_id,
                &session_tx,
                "codex session did not become ready",
            )
            .await;
        }
    };
    if admitted.is_err() {
        return wait_ready(
            &daemon,
            session_id,
            &session_tx,
            ready_rx,
            deadline,
            &handle,
        )
        .await;
    }
    let commit_result = commit().await;
    let commit_notice = match &commit_result {
        Ok(()) => Ok(()),
        Err(error) => Err(error.to_string()),
    };
    let _ = commit_tx.send(commit_notice);
    if let Err(error) = commit_result {
        return failed_status(&daemon, session_id, &session_tx, &error.to_string()).await;
    }
    wait_ready(
        &daemon,
        session_id,
        &session_tx,
        ready_rx,
        deadline,
        &handle,
    )
    .await
}

async fn wait_ready(
    daemon: &Arc<Daemon>,
    session_id: Uuid,
    session_tx: &mpsc::UnboundedSender<SessionCommand>,
    ready_rx: std::pin::Pin<&mut oneshot::Receiver<CodexStatus>>,
    deadline: tokio::time::Instant,
    handle: &tokio::task::JoinHandle<()>,
) -> CodexStatus {
    let status = tokio::select! {
        biased;
        status = ready_rx => status,
        _ = tokio::time::sleep_until(deadline) => {
            handle.abort();
            return failed_status(
                daemon,
                session_id,
                session_tx,
                "codex session did not become ready",
            )
            .await;
        }
    };
    observe_ready(daemon, session_id, session_tx, status).await
}

async fn observe_ready(
    daemon: &Arc<Daemon>,
    session_id: Uuid,
    session_tx: &mpsc::UnboundedSender<SessionCommand>,
    status: Result<CodexStatus, oneshot::error::RecvError>,
) -> CodexStatus {
    let status = match status {
        Ok(status) => status,
        Err(_) => CodexStatus::Failed("codex session ended before it was ready".into()),
    };
    if matches!(status, CodexStatus::Failed(_)) {
        return failed_status(daemon, session_id, session_tx, &status_error(&status)).await;
    }
    status
}

fn status_error(status: &CodexStatus) -> String {
    match status {
        CodexStatus::Failed(error) => error.clone(),
        CodexStatus::RoutingRejected(error) => error.clone(),
        CodexStatus::Starting => "codex session is not ready".into(),
        CodexStatus::Ready => "codex session is ready".into(),
    }
}

async fn failed_status(
    daemon: &Daemon,
    session_id: Uuid,
    session_tx: &mpsc::UnboundedSender<SessionCommand>,
    error: &str,
) -> CodexStatus {
    let status = CodexStatus::Failed(error.to_owned());
    daemon
        .codex_status
        .lock()
        .await
        .insert(session_id, status.clone());
    remove_turn_command(daemon, session_id, session_tx).await;
    status
}

async fn remove_turn_command(
    daemon: &Daemon,
    session_id: Uuid,
    sender: &mpsc::UnboundedSender<SessionCommand>,
) {
    let mut commands = daemon.turn_commands.lock().await;
    if commands
        .get(&session_id)
        .is_some_and(|current| current.same_channel(sender))
    {
        commands.remove(&session_id);
    }
}

enum TurnDispatch {
    Wait(oneshot::Receiver<Result<i64, String>>),
    Accept(i64),
    Reject(String),
}

pub(crate) async fn handle_turn_command(
    daemon: &Arc<Daemon>,
    outbound: &mpsc::Sender<Vec<u8>>,
    request: &shardrail_protocol::CommandRequest,
) -> io::Result<()> {
    let dispatch = dispatch_turn(daemon, request).await?;
    match dispatch {
        TurnDispatch::Reject(error) => reject(outbound, request.command_id, &error).await,
        TurnDispatch::Accept(event_id) => {
            accept(daemon, outbound, request.command_id, event_id).await
        }
        TurnDispatch::Wait(done_rx) => {
            match tokio::time::timeout(Duration::from_secs(20), done_rx).await {
                Ok(Ok(Ok(event_id))) => {
                    accept(daemon, outbound, request.command_id, event_id).await
                }
                Ok(Ok(Err(error))) => reject(outbound, request.command_id, &error).await,
                _ => reject(outbound, request.command_id, "codex turn command failed").await,
            }
        }
    }
}

async fn dispatch_turn(
    daemon: &Arc<Daemon>,
    request: &shardrail_protocol::CommandRequest,
) -> io::Result<TurnDispatch> {
    let _guard = daemon.command_lock.lock().await;
    let session_id = match request.payload.get("session_id").and_then(Value::as_str) {
        Some(text) => match Uuid::parse_str(text) {
            Ok(session_id) => session_id,
            Err(_) => {
                return Ok(TurnDispatch::Reject(
                    "turn command session_id is invalid".into(),
                ));
            }
        },
        None => {
            return Ok(TurnDispatch::Reject(
                "turn command requires session_id".into(),
            ));
        }
    };
    let prompt = if request.kind == "turn.start" {
        match request.payload.get("prompt").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => Some(text.to_owned()),
            _ => {
                return Ok(TurnDispatch::Reject(
                    "turn.start requires prompt text".into(),
                ));
            }
        }
    } else {
        None
    };
    if let Some(event_id) = applied_command(daemon, session_id, request.command_id).await? {
        return Ok(TurnDispatch::Accept(event_id));
    }
    let status = daemon.codex_status.lock().await.get(&session_id).cloned();
    let rejection = match status {
        Some(CodexStatus::Ready) => None,
        Some(CodexStatus::RoutingRejected(reason)) => Some(reason),
        Some(CodexStatus::Failed(error)) => Some(error),
        _ => Some("codex session is not ready".into()),
    };
    if let Some(error) = rejection {
        return Ok(TurnDispatch::Reject(error));
    }
    let Some(sender) = daemon.turn_commands.lock().await.get(&session_id).cloned() else {
        return Ok(TurnDispatch::Reject("codex session is not running".into()));
    };
    let (done_tx, done_rx) = oneshot::channel();
    let command = if request.kind == "turn.start" {
        SessionCommand::StartTurn {
            command_id: request.command_id,
            prompt: prompt.unwrap_or_default(),
            done: done_tx,
        }
    } else {
        SessionCommand::InterruptTurn {
            command_id: request.command_id,
            done: done_tx,
        }
    };
    if sender.send(command).is_err() {
        return Ok(TurnDispatch::Reject("codex session is not running".into()));
    }
    drop(_guard);
    Ok(TurnDispatch::Wait(done_rx))
}

async fn applied_command(
    daemon: &Daemon,
    session_id: Uuid,
    command_id: Uuid,
) -> io::Result<Option<i64>> {
    let journal = daemon.journal.lock().await;
    let events = journal
        .events_for_session(session_id)
        .map_err(io::Error::other)?;
    let needle = command_id.to_string();
    Ok(events.into_iter().rev().find_map(|event| {
        matches!(
            event.kind.as_str(),
            "turn_started" | "turn_interrupt" | "turn_command"
        )
        .then_some(())
        .and_then(|_| {
            (event.payload.get("command_id").and_then(Value::as_str) == Some(needle.as_str()))
                .then_some(event.event_id)
        })
    }))
}

async fn accept(
    daemon: &Daemon,
    outbound: &mpsc::Sender<Vec<u8>>,
    command_id: Uuid,
    event_id: i64,
) -> io::Result<()> {
    let receipt = daemon
        .journal
        .lock()
        .await
        .event_receipt(event_id)
        .map_err(io::Error::other)?;
    crate::write_frame(
        outbound,
        &shardrail_protocol::DaemonFrame::CommandAccepted {
            command_id,
            cursor: event_id,
            receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
            receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
        },
    )
    .await
}

async fn reject(outbound: &mpsc::Sender<Vec<u8>>, command_id: Uuid, error: &str) -> io::Result<()> {
    crate::write_frame(
        outbound,
        &shardrail_protocol::DaemonFrame::CommandRejected {
            command_id,
            error: error.to_owned(),
        },
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the task keeps its command sender so a later retry is not removed with it"
)]
async fn run_codex_session(
    daemon: Arc<Daemon>,
    config: CodexConfig,
    session_id: Uuid,
    approval_tx: mpsc::UnboundedSender<ProviderCommand>,
    approval_rx: mpsc::UnboundedReceiver<ProviderCommand>,
    command_tx: mpsc::UnboundedSender<SessionCommand>,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    ready_tx: oneshot::Sender<CodexStatus>,
    verified_tx: oneshot::Sender<()>,
    commit_rx: oneshot::Receiver<Result<(), String>>,
) {
    let mut ready = Some(ready_tx);
    let enabled = match preflight_enabled_skills(&config).await {
        Ok(names) => names,
        Err(reason) => {
            reject_extensions_now(&daemon, session_id, &mut ready, &reason).await;
            return;
        }
    };
    let skills_config = match skills_config_override(&enabled) {
        Ok(value) => value,
        Err(reason) => {
            reject_extensions_now(&daemon, session_id, &mut ready, &reason).await;
            return;
        }
    };
    let mut child = match spawn_child(&config, &skills_config) {
        Ok(child) => child,
        Err(error) => {
            let status = CodexStatus::Failed(format!("codex app-server failed to start: {error}"));
            set_status(&daemon, session_id, status.clone()).await;
            let _ = ready.take().map(|tx| tx.send(status));
            return;
        }
    };
    let pid = child.id().unwrap_or(0);
    track_group(pid);
    let _guard = GroupGuard(pid);
    let Some(stdin) = child.stdin.take() else {
        let status = CodexStatus::Failed("codex app-server stdin is unavailable".into());
        set_status(&daemon, session_id, status.clone()).await;
        let _ = ready.take().map(|tx| tx.send(status));
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let status = CodexStatus::Failed("codex app-server stdout is unavailable".into());
        set_status(&daemon, session_id, status.clone()).await;
        let _ = ready.take().map(|tx| tx.send(status));
        return;
    };
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut buffer = [0u8; 1024];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
    }
    let (writer_tx, writer_rx) = mpsc::unbounded_channel();
    tokio::spawn(write_loop(stdin, writer_rx));
    let routes = Arc::new(StdMutex::new(HashMap::<Uuid, Value>::new()));
    tokio::spawn(approval_loop(
        writer_tx.clone(),
        approval_rx,
        Arc::clone(&routes),
    ));
    let mut session = CodexRun {
        daemon: Arc::clone(&daemon),
        session_id,
        workdir: config.workdir.display().to_string(),
        writer: writer_tx,
        approval_tx,
        routes,
        thread_id: None,
        turn_id: None,
        turns_allowed: false,
        turn_open: false,
        process_marked: false,
        interrupt_when_turn_known: false,
        next_id: 1,
        pending: HashMap::new(),
        approval_by_rpc: HashMap::new(),
        approval_by_item: HashMap::new(),
        items: HashMap::new(),
        sandbox: ConfirmedSandbox::WorkspaceWrite,
        admission_verified: Some(verified_tx),
        admission_commit: Some(commit_rx),
        held_events: StdMutex::new(Some(Vec::new())),
        #[cfg(test)]
        fail_command_lookup: false,
    };
    crate::daemon_log::log(
        "INFO",
        "codex",
        &format!("codex session {session_id} spawned"),
    );
    session.send_request("initialize", initialize_params(), PendingRpc::Initialize);
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            biased;
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let frame = match serde_json::from_str::<Value>(&line) {
                            Ok(frame) => frame,
                            Err(_) => {
                                session.fail_transport(&mut ready, "codex app-server sent invalid JSON").await;
                                break;
                            }
                        };
                        if session.handle_frame(frame, &mut ready).await {
                            break;
                        }
                    }
                    Ok(None) => {
                        session.fail_transport(&mut ready, "codex app-server disconnected").await;
                        break;
                    }
                    Err(_) => {
                        session.fail_transport(&mut ready, "codex app-server disconnected").await;
                        break;
                    }
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    session.fail_transport(&mut ready, "codex session command channel closed").await;
                    break;
                };
                session.handle_command(command).await;
            }
            status = child.wait() => {
                let _ = status;
                session.fail_transport(&mut ready, "codex app-server exited").await;
                break;
            }
        }
    }
    session.release_approvals().await;
    remove_turn_command(&daemon, session_id, &command_tx).await;
    crate::daemon_log::log(
        "INFO",
        "codex",
        &format!("codex session {session_id} ended"),
    );
}

async fn write_loop(
    mut stdin: tokio::process::ChildStdin,
    mut inbound: mpsc::UnboundedReceiver<Outbound>,
) {
    use tokio::io::AsyncWriteExt;
    while let Some(message) = inbound.recv().await {
        let result = async {
            stdin.write_all(message.line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await;
        if let Some(done) = message.done {
            let _ = done.send(match &result {
                Ok(()) => Ok(()),
                Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
            });
        }
        if result.is_err() {
            break;
        }
    }
}

async fn approval_loop(
    writer: mpsc::UnboundedSender<Outbound>,
    mut inbound: mpsc::UnboundedReceiver<ProviderCommand>,
    routes: Arc<StdMutex<HashMap<Uuid, Value>>>,
) {
    while let Some(command) = inbound.recv().await {
        if !command.delay.is_zero() {
            tokio::time::sleep(command.delay).await;
        }
        let decision = codex_decision(command.option_id.as_deref());
        let targets = {
            let mut routes = routes.lock().unwrap_or_else(|error| error.into_inner());
            if command.approval_id.is_nil() && command.option_id.is_none() {
                routes.drain().map(|(_id, rpc)| rpc).collect::<Vec<_>>()
            } else if let Some(rpc) = routes.remove(&command.approval_id) {
                vec![rpc]
            } else {
                Vec::new()
            }
        };
        if targets.is_empty() {
            let _ = command.written.send(Err(io::Error::new(
                ErrorKind::NotFound,
                "approval is not pending on the codex session",
            )));
            continue;
        }
        let mut failed = false;
        for (index, rpc_id) in targets.iter().enumerate() {
            let line = json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "result": {"decision": decision}
            })
            .to_string();
            let last = index + 1 == targets.len();
            let done = last.then_some(command.written.clone());
            if writer.send(Outbound { line, done }).is_err() {
                failed = true;
                break;
            }
        }
        if failed {
            let _ = command.written.send(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "codex stdin closed",
            )));
        }
    }
}

struct CodexRun {
    daemon: Arc<Daemon>,
    session_id: Uuid,
    workdir: String,
    writer: mpsc::UnboundedSender<Outbound>,
    approval_tx: mpsc::UnboundedSender<ProviderCommand>,
    routes: Arc<StdMutex<HashMap<Uuid, Value>>>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    turns_allowed: bool,
    turn_open: bool,
    process_marked: bool,
    interrupt_when_turn_known: bool,
    next_id: i64,
    pending: HashMap<String, PendingRpc>,
    approval_by_rpc: HashMap<String, Uuid>,
    approval_by_item: HashMap<String, Uuid>,
    items: HashMap<String, Value>,
    sandbox: ConfirmedSandbox,
    admission_verified: Option<oneshot::Sender<()>>,
    admission_commit: Option<oneshot::Receiver<Result<(), String>>>,
    /// Provider events seen before `session_created` is committed. The session does
    /// not exist in the journal yet, so they are held and appended after admission.
    held_events: StdMutex<Option<Vec<(String, Value)>>>,
    #[cfg(test)]
    fail_command_lookup: bool,
}

impl CodexRun {
    fn send_request(&mut self, method: &str, params: Value, pending: PendingRpc) {
        let id = self.next_id;
        self.next_id += 1;
        let key = id.to_string();
        self.pending.insert(key, pending);
        let line =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        if self.writer.send(Outbound { line, done: None }).is_err() {
            self.pending.remove(&id.to_string());
        }
    }

    fn send_notification(&self, method: &str, params: Value) {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        let _ = self.writer.send(Outbound { line, done: None });
    }

    async fn handle_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::StartTurn {
                command_id,
                prompt,
                done,
            } => self.start_turn(command_id, prompt, done).await,
            SessionCommand::InterruptTurn { command_id, done } => {
                self.interrupt_turn(command_id, done).await
            }
        }
    }

    async fn start_turn(
        &mut self,
        command_id: Uuid,
        prompt: String,
        done: oneshot::Sender<Result<i64, String>>,
    ) {
        if !self.turns_allowed {
            let _ = done.send(Err("approvals are not routed to the user".into()));
            return;
        }
        let Some(thread_id) = self.thread_id.clone() else {
            let _ = done.send(Err("codex thread is not ready".into()));
            return;
        };
        if self.turn_state().await != Some(TurnState::Idle) || !self.process_marked {
            let _ = done.send(Err("a turn is already running".into()));
            return;
        }
        let payload = json!({
            "command_id": command_id,
            "thread_id": thread_id,
            "source": "codex"
        });
        if let Err(error) = self.transition("turn_started", payload).await {
            let _ = done.send(Err(error));
            return;
        }
        self.turn_open = true;
        let event_id = match self.command_event_id(command_id).await {
            Ok(event_id) => event_id,
            Err(error) => {
                self.finish_turn(TurnOutcome::Interrupted, "error").await;
                let _ = done.send(Err(error));
                return;
            }
        };
        let params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "approvalsReviewer": "user",
            "approvalPolicy": APPROVAL_POLICY,
            "cwd": self.workdir,
            "sandboxPolicy": {"type": self.sandbox.turn_type()}
        });
        self.send_request(
            "turn/start",
            params,
            PendingRpc::TurnStart { done, event_id },
        );
    }

    async fn interrupt_turn(
        &mut self,
        command_id: Uuid,
        done: oneshot::Sender<Result<i64, String>>,
    ) {
        let turn = self.turn_state().await;
        if !matches!(turn, Some(TurnState::Running | TurnState::Stalled)) {
            let _ = done.send(Err("no active turn".into()));
            return;
        }
        if let Err(error) = self
            .transition(
                "turn_cancel_requested",
                json!({"command_id": command_id, "source": "codex"}),
            )
            .await
        {
            let _ = done.send(Err(error));
            return;
        }
        self.cancel_pending_approvals().await;
        let event_id = match self
            .append_notice(
                "turn_interrupt",
                json!({
                    "command_id": command_id,
                    "thread_id": self.thread_id,
                    "turn_id": self.turn_id,
                    "source": "codex"
                }),
            )
            .await
        {
            Ok(event_id) => event_id,
            Err(error) => {
                let _ = done.send(Err(error));
                return;
            }
        };
        let (Some(thread_id), Some(turn_id)) = (self.thread_id.clone(), self.turn_id.clone())
        else {
            self.interrupt_when_turn_known = true;
            let _ = done.send(Ok(event_id));
            return;
        };
        self.send_request(
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": turn_id}),
            PendingRpc::Interrupt { done, event_id },
        );
    }

    /// Returns true when the session loop should stop.
    async fn handle_frame(
        &mut self,
        frame: Value,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
    ) -> bool {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let has_id = frame.get("id").is_some_and(|id| !id.is_null());
        if method.is_some() && has_id {
            self.server_request(frame).await;
            return false;
        }
        if method.is_none() && has_id {
            return self.server_response(frame, ready).await;
        }
        if let Some(method) = method {
            self.server_notification(&method, frame.get("params").cloned().unwrap_or(Value::Null))
                .await;
        }
        false
    }

    async fn server_response(
        &mut self,
        frame: Value,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
    ) -> bool {
        let Some(key) = frame.get("id").and_then(id_key) else {
            return false;
        };
        let Some(pending) = self.pending.remove(&key) else {
            return false;
        };
        if let Some(error) = frame.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("codex request failed");
            self.fail_pending(pending, ready, message).await;
            return matches!(
                self.codex_status_now().await,
                CodexStatus::Failed(_) | CodexStatus::RoutingRejected(_)
            ) && !self.turns_allowed
                && self.thread_id.is_none();
        }
        let result = frame.get("result").cloned().unwrap_or(Value::Null);
        match pending {
            PendingRpc::Initialize => {
                self.send_notification("initialized", json!({}));
                self.send_request(
                    "skills/list",
                    skills_list_params(&self.workdir),
                    PendingRpc::SkillsList,
                );
                false
            }
            PendingRpc::SkillsList => {
                if let Err(reason) = skills_are_clear(&result) {
                    self.reject_extensions(ready, reason).await;
                    return true;
                }
                self.send_request(
                    "hooks/list",
                    json!({"cwds": [self.workdir]}),
                    PendingRpc::HooksList,
                );
                false
            }
            PendingRpc::HooksList => {
                if let Err(reason) = hooks_are_clear(&result) {
                    self.reject_extensions(ready, reason).await;
                    return true;
                }
                self.send_request("plugin/installed", json!({}), PendingRpc::PluginsInstalled);
                false
            }
            PendingRpc::PluginsInstalled => {
                if let Err(reason) = plugins_are_clear(&result) {
                    self.reject_extensions(ready, reason).await;
                    return true;
                }
                self.send_request("app/list", json!({}), PendingRpc::AppsList);
                false
            }
            PendingRpc::AppsList => {
                if let Err(reason) = apps_are_clear(&result) {
                    self.reject_extensions(ready, reason).await;
                    return true;
                }
                if let Err(error) = self.wait_for_admission().await {
                    self.fail_transport(ready, &error).await;
                    return true;
                }
                if self
                    .journal_extension_event(
                        "codex_extensions_verified",
                        json!({"enabled_skills": 0, "hooks": 0, "enabled_plugins": 0, "apps": 0}),
                    )
                    .await
                    .is_err()
                {
                    self.reject_extensions(
                        ready,
                        "codex extension verification could not be journaled",
                    )
                    .await;
                    return true;
                }
                crate::daemon_log::log(
                    "INFO",
                    "codex",
                    &format!(
                        "codex extensions verified session {} skills=0 hooks=0 plugins=0 apps=0",
                        self.session_id
                    ),
                );
                self.send_request(
                    "thread/start",
                    json!({
                        "cwd": self.workdir,
                        "approvalPolicy": APPROVAL_POLICY,
                        "sandbox": "workspace-write",
                        "approvalsReviewer": "user"
                    }),
                    PendingRpc::ThreadStart,
                );
                false
            }
            PendingRpc::ThreadStart => match thread_routing_accepted(&result, &self.workdir) {
                Ok(sandbox) => {
                    self.sandbox = sandbox;
                    self.thread_id = result
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if self.thread_id.is_none() {
                        self.reject_routing(ready, "codex thread id is missing", &result)
                            .await;
                        return true;
                    }
                    if let Err(error) = self
                        .transition("process_started", json!({"provider": "codex"}))
                        .await
                    {
                        self.fail_transport(ready, &error).await;
                        return true;
                    }
                    self.process_marked = true;
                    self.turns_allowed = true;
                    let status = CodexStatus::Ready;
                    set_status(&self.daemon, self.session_id, status.clone()).await;
                    if let Some(tx) = ready.take() {
                        let _ = tx.send(status);
                    }
                    false
                }
                Err(error) => {
                    self.reject_routing(ready, &error, &result).await;
                    true
                }
            },
            PendingRpc::TurnStart { done, event_id } => {
                if let Some(reviewer) = result.get("approvalsReviewer")
                    && reviewer.as_str() != Some("user")
                {
                    self.turns_allowed = false;
                    set_status(
                        &self.daemon,
                        self.session_id,
                        CodexStatus::RoutingRejected("approvals are not routed to the user".into()),
                    )
                    .await;
                    let _ = self
                        .append_notice(
                            "approval_routing_rejected",
                            json!({"reason": "turn/start approvalsReviewer was not user", "result": result}),
                        )
                        .await;
                    self.finish_turn(TurnOutcome::Interrupted, "routing_rejected")
                        .await;
                    let _ = done.send(Err("approvals are not routed to the user".into()));
                    return false;
                }
                if let Some(policy) = result.get("approvalPolicy")
                    && policy.as_str() != Some(APPROVAL_POLICY)
                {
                    self.turn_id = result
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.fail_closed_turn(
                        "turn/start approvalPolicy is weaker than untrusted",
                        &result,
                    )
                    .await;
                    self.finish_turn(TurnOutcome::Interrupted, "routing_rejected")
                        .await;
                    let _ = done.send(Err("codex approval policy is weaker than untrusted".into()));
                    return false;
                }
                if result.get("approvalsReviewer").is_none() {
                    let _ = self
                        .append_notice(
                            "provider_notice",
                            json!({"method": "turn/start", "note": "response omitted approvalsReviewer; thread confirmation stands"}),
                        )
                        .await;
                }
                self.turn_id = result
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if self.interrupt_when_turn_known
                    && let (Some(thread_id), Some(turn_id)) =
                        (self.thread_id.clone(), self.turn_id.clone())
                {
                    self.interrupt_when_turn_known = false;
                    let line = json!({
                        "jsonrpc": "2.0",
                        "id": self.next_id,
                        "method": "turn/interrupt",
                        "params": {"threadId": thread_id, "turnId": turn_id}
                    })
                    .to_string();
                    self.next_id += 1;
                    let _ = self.writer.send(Outbound { line, done: None });
                }
                let _ = done.send(Ok(event_id));
                false
            }
            PendingRpc::Interrupt { done, event_id } => {
                let _ = self
                    .append_notice(
                        "provider_notice",
                        json!({"method": "turn/interrupt", "result": result}),
                    )
                    .await;
                let _ = done.send(Ok(event_id));
                false
            }
        }
    }

    async fn fail_pending(
        &mut self,
        pending: PendingRpc,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
        message: &str,
    ) {
        match pending {
            PendingRpc::Initialize | PendingRpc::ThreadStart => {
                self.fail_transport(ready, message).await;
            }
            PendingRpc::SkillsList => self.reject_extensions(ready, "skills/list failed").await,
            PendingRpc::HooksList => self.reject_extensions(ready, "hooks/list failed").await,
            PendingRpc::PluginsInstalled => {
                self.reject_extensions(ready, "plugin/installed failed")
                    .await;
            }
            PendingRpc::AppsList => self.reject_extensions(ready, "app/list failed").await,
            PendingRpc::TurnStart { done, .. } => {
                self.finish_turn(TurnOutcome::Interrupted, "turn_start_failed")
                    .await;
                let _ = done.send(Err(message.to_owned()));
            }
            PendingRpc::Interrupt { done, event_id } => {
                let _ = done.send(Ok(event_id));
            }
        }
    }

    async fn reject_routing(
        &mut self,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
        reason: &str,
        result: &Value,
    ) {
        self.turns_allowed = false;
        let _ = self
            .append_notice(
                "approval_routing_rejected",
                json!({"reason": reason, "result": result}),
            )
            .await;
        let status = CodexStatus::RoutingRejected(reason.to_owned());
        set_status(&self.daemon, self.session_id, status.clone()).await;
        if let Some(tx) = ready.take() {
            let _ = tx.send(status);
        }
    }

    /// Approvals are not reaching the user: block later turns, journal why, and
    /// interrupt the running turn.
    async fn fail_closed_turn(&mut self, reason: &str, evidence: &Value) {
        self.turns_allowed = false;
        set_status(
            &self.daemon,
            self.session_id,
            CodexStatus::RoutingRejected(reason.to_owned()),
        )
        .await;
        let _ = self
            .append_notice(
                "approval_routing_rejected",
                json!({"reason": reason, "result": evidence}),
            )
            .await;
        if let (Some(thread_id), Some(turn_id)) = (self.thread_id.clone(), self.turn_id.clone()) {
            let line = json!({
                "jsonrpc": "2.0",
                "id": self.next_id,
                "method": "turn/interrupt",
                "params": {"threadId": thread_id, "turnId": turn_id}
            })
            .to_string();
            self.next_id += 1;
            let _ = self.writer.send(Outbound { line, done: None });
        } else {
            self.interrupt_when_turn_known = true;
        }
    }

    async fn server_request(&mut self, frame: Value) {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let id = frame.get("id").cloned().unwrap_or(Value::Null);
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        if method == COMMAND_APPROVAL || method == FILE_APPROVAL {
            self.approval_request(&method, id, params).await;
            return;
        }
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "method not supported"}
        })
        .to_string();
        let _ = self.writer.send(Outbound { line, done: None });
        let _ = self
            .append_notice(
                "provider_request_refused",
                json!({"method": method, "id": id, "params": params}),
            )
            .await;
    }

    async fn approval_request(&mut self, method: &str, rpc_id: Value, params: Value) {
        if !self.turn_open {
            let line = json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "result": {"decision": "decline"}
            })
            .to_string();
            let _ = self.writer.send(Outbound { line, done: None });
            return;
        }
        let item_id = params
            .get("itemId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let grant_root = params
            .get("grantRoot")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        let network = network_label(params.get("networkApprovalContext"));
        let class = if grant_root.is_some() {
            OptionClass::BoundaryBreaking
        } else if network.is_some() {
            OptionClass::Widening
        } else {
            OptionClass::Once
        };
        let options = vec![
            ProviderOption {
                id: "accept".into(),
                class,
                decision: ApprovalDecision::Allow,
            },
            ProviderOption {
                id: "decline".into(),
                class: OptionClass::Once,
                decision: ApprovalDecision::Deny,
            },
        ];
        let mut approval = Approval::new(wall_clock_ms());
        approval.session_id = self.session_id;
        let approval_id = approval.id;
        if self
            .with_coordinator(|coordinator| coordinator.create_approval(approval.clone()))
            .await
            .is_err()
        {
            let line = json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "result": {"decision": "decline"}
            })
            .to_string();
            let _ = self.writer.send(Outbound { line, done: None });
            return;
        }
        let mut payload = json!({
            "approval_id": approval_id,
            "approval_kind": if method == FILE_APPROVAL { "file_change" } else { "command_execution" },
            "tool_call_id": item_id,
            "sandbox": self.sandbox.label(),
            "options": options.iter().map(|option| json!({
                "option_id": option.id,
                "name": if option.decision == ApprovalDecision::Allow { "Allow once" } else { "Deny" },
                "decision": option.decision,
                "class": option.class
            })).collect::<Vec<_>>()
        });
        if let Some(object) = payload.as_object_mut() {
            if let Some(command) = command_text(&params) {
                object.insert("command".into(), json!(command));
            }
            if let Some(cwd) = params
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                object.insert("cwd".into(), json!(cwd));
            }
            if let Some(actions) = params
                .get("commandActions")
                .filter(|value| !value.is_null())
            {
                object.insert("raw_input".into(), actions.clone());
            }
            if let Some(network) = &network {
                object.insert("network".into(), json!(network));
            }
            if let Some(root) = &grant_root {
                object.insert("locations".into(), json!(root));
            }
            if let Some(changes) = self
                .items
                .get(&item_id)
                .and_then(|item| item.get("changes"))
                .cloned()
            {
                object.insert("changes".into(), changes);
            }
            if let Some(reason) = params
                .get("reason")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                object.insert("agent_description".into(), json!({"description": reason}));
            }
        }
        if self
            .append_notice("approval_requested", payload)
            .await
            .is_err()
        {
            let line = json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "result": {"decision": "decline"}
            })
            .to_string();
            let _ = self.writer.send(Outbound { line, done: None });
            return;
        }
        if let Ok(mut routes) = self.routes.lock() {
            routes.insert(approval_id, rpc_id.clone());
        }
        if let Some(key) = id_key(&rpc_id) {
            self.approval_by_rpc.insert(key, approval_id);
        }
        if !item_id.is_empty() {
            self.approval_by_item.insert(item_id, approval_id);
        }
        let timeout_daemon = Arc::clone(&self.daemon);
        let timeout_secs = self.daemon.approval_timeout_secs;
        let timeout_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
            expire_approval(&timeout_daemon, approval_id).await;
        });
        self.daemon.approvals.lock().await.insert(
            approval_id,
            DaemonApproval {
                session_id: self.session_id,
                approval,
                options,
                provider: Some(self.approval_tx.clone()),
                timeout_task: Some(timeout_task.abort_handle()),
            },
        );
    }

    async fn server_notification(&mut self, method: &str, params: Value) {
        match method {
            "item/agentMessage/delta" => {
                let _ = self
                    .append_notice(
                        "agent_message_chunk",
                        json!({
                            "content": {"text": params.get("delta").and_then(Value::as_str).unwrap_or("")},
                            "thread_id": params.get("threadId"),
                            "turn_id": params.get("turnId"),
                            "item_id": params.get("itemId")
                        }),
                    )
                    .await;
            }
            "item/started" | "item/completed" => {
                if let Some(item) = params.get("item")
                    && let Some(id) = item.get("id").and_then(Value::as_str)
                {
                    self.items.insert(id.to_owned(), item.clone());
                    if method == "item/completed"
                        && unapproved_file_change(item, &self.approval_by_item).is_some()
                    {
                        let item = item.clone();
                        let _ = self
                            .append_notice(
                                "provider_item",
                                json!({"method": method, "params": params}),
                            )
                            .await;
                        self.fail_closed_turn(
                            "codex applied a file change without an approval request",
                            &item,
                        )
                        .await;
                        return;
                    }
                    if method == "item/completed"
                        && let Some(approval_id) = self.approval_by_item.get(id).copied()
                    {
                        spawn_ack(Arc::clone(&self.daemon), self.session_id, approval_id);
                    }
                }
                let _ = self
                    .append_notice("provider_item", json!({"method": method, "params": params}))
                    .await;
            }
            "turn/completed" => {
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                let outcome = if status == "completed" {
                    TurnOutcome::Provider
                } else {
                    TurnOutcome::Interrupted
                };
                self.finish_turn(outcome, status).await;
            }
            "error" => {
                let _ = self
                    .append_notice("provider_error", json!({"params": params}))
                    .await;
            }
            "serverRequest/resolved" => {
                if let Some(approval_id) = params
                    .get("requestId")
                    .and_then(id_key)
                    .and_then(|key| self.approval_by_rpc.get(&key).copied())
                {
                    spawn_ack(Arc::clone(&self.daemon), self.session_id, approval_id);
                }
                let _ = self
                    .append_notice(
                        "provider_notice",
                        json!({"method": method, "params": params}),
                    )
                    .await;
            }
            "turn/started" | "thread/tokenUsage/updated" | "turn/diff/updated" => {
                let _ = self
                    .append_notice(
                        "provider_notice",
                        json!({"method": method, "params": params}),
                    )
                    .await;
            }
            _ => {
                let _ = self
                    .append_notice(
                        "provider_notice",
                        json!({"method": method, "params": params}),
                    )
                    .await;
            }
        }
    }

    async fn finish_turn(&mut self, outcome: TurnOutcome, status: &str) {
        if !self.turn_open && self.turn_state().await == Some(TurnState::Idle) {
            return;
        }
        let thread_id = self.thread_id.clone();
        let turn_id = self.turn_id.clone();
        let status = status.to_owned();
        let _ = self
            .with_coordinator(move |coordinator| {
                if coordinator.session().turn == TurnState::Idle {
                    return Ok(());
                }
                coordinator.turn_finished(
                    wall_clock_ms(),
                    outcome,
                    false,
                    json!({
                        "source": "codex",
                        "provider_status": status,
                        "thread_id": thread_id,
                        "turn_id": turn_id
                    }),
                    || Ok(()),
                )?;
                Ok(())
            })
            .await;
        self.turn_open = false;
        self.turn_id = None;
        bump(&self.daemon);
    }

    fn decline_open_routes(&self) {
        let Ok(mut routes) = self.routes.lock() else {
            return;
        };
        for (_id, rpc_id) in routes.drain() {
            let line = json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "result": {"decision": "decline"}
            })
            .to_string();
            let _ = self.writer.send(Outbound { line, done: None });
        }
    }

    async fn fail_transport(
        &mut self,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
        message: &str,
    ) {
        self.decline_open_routes();
        self.finish_turn(TurnOutcome::Interrupted, "disconnected")
            .await;
        if self.process_marked {
            let _ = self
                .transition(
                    "process_exited",
                    json!({"source": "codex", "reason": message}),
                )
                .await;
            self.process_marked = false;
        }
        let pending = std::mem::take(&mut self.pending);
        for (_id, pending) in pending {
            match pending {
                PendingRpc::TurnStart { done, .. } | PendingRpc::Interrupt { done, .. } => {
                    let _ = done.send(Err(message.to_owned()));
                }
                PendingRpc::Initialize
                | PendingRpc::SkillsList
                | PendingRpc::HooksList
                | PendingRpc::PluginsInstalled
                | PendingRpc::AppsList
                | PendingRpc::ThreadStart => {}
            }
        }
        if ready.is_some() {
            let status = CodexStatus::Failed(message.to_owned());
            set_status(&self.daemon, self.session_id, status.clone()).await;
            if let Some(tx) = ready.take() {
                let _ = tx.send(status);
            }
        } else {
            set_status(
                &self.daemon,
                self.session_id,
                CodexStatus::Failed(message.to_owned()),
            )
            .await;
        }
        let _ = self
            .append_notice("provider_error", json!({"message": message}))
            .await;
    }

    async fn release_approvals(&self) {
        let mut approvals = self.daemon.approvals.lock().await;
        let ids = approvals
            .iter()
            .filter_map(|(id, entry)| (entry.session_id == self.session_id).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(entry) = approvals.get_mut(&id) {
                entry.provider.take();
                if let Some(task) = entry.timeout_task.take() {
                    task.abort();
                }
            }
        }
    }

    async fn cancel_pending_approvals(&self) {
        let ids = {
            let approvals = self.daemon.approvals.lock().await;
            approvals
                .iter()
                .filter_map(|(id, entry)| {
                    (entry.session_id == self.session_id
                        && entry.approval.state == ApprovalState::Pending)
                        .then_some(*id)
                })
                .collect::<Vec<_>>()
        };
        for approval_id in ids {
            self.cancel_approval(approval_id).await;
        }
    }

    async fn cancel_approval(&self, approval_id: Uuid) {
        let mut approvals = self.daemon.approvals.lock().await;
        let Some(entry) = approvals.get_mut(&approval_id) else {
            return;
        };
        if entry.approval.state != ApprovalState::Pending {
            return;
        }
        let Some(provider) = entry.provider.clone() else {
            return;
        };
        let Some(coordinator) = self
            .daemon
            .sessions
            .lock()
            .await
            .get(&self.session_id)
            .cloned()
        else {
            return;
        };
        let mut coordinator = coordinator.lock().await;
        let result = coordinator.respond_approval(
            &mut entry.approval,
            shardrail_coordinator::ApprovalResponseRequest {
                decision: ApprovalDecision::Cancel,
                authorized: true,
                now_ms: wall_clock_ms(),
                provider_limit_secs: None,
                option_id: Some("cancel".into()),
                responder: "codex-interrupt",
                option_class: OptionClass::Once,
                client_scope: ClientScope::Full,
            },
            move |_decision, option_id| {
                let (written, result) = std::sync::mpsc::sync_channel(1);
                provider
                    .send(ProviderCommand {
                        approval_id,
                        option_id: option_id.map(str::to_owned),
                        written: written.clone(),
                        delay: Duration::ZERO,
                    })
                    .map_err(|_| {
                        io::Error::new(ErrorKind::BrokenPipe, "codex approval channel closed")
                    })?;
                tokio::task::block_in_place(|| result.recv().map_err(io::Error::other))?
            },
        );
        if result.is_ok()
            && let Some(task) = entry.timeout_task.take()
        {
            task.abort();
            approvals.remove(&approval_id);
        }
        drop(coordinator);
        drop(approvals);
        bump(&self.daemon);
    }

    async fn turn_state(&self) -> Option<TurnState> {
        let coordinator = self
            .daemon
            .sessions
            .lock()
            .await
            .get(&self.session_id)
            .cloned()?;
        let coordinator = coordinator.lock().await;
        Some(coordinator.session().turn)
    }

    async fn codex_status_now(&self) -> CodexStatus {
        self.daemon
            .codex_status
            .lock()
            .await
            .get(&self.session_id)
            .cloned()
            .unwrap_or(CodexStatus::Failed(
                "codex session status is missing".into(),
            ))
    }

    async fn transition(&self, kind: &str, payload: Value) -> Result<(), String> {
        self.with_coordinator(|coordinator| {
            coordinator.transition(wall_clock_ms(), kind, payload, || Ok(()))?;
            Ok(())
        })
        .await?;
        bump(&self.daemon);
        Ok(())
    }

    async fn with_coordinator(
        &self,
        body: impl FnOnce(
            &mut shardrail_coordinator::Coordinator,
        ) -> Result<(), shardrail_coordinator::CoordinatorError>,
    ) -> Result<(), String> {
        let Some(coordinator) = self
            .daemon
            .sessions
            .lock()
            .await
            .get(&self.session_id)
            .cloned()
        else {
            return Err("session coordinator unavailable".into());
        };
        let mut coordinator = coordinator.lock().await;
        body(&mut coordinator).map_err(|error| error.to_string())
    }

    async fn reject_extensions(
        &mut self,
        ready: &mut Option<oneshot::Sender<CodexStatus>>,
        reason: &str,
    ) {
        self.turns_allowed = false;
        reject_extensions_now(&self.daemon, self.session_id, ready, reason).await;
    }

    async fn wait_for_admission(&mut self) -> Result<(), String> {
        if let Some(verified) = self.admission_verified.take()
            && verified.send(()).is_err()
        {
            return Err("codex session admission was dropped".into());
        }
        if let Some(commit) = self.admission_commit.take() {
            commit
                .await
                .map_err(|_| "codex session admission was dropped".to_owned())??;
        }
        let held = self
            .held_events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .unwrap_or_default();
        for (kind, payload) in held {
            self.append_notice(&kind, payload).await?;
        }
        Ok(())
    }

    async fn journal_extension_event(&self, kind: &str, payload: Value) -> Result<(), String> {
        journal_session_notice(&self.daemon, self.session_id, kind, payload).await
    }

    async fn append_notice(&self, kind: &str, payload: Value) -> Result<i64, String> {
        {
            let mut held = self
                .held_events
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(events) = held.as_mut() {
                if events.len() >= MAX_HELD_EVENTS {
                    return Err("too many provider events before session admission".into());
                }
                events.push((kind.to_owned(), payload));
                return Ok(0);
            }
        }
        let mut journal = self.daemon.journal.lock().await;
        let journal_epoch = journal.journal_epoch().map_err(|error| error.to_string())?;
        let host_id = journal.host_id().map_err(|error| error.to_string())?;
        let event_id = journal
            .append_event(&DurableEvent {
                event_id: 0,
                journal_epoch,
                host_id,
                session_id: self.session_id,
                schema_version: 1,
                kind: kind.to_owned(),
                occurred_at_ms: wall_clock_ms(),
                provider_ref: self.thread_id.clone(),
                causation: None,
                redaction_flags: Vec::new(),
                payload,
            })
            .map_err(|error| error.to_string())?;
        drop(journal);
        bump(&self.daemon);
        Ok(event_id)
    }

    async fn command_event_id(&self, command_id: Uuid) -> Result<i64, String> {
        #[cfg(test)]
        if self.fail_command_lookup {
            return Err("turn event was not committed".into());
        }
        let journal = self.daemon.journal.lock().await;
        let events = journal
            .events_for_session(self.session_id)
            .map_err(|error| error.to_string())?;
        let needle = command_id.to_string();
        events
            .into_iter()
            .rev()
            .find_map(|event| {
                (event.payload.get("command_id").and_then(Value::as_str) == Some(needle.as_str()))
                    .then_some(event.event_id)
            })
            .ok_or_else(|| "turn event was not committed".into())
    }
}

fn codex_command<'a>(
    config: &'a CodexConfig,
    skills_config: Option<&str>,
) -> io::Result<(Command, crate::session_workdir::InheritedDirFd<'a>)> {
    let Some(directory) = config.directory.as_deref() else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "session workdir is not pinned",
        ));
    };
    let inherited = crate::session_workdir::InheritedDirFd::acquire(directory)?;
    let mut command = Command::new(crate::session_workdir::chdir_helper());
    command
        .arg(inherited.raw().to_string())
        .arg(&config.program)
        .args(command_arguments(&config.extra, skills_config))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_AGENT_PID")
        .env_remove("GPG_AGENT_INFO");
    #[cfg(unix)]
    command.process_group(0);
    Ok((command, inherited))
}

fn spawn_child(config: &CodexConfig, skills_config: &str) -> io::Result<tokio::process::Child> {
    let (mut command, inherited) = codex_command(config, Some(skills_config))?;
    command.stderr(Stdio::piped());
    let child = command.spawn()?;
    drop(inherited);
    Ok(child)
}

async fn reject_extensions_now(
    daemon: &Daemon,
    session_id: Uuid,
    ready: &mut Option<oneshot::Sender<CodexStatus>>,
    reason: &str,
) {
    journal_extension_rejection(daemon, session_id, reason).await;
    fail_extensions(daemon, session_id, ready, reason).await;
}

async fn journal_extension_rejection(daemon: &Daemon, session_id: Uuid, reason: &str) {
    let in_memory = daemon.sessions.lock().await.contains_key(&session_id);
    if in_memory {
        let _ = journal_session_notice(
            daemon,
            session_id,
            "codex_extensions_rejected",
            json!({"reason": reason}),
        )
        .await;
        return;
    }
    let _ = append_unscoped_event(
        daemon,
        "codex_extensions_rejected",
        json!({"reason": reason, "session_snapshot": Value::Null}),
    )
    .await;
}

async fn append_unscoped_event(daemon: &Daemon, kind: &str, payload: Value) -> Result<(), String> {
    let mut journal = daemon.journal.lock().await;
    let journal_epoch = journal.journal_epoch().map_err(|error| error.to_string())?;
    let host_id = journal.host_id().map_err(|error| error.to_string())?;
    journal
        .append_event(&DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id: Uuid::nil(),
            schema_version: 1,
            kind: kind.to_owned(),
            occurred_at_ms: wall_clock_ms(),
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload,
        })
        .map_err(|error| error.to_string())?;
    drop(journal);
    bump(daemon);
    Ok(())
}

async fn fail_extensions(
    daemon: &Daemon,
    session_id: Uuid,
    ready: &mut Option<oneshot::Sender<CodexStatus>>,
    reason: &str,
) {
    crate::daemon_log::log(
        "WARN",
        "codex",
        &format!("codex extensions rejected session {session_id} reason={reason}"),
    );
    let status = CodexStatus::Failed(format!("codex extensions rejected: {reason}"));
    set_status(daemon, session_id, status.clone()).await;
    if let Some(tx) = ready.take() {
        let _ = tx.send(status);
    }
}

async fn journal_session_notice(
    daemon: &Daemon,
    session_id: Uuid,
    kind: &str,
    mut payload: Value,
) -> Result<(), String> {
    let snapshot = {
        let Some(coordinator) = daemon.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session coordinator unavailable".into());
        };
        let coordinator = coordinator.lock().await;
        serde_json::to_value(coordinator.session()).map_err(|error| error.to_string())?
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert("session_snapshot".to_owned(), snapshot);
    }
    let mut journal = daemon.journal.lock().await;
    let journal_epoch = journal.journal_epoch().map_err(|error| error.to_string())?;
    let host_id = journal.host_id().map_err(|error| error.to_string())?;
    journal
        .append_event(&DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id,
            schema_version: 1,
            kind: kind.to_owned(),
            occurred_at_ms: wall_clock_ms(),
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload,
        })
        .map_err(|error| error.to_string())?;
    drop(journal);
    bump(daemon);
    Ok(())
}

fn skills_list_params(workdir: &str) -> Value {
    json!({"cwds": [workdir], "forceReload": true})
}

async fn preflight_enabled_skills(config: &CodexConfig) -> Result<Vec<String>, String> {
    match tokio::time::timeout(Duration::from_secs(10), preflight_skills(config)).await {
        Ok(result) => result,
        Err(_) => Err("codex extension preflight timed out".into()),
    }
}

async fn preflight_skills(config: &CodexConfig) -> Result<Vec<String>, String> {
    let (mut command, inherited) = codex_command(config, None).map_err(|error| {
        crate::daemon_log::log(
            "WARN",
            "codex",
            &format!("codex extension preflight failed to start: {error}"),
        );
        "codex extension preflight failed to start".to_owned()
    })?;
    command.stderr(Stdio::null());
    let spawned = command.spawn();
    drop(inherited);
    let mut child = spawned.map_err(|error| {
        crate::daemon_log::log(
            "WARN",
            "codex",
            &format!("codex extension preflight failed to start: {error}"),
        );
        "codex extension preflight failed to start".to_owned()
    })?;
    let pid = child.id().unwrap_or(0);
    track_group(pid);
    let _guard = GroupGuard(pid);
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "codex extension preflight stdin is unavailable".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex extension preflight stdout is unavailable".to_owned())?;
    let mut lines = BufReader::new(stdout).lines();
    let workdir = config.workdir.display().to_string();
    write_rpc(&mut stdin, 1, "initialize", initialize_params()).await?;
    let _initialized = read_rpc_result(&mut lines, 1, "initialize").await?;
    write_notification(&mut stdin, "initialized", json!({})).await?;
    write_rpc(&mut stdin, 2, "skills/list", skills_list_params(&workdir)).await?;
    let listed = read_rpc_result(&mut lines, 2, "skills/list").await?;
    enabled_skill_names(&listed)
}

async fn write_rpc(
    stdin: &mut tokio::process::ChildStdin,
    id: i64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write_line(
        stdin,
        &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
    )
    .await
}

async fn write_notification(
    stdin: &mut tokio::process::ChildStdin,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write_line(
        stdin,
        &json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .await
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, line: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| "codex extension preflight disconnected".to_owned())?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| "codex extension preflight disconnected".to_owned())?;
    stdin
        .flush()
        .await
        .map_err(|_| "codex extension preflight disconnected".to_owned())?;
    Ok(())
}

async fn read_rpc_result(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: i64,
    method: &'static str,
) -> Result<Value, String> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|_| "codex extension preflight disconnected".to_owned())?
            .ok_or_else(|| "codex extension preflight disconnected".to_owned())?;
        if line.trim().is_empty() {
            continue;
        }
        let frame: Value = serde_json::from_str(&line)
            .map_err(|_| format!("codex extension preflight {method} response is invalid"))?;
        let Some(frame_id) = frame.get("id") else {
            continue;
        };
        if frame_id.as_i64() != Some(id) {
            continue;
        }
        if frame.get("error").is_some() {
            return Err(format!("{method} failed"));
        }
        return frame
            .get("result")
            .cloned()
            .ok_or_else(|| format!("codex extension preflight {method} response is invalid"));
    }
}

fn enabled_skill_names(result: &Value) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for skill in listed_skills(result).map_err(str::to_owned)? {
        let enabled = skill
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or("skills/list response is invalid")?;
        if !enabled {
            continue;
        }
        let name = skill
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or("skills/list response is invalid")?;
        if seen.insert(name.to_owned()) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn skills_are_clear(result: &Value) -> Result<(), &'static str> {
    for skill in listed_skills(result)? {
        match skill.get("enabled").and_then(Value::as_bool) {
            Some(false) => {}
            Some(true) => return Err("enabled skills remain"),
            None => return Err("skills/list response is invalid"),
        }
    }
    Ok(())
}

fn listed_skills(result: &Value) -> Result<Vec<&Value>, &'static str> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or("skills/list response is invalid")?;
    let mut skills = Vec::new();
    for entry in data {
        let errors = entry
            .get("errors")
            .and_then(Value::as_array)
            .ok_or("skills/list response is invalid")?;
        if !errors.is_empty() {
            return Err("skills/list reported errors");
        }
        let list = entry
            .get("skills")
            .and_then(Value::as_array)
            .ok_or("skills/list response is invalid")?;
        skills.extend(list);
    }
    Ok(skills)
}

fn hooks_are_clear(result: &Value) -> Result<(), &'static str> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or("hooks/list response is invalid")?;
    for entry in data {
        let errors = entry
            .get("errors")
            .and_then(Value::as_array)
            .ok_or("hooks/list response is invalid")?;
        if !errors.is_empty() {
            return Err("hooks/list reported errors");
        }
        if !entry.get("warnings").is_some_and(Value::is_array) {
            return Err("hooks/list response is invalid");
        }
        let hooks = entry
            .get("hooks")
            .and_then(Value::as_array)
            .ok_or("hooks/list response is invalid")?;
        if !hooks.is_empty() {
            return Err("hooks remain");
        }
    }
    Ok(())
}

fn plugins_are_clear(result: &Value) -> Result<(), &'static str> {
    let marketplaces = result
        .get("marketplaces")
        .and_then(Value::as_array)
        .ok_or("plugin/installed response is invalid")?;
    if let Some(errors) = result.get("marketplaceLoadErrors") {
        let errors = errors
            .as_array()
            .ok_or("plugin/installed response is invalid")?;
        if !errors.is_empty() {
            return Err("plugin/installed reported errors");
        }
    }
    for marketplace in marketplaces {
        let plugins = marketplace
            .get("plugins")
            .and_then(Value::as_array)
            .ok_or("plugin/installed response is invalid")?;
        for plugin in plugins {
            match plugin.get("enabled").and_then(Value::as_bool) {
                Some(false) => {}
                Some(true) => return Err("enabled plugins remain"),
                None => return Err("plugin/installed response is invalid"),
            }
        }
    }
    Ok(())
}

fn apps_are_clear(result: &Value) -> Result<(), &'static str> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or("app/list response is invalid")?;
    if data.is_empty() {
        Ok(())
    } else {
        Err("apps remain")
    }
}

fn bump(daemon: &Daemon) {
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
}

async fn set_status(daemon: &Daemon, session_id: Uuid, status: CodexStatus) {
    daemon.codex_status.lock().await.insert(session_id, status);
}

fn id_key(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(format!("s:{text}")),
        _ => None,
    }
}

fn command_text(params: &Value) -> Option<String> {
    nonempty(params.get("command")).or_else(|| {
        params
            .get("commandActions")
            .and_then(Value::as_array)
            .and_then(|actions| {
                actions
                    .iter()
                    .find_map(|action| nonempty(action.get("command")))
            })
    })
}

fn network_label(value: Option<&Value>) -> Option<String> {
    let object = value?.as_object()?;
    let host = object.get("host").and_then(Value::as_str)?;
    let protocol = object.get("protocol").and_then(Value::as_str).unwrap_or("");
    let label = if protocol.is_empty() {
        host.to_owned()
    } else {
        format!("{protocol} {host}")
    };
    Some(label)
}

fn nonempty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn ack_still_pending(state: Option<ApprovalState>) -> bool {
    !matches!(
        state,
        Some(
            ApprovalState::Allowed
                | ApprovalState::Denied
                | ApprovalState::Expired
                | ApprovalState::Withdrawn
                | ApprovalState::DeliveredUnconfirmed
        )
    )
}

fn spawn_ack(daemon: Arc<Daemon>, session_id: Uuid, approval_id: Uuid) {
    tokio::spawn(async move {
        for _ in 0..20 {
            let Some(coordinator) = daemon.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            let mut coordinator = coordinator.lock().await;
            match coordinator.acknowledge_approval(approval_id, wall_clock_ms()) {
                Ok(()) => {
                    drop(coordinator);
                    bump(&daemon);
                    return;
                }
                Err(shardrail_coordinator::CoordinatorError::InvalidApproval) => {
                    drop(coordinator);
                    let state = daemon
                        .journal
                        .lock()
                        .await
                        .load_approvals(session_id)
                        .ok()
                        .and_then(|approvals| {
                            approvals
                                .into_iter()
                                .find(|approval| approval.id == approval_id)
                                .map(|approval| approval.state)
                        });
                    if !ack_still_pending(state) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_overrides_follow_extra_config_and_disable_web_search() {
        let args = command_arguments(
            &["model_reasoning_effort=\"low\"".into()],
            Some("skills.config=[]"),
        );
        assert_eq!(args[0], "app-server");
        assert_eq!(args[1], "-c");
        assert_eq!(args[2], "model_reasoning_effort=\"low\"");
        let paired = args
            .iter()
            .skip(1)
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|chunk| chunk[1].as_str())
            .collect::<Vec<_>>();
        let mut expected = SAFETY_CONFIG.to_vec();
        expected.extend(EXTENSION_CONFIG);
        expected.push("skills.config=[]");
        assert!(paired.ends_with(expected.as_slice()));
        assert!(paired.contains(&"web_search=\"disabled\""));
        assert!(paired.contains(&"features.js_repl=false"));
        assert!(paired.contains(&"features.plugins=false"));
        let preflight = command_arguments(&[], None);
        assert!(
            !preflight
                .iter()
                .any(|arg| arg.starts_with("skills.config="))
        );
    }

    #[test]
    fn skill_disable_override_escapes_names_and_fails_closed() {
        assert_eq!(
            skills_config_override(&["cloudflare".into(), "pdf:pdf".into()]).unwrap(),
            "skills.config=[{name=\"cloudflare\",enabled=false},{name=\"pdf:pdf\",enabled=false}]"
        );
        assert_eq!(
            skills_config_override(&["a\"b\\c".into()]).unwrap(),
            "skills.config=[{name=\"a\\\"b\\\\c\",enabled=false}]"
        );
        assert_eq!(skills_config_override(&[]).unwrap(), "skills.config=[]");
        assert!(skills_config_override(&["has\nnewline".into()]).is_err());
        assert!(skills_config_override(&["has\u{007f}delete".into()]).is_err());
        assert!(skills_config_override(&[" ".repeat(MAX_SKILL_NAME_CHARS + 1)]).is_err());
        let long = "n".repeat(80);
        let names = vec![long; MAX_SKILL_NAMES];
        assert!(skills_config_override(&names).is_err());
    }

    #[test]
    fn initialize_does_not_opt_into_experimental_api() {
        let params = initialize_params();
        assert!(params.get("experimentalApi").is_none());
        assert!(params["capabilities"].get("experimentalApi").is_none());
        assert_eq!(params["clientInfo"]["name"], "shardrail");
    }

    #[test]
    fn completed_file_change_without_a_request_is_detected() {
        let mut requested = HashMap::new();
        let applied =
            json!({"type": "fileChange", "id": "f1", "status": "completed", "changes": []});
        assert_eq!(unapproved_file_change(&applied, &requested), Some("f1"));
        let unknown_status = json!({"type": "fileChange", "id": "f1", "changes": []});
        assert_eq!(
            unapproved_file_change(&unknown_status, &requested),
            Some("f1")
        );
        for status in ["declined", "failed", "inProgress"] {
            let item = json!({"type": "fileChange", "id": "f1", "status": status, "changes": []});
            assert_eq!(unapproved_file_change(&item, &requested), None);
        }
        let command = json!({"type": "commandExecution", "id": "c1", "status": "completed"});
        assert_eq!(unapproved_file_change(&command, &requested), None);
        requested.insert("f1".to_owned(), Uuid::nil());
        assert_eq!(unapproved_file_change(&applied, &requested), None);
    }

    #[test]
    fn decisions_never_grant_the_session() {
        assert_eq!(codex_decision(Some("accept")), "accept");
        assert_eq!(codex_decision(Some("decline")), "decline");
        assert_eq!(codex_decision(Some("cancel")), "cancel");
        assert_eq!(codex_decision(None), "cancel");
        assert_eq!(codex_decision(Some("acceptForSession")), "decline");
        assert_eq!(codex_decision(Some("allow_always")), "decline");
    }

    #[test]
    fn thread_echo_fails_closed_unless_user_routing_is_confirmed() {
        let workdir = "/tmp/s1-work";
        let weak = json!({
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "cwd": workdir,
            "sandbox": {"type": "workspaceWrite"},
            "thread": {"id": "thread-1"}
        });
        assert!(
            thread_routing_accepted(&weak, workdir)
                .unwrap_err()
                .contains("weaker")
        );
        let granular = json!({
            "approvalPolicy": {"granular": {"mcp_elicitations": true, "rules": true, "sandbox_approval": true}},
            "approvalsReviewer": "user",
            "cwd": workdir,
            "sandbox": {"type": "workspaceWrite"},
            "thread": {"id": "thread-1"}
        });
        assert!(thread_routing_accepted(&granular, workdir).is_err());
        let ok = json!({
            "approvalPolicy": "untrusted",
            "approvalsReviewer": "user",
            "cwd": workdir,
            "sandbox": {"type": "workspaceWrite"},
            "thread": {"id": "thread-1"}
        });
        assert_eq!(
            thread_routing_accepted(&ok, workdir).unwrap(),
            ConfirmedSandbox::WorkspaceWrite
        );
        let read_only = json!({
            "approvalPolicy": "untrusted",
            "approvalsReviewer": "user",
            "cwd": workdir,
            "sandbox": {"type": "readOnly"},
            "thread": {"id": "thread-1"}
        });
        assert_eq!(
            thread_routing_accepted(&read_only, workdir).unwrap(),
            ConfirmedSandbox::ReadOnly
        );
        let mut bad = ok.clone();
        bad["approvalsReviewer"] = json!("auto_review");
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad.as_object_mut().unwrap().remove("approvalsReviewer");
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["sandbox"] = json!({"type": "dangerFullAccess"});
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["sandbox"] = json!({"type": "workspaceWrite", "networkAccess": true});
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["sandbox"] = json!({"type": "readOnly", "networkAccess": true});
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["sandbox"] = json!({"type": "readOnly", "networkAccess": "restricted"});
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["sandbox"] = json!({"type": "readOnly", "networkAccess": false});
        assert_eq!(
            thread_routing_accepted(&bad, workdir).unwrap(),
            ConfirmedSandbox::ReadOnly
        );
        bad = ok.clone();
        bad["approvalPolicy"] = json!("never");
        assert!(thread_routing_accepted(&bad, workdir).is_err());
        bad = ok.clone();
        bad["approvalPolicy"] = json!({"granular": {"sandbox_approval": true}});
        assert!(thread_routing_accepted(&bad, workdir).is_err());
    }

    #[test]
    fn experimental_config_overrides_are_rejected() {
        assert!(config_override_allowed("model_reasoning_effort=\"low\""));
        assert!(config_override_allowed("model=\"gpt-5.4-mini\""));
        assert!(!config_override_allowed(
            "capabilities.experimentalApi=true"
        ));
        assert!(!config_override_allowed("mcp_servers.other.enabled=true"));
        assert!(!config_override_allowed("features.js_repl=true"));
        assert!(!config_override_allowed(
            "sandbox_mode=\"danger-full-access\""
        ));
        assert!(!config_override_allowed("model\n=oops"));
        assert!(!config_override_allowed("no-equals"));
        assert!(!config_override_allowed("model="));
    }

    #[test]
    fn acknowledgement_retries_stop_once_the_approval_is_terminal() {
        assert!(ack_still_pending(None));
        assert!(ack_still_pending(Some(ApprovalState::Pending)));
        assert!(ack_still_pending(Some(ApprovalState::Responding)));
        assert!(ack_still_pending(Some(ApprovalState::RespondedUnacked)));
        assert!(!ack_still_pending(Some(ApprovalState::Allowed)));
        assert!(!ack_still_pending(Some(ApprovalState::Denied)));
        assert!(!ack_still_pending(Some(ApprovalState::Expired)));
        assert!(!ack_still_pending(Some(ApprovalState::Withdrawn)));
        assert!(!ack_still_pending(Some(
            ApprovalState::DeliveredUnconfirmed
        )));
    }

    #[tokio::test]
    async fn missing_turn_event_rolls_the_turn_back_to_idle() {
        let directory = tempfile::tempdir().unwrap();
        let data = shardrail_coordinator::DaemonDataDir::open(directory.path()).unwrap();
        let session_id = Uuid::now_v7();
        let journal_path = data.journal_path();
        let mut coordinator = shardrail_coordinator::Coordinator::open(
            shardrail_journal::Journal::open(&journal_path).unwrap(),
            session_id,
        )
        .unwrap();
        coordinator
            .transition(0, "process_started", json!({}), || Ok::<(), io::Error>(()))
            .unwrap();
        let (event_generation, _) = tokio::sync::watch::channel(0);
        let daemon = Arc::new(crate::Daemon {
            data,
            journal: tokio::sync::Mutex::new(
                shardrail_journal::Journal::open(&journal_path).unwrap(),
            ),
            event_generation,
            command_lock: tokio::sync::Mutex::new(()),
            host_key: ed25519_dalek::SigningKey::from_bytes(&[0xab; 32]),
            fake_acp_path: None,
            fake_acp_no_reject: false,
            fake_acp_disconnect_after_permission: false,
            approval_timeout_secs: 30,
            test_write_ahead_delay_ms: 0,
            approvals: tokio::sync::Mutex::new(HashMap::new()),
            sessions: tokio::sync::Mutex::new(HashMap::from([(
                session_id,
                Arc::new(tokio::sync::Mutex::new(coordinator)),
            )])),
            codex: None,
            session_workdirs: Vec::new(),
            codex_status: tokio::sync::Mutex::new(HashMap::new()),
            turn_commands: tokio::sync::Mutex::new(HashMap::new()),
        });
        let (writer, _writer_rx) = mpsc::unbounded_channel();
        let (approval_tx, _approval_rx) = mpsc::unbounded_channel();
        let mut run = CodexRun {
            daemon,
            session_id,
            workdir: directory.path().display().to_string(),
            writer,
            approval_tx,
            routes: Arc::new(StdMutex::new(HashMap::new())),
            thread_id: Some("thread-1".into()),
            turn_id: None,
            turns_allowed: true,
            turn_open: false,
            process_marked: true,
            interrupt_when_turn_known: false,
            next_id: 1,
            pending: HashMap::new(),
            approval_by_rpc: HashMap::new(),
            approval_by_item: HashMap::new(),
            items: HashMap::new(),
            sandbox: ConfirmedSandbox::WorkspaceWrite,
            admission_verified: None,
            admission_commit: None,
            held_events: StdMutex::new(None),
            fail_command_lookup: true,
        };
        let (done_tx, done_rx) = oneshot::channel();
        run.start_turn(Uuid::now_v7(), "first".into(), done_tx)
            .await;
        let error = done_rx.await.unwrap().unwrap_err();
        assert!(error.contains("not committed"), "{error}");
        assert!(!run.turn_open);
        assert_eq!(run.turn_state().await, Some(TurnState::Idle));

        run.fail_command_lookup = false;
        let (done_tx, _done_rx) = oneshot::channel();
        run.start_turn(Uuid::now_v7(), "second".into(), done_tx)
            .await;
        assert!(run.turn_open);
        assert_eq!(run.turn_state().await, Some(TurnState::Running));
        assert!(
            run.pending
                .values()
                .any(|pending| matches!(pending, PendingRpc::TurnStart { .. }))
        );
    }
}
