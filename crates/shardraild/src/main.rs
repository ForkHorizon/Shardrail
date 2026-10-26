use clap::{Parser, Subcommand, ValueEnum};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, fchmod, fstat};
use nix::sys::statfs::statfs;
use serde_json::json;
use sha2::{Digest, Sha256};
use shardrail_coordinator::{
    Coordinator, DaemonDataDir, open_private_data_directory, pin_data_directory,
};
use shardrail_journal::Journal;
use shardrail_protocol::{
    ApprovalBinding, ApprovalDecision, ApprovalResolutionRequest, DurableEvent, OptionClass,
};
use shardrail_protocol::{ClientFrame, ClientHello, ClientScope, DaemonFrame, ServerHello};
use shardrail_protocol::{CommandRequest, SessionCreateAudit};
use shardrail_session_core::{Approval, Session};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::AsFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::process::{ChildStdin, Command as TokioCommand};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use uuid::Uuid;

mod approval_binding;
mod codex_app_server;
mod daemon_log;
mod session_checks;
mod session_workdir;

const PROTOCOL_VERSION: u32 = 1;
const CLIENT_QUEUE_CAPACITY: usize = 32;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(name = "shardraild", about = "Shardrail local coordinator daemon")]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,
    #[arg(long)]
    no_file_log: bool,
    #[arg(long)]
    fake_acp_path: Option<PathBuf>,
    #[arg(long)]
    fake_acp_no_reject: bool,
    #[arg(long, hide = true)]
    fake_acp_disconnect_after_permission: bool,
    #[arg(long, default_value_t = 180)]
    approval_timeout_secs: u64,
    #[arg(long, default_value_t = 0)]
    test_write_ahead_delay_ms: u64,
    #[arg(long, value_name = "127.0.0.1:PORT")]
    browser_listen: Option<SocketAddr>,
    #[arg(long, requires = "browser_listen")]
    browser_assets_dir: Option<PathBuf>,
    /// Codex CLI used as `codex app-server` for one child process per session.
    #[arg(long, value_name = "PATH")]
    codex_app_server: Option<PathBuf>,
    /// Session workdir allowlist entry. Repeatable. Canonicalized at startup.
    #[arg(long, value_name = "DIR")]
    session_workdir: Vec<PathBuf>,
    /// Working directory for Codex threads. Adds the default allowlist entry.
    #[arg(long, value_name = "DIR")]
    codex_workdir: Option<PathBuf>,
    /// Extra Codex `-c` override, placed before the mandatory safety overrides.
    #[arg(long = "codex-config", value_name = "KEY=VALUE")]
    codex_config: Vec<String>,
    /// Exit when stdin reaches EOF, after stopping Codex children.
    /// The parent holds the write end of that pipe.
    #[arg(long)]
    exit_with_stdin: bool,
    /// Checkout refused as a session workdir. Defaults to this build's repository.
    #[arg(long, value_name = "DIR")]
    shardrail_root: Option<PathBuf>,
    /// Test hook: replace the pinned workdir path before the helper opens it.
    #[arg(long, hide = true)]
    test_replace_workdir_after_pin: bool,
    #[command(subcommand)]
    command: Option<DaemonCommand>,
}

struct BrowserListenerOptions {
    address: Option<SocketAddr>,
    assets_dir: Option<PathBuf>,
}

async fn resolve_approval(
    daemon: &Daemon,
    client: &AuthorizedClient,
    request: ApprovalResolutionRequest,
) -> Result<(), String> {
    let current_epoch = {
        let journal = daemon.journal.lock().await;
        journal.journal_epoch().map_err(|error| error.to_string())?
    };
    let snapshot = {
        let approvals = daemon.approvals.lock().await;
        approvals.get(&request.approval_id).map(|entry| {
            (
                entry.session_id,
                entry.binding.clone(),
                entry.approval.state.clone(),
                entry.provider.is_some(),
                entry.options.clone(),
            )
        })
    };
    let Some((session_id, binding, state, has_provider, options)) = snapshot else {
        return reject_missing(daemon, client, &request).await;
    };
    if !has_provider || state != shardrail_protocol::ApprovalState::Pending {
        let reason = if has_provider {
            "already_decided"
        } else {
            "unavailable"
        };
        return reject_decision(daemon, client, &request, session_id, reason).await;
    }
    let class = options
        .iter()
        .find(|option| option.id == request.option_id)
        .map(|option| option.class);
    if let Some(reason) = approval_binding::decision_rejection(
        client.allow_approvals,
        client.scope,
        &request.option_id,
        class,
    ) {
        return reject_decision(daemon, client, &request, session_id, reason).await;
    }
    let current_turn = daemon.session_turns.lock().await.get(&session_id).cloned();
    if let Some(reason) = approval_binding::binding_mismatch(
        &binding,
        request.session_id,
        &request.turn_id,
        &request.provider_request_id,
        &request.content_hash,
        current_epoch,
        current_turn.as_deref(),
    ) {
        return reject_decision(daemon, client, &request, session_id, reason).await;
    }

    let mut approvals = daemon.approvals.lock().await;
    let Some(entry) = approvals.get_mut(&request.approval_id) else {
        drop(approvals);
        return reject_missing(daemon, client, &request).await;
    };
    if entry.provider.is_none()
        || entry.approval.state != shardrail_protocol::ApprovalState::Pending
    {
        drop(approvals);
        return reject_decision(daemon, client, &request, session_id, "already_decided").await;
    }
    let Some(option) = entry
        .options
        .iter()
        .find(|option| option.id == request.option_id)
        .cloned()
    else {
        drop(approvals);
        return reject_decision(daemon, client, &request, session_id, "option_not_offered").await;
    };
    if let Some(reason) = approval_binding::decision_rejection(
        client.allow_approvals,
        client.scope,
        &request.option_id,
        Some(option.class),
    ) {
        drop(approvals);
        return reject_decision(daemon, client, &request, session_id, reason).await;
    }
    if let Some(reason) = approval_binding::binding_mismatch(
        &entry.binding,
        request.session_id,
        &request.turn_id,
        &request.provider_request_id,
        &request.content_hash,
        current_epoch,
        current_turn.as_deref(),
    ) {
        drop(approvals);
        return reject_decision(daemon, client, &request, session_id, reason).await;
    }
    let sender = entry
        .provider
        .as_ref()
        .cloned()
        .ok_or_else(|| "approval is already resolved".to_owned())?;
    entry.approval.content_hash = entry.binding.content_hash.clone();
    entry.approval.client_label = sanitize_label(&client.label);
    let content_hash = entry.approval.content_hash.clone();
    let coordinator = daemon
        .sessions
        .lock()
        .await
        .get(&entry.session_id)
        .cloned()
        .ok_or_else(|| "session coordinator unavailable".to_owned())?;
    let mut coordinator = coordinator.lock().await;
    let write_delay = Duration::from_millis(daemon.test_write_ahead_delay_ms);
    let approval_id = request.approval_id;
    let option_id = option.id.clone();
    let result = coordinator.respond_approval(
        &mut entry.approval,
        shardrail_coordinator::ApprovalResponseRequest {
            decision: option.decision,
            authorized: true,
            now_ms: wall_clock_ms(),
            provider_limit_secs: None,
            option_id: Some(option.id),
            responder: &client.client_id.to_string(),
            option_class: option.class,
            client_scope: client.scope,
        },
        move |_decision, option_id| {
            let (written, result) = std::sync::mpsc::sync_channel(1);
            sender
                .send(ProviderCommand {
                    approval_id,
                    option_id: option_id.map(str::to_owned),
                    written,
                    delay: write_delay,
                })
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "fake provider disconnected")
                })?;
            tokio::task::block_in_place(|| result.recv().map_err(io::Error::other))?
        },
    );
    if let Err(error) = result {
        if matches!(
            error,
            shardrail_coordinator::CoordinatorError::InvalidApproval
        ) {
            return Err("approval is already resolved".into());
        }
        if let Some(provider) = entry.provider.take() {
            let _ = provider.send(ProviderCommand::cancel(approval_id));
        }
        return Err(error.to_string());
    }
    if let Some(task) = entry.timeout_task.take() {
        task.abort();
    }
    approvals.remove(&approval_id);
    drop(approvals);
    drop(coordinator);
    daemon_log::log(
        "INFO",
        "approval",
        &format!(
            "approval decided approval_id={approval_id} session_id={session_id} client_id={} client_label={} scope={:?} option_id={} content_hash={content_hash}",
            client.client_id,
            sanitize_label(&client.label),
            client.scope,
            option_id
                .chars()
                .filter(|character| !character.is_control())
                .take(40)
                .collect::<String>(),
        ),
    );
    Ok(())
}

async fn missing_approval_reason(
    daemon: &Daemon,
    approval_id: Uuid,
) -> Result<(Option<Uuid>, &'static str), String> {
    let disposition = {
        let journal = daemon.journal.lock().await;
        journal
            .approval_disposition(approval_id)
            .map_err(|error| error.to_string())?
    };
    Ok(match disposition {
        Some((session_id, state, _)) if state == "expired" => (Some(session_id), "expired"),
        Some((session_id, state, reason)) if state == "withdrawn" => {
            let reason = match reason.as_deref() {
                Some("turn_end") => "stale_turn",
                _ => "abandoned",
            };
            (Some(session_id), reason)
        }
        Some((session_id, state, _))
            if matches!(
                state.as_str(),
                "allowed" | "denied" | "responding" | "responded_unacked" | "delivered_unconfirmed"
            ) =>
        {
            (Some(session_id), "already_decided")
        }
        Some((session_id, _, _)) => (Some(session_id), "unavailable"),
        None => (None, "unknown_approval"),
    })
}

async fn reject_missing(
    daemon: &Daemon,
    client: &AuthorizedClient,
    request: &ApprovalResolutionRequest,
) -> Result<(), String> {
    let (stored_session, reason) = missing_approval_reason(daemon, request.approval_id).await?;
    reject_decision(
        daemon,
        client,
        request,
        stored_session.unwrap_or(request.session_id),
        reason,
    )
    .await
}

async fn reject_decision(
    daemon: &Daemon,
    client: &AuthorizedClient,
    request: &ApprovalResolutionRequest,
    session_id: Uuid,
    reason: &str,
) -> Result<(), String> {
    let label = sanitize_label(&client.label);
    let option_id: String = request
        .option_id
        .chars()
        .filter(|character| !character.is_control())
        .take(40)
        .collect();
    let payload = json!({
        "approval_id": request.approval_id,
        "session_id": session_id,
        "turn_id": request.turn_id,
        "provider_request_id": request.provider_request_id,
        "content_hash": request.content_hash,
        "reason": reason,
        "client_id": client.client_id,
        "client_label": label,
        "client_scope": client.scope,
        "option_id": option_id,
    });
    {
        let mut journal = daemon.journal.lock().await;
        journal
            .record_approval_rejection(request.approval_id, session_id, wall_clock_ms(), &payload)
            .map_err(|_| "approval rejection could not be journaled".to_owned())?;
    }
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
    let hash: String = request
        .content_hash
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(64)
        .collect();
    daemon_log::log(
        "WARN",
        "approval",
        &format!(
            "approval rejected approval_id={} session_id={session_id} client_id={} client_label={label} scope={:?} option_id={option_id} reason={reason} content_hash={hash}",
            request.approval_id, client.client_id, client.scope,
        ),
    );
    Err(approval_binding::rejection_message(reason))
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect()
}

async fn run_fake_acp(daemon: Arc<Daemon>, path: PathBuf, session_id: Uuid) {
    if let Err(error) = run_fake_acp_inner(Arc::clone(&daemon), path, session_id).await {
        daemon_log::log(
            "WARN",
            "fake_acp",
            &format!("session {session_id} ended: {:?}", error.kind()),
        );
        if let Some(coordinator) = daemon.sessions.lock().await.get(&session_id).cloned() {
            let mut coordinator = coordinator.lock().await;
            let _ = coordinator.turn_finished(
                wall_clock_ms(),
                shardrail_session_core::TurnOutcome::Interrupted,
                false,
                json!({"source":"fake-acp","error":"provider_failed"}),
                || Ok(()),
            );
            let _ = coordinator.transition(
                wall_clock_ms(),
                "process_exited",
                json!({"source":"fake-acp","error":"provider_failed"}),
                || Ok(()),
            );
        }
    }
    let mut approvals = daemon.approvals.lock().await;
    let ids = approvals
        .iter()
        .filter_map(|(id, entry)| (entry.session_id == session_id).then_some(*id))
        .collect::<Vec<_>>();
    for id in ids {
        if let Some(mut entry) = approvals.remove(&id)
            && let Some(task) = entry.timeout_task.take()
        {
            task.abort();
        }
    }
    drop(approvals);
    daemon.session_turns.lock().await.remove(&session_id);
    // Chunk wakes can run before approval acknowledgement and process exit.
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
}

