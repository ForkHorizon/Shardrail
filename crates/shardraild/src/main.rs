use clap::{Parser, Subcommand, ValueEnum};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
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
use std::os::fd::AsFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{ChildStdin, Command as TokioCommand};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::time::{Duration, timeout};
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
    #[command(subcommand)]
    command: Option<DaemonCommand>,
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
}

#[derive(Debug, Subcommand)]
enum ClientCommand {
    Add {
        #[arg(long)]
        label: String,
        #[arg(long, value_enum)]
        scope: ScopeArg,
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
    if let Some(DaemonCommand::Client { command }) = args.command {
        manage_clients(&path, command)?;
    } else {
        run(
            path,
            args.fake_acp_path,
            args.fake_acp_no_reject,
            args.fake_acp_disconnect_after_permission,
            args.approval_timeout_secs,
            args.test_write_ahead_delay_ms,
        )
        .await?;
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
        ClientCommand::Add { label, scope } => {
            let id = Uuid::now_v7();
            let key = generate_signing_key()?;
            let public_key = encode_hex(&key.verifying_key().to_bytes());
            let host_key = load_or_create_signing_key(&path.join("host.key"))
                .map_err(|error| io::Error::new(error.kind(), format!("load host key: {error}")))?;
            write_private_bytes(&path.join(format!("client-{id}.key")), &key.to_bytes()).map_err(
                |error| io::Error::new(error.kind(), format!("write client key: {error}")),
            )?;
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
                "client_id={id} public_key={public_key} scope={} key_file=client-{id}.key host_public_key={}",
                scope_name(client_scope),
                encode_hex(&host_key.verifying_key().to_bytes())
            );
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
    Ok(())
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
            let result = resolve_approval(&daemon, client, request).await;
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
    use proptest::prelude::*;
    use shardrail_protocol::ClientFrame;

    proptest! {
        #[test]
        fn newline_json_resume_frames_round_trip(cursor in any::<i64>()) {
            let frame = ClientFrame::Resume { resume_after: cursor };
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
        ) {
            let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let fields = [hex(&client_nonce), hex(&server_nonce), hex(&client_key), hex(&host_key)];
            let digest = transcript_hash([1, 1, 1], &fields[0], &fields[1], &fields[2], &fields[3]).unwrap();
            let changed_nonce = format!("{}{}", if fields[0].starts_with("00") { "01" } else { "00" }, &fields[0][2..]);
            let changed = transcript_hash([1, 1, 1], &changed_nonce, &fields[1], &fields[2], &fields[3]).unwrap();
            prop_assert_ne!(digest, changed);
            prop_assert!(transcript_hash([1, 1, 1], "00", &fields[1], &fields[2], &fields[3]).is_err());
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
