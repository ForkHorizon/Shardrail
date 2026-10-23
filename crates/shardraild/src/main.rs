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
use shardrail_protocol::{ApprovalDecision, ApprovalResolutionRequest, DurableEvent, OptionClass};
use shardrail_protocol::{ClientFrame, ClientHello, ClientScope, DaemonFrame, ServerHello};
use shardrail_session_core::{Approval, Session};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::AsFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

const PROTOCOL_VERSION: u32 = 1;
const CLIENT_QUEUE_CAPACITY: usize = 32;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(name = "shardraild", about = "Shardrail local coordinator daemon")]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
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
    let mut approvals = daemon.approvals.lock().await;
    let entry = approvals
        .get_mut(&request.approval_id)
        .ok_or_else(|| "approval is already resolved or unavailable".to_owned())?;
    if entry.provider.is_none()
        || entry.approval.state != shardrail_protocol::ApprovalState::Pending
    {
        return Err("approval is already resolved".into());
    }
    let option = entry
        .options
        .iter()
        .find(|option| option.id == request.option_id)
        .cloned()
        .ok_or_else(|| "option is not offered for this approval".to_owned())?;
    if client.scope == ClientScope::Limited && option.class != OptionClass::Once {
        return Err("limited clients cannot choose widening or boundary-breaking options".into());
    }
    let sender = entry
        .provider
        .as_ref()
        .cloned()
        .ok_or_else(|| "approval is already resolved".to_owned())?;
    let coordinator = daemon
        .sessions
        .lock()
        .await
        .get(&entry.session_id)
        .cloned()
        .ok_or_else(|| "session coordinator unavailable".to_owned())?;
    let mut coordinator = coordinator.lock().await;
    let write_delay = Duration::from_millis(daemon.test_write_ahead_delay_ms);
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
            let _ = provider.send(ProviderCommand::cancel());
        }
        return Err(error.to_string());
    }
    if let Some(task) = entry.timeout_task.take() {
        task.abort();
    }
    approvals.remove(&request.approval_id);
    Ok(())
}

async fn run_fake_acp(daemon: Arc<Daemon>, path: PathBuf, session_id: Uuid) {
    if let Err(error) = run_fake_acp_inner(Arc::clone(&daemon), path, session_id).await {
        eprintln!("fake ACP session {session_id} ended: {error}");
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
    let details = json!({
        "approval_id": approval_id,
        "approval_kind": kind,
        "tool_call_id": params["toolCall"]["toolCallId"],
        "options": options.iter().zip(provider_options.iter()).map(|(raw, option)| json!({"option_id":option.id,"name":raw["name"],"decision":option.decision,"class":option.class})).collect::<Vec<_>>(),
        "agent_description": {"title":params["toolCall"]["title"],"description":params["description"],"reason":params["reason"]},
    });
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
        }
        Err(error) => {
            if let Some(provider) = entry.provider.take() {
                let _ = provider.send(ProviderCommand::cancel());
            }
            eprintln!("approval expiry failed closed: {error}");
        }
    }
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

struct ProviderCommand {
    option_id: Option<String>,
    written: std::sync::mpsc::SyncSender<io::Result<()>>,
    delay: Duration,
}

impl ProviderCommand {
    fn cancel() -> Self {
        let (written, _result) = std::sync::mpsc::sync_channel(1);
        Self {
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
}

#[derive(Debug, serde::Deserialize)]
struct EnrollmentRequest {
    kind: String,
    code: String,
    public_key: String,
}

struct Daemon {
    data: DaemonDataDir,
    journal: Mutex<Journal>,
    event_generation: watch::Sender<u64>,
    command_lock: Mutex<()>,
    host_key: SigningKey,
    fake_acp_path: Option<PathBuf>,
    fake_acp_no_reject: bool,
    fake_acp_disconnect_after_permission: bool,
    approval_timeout_secs: u64,
    test_write_ahead_delay_ms: u64,
    approvals: Mutex<HashMap<Uuid, DaemonApproval>>,
    sessions: Mutex<HashMap<Uuid, Arc<Mutex<Coordinator>>>>,
}

struct DaemonApproval {
    session_id: Uuid,
    approval: Approval,
    options: Vec<ProviderOption>,
    provider: Option<mpsc::UnboundedSender<ProviderCommand>>,
    timeout_task: Option<tokio::task::AbortHandle>,
}

#[derive(Clone)]
struct ProviderOption {
    id: String,
    class: OptionClass,
    decision: ApprovalDecision,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let path = args.data_dir.unwrap_or_else(default_data_dir);
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
            )
            .await?
        }
    }
    Ok(())
}