async fn run_fake_acp_inner(
    daemon: Arc<Daemon>,
    path: PathBuf,
    session_id: Uuid,
) -> io::Result<()> {
    let mut command = TokioCommand::new(path);
    if daemon.fake_acp_no_reject {
        command.env("SHARDRAIL_FAKE_ACP_NO_REJECT", "1");
    }
    if daemon.fake_acp_disconnect_after_permission {
        command.env("SHARDRAIL_FAKE_ACP_DISCONNECT_AFTER_PERMISSION", "1");
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("fake ACP stdin unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("fake ACP stdout unavailable"))?;
    let mut input = input;
    let mut output = BufReader::new(output);
    let coordinator = daemon
        .sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| io::Error::other("session coordinator unavailable"))?;
    let mut coordinator = coordinator.lock().await;
    coordinator
        .transition(
            wall_clock_ms(),
            "process_started",
            json!({"provider":"fake-acp"}),
            || Ok(()),
        )
        .map_err(io::Error::other)?;
    coordinator
        .transition(
            wall_clock_ms(),
            "turn_started",
            json!({"source":"fake-acp"}),
            || Ok(()),
        )
        .map_err(io::Error::other)?;
    drop(coordinator);
    let turn_id = Uuid::now_v7().to_string();
    daemon
        .session_turns
        .lock()
        .await
        .insert(session_id, turn_id.clone());
    session_checks::journal_checks(&daemon, session_id, &session_checks::fake_acp_checks())
        .await
        .map_err(io::Error::other)?;
    send_agent_request(&mut input, 1, "initialize", json!({"protocolVersion":1})).await?;
    let _ = read_agent_line(&mut output).await?;
    send_agent_request(&mut input, 2, "session/new", json!({"cwd":"/tmp"})).await?;
    let _ = read_agent_line(&mut output).await?;
    send_agent_request(
        &mut input,
        3,
        "session/prompt",
        json!({"sessionId":"fake-session-1","prompt":[]}),
    )
    .await?;
    // shortcut: the fake ACP fixture emits one permission request per prompt; loop when fixture coverage adds multiple requests.
    let permission = loop {
        let frame: serde_json::Value =
            serde_json::from_str(&read_agent_line(&mut output).await?).map_err(io::Error::other)?;
        if frame["method"] == "session/request_permission" {
            break frame;
        }
        if frame["method"] == "session/update" {
            let params = &frame["params"];
            let update = &params["update"];
            {
                let mut journal = daemon.journal.lock().await;
                let journal_epoch = journal.journal_epoch().map_err(io::Error::other)?;
                let host_id = journal.host_id().map_err(io::Error::other)?;
                journal
                    .append_event(&DurableEvent {
                        event_id: 0,
                        journal_epoch,
                        host_id,
                        session_id,
                        schema_version: 1,
                        kind: update["sessionUpdate"]
                            .as_str()
                            .unwrap_or("agent_update")
                            .to_owned(),
                        occurred_at_ms: wall_clock_ms(),
                        provider_ref: params["sessionId"].as_str().map(str::to_owned),
                        causation: None,
                        redaction_flags: Vec::new(),
                        payload: update.clone(),
                    })
                    .map_err(io::Error::other)?;
            }
            daemon
                .event_generation
                .send_modify(|generation| *generation += 1);
        }
    };
    let params = &permission["params"];
    let options = params["options"].as_array().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "fake provider options missing")
    })?;
    let mut approval = Approval::new(wall_clock_ms());
    approval.session_id = session_id;
    let approval_id = approval.id;
    let provider_options = options
        .iter()
        .map(|option| {
            let kind = option["kind"].as_str().unwrap_or("unknown");
            let id = option["optionId"].as_str().unwrap_or("").to_owned();
            let decision = if kind.starts_with("allow") {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny
            };
            let class = classify_option(kind);
            ProviderOption {
                id,
                class,
                decision,
            }
        })
        .collect::<Vec<_>>();
    if provider_options.is_empty() || provider_options.iter().any(|option| option.id.is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "fake provider options are malformed",
        ));
    }
    let (provider_tx, mut provider_rx) = mpsc::unbounded_channel();
    let coordinator = daemon
        .sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| io::Error::other("session coordinator unavailable"))?;
    let mut coordinator = coordinator.lock().await;
    coordinator
        .create_approval(approval.clone())
        .map_err(io::Error::other)?;
    drop(coordinator);
    let (journal_epoch, host_id) = {
        let journal = daemon.journal.lock().await;
        (
            journal.journal_epoch().map_err(io::Error::other)?,
            journal.host_id().map_err(io::Error::other)?,
        )
    };
    let kind = params["toolCall"]["kind"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned();
    let disclosed = disclosed_action(params);
    let content =
        approval_content_from_disclosed(&kind, &disclosed, params["reason"].as_str().unwrap_or(""));
    let content_hash = approval_binding::content_hash(&content);
    let binding = ApprovalBinding {
        approval_id,
        session_id,
        turn_id: turn_id.clone(),
        provider_request_id: approval_binding::provider_request_id(&permission["id"]),
        content_hash: content_hash.clone(),
        journal_epoch,
    };
    approval.content_hash = content_hash.clone();
    let mut details = json!({
        "approval_id": approval_id,
        "approval_kind": kind,
        "tool_call_id": params["toolCall"]["toolCallId"],
        "options": options.iter().zip(provider_options.iter()).map(|(raw, option)| json!({"option_id":option.id,"name":raw["name"],"decision":option.decision,"class":option.class})).collect::<Vec<_>>(),
        "agent_description": {"title":params["toolCall"]["title"],"description":params["description"],"reason":params["reason"]},
        "binding": binding.clone(),
    });
    if let Some(object) = details.as_object_mut() {
        object.extend(disclosed);
    }
    daemon_log::log(
        "INFO",
        "approval",
        &format!(
            "approval requested approval_id={approval_id} session_id={session_id} turn_id={turn_id} content_hash={content_hash}"
        ),
    );
    daemon
        .journal
        .lock()
        .await
        .append_event(&DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id,
            schema_version: 1,
            kind: "approval_requested".into(),
            occurred_at_ms: wall_clock_ms(),
            provider_ref: Some(approval_id.to_string()),
            causation: None,
            redaction_flags: Vec::new(),
            payload: details,
        })
        .map_err(io::Error::other)?;
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
    daemon.approvals.lock().await.insert(
        approval_id,
        DaemonApproval {
            session_id,
            approval,
            binding,
            options: provider_options,
            provider: Some(provider_tx),
            timeout_task: None,
        },
    );
    let timeout_daemon = Arc::clone(&daemon);
    let timeout_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(timeout_daemon.approval_timeout_secs)).await;
        expire_approval(&timeout_daemon, approval_id).await;
    });
    if let Some(entry) = daemon.approvals.lock().await.get_mut(&approval_id) {
        entry.timeout_task = Some(timeout_task.abort_handle());
    }
    let command = tokio::select! {
        command = provider_rx.recv() => command.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "approval response channel closed"))?,
        status = child.wait() => return Err(io::Error::other(format!("fake ACP exited while approval was pending: {status:?}"))),
    };
    tokio::time::sleep(command.delay).await;
    let responded = command.option_id.is_some();
    let write_result = if let Some(option_id) = command.option_id.as_ref() {
        write_agent(
            &mut input,
            &json!({
                "jsonrpc":"2.0", "id":permission["id"],
                "result":{"outcome":{"outcome":"selected","optionId":option_id}}
            }),
        )
        .await
    } else {
        write_agent(&mut input, &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"fake-session-1"}})).await
    };
    let _ = command.written.send(
        write_result
            .as_ref()
            .map(|_| ())
            .map_err(|error| io::Error::new(error.kind(), error.to_string())),
    );
    write_result?;
    if responded {
        loop {
            let frame: serde_json::Value =
                serde_json::from_str(&read_agent_line(&mut output).await?)
                    .map_err(io::Error::other)?;
            if frame["id"] == 3 {
                let coordinator = daemon
                    .sessions
                    .lock()
                    .await
                    .get(&session_id)
                    .cloned()
                    .ok_or_else(|| io::Error::other("session coordinator unavailable"))?;
                let mut coordinator = coordinator.lock().await;
                coordinator
                    .acknowledge_approval(approval_id, wall_clock_ms())
                    .map_err(io::Error::other)?;
                break;
            }
        }
        daemon
            .event_generation
            .send_modify(|generation| *generation += 1);
        // The fixture waits on stdin after the prompt reply. Close it so the
        // process can exit and the terminal lifecycle events can be journaled.
        drop(input);
    }
    let _ = child.wait().await?;
    let coordinator = daemon
        .sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| io::Error::other("session coordinator unavailable"))?;
    let mut coordinator = coordinator.lock().await;
    coordinator
        .turn_finished(
            wall_clock_ms(),
            if responded {
                shardrail_session_core::TurnOutcome::Provider
            } else {
                shardrail_session_core::TurnOutcome::Interrupted
            },
            false,
            json!({"source":"fake-acp"}),
            || Ok(()),
        )
        .map_err(io::Error::other)?;
    coordinator
        .transition(
            wall_clock_ms(),
            "process_exited",
            json!({"source":"fake-acp"}),
            || Ok(()),
        )
        .map_err(io::Error::other)?;
    Ok(())
}

async fn expire_approval(daemon: &Daemon, approval_id: Uuid) {
    let mut approvals = daemon.approvals.lock().await;
    let Some(entry) = approvals.get_mut(&approval_id) else {
        return;
    };
    let Some(provider) = entry.provider.as_ref().cloned() else {
        return;
    };
    let reject_id = entry
        .options
        .iter()
        .find(|option| {
            option.decision == ApprovalDecision::Deny && option.class == OptionClass::Once
        })
        .map(|option| option.id.clone());
    let Some(coordinator) = daemon.sessions.lock().await.get(&entry.session_id).cloned() else {
        return;
    };
    let mut coordinator = coordinator.lock().await;
    let request = shardrail_coordinator::ApprovalTimeoutRequest {
        now_ms: wall_clock_ms(),
        provider_limit_secs: Some(daemon.approval_timeout_secs),
        option_id: reject_id,
    };
    let response = coordinator.timeout_approval(
        &mut entry.approval,
        request,
        |_decision, option_id| {
            let (written, result) = std::sync::mpsc::sync_channel(1);
            provider
                .send(ProviderCommand {
                    approval_id,
                    option_id: option_id.map(str::to_owned),
                    written,
                    delay: Duration::ZERO,
                })
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "fake provider disconnected")
                })?;
            tokio::task::block_in_place(|| result.recv().map_err(io::Error::other))?
        },
        || Ok(()),
    );
    match response {
        Ok(()) => {
            approvals.remove(&approval_id);
            daemon_log::log(
                "INFO",
                "approval",
                &format!("approval expired approval_id={approval_id}"),
            );
        }
        Err(_error) => {
            if let Some(provider) = entry.provider.take() {
                let _ = provider.send(ProviderCommand::cancel(approval_id));
            }
            daemon_log::log("ERROR", "approval", "expiry failed closed");
        }
    }
}

fn disclosed_action(params: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut action = serde_json::Map::new();
    let tool = &params["toolCall"];
    if let Some(command) =
        nonempty_text(&tool["command"]).or_else(|| nonempty_text(&params["command"]))
    {
        action.insert("command".into(), json!(command));
    }
    if let Some(argv) = string_list(&tool["argv"]).or_else(|| string_list(&tool["args"])) {
        action.insert("argv".into(), serde_json::Value::Array(argv));
    }
    if let Some(raw_input) = disclosed_json(&tool["rawInput"]) {
        action.insert("raw_input".into(), raw_input);
    }
    if let Some(cwd) = nonempty_text(&tool["cwd"]).or_else(|| nonempty_text(&params["cwd"])) {
        action.insert("cwd".into(), json!(cwd));
    }
    if let Some(locations) = disclosed_json(&tool["locations"]) {
        action.insert("locations".into(), locations);
    }
    if let Some(changes) =
        disclosed_json(&tool["changes"]).or_else(|| disclosed_json(&params["changes"]))
    {
        action.insert("changes".into(), changes);
    }
    if let Some(network) =
        nonempty_text(&tool["network"]).or_else(|| nonempty_text(&params["network"]))
    {
        action.insert("network".into(), json!(network));
    }
    if let Some(sandbox) =
        nonempty_text(&tool["sandbox"]).or_else(|| nonempty_text(&params["sandbox"]))
    {
        action.insert("sandbox".into(), json!(sandbox));
    }
    action
}

fn nonempty_text(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn string_list(value: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    let items = value.as_array()?;
    if items.is_empty()
        || !items
            .iter()
            .all(|item| item.as_str().is_some_and(|text| !text.is_empty()))
    {
        return None;
    }
    Some(items.clone())
}

fn disclosed_json(value: &serde_json::Value) -> Option<serde_json::Value> {
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then(|| value.clone());
    }
    if value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(|object| object.is_empty())
    {
        return None;
    }
    Some(value.clone())
}

fn classify_option(kind: &str) -> OptionClass {
    if kind == "allow_once" || kind == "reject_once" || kind == "deny_once" {
        OptionClass::Once
    } else if kind.contains("sandbox")
        || kind.contains("permission")
        || kind == "allow_boundary_breaking"
    {
        OptionClass::BoundaryBreaking
    } else {
        OptionClass::Widening
    }
}

struct MalformedApproval {
    approval_id: Uuid,
    option_id: String,
    content_hash: String,
    session_id: Uuid,
    turn_id: String,
    provider_request_id: String,
    reason: &'static str,
}

