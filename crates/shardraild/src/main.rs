use clap::{Parser, Subcommand, ValueEnum};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use nix::sys::stat::{Mode, fchmod};
use nix::sys::statfs::statfs;
use serde_json::json;
use sha2::{Digest, Sha256};
use shardrail_coordinator::{Coordinator, DaemonDataDir, open_private_data_directory};
use shardrail_journal::Journal;
use shardrail_protocol::{ClientFrame, ClientHello, ClientScope, DaemonFrame, ServerHello};
use shardrail_session_core::Session;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, MissedTickBehavior, interval};
use uuid::Uuid;

const PROTOCOL_VERSION: u32 = 1;
const CLIENT_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Parser)]
#[command(name = "shardraild", about = "Shardrail local coordinator daemon")]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<DaemonCommand>,
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
    command_lock: Mutex<()>,
    host_key: SigningKey,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let path = args.data_dir.unwrap_or_else(default_data_dir);
    if let Some(DaemonCommand::Client { command }) = args.command {
        manage_clients(&path, command)?;
    } else {
        run(path).await?;
    }
    Ok(())
}

async fn run(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // Startup order is kept in this function so the listener cannot precede recovery.
    refuse_synced_path(&path)?;
    refuse_network_filesystem(&path)?;
    let data = DaemonDataDir::open(&path)?;
    let path = data.directory_path().to_path_buf();
    let _journal_fd = data.open_journal_entry()?;
    let mut journal = data.open_journal()?;
    let epoch = journal.journal_epoch()?;
    let host_id = journal.host_id()?;
    let host_key = load_or_create_signing_key(&path.join("host.key"))?;
    Coordinator::recover_journal(&mut journal)?;
    let socket = path.join("shardraild.sock");
    let listener = bind_private_socket(&socket)?;
    let daemon = Arc::new(Daemon {
        data,
        journal: Mutex::new(journal),
        command_lock: Mutex::new(()),
        host_key,
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
    let (_directory, path) = open_private_data_directory(path)?;
    let path = path.as_path();
    let mut entries = load_authorized_clients(path)?;
    match command {
        ClientCommand::Add { label, scope } => {
            let id = Uuid::now_v7();
            let key = generate_signing_key()?;
            let public_key = encode_hex(&key.verifying_key().to_bytes());
            let host_key = load_or_create_signing_key(&path.join("host.key"))?;
            write_private_bytes(&path.join(format!("client-{id}.key")), &key.to_bytes())?;
            entries.clients.push(AuthorizedClient {
                client_id: id,
                public_key: public_key.clone(),
                label,
                scope: match scope {
                    ScopeArg::Full => ClientScope::Full,
                    ScopeArg::Limited => ClientScope::Limited,
                },
            });
            save_authorized_clients(path, &entries)?;
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
    let _abort_writer = AbortWriter(writer_task);
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if read.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let hello: ClientHello = serde_json::from_str(&line).map_err(io::Error::other)?;
    if hello.min_version > PROTOCOL_VERSION || hello.max_version < PROTOCOL_VERSION {
        return write_frame(
            &outbound,
            &DaemonFrame::Error {
                error: "no mutually supported protocol version".into(),
            },
        )
        .await;
    }
    let authorized_clients = load_authorized_clients(daemon.data.directory_path())?;
    let Some(client) = authorized_clients
        .clients
        .iter()
        .find(|client| client.public_key == hello.public_key)
    else {
        return write_frame(
            &outbound,
            &DaemonFrame::Error {
                error: "client key is not authorized".into(),
            },
        )
        .await;
    };
    if hello.client_id != client.client_id {
        return write_frame(
            &outbound,
            &DaemonFrame::Error {
                error: "client id does not match its authorized key".into(),
            },
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
    if read.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let auth: ClientFrame = serde_json::from_str(&line).map_err(io::Error::other)?;
    let ClientFrame::Authenticate { signature } = auth else {
        return write_frame(
            &outbound,
            &DaemonFrame::Error {
                error: "authentication required".into(),
            },
        )
        .await;
    };
    let signature = Signature::from_slice(&decode_hex(&signature)?).map_err(io::Error::other)?;
    client_key
        .verify_strict(&transcript, &signature)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "client transcript signature is invalid",
            )
        })?;
    write_frame(&outbound, &DaemonFrame::Authenticated).await?;
    push_events(&outbound, &daemon, &mut cursor).await?;
    let mut tick = interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        line.clear();
        let read_result = tokio::select! {
            result = read.read_line(&mut line) => Some(result?),
            _ = tick.tick() => None,
        };
        if let Some(0) = read_result {
            return Ok(());
        }
        if read_result.is_none() {
            push_events(&outbound, &daemon, &mut cursor).await?;
            continue;
        }
        let frame: ClientFrame = serde_json::from_str(&line).map_err(io::Error::other)?;
        let (request, resume_after) = match frame {
            ClientFrame::Authenticate { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authentication is only valid during handshake",
                ));
            }
            ClientFrame::Request { request } => (Some(request), None),
            ClientFrame::Resume { resume_after } => (None, Some(resume_after)),
        };
        if let Some(request) = request {
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
        }
        if let Some(resume_after) = resume_after {
            cursor = cursor.max(resume_after).max(0);
            push_events(&outbound, &daemon, &mut cursor).await?;
        }
    }
}

async fn push_events(
    outbound: &mpsc::Sender<Vec<u8>>,
    daemon: &Daemon,
    cursor: &mut i64,
) -> io::Result<()> {
    let events = {
        let journal = daemon.journal.lock().await;
        journal
            .events_after_with_receipts(*cursor, 16)
            .map_err(io::Error::other)?
    };
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
    }
    Ok(())
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
    outbound.try_send(bytes).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => io::Error::new(
            io::ErrorKind::WouldBlock,
            "client outbound queue overflow; reconnect and resume from your last received cursor",
        ),
        mpsc::error::TrySendError::Closed(_) => {
            io::Error::new(io::ErrorKind::BrokenPipe, "client writer closed")
        }
    })
}

struct AbortWriter(tokio::task::JoinHandle<()>);

impl Drop for AbortWriter {
    fn drop(&mut self) {
        self.0.abort();
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
}