async fn run(
    path: PathBuf,
    fake_acp_path: Option<PathBuf>,
    fake_acp_no_reject: bool,
    fake_acp_disconnect_after_permission: bool,
    approval_timeout_secs: u64,
    test_write_ahead_delay_ms: u64,
    browser: BrowserListenerOptions,
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
    Coordinator::recover_journal(&mut journal)?;
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
    });
    eprintln!(
        "shardraild listening at {} (host {host_id}, epoch {epoch})",
        socket.display()
    );
    if let Some(listener) = browser_listener {
        let address = listener.local_addr()?;
        eprintln!("browser WebSocket listening at ws://{address}/ws");
        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let daemon = Arc::clone(&daemon);
                        let browser_assets_dir = browser.assets_dir.clone();
                        tokio::spawn(async move {
                            if let Err(error) = serve_browser_connection(
                                stream,
                                daemon,
                                address.port(),
                                browser_assets_dir,
                            )
                            .await
                            {
                                eprintln!("browser client connection ended: {error}");
                            }
                        });
                    }
                    Err(error) => eprintln!("browser listener accept failed: {error}"),
                }
            }
        });
    }
    loop {
        let (stream, _) = listener.accept().await?;
        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            if let Err(error) = serve_client(stream, daemon).await {
                eprintln!("client connection ended: {error}");
            }
        });
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
    let mut first = [0u8; 256];
    let count = timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first))
        .await
        .map_err(|_| handshake_timeout())??;
    let request_line = first[..count]
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    if request_line.starts_with(b"GET /ws HTTP/") {
        return serve_browser_client(stream, daemon, port).await;
    }
    serve_browser_asset(&mut stream, port, assets_dir.as_deref()).await
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
            entries.clients.push(AuthorizedClient {
                client_id: id,
                public_key: public_key.clone(),
                label,
                scope: match scope {
                    ScopeArg::Full => ClientScope::Full,
                    ScopeArg::Limited => ClientScope::Limited,
                },
            });
            save_authorized_clients(path, &entries).map_err(|error| {
                io::Error::new(error.kind(), format!("save client registry: {error}"))
            })?;
            let client_scope = match scope {
                ScopeArg::Full => ClientScope::Full,
                ScopeArg::Limited => ClientScope::Limited,
            };
            println!(
                "client_id={id} public_key={public_key} scope={} host_public_key={}",
                scope_name(client_scope),
                encode_hex(&host_key.verifying_key().to_bytes())
            );
        }
        ClientCommand::Enroll { label } => {
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
                },
            )?;
            println!("enrollment_code={text} expires_in_seconds=300");
        }
        ClientCommand::List => {
            for client in entries.clients {
                println!(
                    "{}\t{}\t{}\t{}",
                    client.client_id,
                    client.label,
                    scope_name(client.scope),
                    client.public_key
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
        let frame = parse_client_frame(&line)?;
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
            if client.scope == ClientScope::Limited {
                write_frame(
                    &outbound,
                    &DaemonFrame::CommandRejected {
                        command_id: request.command_id,
                        error: "limited clients cannot create sessions".into(),
                    },
                )
                .await?;
                continue;
            }
            if request.kind != "session.create"
                || request.expected_state.is_some()
                || request.expected_turn_id.is_some()
            {
                write_frame(
                    &outbound,
                    &DaemonFrame::CommandRejected {
                        command_id: request.command_id,
                        error: "unsupported command or invalid preconditions".into(),
                    },
                )
                .await?;
                continue;
            }
            let _command = daemon.command_lock.lock().await;
            let existing = daemon
                .journal
                .lock()
                .await
                .created_session_for_command(client.client_id, request.command_id)
                .map_err(io::Error::other)?;
            if let Some((_session_id, event_id)) = existing {
                let receipt = daemon
                    .journal
                    .lock()
                    .await
                    .event_receipt(event_id)
                    .map_err(io::Error::other)?;
                write_frame(
                    &outbound,
                    &DaemonFrame::CommandAccepted {
                        command_id: request.command_id,
                        receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                        receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                        cursor: event_id,
                    },
                )
                .await?;
                push_events(&outbound, &daemon, &mut cursor).await?;
                continue;
            }
            let session_id = Uuid::now_v7();
            let now_ms = wall_clock_ms();
            let mut coordinator = Coordinator::open(
                Journal::open(daemon.data.journal_path()).map_err(io::Error::other)?,
                session_id,
            )
            .map_err(io::Error::other)?;
            coordinator
                .transition_session(
                    now_ms,
                    "session_created",
                    json!({"session_id": session_id, "client_id": client.client_id, "command_id": request.command_id}),
                    Session::default(),
                    || Ok::<_, io::Error>(()),
                )
                .map_err(io::Error::other)?;
            daemon
                .event_generation
                .send_modify(|generation| *generation += 1);
            daemon
                .sessions
                .lock()
                .await
                .insert(session_id, Arc::new(Mutex::new(coordinator)));
            let journal = daemon.journal.lock().await;
            let event = journal
                .events_for_session(session_id)
                .map_err(io::Error::other)?
                .into_iter()
                .last()
                .ok_or_else(|| io::Error::other("session creation event was not committed"))?;
            let event_id = event.event_id;
            let receipt = journal.event_receipt(event_id).map_err(io::Error::other)?;
            write_frame(
                &outbound,
                &DaemonFrame::CommandAccepted {
                    command_id: request.command_id,
                    receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                    receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                    cursor: event_id,
                },
            )
            .await?;
            drop(journal);
            push_events(&outbound, &daemon, &mut cursor).await?;
            if let Some(path) = daemon.fake_acp_path.clone() {
                tokio::spawn(run_fake_acp(Arc::clone(&daemon), path, session_id));
            }
        }
        if let Some(resume_after) = resume_after {
            cursor = resume_after.max(0);
            push_events(&outbound, &daemon, &mut cursor).await?;
        }
    }
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
                    option_id: text,
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