fn malformed_approval_attempt(line: &str) -> Option<MalformedApproval> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    if object.get("kind").and_then(serde_json::Value::as_str) != Some("approval_resolution") {
        return None;
    }
    let request = object.get("request")?.as_object()?;
    let approval_id = request
        .get("approval_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())?;
    let text = |key: &str| {
        request
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let session_id = request
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .unwrap_or(Uuid::nil());
    let required = [
        "option_id",
        "content_hash",
        "session_id",
        "turn_id",
        "provider_request_id",
    ];
    let reason = if required.iter().any(|key| {
        request
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_none()
    }) {
        "missing_binding"
    } else {
        "malformed_binding"
    };
    Some(MalformedApproval {
        approval_id,
        option_id: text("option_id"),
        content_hash: text("content_hash"),
        session_id,
        turn_id: text("turn_id"),
        provider_request_id: text("provider_request_id"),
        reason,
    })
}

async fn record_malformed_approval(
    daemon: &Daemon,
    client: &AuthorizedClient,
    attempt: &MalformedApproval,
) -> Result<(), String> {
    let request = ApprovalResolutionRequest {
        approval_id: attempt.approval_id,
        option_id: attempt.option_id.clone(),
        content_hash: attempt.content_hash.clone(),
        session_id: attempt.session_id,
        turn_id: attempt.turn_id.clone(),
        provider_request_id: attempt.provider_request_id.clone(),
    };
    let (stored_session, _) = missing_approval_reason(daemon, attempt.approval_id).await?;
    reject_decision(
        daemon,
        client,
        &request,
        stored_session.unwrap_or(attempt.session_id),
        attempt.reason,
    )
    .await
}

fn approval_content_from_disclosed(
    kind: &str,
    disclosed: &serde_json::Map<String, serde_json::Value>,
    reason: &str,
) -> approval_binding::ApprovalContent {
    let argv = disclosed
        .get("argv")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut paths = Vec::new();
    let mut diff = String::new();
    if let Some(locations) = disclosed.get("locations") {
        push_displayed_paths(locations, &mut paths);
    }
    if let Some(changes) = disclosed.get("changes") {
        push_change_paths(changes, &mut paths, &mut diff);
    }
    approval_binding::ApprovalContent {
        kind: kind.to_owned(),
        command: disclosed
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        argv,
        cwd: disclosed
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        paths,
        diff,
        raw_input: disclosed
            .get("raw_input")
            .map(approval_binding::canonical_json)
            .unwrap_or_default(),
        reason: reason.to_owned(),
        network: disclosed
            .get("network")
            .map(approval_binding::canonical_json)
            .unwrap_or_default(),
    }
}

fn push_displayed_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.is_empty() => paths.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                push_displayed_paths(item, paths);
            }
        }
        serde_json::Value::Object(object) => {
            if let Some(path) = object.get("path").and_then(serde_json::Value::as_str)
                && !path.is_empty()
            {
                paths.push(path.to_owned());
            }
        }
        _ => {}
    }
}

fn push_change_paths(value: &serde_json::Value, paths: &mut Vec<String>, diff: &mut String) {
    let Some(items) = value.as_array() else {
        push_displayed_paths(value, paths);
        return;
    };
    for item in items {
        if let Some(path) = item.get("path").and_then(serde_json::Value::as_str) {
            paths.push(path.to_owned());
        }
        if let Some(text) = item.get("diff").and_then(serde_json::Value::as_str) {
            if !diff.is_empty() {
                diff.push('\n');
            }
            diff.push_str(text);
        }
    }
}

fn parse_client_frame(line: &str) -> io::Result<ClientFrame> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;
    let object = value.as_object().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "client frame must be an object")
    })?;
    let allowed = match object.get("kind").and_then(serde_json::Value::as_str) {
        Some("authenticate") => &["kind", "signature"][..],
        Some("request" | "approval_resolution") => &["kind", "request"][..],
        Some("resume") => &["kind", "resume_after"][..],
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown client frame kind",
            ));
        }
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown client frame field",
        ));
    }
    serde_json::from_value(value).map_err(io::Error::other)
}

pub(crate) struct ProviderCommand {
    pub(crate) approval_id: Uuid,
    pub(crate) option_id: Option<String>,
    pub(crate) written: std::sync::mpsc::SyncSender<io::Result<()>>,
    pub(crate) delay: Duration,
}

impl ProviderCommand {
    pub(crate) fn cancel(approval_id: Uuid) -> Self {
        let (written, _result) = std::sync::mpsc::sync_channel(1);
        Self {
            approval_id,
            option_id: None,
            written,
            delay: Duration::ZERO,
        }
    }
}

async fn send_agent_request(
    input: &mut ChildStdin,
    id: u32,
    method: &str,
    params: serde_json::Value,
) -> io::Result<()> {
    write_agent(
        input,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await
}

async fn write_agent(input: &mut ChildStdin, frame: &serde_json::Value) -> io::Result<()> {
    input
        .write_all(
            serde_json::to_string(frame)
                .map_err(io::Error::other)?
                .as_bytes(),
        )
        .await?;
    input.write_all(b"\n").await?;
    input.flush().await
}

async fn read_agent_line(
    output: &mut BufReader<tokio::process::ChildStdout>,
) -> io::Result<String> {
    let mut line = String::new();
    if output.read_line(&mut line).await? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "fake ACP disconnected",
        ));
    }
    Ok(line)
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Client {
        #[command(subcommand)]
        command: ClientCommand,
    },
    Identity,
}

#[derive(Debug, Subcommand)]
enum ClientCommand {
    /// Issue a five-minute, single-use code for limited-scope browser enrollment.
    Enroll {
        #[arg(long, default_value = "browser")]
        label: String,
        /// Grant this limited client the `session.create` capability.
        #[arg(long)]
        allow_session_create: bool,
    },
    Add {
        #[arg(long)]
        label: String,
        #[arg(long, value_enum)]
        scope: ScopeArg,
        #[arg(long)]
        public_key: Option<String>,
    },
    List,
    Remove {
        client_id: Uuid,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScopeArg {
    Full,
    Limited,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AuthorizedClient {
    client_id: Uuid,
    public_key: String,
    label: String,
    scope: ClientScope,
    #[serde(default)]
    allow_session_create: bool,
    /// False for an observer. Missing on older client files, which may still decide.
    #[serde(default = "default_allow_approvals")]
    allow_approvals: bool,
}

fn default_allow_approvals() -> bool {
    true
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct AuthorizedClients {
    clients: Vec<AuthorizedClient>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Enrollment {
    code_hash: String,
    expires_at_ms: u128,
    failed_attempts: u8,
    label: String,
    #[serde(default)]
    allow_session_create: bool,
}

#[derive(Debug, serde::Deserialize)]
struct EnrollmentRequest {
    kind: String,
    code: String,
    public_key: String,
}

pub(crate) struct Daemon {
    data: DaemonDataDir,
    pub(crate) journal: Mutex<Journal>,
    pub(crate) event_generation: watch::Sender<u64>,
    command_lock: Mutex<()>,
    host_key: SigningKey,
    fake_acp_path: Option<PathBuf>,
    fake_acp_no_reject: bool,
    fake_acp_disconnect_after_permission: bool,
    pub(crate) approval_timeout_secs: u64,
    pub(crate) test_write_ahead_delay_ms: u64,
    pub(crate) approvals: Mutex<HashMap<Uuid, DaemonApproval>>,
    pub(crate) sessions: Mutex<HashMap<Uuid, Arc<Mutex<Coordinator>>>>,
    pub(crate) codex: Option<codex_app_server::CodexConfig>,
    pub(crate) session_workdirs: Vec<session_workdir::AllowedWorkdir>,
    pub(crate) codex_status: Mutex<HashMap<Uuid, codex_app_server::CodexStatus>>,
    pub(crate) turn_commands:
        Mutex<HashMap<Uuid, mpsc::UnboundedSender<codex_app_server::SessionCommand>>>,
    /// Provider turn id of the turn that is open for each session.
    pub(crate) session_turns: Mutex<HashMap<Uuid, String>>,
}

pub(crate) struct DaemonApproval {
    pub(crate) session_id: Uuid,
    pub(crate) approval: Approval,
    pub(crate) binding: ApprovalBinding,
    pub(crate) options: Vec<ProviderOption>,
    pub(crate) provider: Option<mpsc::UnboundedSender<ProviderCommand>>,
    pub(crate) timeout_task: Option<tokio::task::AbortHandle>,
}

#[derive(Clone)]
pub(crate) struct ProviderOption {
    pub(crate) id: String,
    pub(crate) class: OptionClass,
    pub(crate) decision: ApprovalDecision,
}

fn codex_config(
    args: &Args,
    allowlist: &[session_workdir::AllowedWorkdir],
) -> Result<Option<codex_app_server::CodexConfig>, io::Error> {
    if args.fake_acp_path.is_some() && args.codex_app_server.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "choose either --fake-acp-path or --codex-app-server",
        ));
    }
    if args.codex_app_server.is_none() && !args.codex_config.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--codex-config requires --codex-app-server",
        ));
    }
    let Some(program) = args.codex_app_server.clone() else {
        return Ok(None);
    };
    let Some(workdir) = allowlist.first().map(|entry| entry.path.clone()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--codex-app-server requires --codex-workdir or --session-workdir",
        ));
    };
    for item in &args.codex_config {
        if !codex_app_server::config_override_allowed(item) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "codex config override must be model or model_reasoning_effort as a single-line KEY=VALUE",
            ));
        }
    }
    Ok(Some(codex_app_server::CodexConfig {
        program,
        workdir,
        extra: args.codex_config.clone(),
        directory: None,
        replace_after_pin: args.test_replace_workdir_after_pin,
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let path = args.data_dir.clone().unwrap_or_else(default_data_dir);
    match args.command {
        Some(DaemonCommand::Client { command }) => manage_clients(&path, command)?,
        Some(DaemonCommand::Identity) => {
            let (directory, anchored_path) = open_private_data_directory(&path)?;
            pin_data_directory(&directory)?;
            let path = if cfg!(target_os = "macos") {
                Path::new(".")
            } else {
                anchored_path.as_path()
            };
            let key = load_or_create_signing_key(&path.join("host.key"))?;
            let public_key = key.verifying_key().to_bytes();
            println!(
                "host_public_key={} host_fingerprint={}",
                encode_hex(&public_key),
                encode_hex(&Sha256::digest(public_key))
            );
        }
        None => {
            if let Some(root) = args
                .shardrail_root
                .clone()
                .or_else(|| std::env::var_os("SHARDRAIL_REPO_ROOT").map(PathBuf::from))
            {
                session_workdir::install_repo_root(root);
            }
            let session_workdirs = session_workdir::collect_session_workdirs(
                args.codex_workdir.as_deref(),
                &args.session_workdir,
            )?;
            let codex = codex_config(&args, &session_workdirs)?;
            daemon_log::init(daemon_log::configured_dir(args.log_dir, args.no_file_log));
            run(
                path,
                args.fake_acp_path,
                args.fake_acp_no_reject,
                args.fake_acp_disconnect_after_permission,
                args.approval_timeout_secs,
                args.test_write_ahead_delay_ms,
                BrowserListenerOptions {
                    address: args.browser_listen,
                    assets_dir: args.browser_assets_dir,
                },
                codex,
                session_workdirs,
                args.exit_with_stdin,
            )
            .await?
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "Codex config is one more startup input beside the existing daemon options"
)]
async fn run(
    path: PathBuf,
    fake_acp_path: Option<PathBuf>,
    fake_acp_no_reject: bool,
    fake_acp_disconnect_after_permission: bool,
    approval_timeout_secs: u64,
    test_write_ahead_delay_ms: u64,
    browser: BrowserListenerOptions,
    codex: Option<codex_app_server::CodexConfig>,
    session_workdirs: Vec<session_workdir::AllowedWorkdir>,
    exit_with_stdin: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Startup order is kept in this function so the listener cannot precede recovery.
    refuse_synced_path(&path)?;
    refuse_network_filesystem(&path)?;
    let mut data = DaemonDataDir::open(&path)?;
    data.pin_path_only_apis()?;
    let path = if cfg!(target_os = "macos") {
        PathBuf::from(".")
    } else {
        data.directory_path().to_path_buf()
    };
    let _journal_fd = data.open_journal_entry()?;
    let mut journal = data.open_journal()?;
    let epoch = journal.journal_epoch()?;
    let host_id = journal.host_id()?;
    let host_key = load_or_create_signing_key(&path.join("host.key"))?;
    let abandoned = Coordinator::recover_journal(&mut journal)?;
    let socket = path.join("shardraild.sock");
    let listener = bind_private_socket(&socket)?;
    let browser_listener = if let Some(address) = browser.address {
        Some(bind_browser_listener(address).await?)
    } else {
        None
    };
    let (event_generation, _) = watch::channel(0);
    let daemon = Arc::new(Daemon {
        data,
        journal: Mutex::new(journal),
        event_generation,
        command_lock: Mutex::new(()),
        host_key,
        fake_acp_path,
        fake_acp_no_reject,
        fake_acp_disconnect_after_permission,
        approval_timeout_secs,
        test_write_ahead_delay_ms,
        approvals: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        codex,
        session_workdirs: session_workdirs.clone(),
        codex_status: Mutex::new(HashMap::new()),
        turn_commands: Mutex::new(HashMap::new()),
        session_turns: Mutex::new(HashMap::new()),
    });
    codex_app_server::install_shutdown_kill();
    daemon_log::log("INFO", "daemon", &startup_message(&socket, host_id, epoch));
    daemon_log::log(
        "INFO",
        "approval",
        &format!("startup abandoned {abandoned} pending approvals"),
    );
    daemon_log::log(
        "INFO",
        "daemon",
        &format!(
            "session workdir allowlist: {}",
            session_workdir::format_allowlist(&session_workdirs)
        ),
    );
    if let Some(listener) = browser_listener {
        let address = listener.local_addr()?;
        daemon_log::log(
            "INFO",
            "browser",
            &format!("WebSocket listening at ws://{address}/ws"),
        );
        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let daemon = Arc::clone(&daemon);
                        let browser_assets_dir = browser.assets_dir.clone();
                        tokio::spawn(async move {
                            if let Err(_error) = serve_browser_connection(
                                stream,
                                daemon,
                                address.port(),
                                browser_assets_dir,
                            )
                            .await
                            {
                                daemon_log::log(
                                    "WARN",
                                    "browser",
                                    &format!("client connection ended: {:?}", _error.kind()),
                                );
                            }
                        });
                    }
                    Err(error) => daemon_log::log(
                        "ERROR",
                        "browser",
                        &format!("listener accept failed: {:?}", error.kind()),
                    ),
                }
            }
        });
    }
    let parent_closed = async move {
        if !exit_with_stdin {
            std::future::pending::<()>().await;
            return;
        }
        let _ = tokio::task::spawn_blocking(|| {
            let mut stdin = io::stdin();
            let mut buffer = [0u8; 256];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await;
    };
    tokio::pin!(parent_closed);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let daemon = Arc::clone(&daemon);
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, daemon).await {
                        daemon_log::log(
                            "WARN",
                            "client",
                            &format!("connection ended: {:?}", error.kind()),
                        );
                    }
                });
            }
            _ = &mut parent_closed => {
                daemon_log::log("INFO", "daemon", "parent stdin closed; stopping");
                codex_app_server::shutdown_children();
                return Ok(());
            }
        }
    }
}

async fn bind_browser_listener(address: SocketAddr) -> io::Result<TcpListener> {
    if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "browser listener must bind exactly to 127.0.0.1; IPv6 and non-loopback addresses are unsupported",
        ));
    }
    TcpListener::bind(address).await
}

fn browser_request_allowed(request: &WsRequest, port: u16) -> bool {
    let expected_host = format!("127.0.0.1:{port}");
    let expected_origin = format!("http://127.0.0.1:{port}");
    let hosts = request.headers().get_all("host");
    let origins = request.headers().get_all("origin");
    hosts.iter().count() == 1
        && origins.iter().count() == 1
        && hosts.iter().next().and_then(|value| value.to_str().ok()) == Some(expected_host.as_str())
        && origins.iter().next().and_then(|value| value.to_str().ok())
            == Some(expected_origin.as_str())
        && request.uri().path() == "/ws"
        && request.uri().scheme().is_none()
        && request.uri().authority().is_none()
        && request.uri().query().is_none()
}

#[cfg(test)]
mod browser_listener_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn request(uri: &str, host: &[&str], origin: &[&str]) -> WsRequest {
        let mut builder = WsRequest::builder().uri(uri);
        for value in host {
            builder = builder.header("host", *value);
        }
        for value in origin {
            builder = builder.header("origin", *value);
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn browser_upgrade_requires_exact_loopback_host_origin_and_clean_url() {
        assert!(browser_request_allowed(
            &request("/ws", &["127.0.0.1:4411"], &["http://127.0.0.1:4411"]),
            4411
        ));
        for bad in [
            request("/ws", &["127.0.0.1:4411.evil"], &["http://127.0.0.1:4411"]),
            request("/ws", &["localhost:4411"], &["http://127.0.0.1:4411"]),
            request("/ws", &["127.0.0.1:4411"], &[]),
            request("/ws", &["127.0.0.1:4411"], &["http://evil.test"]),
            request(
                "/ws?token=x",
                &["127.0.0.1:4411"],
                &["http://127.0.0.1:4411"],
            ),
            request(
                "/ws",
                &["127.0.0.1:4411", "127.0.0.1:4411"],
                &["http://127.0.0.1:4411"],
            ),
            request(
                "/ws",
                &["127.0.0.1:4411"],
                &["http://127.0.0.1:4411", "http://127.0.0.1:4411"],
            ),
            request(
                "http://attacker.invalid/ws",
                &["127.0.0.1:4411"],
                &["http://127.0.0.1:4411"],
            ),
            request(
                "http://127.0.0.1:4411/ws",
                &["127.0.0.1:4411"],
                &["http://127.0.0.1:4411"],
            ),
        ] {
            assert!(!browser_request_allowed(&bad, 4411));
        }
    }

    #[tokio::test]
    async fn browser_listener_refuses_non_loopback_addresses() {
        for address in ["0.0.0.0:4411", "192.0.2.1:4411", "[::1]:4411"] {
            let error = bind_browser_listener(address.parse().unwrap())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("127.0.0.1"));
        }
    }

    #[tokio::test]
    async fn browser_router_waits_for_a_fragmented_websocket_request_line() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            browser_request_line(&stream).await.unwrap()
        });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client.write_all(b"G").await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        client
            .write_all(b"ET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\n")
            .await
            .unwrap();
        assert_eq!(server.await.unwrap(), b"GET /ws HTTP/1.1");
    }

    #[tokio::test]
    async fn browser_assets_require_same_origin_and_stay_inside_the_build_directory() {
        let assets = tempfile::tempdir().unwrap();
        fs::write(assets.path().join("index.html"), "browser app").unwrap();
        fs::write(assets.path().join("app.js"), "console.log('ok')").unwrap();
        fs::write(assets.path().join("app.css"), "body { color: black }\n").unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let root = assets.path().to_path_buf();
        let server = tokio::spawn(async move {
            for _ in 0..7 {
                let (mut stream, _) = listener.accept().await.unwrap();
                serve_browser_asset(&mut stream, address.port(), Some(&root))
                    .await
                    .unwrap();
            }
        });
        for (target, host, origin, extra, expected, content_type) in [
            (
                "/",
                address.to_string(),
                format!("http://{address}"),
                "",
                "200 OK",
                Some("text/html; charset=utf-8"),
            ),
            (
                "/app.js",
                address.to_string(),
                format!("http://{address}"),
                "",
                "200 OK",
                Some("text/javascript; charset=utf-8"),
            ),
            (
                "/app.css",
                address.to_string(),
                format!("http://{address}"),
                "",
                "200 OK",
                Some("text/css; charset=utf-8"),
            ),
            (
                "/../secret",
                address.to_string(),
                format!("http://{address}"),
                "",
                "404 Not Found",
                None,
            ),
            (
                "/",
                address.to_string(),
                "http://evil.test".to_owned(),
                "",
                "403 Forbidden",
                None,
            ),
            (
                "/",
                address.to_string(),
                format!("http://{address}"),
                "host: evil.test\r\n",
                "403 Forbidden",
                None,
            ),
            (
                "/",
                "evil.test".to_owned(),
                format!("http://{address}"),
                "",
                "403 Forbidden",
                None,
            ),
        ] {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    format!(
                        "GET {target} HTTP/1.1\r\nHost: {host}\r\n{extra}Origin: {origin}\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            let response = String::from_utf8_lossy(&response);
            assert!(response.starts_with(&format!("HTTP/1.1 {expected}")));
            if let Some(content_type) = content_type {
                assert!(response.contains(&format!("Content-Type: {content_type}\r\n")));
                assert!(
                    response
                        .contains("Content-Security-Policy: default-src 'self'; script-src 'self'")
                );
            }
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn browser_close_releases_unix_gateway_without_daemon_traffic() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (unix_gateway, mut unix_peer) = UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            tunnel_browser_client(websocket, unix_gateway)
                .await
                .unwrap();
        });
        let (mut browser, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();

        browser.close(None).await.unwrap();
        let mut byte = [0];
        assert_eq!(
            timeout(Duration::from_secs(1), unix_peer.read(&mut byte))
                .await
                .expect("gateway write half should close promptly")
                .unwrap(),
            0
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejected_absolute_form_request_never_upgrades() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept_browser_websocket(stream, address.port()).await
        });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://attacker.invalid/ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                    address.port(), address.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("rejected connection should close")
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 403"));
        assert!(server.await.unwrap().is_err());
    }
}

async fn serve_browser_connection(
    mut stream: tokio::net::TcpStream,
    daemon: Arc<Daemon>,
    port: u16,
    assets_dir: Option<PathBuf>,
) -> io::Result<()> {
    let request_line = browser_request_line(&stream).await?;
    if request_line.starts_with(b"GET /ws HTTP/") {
        return serve_browser_client(stream, daemon, port).await;
    }
    serve_browser_asset(&mut stream, port, assets_dir.as_deref()).await
}

async fn browser_request_line(stream: &tokio::net::TcpStream) -> io::Result<Vec<u8>> {
    let mut request = [0u8; 16 * 1024];
    loop {
        let count = timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut request))
            .await
            .map_err(|_| handshake_timeout())??;
        if let Some(end) = request[..count].iter().position(|byte| *byte == b'\n') {
            let line_end = if end > 0 && request[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            return Ok(request[..line_end].to_vec());
        }
        if count == request.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser request line is too large",
            ));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn serve_browser_asset(
    stream: &mut tokio::net::TcpStream,
    port: u16,
    assets_dir: Option<&Path>,
) -> io::Result<()> {
    let mut request = Vec::with_capacity(1024);
    while !request.ends_with(b"\r\n\r\n") {
        if request.len() >= 16 * 1024 {
            return write_http_error(stream, 431, "Request Header Fields Too Large").await;
        }
        let mut byte = [0u8; 1];
        if timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut byte))
            .await
            .map_err(|_| handshake_timeout())??
            == 0
        {
            return Ok(());
        }
        request.push(byte[0]);
    }
    let Ok(text) = std::str::from_utf8(&request) else {
        return write_http_error(stream, 400, "Bad Request").await;
    };
    let mut lines = text.split("\r\n");
    let Some(line) = lines.next() else {
        return write_http_error(stream, 400, "Bad Request").await;
    };
    let mut parts = line.split(' ');
    let (Some("GET"), Some(target), Some("HTTP/1.1"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return write_http_error(stream, 405, "Method Not Allowed").await;
    };
    let expected_host = format!("127.0.0.1:{port}");
    let expected_origin = format!("http://{expected_host}");
    let mut hosts = Vec::new();
    let mut origins = Vec::new();
    for header in lines {
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "host" => hosts.push(value.trim()),
            "origin" => origins.push(value.trim()),
            _ => {}
        }
    }
    if hosts.as_slice() != [expected_host.as_str()]
        || origins.len() > 1
        || origins
            .first()
            .is_some_and(|origin| *origin != expected_origin)
    {
        return write_http_error(stream, 403, "Forbidden").await;
    }
    let Some(assets_dir) = assets_dir else {
        return write_http_error(stream, 404, "Not Found").await;
    };
    let relative = if target == "/" {
        "index.html"
    } else if target.starts_with('/')
        && !target.contains(['?', '%', '\\'])
        && target[1..]
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    {
        &target[1..]
    } else {
        return write_http_error(stream, 404, "Not Found").await;
    };
    let Ok(root) = fs::canonicalize(assets_dir) else {
        return write_http_error(stream, 404, "Not Found").await;
    };
    let Ok(path) = fs::canonicalize(root.join(relative)) else {
        return write_http_error(stream, 404, "Not Found").await;
    };
    if !path.starts_with(&root) || !path.is_file() {
        return write_http_error(stream, 404, "Not Found").await;
    }
    let content_type = match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => return write_http_error(stream, 404, "Not Found").await,
    };
    let body = fs::read(path)?;
    if body.len() > 8 * 1024 * 1024 {
        return write_http_error(stream, 413, "Content Too Large").await;
    }
    let content_security_policy = format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws://127.0.0.1:{port}; frame-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    );
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-cache\r\nContent-Security-Policy: {content_security_policy}\r\nConnection: close\r\n\r\n",
                body.len(),
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&body).await
}

async fn write_http_error(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
) -> io::Result<()> {
    stream
        .write_all(
            format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
}

async fn serve_browser_client(
    stream: tokio::net::TcpStream,
    daemon: Arc<Daemon>,
    port: u16,
) -> io::Result<()> {
    let websocket = timeout(HANDSHAKE_TIMEOUT, accept_browser_websocket(stream, port))
        .await
        .map_err(|_| handshake_timeout())?
        .map_err(io::Error::other)?;
    let unix = UnixStream::connect(daemon.data.directory_path().join("shardraild.sock")).await?;
    tunnel_browser_client(websocket, unix).await
}

#[allow(clippy::result_large_err)] // tungstenite's callback requires an HTTP response as its rejection value.
async fn accept_browser_websocket(
    stream: tokio::net::TcpStream,
    port: u16,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    tokio_tungstenite::tungstenite::Error,
> {
    tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        move |request: &WsRequest, response: WsResponse| {
            if browser_request_allowed(request, port) {
                Ok(response)
            } else {
                Err(WsResponse::builder()
                    .status(403)
                    .body(Some(
                        "browser listener rejected Host, Origin, path, or query".into(),
                    ))
                    .expect("static rejection response is valid"))
            }
        },
        Some(
            WebSocketConfig::default()
                .max_message_size(Some(1_048_576))
                .max_frame_size(Some(1_048_576)),
        ),
    )
    .await
}

async fn tunnel_browser_client(
    websocket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    unix: UnixStream,
) -> io::Result<()> {
    let (mut ws_write, mut ws_read) = websocket.split();
    let (unix_read, mut unix_write) = unix.into_split();
    let client_to_daemon = async {
        while let Some(message) = ws_read.next().await {
            match message.map_err(io::Error::other)? {
                Message::Text(text) => {
                    if text.len() > 1_048_576 || text.contains('\n') {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid browser frame",
                        ));
                    }
                    unix_write.write_all(text.as_bytes()).await?;
                    unix_write.write_all(b"\n").await?;
                }
                Message::Close(_) => return Ok(()),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "browser frames must be text",
                    ));
                }
            }
        }
        Ok::<(), io::Error>(())
    };
    let daemon_to_client = async {
        let mut reader = BufReader::new(unix_read);
        let mut line = String::new();
        while reader.read_line(&mut line).await? != 0 {
            if line.len() > 1_048_577 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "daemon frame exceeds 1 MiB",
                ));
            }
            let frame = line.trim_end_matches(['\r', '\n']);
            ws_write
                .send(Message::Text(frame.to_owned().into()))
                .await
                .map_err(io::Error::other)?;
            line.clear();
        }
        Ok::<(), io::Error>(())
    };
    tokio::select! {
        result = client_to_daemon => result,
        result = daemon_to_client => result,
    }
}

fn default_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Shardrail")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("shardrail")
    }
}

fn startup_message(socket: &Path, host_id: Uuid, epoch: Uuid) -> String {
    format!(
        "shardraild listening at {} (host {host_id}, epoch {epoch})",
        socket.display()
    )
}

fn manage_clients(path: &Path, command: ClientCommand) -> io::Result<()> {
    let (directory, path) = open_private_data_directory(path)?;
    pin_data_directory(&directory)?;
    let _registry_lock = lock_client_registry(&directory)
        .map_err(|error| io::Error::new(error.kind(), format!("lock client registry: {error}")))?;
    let path = if cfg!(target_os = "macos") {
        Path::new(".")
    } else {
        path.as_path()
    };
    let mut entries = load_authorized_clients(path)
        .map_err(|error| io::Error::new(error.kind(), format!("load client registry: {error}")))?;
    match command {
        ClientCommand::Add {
            label,
            scope,
            public_key,
        } => {
            let id = Uuid::now_v7();
            let host_key = load_or_create_signing_key(&path.join("host.key"))
                .map_err(|error| io::Error::new(error.kind(), format!("load host key: {error}")))?;
            let public_key = match public_key {
                Some(key) => {
                    let bytes = decode_hex(&key)?;
                    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "public key must be 32 bytes")
                    })?;
                    VerifyingKey::from_bytes(&bytes).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "invalid public key")
                    })?;
                    key
                }
                None => {
                    let key = generate_signing_key()?;
                    write_private_bytes(&path.join(format!("client-{id}.key")), &key.to_bytes())?;
                    encode_hex(&key.verifying_key().to_bytes())
                }
            };
            let client_scope = match scope {
                ScopeArg::Full => ClientScope::Full,
                ScopeArg::Limited => ClientScope::Limited,
            };
            entries.clients.push(AuthorizedClient {
                client_id: id,
                public_key: public_key.clone(),
                label,
                scope: client_scope,
                allow_session_create: client_scope == ClientScope::Full,
                allow_approvals: true,
            });
            save_authorized_clients(path, &entries).map_err(|error| {
                io::Error::new(error.kind(), format!("save client registry: {error}"))
            })?;
            println!(
                "client_id={id} public_key={public_key} scope={} host_public_key={}",
                scope_name(client_scope),
                encode_hex(&host_key.verifying_key().to_bytes())
            );
        }
        ClientCommand::Enroll {
            label,
            allow_session_create,
        } => {
            let mut code = [0u8; 32];
            fs::File::open("/dev/urandom")?.read_exact(&mut code)?;
            let text = encode_hex(&code);
            save_enrollment(
                path,
                &Enrollment {
                    code_hash: encode_hex(&Sha256::digest(text.as_bytes())),
                    expires_at_ms: wall_clock_ms() as u128 + 300_000,
                    failed_attempts: 0,
                    label,
                    allow_session_create,
                },
            )?;
            println!("enrollment_code={text} expires_in_seconds=300");
        }
        ClientCommand::List => {
            for client in entries.clients {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    client.client_id,
                    client.label,
                    scope_name(client.scope),
                    client.public_key,
                    if client.allow_session_create {
                        "session.create"
                    } else {
                        "-"
                    }
                );
            }
        }
        ClientCommand::Remove { client_id } => {
            let old_len = entries.clients.len();
            entries
                .clients
                .retain(|client| client.client_id != client_id);
            if entries.clients.len() == old_len {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "client key not found",
                ));
            }
            save_authorized_clients(path, &entries)?;
            match fs::remove_file(path.join(format!("client-{client_id}.key"))) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            println!("removed client {client_id}");
        }
    }
    Ok(())
}

fn save_enrollment(path: &Path, enrollment: &Enrollment) -> io::Result<()> {
    let target = path.join("browser-enrollment.json");
    if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.file_type().is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "enrollment file must be regular",
        ));
    }
    let temporary = path.join(format!(".browser-enrollment-{}.tmp", Uuid::now_v7()));
    let mut file = create_private_file(&temporary)?;
    file.write_all(&serde_json::to_vec(enrollment).map_err(io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(temporary, target)?;
    sync_directory(path)
}

fn redeem_enrollment(path: &Path, request: EnrollmentRequest) -> io::Result<serde_json::Value> {
    let reject = |message: &str| Ok(json!({"kind":"error","error":message}));
    if request.kind != "enroll" {
        return reject("invalid enrollment request");
    }
    // Open the pinned directory directly; reopening via the data-dir helper would fchdir during
    // concurrent WebSocket redemptions on macOS.
    let directory = fs::File::open(path)?;
    let _registry_lock = lock_client_registry(&directory)?;
    let enrollment_path = path.join("browser-enrollment.json");
    let mut enrollment: Enrollment = match fs::read(&enrollment_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return reject("enrollment unavailable");
        }
        Err(error) => return Err(error),
    };
    let hash = encode_hex(&Sha256::digest(request.code.as_bytes()));
    if enrollment.expires_at_ms <= wall_clock_ms() as u128 {
        fs::remove_file(enrollment_path)?;
        return reject("enrollment expired");
    }
    if enrollment.failed_attempts >= 5 {
        return reject("enrollment throttled");
    }
    if request.code.len() != 64 || hash != enrollment.code_hash {
        enrollment.failed_attempts += 1;
        save_enrollment(path, &enrollment)?;
        return reject("invalid enrollment code");
    }
    let public_key = match decode_hex(&request.public_key).and_then(|bytes| {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "public key must be 32 bytes")
        })?;
        VerifyingKey::from_bytes(&bytes).map_err(io::Error::other)?;
        Ok(encode_hex(&bytes))
    }) {
        Ok(key) => key,
        Err(_) => return reject("invalid browser public key"),
    };
    // Consume first so a crash can lose an enrollment, but can never register its key twice.
    fs::remove_file(enrollment_path)?;
    sync_directory(path)?;
    let mut clients = load_authorized_clients(path)?;
    let id = Uuid::now_v7();
    clients.clients.push(AuthorizedClient {
        client_id: id,
        public_key,
        label: enrollment.label,
        scope: ClientScope::Limited,
        allow_session_create: enrollment.allow_session_create,
        allow_approvals: true,
    });
    save_authorized_clients(path, &clients)?;
    let host_key = load_or_create_signing_key(&path.join("host.key"))?;
    Ok(
        json!({"kind":"enrolled","client_id":id,"scope":"limited","host_public_key":encode_hex(&host_key.verifying_key().to_bytes())}),
    )
}

fn lock_client_registry(directory: &fs::File) -> io::Result<fs::File> {
    let create_flags =
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW;
    let open_flags = OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW;
    let descriptor = match openat(
        directory.as_fd(),
        "authorized-clients.lock",
        create_flags,
        Mode::from_bits_truncate(0o600),
    ) {
        Ok(descriptor) => Ok(descriptor),
        Err(nix::errno::Errno::EEXIST) => openat(
            directory.as_fd(),
            "authorized-clients.lock",
            open_flags,
            Mode::empty(),
        ),
        Err(nix::errno::Errno::ENOENT) => match openat(
            directory.as_fd(),
            "authorized-clients.lock",
            open_flags,
            Mode::empty(),
        ) {
            Ok(descriptor) => Ok(descriptor),
            Err(nix::errno::Errno::ENOENT) => openat(
                directory.as_fd(),
                "authorized-clients.lock",
                create_flags,
                Mode::from_bits_truncate(0o600),
            ),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
    .map_err(io::Error::other)?;
    let metadata = fstat(&descriptor).map_err(io::Error::other)?;
    if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
        .contains(nix::sys::stat::SFlag::S_IFREG)
        || metadata.st_uid != nix::unistd::geteuid().as_raw()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "authorized-clients lock must be an owner-owned regular file",
        ));
    }
    fchmod(&descriptor, Mode::from_bits_truncate(0o600)).map_err(io::Error::other)?;
    let lock: fs::File = descriptor.into();
    lock.lock()?;
    Ok(lock)
}

fn scope_name(scope: ClientScope) -> &'static str {
    match scope {
        ClientScope::Full => "full",
        ClientScope::Limited => "limited",
    }
}

fn load_authorized_clients(path: &Path) -> io::Result<AuthorizedClients> {
    let file_path = path.join("authorized-clients.json");
    match fs::symlink_metadata(&file_path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && metadata.uid() == nix::unistd::geteuid().as_raw()
                && metadata.permissions().mode() & 0o777 == 0o600 =>
        {
            serde_json::from_slice(&fs::read(file_path)?).map_err(io::Error::other)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "authorized-clients file must be owner-owned, regular and mode 0600",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AuthorizedClients::default()),
        Err(error) => Err(error),
    }
}

fn authorized_client(
    path: &Path,
    client_id: Uuid,
    public_key: &str,
) -> io::Result<Option<AuthorizedClient>> {
    let directory = fs::File::open(path)?;
    let _registry_lock = lock_client_registry(&directory)?;
    Ok(load_authorized_clients(path)?
        .clients
        .into_iter()
        .find(|client| client.client_id == client_id && client.public_key == public_key))
}

fn save_authorized_clients(path: &Path, clients: &AuthorizedClients) -> io::Result<()> {
    let target = path.join("authorized-clients.json");
    if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.file_type().is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "authorized-clients file must be regular",
        ));
    }
    let temporary = path.join(format!(".authorized-clients-{}.tmp", Uuid::now_v7()));
    let mut file = create_private_file(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(clients).map_err(io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(temporary, target)?;
    sync_directory(path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

fn create_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    fchmod(&file, Mode::from_bits_truncate(0o600)).map_err(io::Error::other)?;
    Ok(file)
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn load_or_create_signing_key(path: &Path) -> io::Result<SigningKey> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.uid() != nix::unistd::geteuid().as_raw()
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "private key must be an owner-only regular file",
                ));
            }
            let bytes: [u8; 32] = fs::read(path)?.try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "private key has invalid length")
            })?;
            Ok(SigningKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let key = generate_signing_key()?;
            write_private_bytes(path, &key.to_bytes())?;
            Ok(key)
        }
        Err(error) => Err(error),
    }
}

fn generate_signing_key() -> io::Result<SigningKey> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> io::Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid hexadecimal encoding",
        ));
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |byte: u8| {
                (byte as char)
                    .to_digit(16)
                    .map(|digit| digit as u8)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid hexadecimal encoding")
                    })
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}

fn transcript_hash(
    versions: [u32; 3],
    client_nonce: &str,
    server_nonce: &str,
    client_key: &str,
    host_key: &str,
) -> io::Result<[u8; 32]> {
    let client_nonce = decode_hex(client_nonce)?;
    let server_nonce = decode_hex(server_nonce)?;
    let client_key = decode_hex(client_key)?;
    let host_key = decode_hex(host_key)?;
    if client_nonce.len() != 32
        || server_nonce.len() != 32
        || client_key.len() != 32
        || host_key.len() != 32
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "handshake nonce or public key has invalid length",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"Shardrail mutual client handshake v1\0");
    for version in versions {
        hasher.update(version.to_be_bytes());
    }
    for field in [&client_nonce, &server_nonce, &client_key, &host_key] {
        hasher.update((field.len() as u32).to_be_bytes());
        hasher.update(field);
    }
    Ok(hasher.finalize().into())
}

fn refuse_synced_path(path: &Path) -> io::Result<()> {
    let absolute = absolute_path(path)?;
    let ancestor = absolute
        .ancestors()
        .find(|candidate| candidate.exists())
        .unwrap_or(Path::new("/"));
    let canonical =
        fs::canonicalize(ancestor)?.join(absolute.strip_prefix(ancestor).unwrap_or(&absolute));
    let path =
        format!("{} {}", path.to_string_lossy(), canonical.to_string_lossy()).to_ascii_lowercase();
    let blocked = ["icloud drive", "mobile documents", "dropbox", "onedrive"];
    if blocked.iter().any(|name| path.contains(name)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Shardrail data must be stored on a local, unsynced filesystem (iCloud, Dropbox, and OneDrive are not supported)",
        ));
    }
    Ok(())
}

fn refuse_network_filesystem(path: &Path) -> io::Result<()> {
    let absolute = absolute_path(path)?;
    let probe = absolute
        .ancestors()
        .find(|candidate| candidate.exists())
        .unwrap_or(Path::new("/"));
    let stats = statfs(probe).map_err(io::Error::other)?;
    #[cfg(target_os = "linux")]
    let is_network = linux_network_fs_type(stats.filesystem_type().0 as i64);
    #[cfg(target_os = "macos")]
    let is_network = network_fs_name(&stats.filesystem_type_name().to_ascii_lowercase());
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let is_network = network_fs_name(&stats.filesystem_type_name().to_ascii_lowercase());
    if is_network {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Shardrail data directory is on a network filesystem; choose a local filesystem",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_network_fs_type(fs_type: i64) -> bool {
    matches!(
        fs_type,
        0x6969 | 0x517b | 0xff534d42 | 0xfe534d42 | 0x01021997 | 0x564c
    )
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn network_fs_name(name: &str) -> bool {
    [
        "nfs",
        "cifs",
        "smb",
        "afp",
        "webdav",
        "9p",
        "sshfs",
        "fuse.sshfs",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn bind_private_socket(path: &Path) -> io::Result<UnixListener> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || !meta.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to replace a non-socket daemon path",
            ));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn serve_client(stream: UnixStream, daemon: Arc<Daemon>) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let (outbound, mut queued_frames) = mpsc::channel::<Vec<u8>>(CLIENT_QUEUE_CAPACITY);
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = queued_frames.recv().await {
            if write.write_all(&frame).await.is_err() {
                break;
            }
        }
    });
    let mut abort_writer = AbortWriter(Some(writer_task));
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if timeout(HANDSHAKE_TIMEOUT, read.read_line(&mut line))
        .await
        .map_err(|_| handshake_timeout())??
        == 0
    {
        return Ok(());
    }
    let frame_kind = serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(|kind| kind.as_str())
                .map(str::to_owned)
        });
    if frame_kind.as_deref() == Some("enroll") {
        let request: EnrollmentRequest = serde_json::from_str(&line).map_err(io::Error::other)?;
        let reply = redeem_enrollment(daemon.data.directory_path(), request)?;
        finish_with_json(outbound, &mut abort_writer, &reply).await?;
        return Ok(());
    }
    if frame_kind.as_deref() == Some("identity") {
        finish_with_json(
            outbound,
            &mut abort_writer,
            &json!({
                "kind": "identity",
                "host_public_key": encode_hex(&daemon.host_key.verifying_key().to_bytes()),
            }),
        )
        .await?;
        return Ok(());
    }
    let hello: ClientHello = serde_json::from_str(&line).map_err(io::Error::other)?;
    if hello.min_version > PROTOCOL_VERSION || hello.max_version < PROTOCOL_VERSION {
        return finish_with_error(
            outbound,
            &mut abort_writer,
            "no mutually supported protocol version",
        )
        .await;
    }
    let authorized_clients = load_authorized_clients(daemon.data.directory_path())?;
    let Some(client) = authorized_clients
        .clients
        .iter()
        .find(|client| client.public_key == hello.public_key)
    else {
        return finish_with_error(outbound, &mut abort_writer, "client key is not authorized")
            .await;
    };
    if hello.client_id != client.client_id {
        return finish_with_error(
            outbound,
            &mut abort_writer,
            "client id does not match its authorized key",
        )
        .await;
    }
    let client_key_bytes: [u8; 32] = decode_hex(&hello.public_key)?.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "client public key has invalid length",
        )
    })?;
    let client_key = VerifyingKey::from_bytes(&client_key_bytes).map_err(io::Error::other)?;
    let mut server_nonce_bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut server_nonce_bytes)?;
    let server_nonce = encode_hex(&server_nonce_bytes);
    let transcript = transcript_hash(
        [hello.min_version, hello.max_version, PROTOCOL_VERSION],
        &hello.nonce,
        &server_nonce,
        &hello.public_key,
        &encode_hex(&daemon.host_key.verifying_key().to_bytes()),
    )?;
    let server_signature = daemon.host_key.sign(&transcript);
    let (epoch, host_id, high_water) = {
        let journal = daemon.journal.lock().await;
        (
            journal.journal_epoch().map_err(io::Error::other)?,
            journal.host_id().map_err(io::Error::other)?,
            journal.high_water().map_err(io::Error::other)?,
        )
    };
    let mismatch = hello
        .journal_epoch
        .is_some_and(|client_epoch| client_epoch != epoch);
    let mut cursor = if mismatch {
        0
    } else {
        hello.resume_after.max(0)
    };
    let server = ServerHello {
        protocol_version: PROTOCOL_VERSION,
        host_id,
        journal_epoch: epoch,
        public_key: encode_hex(&daemon.host_key.verifying_key().to_bytes()),
        nonce: server_nonce,
        signature: encode_hex(&server_signature.to_bytes()),
        scope: client.scope,
        cursor: high_water,
        resync_required: mismatch,
    };
    write_json_line(&outbound, &server).await?;
    line.clear();
    if timeout(HANDSHAKE_TIMEOUT, read.read_line(&mut line))
        .await
        .map_err(|_| handshake_timeout())??
        == 0
    {
        return Ok(());
    }
    let auth = parse_client_frame(&line)?;
    let ClientFrame::Authenticate { signature } = auth else {
        return finish_with_error(outbound, &mut abort_writer, "authentication required").await;
    };
    let signature = match decode_hex(&signature)
        .and_then(|bytes| Signature::from_slice(&bytes).map_err(io::Error::other))
    {
        Ok(signature) => signature,
        Err(_) => {
            return finish_with_error(outbound, &mut abort_writer, "client proof is invalid").await;
        }
    };
    if client_key.verify_strict(&transcript, &signature).is_err() {
        return finish_with_error(outbound, &mut abort_writer, "client proof is invalid").await;
    }
    write_frame(&outbound, &DaemonFrame::Authenticated).await?;
    if client.scope == ClientScope::Limited && client.allow_session_create {
        write_frame(
            &outbound,
            &DaemonFrame::SessionWorkdirs {
                workdirs: daemon
                    .session_workdirs
                    .iter()
                    .map(|entry| entry.path.display().to_string())
                    .collect(),
            },
        )
        .await?;
    }
    let mut event_generation = daemon.event_generation.subscribe();
    push_events(&outbound, &daemon, &mut cursor).await?;
    let mut lines = read.lines();
    loop {
        let read_result = tokio::select! {
            result = lines.next_line() => Some(result?),
            changed = event_generation.changed() => {
                changed.map_err(io::Error::other)?;
                None
            },
        };
        let line = match read_result {
            Some(Some(line)) => line,
            Some(None) => return Ok(()),
            None => {
                push_events(&outbound, &daemon, &mut cursor).await?;
                continue;
            }
        };
        let frame = match parse_client_frame(&line) {
            Ok(frame) => frame,
            Err(error) => {
                if let Some(attempt) = malformed_approval_attempt(&line)
                    && let Ok(Some(current)) = authorized_client(
                        daemon.data.directory_path(),
                        client.client_id,
                        &client.public_key,
                    )
                {
                    let _ = record_malformed_approval(&daemon, &current, &attempt).await;
                }
                return Err(error);
            }
        };
        let client = authorized_client(
            daemon.data.directory_path(),
            client.client_id,
            &client.public_key,
        )?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "client authorization was revoked",
            )
        })?;
        let (request, resolution, resume_after) = match frame {
            ClientFrame::Authenticate { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authentication is only valid during handshake",
                ));
            }
            ClientFrame::Request { request } => (Some(request), None, None),
            ClientFrame::ApprovalResolution { request } => (None, Some(request), None),
            ClientFrame::Resume { resume_after } => (None, None, Some(resume_after)),
        };
        if let Some(request) = resolution {
            let approval_id = request.approval_id;
            let _command = daemon.command_lock.lock().await;
            let result = resolve_approval(&daemon, &client, request).await;
            match result {
                Ok(()) => {
                    write_frame(&outbound, &DaemonFrame::ApprovalResolved { approval_id }).await?
                }
                Err(error) => write_frame(&outbound, &DaemonFrame::Error { error }).await?,
            }
            continue;
        }
        if let Some(request) = request {
            if request.expected_state.is_some() || request.expected_turn_id.is_some() {
                if request.kind == "session.create" {
                    let workdir = audit_workdir_text(request.payload.get("workdir"));
                    reject_session_create(
                        &daemon,
                        &client,
                        &outbound,
                        request.command_id,
                        workdir.as_deref(),
                        "invalid_preconditions",
                        "unsupported command or invalid preconditions",
                    )
                    .await?;
                } else {
                    write_frame(
                        &outbound,
                        &DaemonFrame::CommandRejected {
                            command_id: request.command_id,
                            error: "unsupported command or invalid preconditions".into(),
                        },
                    )
                    .await?;
                }
                continue;
            }
            if request.kind == "turn.start" || request.kind == "turn.interrupt" {
                codex_app_server::handle_turn_command(&daemon, &outbound, &request).await?;
                continue;
            }
            if request.kind != "session.create" {
                let error = if client.scope == ClientScope::Limited {
                    "limited clients cannot create sessions"
                } else {
                    "unsupported command or invalid preconditions"
                };
                write_frame(
                    &outbound,
                    &DaemonFrame::CommandRejected {
                        command_id: request.command_id,
                        error: error.into(),
                    },
                )
                .await?;
                continue;
            }
            handle_session_create(&daemon, &client, request, &outbound, &mut cursor).await?;
        }
        if let Some(resume_after) = resume_after {
            cursor = resume_after.max(0);
            push_events(&outbound, &daemon, &mut cursor).await?;
        }
    }
}

enum WorkdirChoice {
    Unset,
    Path(session_workdir::PinnedWorkdir),
}

fn capability_name(client: &AuthorizedClient) -> &'static str {
    if client.scope == ClientScope::Full {
        "full"
    } else if client.allow_session_create {
        "session.create"
    } else {
        "none"
    }
}

fn requested_workdir(payload: &serde_json::Value) -> Result<Option<&str>, &'static str> {
    if payload.is_null() {
        return Ok(None);
    }
    let Some(object) = payload.as_object() else {
        return Err("workdir is invalid");
    };
    match object.get("workdir") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => {
            if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
                Err("workdir is invalid")
            } else {
                Ok(Some(text.as_str()))
            }
        }
        Some(_) => Err("workdir is invalid"),
    }
}

fn audit_workdir_text(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value?.as_str()?;
    if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
        None
    } else {
        Some(text.to_owned())
    }
}

fn choose_workdir(
    client: &AuthorizedClient,
    payload: &serde_json::Value,
    allowlist: &[session_workdir::AllowedWorkdir],
) -> Result<WorkdirChoice, &'static str> {
    match requested_workdir(payload)? {
        None if client.scope != ClientScope::Full => {
            Err("limited clients must name an allowlisted workdir")
        }
        None => match allowlist.first() {
            Some(default) => Ok(WorkdirChoice::Path(session_workdir::pin_allowed(default)?)),
            None => Ok(WorkdirChoice::Unset),
        },
        Some(raw) => Ok(WorkdirChoice::Path(
            session_workdir::accept_requested_workdir(raw, allowlist)?,
        )),
    }
}

async fn record_session_create_audit(
    daemon: &Daemon,
    client: &AuthorizedClient,
    capability: &str,
    workdir: Option<&str>,
    outcome: &str,
    reason: Option<&str>,
) -> io::Result<()> {
    daemon_log::log(
        "INFO",
        "session",
        &format!(
            "client_id={} label={} scope={} capability={} workdir={} outcome={} reason={}",
            client.client_id,
            client.label,
            scope_name(client.scope),
            capability,
            workdir.unwrap_or("-"),
            outcome,
            reason.unwrap_or("-"),
        ),
    );
    let payload = serde_json::to_value(SessionCreateAudit {
        client_id: client.client_id,
        client_label: client.label.clone(),
        scope: client.scope,
        capability: capability.to_owned(),
        workdir: workdir.map(str::to_owned),
        outcome: outcome.to_owned(),
        reason: reason.map(str::to_owned),
    })
    .map_err(io::Error::other)?;
    let mut journal = daemon.journal.lock().await;
    let journal_epoch = journal.journal_epoch().map_err(io::Error::other)?;
    let host_id = journal.host_id().map_err(io::Error::other)?;
    journal
        .append_event(&DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id: Uuid::nil(),
            schema_version: 1,
            kind: "session_create_audit".into(),
            occurred_at_ms: wall_clock_ms(),
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload,
        })
        .map_err(io::Error::other)?;
    drop(journal);
    daemon
        .event_generation
        .send_modify(|generation| *generation += 1);
    Ok(())
}

async fn reject_session_create(
    daemon: &Daemon,
    client: &AuthorizedClient,
    outbound: &mpsc::Sender<Vec<u8>>,
    command_id: Uuid,
    workdir: Option<&str>,
    reason: &str,
    error: &str,
) -> io::Result<()> {
    record_session_create_audit(
        daemon,
        client,
        capability_name(client),
        workdir,
        "denied",
        Some(reason),
    )
    .await?;
    write_frame(
        outbound,
        &DaemonFrame::CommandRejected {
            command_id,
            error: error.to_owned(),
        },
    )
    .await
}

async fn saved_session_workdir(daemon: &Daemon, session_id: Uuid) -> io::Result<Option<PathBuf>> {
    let journal = daemon.journal.lock().await;
    let events = journal
        .events_for_session(session_id)
        .map_err(io::Error::other)?;
    Ok(events
        .iter()
        .find(|event| event.kind == "session_created")
        .and_then(|event| {
            event
                .payload
                .get("workdir")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        })
        .filter(|path| {
            daemon
                .session_workdirs
                .iter()
                .any(|allowed| &allowed.path == path)
        }))
}

async fn handle_session_create(
    daemon: &Arc<Daemon>,
    client: &AuthorizedClient,
    request: CommandRequest,
    outbound: &mpsc::Sender<Vec<u8>>,
    cursor: &mut i64,
) -> io::Result<()> {
    let capability = capability_name(client);
    if client.scope != ClientScope::Full && !client.allow_session_create {
        let workdir = audit_workdir_text(request.payload.get("workdir"));
        return reject_session_create(
            daemon,
            client,
            outbound,
            request.command_id,
            workdir.as_deref(),
            "scope",
            "limited clients cannot create sessions",
        )
        .await;
    }
    let choice = match choose_workdir(client, &request.payload, &daemon.session_workdirs) {
        Ok(choice) => choice,
        Err(error) => {
            let workdir = audit_workdir_text(request.payload.get("workdir"));
            return reject_session_create(
                daemon,
                client,
                outbound,
                request.command_id,
                workdir.as_deref(),
                "workdir",
                error,
            )
            .await;
        }
    };
    let _command = daemon.command_lock.lock().await;
    let existing = daemon
        .journal
        .lock()
        .await
        .created_session_for_command(client.client_id, request.command_id)
        .map_err(io::Error::other)?;
    if let Some((session_id, event_id)) = existing {
        match codex_start_class(daemon, session_id).await? {
            CodexStart::Rejected => {
                write_frame(
                    outbound,
                    &DaemonFrame::CommandRejected {
                        command_id: request.command_id,
                        error: "codex extensions rejected".into(),
                    },
                )
                .await?;
                push_events(outbound, daemon, cursor).await?;
                return Ok(());
            }
            CodexStart::Incomplete => {
                if let Some(mut config) = daemon.codex.clone() {
                    let saved = saved_session_workdir(daemon, session_id).await?;
                    let entry = saved
                        .as_ref()
                        .and_then(|path| {
                            daemon
                                .session_workdirs
                                .iter()
                                .find(|allowed| &allowed.path == path)
                        })
                        .or_else(|| daemon.session_workdirs.first());
                    let Some(entry) = entry else {
                        write_frame(
                            outbound,
                            &DaemonFrame::CommandRejected {
                                command_id: request.command_id,
                                error: "workdir is not allowlisted".into(),
                            },
                        )
                        .await?;
                        push_events(outbound, daemon, cursor).await?;
                        return Ok(());
                    };
                    match session_workdir::pin_allowed(entry) {
                        Ok(pinned) => {
                            config.workdir = pinned.path;
                            config.directory = Some(Arc::new(pinned.directory));
                        }
                        Err(error) => {
                            write_frame(
                                outbound,
                                &DaemonFrame::CommandRejected {
                                    command_id: request.command_id,
                                    error: error.into(),
                                },
                            )
                            .await?;
                            push_events(outbound, daemon, cursor).await?;
                            return Ok(());
                        }
                    }
                    let status = codex_app_server::start_session(
                        Arc::clone(daemon),
                        config,
                        session_id,
                        || async { Ok(()) },
                    )
                    .await;
                    if let codex_app_server::CodexStatus::Failed(error) = status {
                        write_frame(
                            outbound,
                            &DaemonFrame::CommandRejected {
                                command_id: request.command_id,
                                error,
                            },
                        )
                        .await?;
                        push_events(outbound, daemon, cursor).await?;
                        return Ok(());
                    }
                }
            }
            CodexStart::Started => {}
        }
        let receipt = daemon
            .journal
            .lock()
            .await
            .event_receipt(event_id)
            .map_err(io::Error::other)?;
        write_frame(
            outbound,
            &DaemonFrame::CommandAccepted {
                command_id: request.command_id,
                receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                cursor: event_id,
            },
        )
        .await?;
        push_events(outbound, daemon, cursor).await?;
        return Ok(());
    }
    let workdir = match choice {
        WorkdirChoice::Unset if daemon.codex.is_some() => {
            return reject_session_create(
                daemon,
                client,
                outbound,
                request.command_id,
                None,
                "codex",
                "codex is not configured",
            )
            .await;
        }
        WorkdirChoice::Unset => None,
        WorkdirChoice::Path(pinned) if daemon.codex.is_none() => {
            let text = pinned.path.display().to_string();
            return reject_session_create(
                daemon,
                client,
                outbound,
                request.command_id,
                Some(&text),
                "codex",
                "codex is not configured",
            )
            .await;
        }
        WorkdirChoice::Path(pinned) => Some(pinned),
    };
    let workdir_text = workdir
        .as_ref()
        .map(|pinned| pinned.path.display().to_string());
    let session_id = Uuid::now_v7();
    let (event_id, receipt) = if let Some(mut config) = daemon.codex.clone() {
        let Some(pinned) = workdir else {
            return reject_session_create(
                daemon,
                client,
                outbound,
                request.command_id,
                None,
                "codex",
                "codex is not configured",
            )
            .await;
        };
        config.workdir = pinned.path;
        config.directory = Some(Arc::new(pinned.directory));
        let committed = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(std::sync::Mutex::new(None));
        let flag = Arc::clone(&committed);
        let recorded = Arc::clone(&slot);
        let daemon_for_commit = Arc::clone(daemon);
        let client_for_commit = client.clone();
        let capability_for_commit = capability.to_owned();
        let command_id = request.command_id;
        let workdir_for_commit = workdir_text.clone();
        let status = codex_app_server::start_session(
            Arc::clone(daemon),
            config,
            session_id,
            || async move {
                let created = commit_created_session(
                    &daemon_for_commit,
                    &client_for_commit,
                    &capability_for_commit,
                    command_id,
                    session_id,
                    workdir_for_commit.as_deref(),
                )
                .await?;
                *recorded.lock().unwrap_or_else(|error| error.into_inner()) = Some(created);
                flag.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
        if let codex_app_server::CodexStatus::Failed(error) = status {
            if !committed.load(Ordering::SeqCst) {
                return reject_session_create(
                    daemon,
                    client,
                    outbound,
                    request.command_id,
                    workdir_text.as_deref(),
                    session_create_failure_reason(&error),
                    &error,
                )
                .await;
            }
            write_frame(
                outbound,
                &DaemonFrame::CommandRejected {
                    command_id: request.command_id,
                    error,
                },
            )
            .await?;
            push_events(outbound, daemon, cursor).await?;
            return Ok(());
        }
        slot.lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| io::Error::other("session creation event was not committed"))?
    } else {
        commit_created_session(
            daemon,
            client,
            capability,
            request.command_id,
            session_id,
            workdir_text.as_deref(),
        )
        .await?
    };
    write_frame(
        outbound,
        &DaemonFrame::CommandAccepted {
            command_id: request.command_id,
            receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
            receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
            cursor: event_id,
        },
    )
    .await?;
    push_events(outbound, daemon, cursor).await?;
    if daemon.codex.is_none()
        && let Some(path) = daemon.fake_acp_path.clone()
    {
        tokio::spawn(run_fake_acp(Arc::clone(daemon), path, session_id));
    }
    Ok(())
}

async fn commit_created_session(
    daemon: &Arc<Daemon>,
    client: &AuthorizedClient,
    capability: &str,
    command_id: Uuid,
    session_id: Uuid,
    workdir: Option<&str>,
) -> io::Result<(i64, Option<(u128, i64)>)> {
    let now_ms = wall_clock_ms();
    let mut created = json!({
        "session_id": session_id,
        "client_id": client.client_id,
        "command_id": command_id,
    });
    let audit = serde_json::to_value(SessionCreateAudit {
        client_id: client.client_id,
        client_label: client.label.clone(),
        scope: client.scope,
        capability: capability.to_owned(),
        workdir: workdir.map(str::to_owned),
        outcome: "allowed".to_owned(),
        reason: None,
    })
    .map_err(io::Error::other)?;
    if let (Some(object), Some(extra)) = (created.as_object_mut(), audit.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    let mut coordinator = Coordinator::open(
        Journal::open(daemon.data.journal_path()).map_err(io::Error::other)?,
        session_id,
    )
    .map_err(io::Error::other)?;
    coordinator
        .transition_session(
            now_ms,
            "session_created",
            created,
            Session::default(),
            || Ok::<_, io::Error>(()),
        )
        .map_err(io::Error::other)?;
    daemon
        .sessions
        .lock()
        .await
        .insert(session_id, Arc::new(Mutex::new(coordinator)));
    record_session_create_audit(daemon, client, capability, workdir, "allowed", None).await?;
    let journal = daemon.journal.lock().await;
    let event = journal
        .events_for_session(session_id)
        .map_err(io::Error::other)?
        .into_iter()
        .last()
        .ok_or_else(|| io::Error::other("session creation event was not committed"))?;
    let event_id = event.event_id;
    let receipt = journal.event_receipt(event_id).map_err(io::Error::other)?;
    Ok((event_id, receipt))
}

fn session_create_failure_reason(error: &str) -> &'static str {
    if error.starts_with("codex extensions rejected") {
        "extensions"
    } else {
        "codex"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexStart {
    Started,
    Rejected,
    Incomplete,
}

fn classify_codex_start<'a>(kinds: impl IntoIterator<Item = &'a str>) -> CodexStart {
    let mut rejected = false;
    for kind in kinds {
        if matches!(kind, "process_started" | "approval_routing_rejected") {
            return CodexStart::Started;
        }
        if kind == "codex_extensions_rejected" {
            rejected = true;
        }
    }
    if rejected {
        CodexStart::Rejected
    } else {
        CodexStart::Incomplete
    }
}

async fn codex_start_class(daemon: &Daemon, session_id: Uuid) -> io::Result<CodexStart> {
    let journal = daemon.journal.lock().await;
    let events = journal
        .events_for_session(session_id)
        .map_err(io::Error::other)?;
    Ok(classify_codex_start(
        events.iter().map(|event| event.kind.as_str()),
    ))
}

async fn push_events(
    outbound: &mpsc::Sender<Vec<u8>>,
    daemon: &Daemon,
    cursor: &mut i64,
) -> io::Result<()> {
    loop {
        let events = {
            let journal = daemon.journal.lock().await;
            journal
                .events_after_with_receipts(*cursor, 16)
                .map_err(io::Error::other)?
        };
        if events.is_empty() {
            return Ok(());
        }
        let full_page = events.len() == 16;
        for item in events {
            let event = item.event;
            let receipt = item.receipt;
            write_frame(
                outbound,
                &DaemonFrame::DurableEvent {
                    cursor: event.event_id,
                    event: event.clone(),
                    receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                    receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                },
            )
            .await?;
            *cursor = event.event_id;
            tokio::task::yield_now().await;
        }
        if !full_page {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
}

async fn write_frame(outbound: &mpsc::Sender<Vec<u8>>, frame: &DaemonFrame) -> io::Result<()> {
    write_json_line(outbound, frame).await
}

async fn write_json_line<T: serde::Serialize>(
    outbound: &mpsc::Sender<Vec<u8>>,
    value: &T,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    match timeout(Duration::from_secs(1), outbound.send(bytes)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "client writer closed",
        )),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "client outbound queue stayed full for one second; reconnect and resume from your last received cursor",
        )),
    }
}

fn handshake_timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "client handshake timed out")
}

async fn finish_with_error(
    outbound: mpsc::Sender<Vec<u8>>,
    writer: &mut AbortWriter,
    error: &str,
) -> io::Result<()> {
    write_frame(
        &outbound,
        &DaemonFrame::Error {
            error: error.into(),
        },
    )
    .await?;
    drop(outbound);
    if let Some(task) = writer.0.take() {
        task.await.map_err(io::Error::other)?;
    }
    Ok(())
}

async fn finish_with_json<T: serde::Serialize>(
    outbound: mpsc::Sender<Vec<u8>>,
    writer: &mut AbortWriter,
    value: &T,
) -> io::Result<()> {
    write_json_line(&outbound, value).await?;
    drop(outbound);
    if let Some(task) = writer.0.take() {
        task.await.map_err(io::Error::other)?;
    }
    Ok(())
}

struct AbortWriter(Option<tokio::task::JoinHandle<()>>);

impl Drop for AbortWriter {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enrollment_fixture(expires_at_ms: u128) -> (tempfile::TempDir, String, String) {
        enrollment_fixture_with(expires_at_ms, false)
    }

    fn enrollment_fixture_with(
        expires_at_ms: u128,
        allow_session_create: bool,
    ) -> (tempfile::TempDir, String, String) {
        let directory = tempfile::tempdir().unwrap();
        let (_fd, path) = open_private_data_directory(directory.path()).unwrap();
        let secret = "ab".repeat(32);
        let signing_key = SigningKey::from_bytes(&[0xab; 32]);
        let public_key = encode_hex(&signing_key.verifying_key().to_bytes());
        save_enrollment(
            &path,
            &Enrollment {
                code_hash: encode_hex(&Sha256::digest(secret.as_bytes())),
                expires_at_ms,
                failed_attempts: 0,
                label: "test browser".into(),
                allow_session_create,
            },
        )
        .unwrap();
        (directory, secret, public_key)
    }

    fn enrollment_request(code: String, public_key: String) -> EnrollmentRequest {
        EnrollmentRequest {
            kind: "enroll".into(),
            code,
            public_key,
        }
    }

    #[test]
    fn disclosed_action_keeps_machine_fields_and_drops_agent_text() {
        let params = serde_json::json!({
            "cwd": "/tmp/work",
            "toolCall": {
                "title": "Run tests (safe)",
                "command": "curl",
                "argv": ["curl", "https://example.test"],
                "rawInput": {"cmd": "curl"},
                "sandbox": "workspace-write"
            }
        });
        let action = disclosed_action(&params);
        assert_eq!(
            action.get("command").and_then(|value| value.as_str()),
            Some("curl")
        );
        assert_eq!(
            action.get("cwd").and_then(|value| value.as_str()),
            Some("/tmp/work")
        );
        assert_eq!(
            action.get("sandbox").and_then(|value| value.as_str()),
            Some("workspace-write")
        );
        assert!(action.get("title").is_none());
        assert_eq!(
            action
                .get("argv")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(2)
        );
        assert!(disclosed_action(&serde_json::json!({"toolCall": {"title": "Run fixture command", "kind": "execute"}})).is_empty());
    }

    #[test]
    fn enrollment_is_limited_single_use_and_rejects_expiry_and_throttling() {
        let (directory, code, key) = enrollment_fixture(wall_clock_ms() as u128 + 60_000);
        let first = redeem_enrollment(
            directory.path(),
            enrollment_request(code.clone(), key.clone()),
        )
        .unwrap();
        assert_eq!(first["scope"], "limited");
        assert_eq!(first["kind"], "enrolled");
        assert_eq!(
            redeem_enrollment(directory.path(), enrollment_request(code, key.clone())).unwrap()["kind"],
            "error"
        );
        let (expired, code, key) = enrollment_fixture(0);
        assert_eq!(
            redeem_enrollment(expired.path(), enrollment_request(code, key)).unwrap()["error"],
            "enrollment expired"
        );
        let (throttled, _, key) = enrollment_fixture(wall_clock_ms() as u128 + 60_000);
        for _ in 0..5 {
            assert_eq!(
                redeem_enrollment(
                    throttled.path(),
                    enrollment_request("00".repeat(32), key.clone())
                )
                .unwrap()["kind"],
                "error"
            );
        }
        assert_eq!(
            redeem_enrollment(throttled.path(), enrollment_request("00".repeat(32), key)).unwrap()
                ["error"],
            "enrollment throttled"
        );
    }

    #[test]
    fn malformed_enrollment_codes_count_toward_throttle() {
        let (directory, code, key) = enrollment_fixture(wall_clock_ms() as u128 + 60_000);
        for _ in 0..5 {
            assert_eq!(
                redeem_enrollment(
                    directory.path(),
                    enrollment_request("0".repeat(63), key.clone())
                )
                .unwrap()["error"],
                "invalid enrollment code"
            );
        }
        assert_eq!(
            redeem_enrollment(directory.path(), enrollment_request(code, key)).unwrap()["error"],
            "enrollment throttled"
        );
    }

    #[test]
    fn enrollment_consumption_is_durable_before_registry_failure() {
        let (directory, code, key) = enrollment_fixture(wall_clock_ms() as u128 + 60_000);
        fs::create_dir(directory.path().join("authorized-clients.json")).unwrap();
        assert!(
            redeem_enrollment(
                directory.path(),
                enrollment_request(code.clone(), key.clone())
            )
            .is_err()
        );
        assert_eq!(
            redeem_enrollment(directory.path(), enrollment_request(code, key)).unwrap()["error"],
            "enrollment unavailable"
        );
    }

    #[test]
    fn concurrent_enrollment_redemption_registers_only_one_client() {
        let (directory, code, key) = enrollment_fixture(wall_clock_ms() as u128 + 60_000);
        let path = directory.path().to_path_buf();
        let workers = (0..2)
            .map(|_| {
                let path = path.clone();
                let code = code.clone();
                let key = key.clone();
                std::thread::spawn(move || {
                    redeem_enrollment(&path, enrollment_request(code, key)).unwrap()["kind"]
                        == "enrolled"
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|won| *won)
                .count(),
            1
        );
        let clients = load_authorized_clients(directory.path()).unwrap();
        assert_eq!(clients.clients.len(), 1);
        assert_eq!(clients.clients[0].scope, ClientScope::Limited);
        assert!(!clients.clients[0].allow_session_create);
    }

    #[test]
    fn enrollment_flag_persists_and_old_records_default_off() {
        let old_client: AuthorizedClient = serde_json::from_str(
            r#"{"client_id":"00000000-0000-0000-0000-000000000000","public_key":"aa","label":"old","scope":"full"}"#,
        )
        .unwrap();
        assert!(!old_client.allow_session_create);
        assert!(old_client.allow_approvals);
        let old_enrollment: Enrollment = serde_json::from_str(
            r#"{"code_hash":"ab","expires_at_ms":1,"failed_attempts":0,"label":"browser"}"#,
        )
        .unwrap();
        assert!(!old_enrollment.allow_session_create);

        let (directory, code, key) =
            enrollment_fixture_with(wall_clock_ms() as u128 + 60_000, true);
        let enrolled = redeem_enrollment(directory.path(), enrollment_request(code, key)).unwrap();
        assert_eq!(enrolled["scope"], "limited");
        let clients = load_authorized_clients(directory.path()).unwrap();
        assert!(clients.clients[0].allow_session_create);
        assert_eq!(clients.clients[0].scope, ClientScope::Limited);
    }

    #[test]
    fn rejects_common_network_mount_names() {
        for name in ["nfs4", "cifs", "smbfs", "fuse.sshfs"] {
            assert!(network_fs_name(name));
        }
        for name in ["apfs", "ext4", "btrfs"] {
            assert!(!network_fs_name(name));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recognizes_linux_network_filesystem_magic_values() {
        for magic in [
            0x6969,
            0x517b,
            0xff534d42_u32 as i64,
            0xfe534d42_u32 as i64,
            0x01021997,
        ] {
            assert!(linux_network_fs_type(magic));
        }
        assert!(!linux_network_fs_type(0x6969 + 1));
    }

    #[test]
    fn rejects_synced_storage_path_names() {
        assert!(refuse_synced_path(Path::new("/Users/test/Library/Mobile Documents/db")).is_err());
        assert!(refuse_synced_path(Path::new("/var/lib/shardrail")).is_ok());
    }

    #[test]
    fn relative_data_paths_resolve_from_the_working_directory() {
        let resolved = absolute_path(Path::new(".shardrail-test-data")).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with(".shardrail-test-data"));
    }

    #[test]
    fn unknown_permission_options_require_full_scope() {
        assert_eq!(classify_option("allow_once"), OptionClass::Once);
        assert_eq!(classify_option("reject_once"), OptionClass::Once);
        assert_eq!(classify_option("grant_access"), OptionClass::Widening);
        assert_eq!(
            classify_option("grant_sandbox_bypass"),
            OptionClass::BoundaryBreaking
        );
    }

    #[test]
    fn rejects_forged_root_policy_fields() {
        let frame = r#"{"kind":"approval_resolution","scope":"full","request":{"approval_id":"019b0000-0000-7000-8000-000000000001","option_id":"allow_once"}}"#;
        assert!(parse_client_frame(frame).is_err());
        assert_eq!(
            malformed_approval_attempt(frame).unwrap().reason,
            "missing_binding"
        );
        assert!(malformed_approval_attempt(r#"{"kind":"resume","resume_after":0}"#).is_none());
        let extra = r#"{"kind":"approval_resolution","request":{"approval_id":"019b0000-0000-7000-8000-000000000001","option_id":"allow_once","content_hash":"ab","session_id":"019b0000-0000-7000-8000-000000000002","turn_id":"t","provider_request_id":"1","extra":"no"}}"#;
        assert!(parse_client_frame(extra).is_err());
        assert_eq!(
            malformed_approval_attempt(extra).unwrap().reason,
            "malformed_binding"
        );
    }

    #[test]
    fn disclosed_approval_content_keeps_provider_argv_and_raw_command_text() {
        let disclosed = disclosed_action(&json!({
            "toolCall": {
                "command": "echo \"a  b\"",
                "argv": ["echo", "a  b"],
                "rawInput": {"cmd": "echo"},
                "network": "https example.com:443"
            }
        }));
        let content = approval_content_from_disclosed("execute", &disclosed, "because");
        assert_eq!(content.command, "echo \"a  b\"");
        assert_eq!(content.argv, ["echo".to_owned(), "a  b".to_owned()]);
        assert!(content.network.contains("example.com:443"));
        let mut other = content.clone();
        other.command = "echo \"a b\"".into();
        assert_ne!(
            approval_binding::content_hash(&content),
            approval_binding::content_hash(&other)
        );
    }

    #[test]
    fn codex_extensions_rejected_is_terminal_and_a_later_start_wins() {
        assert_eq!(
            classify_codex_start(["session_created"]),
            CodexStart::Incomplete
        );
        assert_eq!(
            classify_codex_start(["session_created", "codex_extensions_rejected"]),
            CodexStart::Rejected
        );
        assert_eq!(
            classify_codex_start(["codex_extensions_rejected", "process_started"]),
            CodexStart::Started
        );
        assert_eq!(
            classify_codex_start(["approval_routing_rejected", "codex_extensions_rejected"]),
            CodexStart::Started
        );
    }
}

#[cfg(test)]
mod protocol_property_tests {
    use super::{parse_client_frame, transcript_hash};
    use ed25519_dalek::{Signer, SigningKey, Verifier};
    use proptest::prelude::*;
    use shardrail_journal::Journal;
    use shardrail_protocol::{
        ApprovalResolutionRequest, ClientFrame, CommandRequest, DurableEvent,
    };
    use uuid::Uuid;

    proptest! {
        #[test]
        fn every_client_frame_variant_round_trips(
            variant in 0u8..4,
            bytes in prop::array::uniform16(any::<u8>()),
            cursor in any::<i64>(),
            text in ".{0,64}",
            payload in any::<bool>(),
        ) {
            let id = Uuid::from_bytes(bytes);
            let frame = match variant {
                0 => ClientFrame::Authenticate { signature: text },
                1 => ClientFrame::Request { request: CommandRequest {
                    command_id: id,
                    kind: text,
                    expected_state: Some(cursor.to_string()),
                    expected_turn_id: Some(id),
                    payload: serde_json::json!({"value": payload}),
                } },
                2 => ClientFrame::ApprovalResolution { request: ApprovalResolutionRequest {
                    approval_id: id,
                    option_id: text.clone(),
                    content_hash: text,
                    session_id: id,
                    turn_id: cursor.to_string(),
                    provider_request_id: cursor.to_string(),
                } },
                _ => ClientFrame::Resume { resume_after: cursor },
            };
            let line = serde_json::to_string(&frame).unwrap();
            let decoded = parse_client_frame(&line).unwrap();
            prop_assert_eq!(decoded, frame);
        }

        #[test]
        fn transcript_binds_all_authentication_fields(
            client_nonce in prop::array::uniform32(any::<u8>()),
            server_nonce in prop::array::uniform32(any::<u8>()),
            client_key in prop::array::uniform32(any::<u8>()),
            host_key in prop::array::uniform32(any::<u8>()),
            signing_bytes in prop::array::uniform32(any::<u8>()),
        ) {
            let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let fields = [hex(&client_nonce), hex(&server_nonce), hex(&client_key), hex(&host_key)];
            let digest = transcript_hash([1, 1, 1], &fields[0], &fields[1], &fields[2], &fields[3]).unwrap();
            let signing_key = SigningKey::from_bytes(&signing_bytes);
            let signature = signing_key.sign(&digest);
            prop_assert!(signing_key.verifying_key().verify(&digest, &signature).is_ok());
            for changed_index in 0..fields.len() {
                let mut changed_fields = fields.clone();
                let first = if changed_fields[changed_index].starts_with("00") { "01" } else { "00" };
                changed_fields[changed_index] = format!("{first}{}", &changed_fields[changed_index][2..]);
                let changed = transcript_hash([1, 1, 1], &changed_fields[0], &changed_fields[1], &changed_fields[2], &changed_fields[3]).unwrap();
                prop_assert_ne!(digest, changed);
                prop_assert!(signing_key.verifying_key().verify(&changed, &signature).is_err());
            }
            let changed_version = transcript_hash([1, 1, 2], &fields[0], &fields[1], &fields[2], &fields[3]).unwrap();
            prop_assert_ne!(digest, changed_version);
            prop_assert!(signing_key.verifying_key().verify(&changed_version, &signature).is_err());
            prop_assert!(transcript_hash([1, 1, 1], "00", &fields[1], &fields[2], &fields[3]).is_err());
        }

        #[test]
        fn journal_resume_returns_each_event_after_the_cursor(
            event_count in 0usize..32,
            cursor in -2i64..34,
        ) {
            let mut journal = Journal::open(":memory:").unwrap();
            let epoch = journal.journal_epoch().unwrap();
            let host = journal.host_id().unwrap();
            for index in 0..event_count {
                journal.append_event(&DurableEvent {
                    event_id: 0,
                    journal_epoch: epoch,
                    host_id: host,
                    session_id: Uuid::nil(),
                    schema_version: 1,
                    kind: "test_event".into(),
                    occurred_at_ms: index as i64,
                    provider_ref: None,
                    causation: None,
                    redaction_flags: Vec::new(),
                    payload: serde_json::json!({"index": index}),
                }).unwrap();
            }
            let replayed = journal.events_after(cursor).unwrap();
            let expected = (1..=event_count as i64).filter(|id| *id > cursor).collect::<Vec<_>>();
            prop_assert_eq!(replayed.iter().map(|event| event.event_id).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn client_frames_reject_unknown_fields_and_trailing_json_values() {
        assert!(
            parse_client_frame(r#"{"kind":"resume","resume_after":0,"scope":"full"}"#).is_err()
        );
        assert!(parse_client_frame(r#"{"kind":"resume","resume_after":0} {}"#).is_err());
    }
}
