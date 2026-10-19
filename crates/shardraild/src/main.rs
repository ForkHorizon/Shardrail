use clap::Parser;
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, fstat};
use nix::sys::statfs::statfs;
use serde_json::json;
use shardrail_coordinator::{Coordinator, DaemonDataDir};
use shardrail_journal::Journal;
use shardrail_protocol::{ClientFrame, ClientHello, ClientScope, DaemonFrame, ServerHello};
use shardrail_session_core::Session;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use uuid::Uuid;

const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(name = "shardraild", about = "Shardrail local coordinator daemon")]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

struct Daemon {
    data: DaemonDataDir,
    journal: Mutex<Journal>,
    coordinators: Mutex<HashMap<Uuid, Coordinator>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    run(args.data_dir.unwrap_or_else(default_data_dir)).await?;
    Ok(())
}

async fn run(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // Startup order is kept in this function so the listener cannot precede recovery.
    refuse_synced_path(&path)?;
    refuse_network_filesystem(&path)?;
    fs::create_dir_all(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    let data = DaemonDataDir::open(&path)?;
    let _journal_fd = open_journal_entry_nofollow(&path)?;
    let mut journal = data.open_journal()?;
    let epoch = journal.journal_epoch()?;
    let host_id = journal.host_id()?;
    Coordinator::recover_journal(&mut journal)?;
    let session_ids = journal
        .events_after(0)?
        .into_iter()
        .map(|event| event.session_id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut coordinators = HashMap::new();
    for session_id in session_ids {
        let session_journal = Journal::open(data.journal_path())?;
        coordinators.insert(session_id, Coordinator::open(session_journal, session_id)?);
    }
    let socket = path.join("shardraild.sock");
    let listener = bind_private_socket(&socket)?;
    let daemon = Arc::new(Daemon {
        data,
        journal: Mutex::new(journal),
        coordinators: Mutex::new(coordinators),
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

fn refuse_synced_path(path: &Path) -> io::Result<()> {
    let canonical = if path.exists() {
        fs::canonicalize(path)?
    } else {
        let ancestor = path
            .ancestors()
            .find(|candidate| candidate.exists())
            .unwrap_or(Path::new("/"));
        let resolved = fs::canonicalize(ancestor)?;
        resolved.join(path.strip_prefix(ancestor).unwrap_or(path))
    };
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
    let probe = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .unwrap_or(Path::new("/"));
    let fs_type = statfs(probe)
        .map_err(io::Error::other)?
        .filesystem_type_name()
        .to_ascii_lowercase();
    if network_fs_name(&fs_type) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "Shardrail data directory is on network filesystem '{fs_type}'; choose a local filesystem"
            ),
        ));
    }
    Ok(())
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

fn open_journal_entry_nofollow(path: &Path) -> io::Result<OwnedFd> {
    let dir = fs::File::open(path)?;
    let entry = openat(
        dir.as_fd(),
        "journal.sqlite3",
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(io::Error::other)?;
    let metadata = fstat(&entry).map_err(io::Error::other)?;
    if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
        .contains(nix::sys::stat::SFlag::S_IFREG)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal must be a regular file",
        ));
    }
    // Keep the descriptor pinned while rusqlite independently opens this path with
    // SQLITE_OPEN_NOFOLLOW; rusqlite exposes no safe API to adopt an existing fd.
    Ok(entry)
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
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if read.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let hello: ClientHello = serde_json::from_str(&line).map_err(io::Error::other)?;
    let journal = daemon.journal.lock().await;
    let epoch = journal.journal_epoch().map_err(io::Error::other)?;
    let host_id = journal.host_id().map_err(io::Error::other)?;
    if hello.min_version > PROTOCOL_VERSION || hello.max_version < PROTOCOL_VERSION {
        return write_frame(
            &mut write,
            &DaemonFrame {
                kind: "error".into(),
                command_id: None,
                event: None,
                receipt_monotonic_ns: None,
                receipt_wall_clock_ms: None,
                cursor: None,
                error: Some("no mutually supported protocol version".into()),
            },
        )
        .await;
    }
    let mismatch = hello
        .journal_epoch
        .is_some_and(|client_epoch| client_epoch != epoch);
    let mut cursor = if mismatch {
        0
    } else {
        hello.resume_after.max(0)
    };
    let current = journal.events_after(0).map_err(io::Error::other)?;
    let high_water = current.last().map_or(0, |event| event.event_id);
    let server = ServerHello {
        protocol_version: PROTOCOL_VERSION,
        host_id,
        journal_epoch: epoch,
        scope: ClientScope::Limited,
        cursor: high_water,
        resync_required: mismatch,
    };
    write_json_line(&mut write, &server).await?;
    if mismatch {
        return Ok(());
    }
    drop(journal);
    loop {
        line.clear();
        if read.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let frame: ClientFrame = serde_json::from_str(&line).map_err(io::Error::other)?;
        if let Some(request) = frame.request {
            if request.kind != "session.create" {
                write_frame(
                    &mut write,
                    &DaemonFrame {
                        kind: "command_rejected".into(),
                        command_id: Some(request.command_id),
                        event: None,
                        receipt_monotonic_ns: None,
                        receipt_wall_clock_ms: None,
                        cursor: None,
                        error: Some("unsupported command".into()),
                    },
                )
                .await?;
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
                    json!({"session_id": session_id, "command_id": request.command_id}),
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
            daemon
                .coordinators
                .lock()
                .await
                .insert(session_id, coordinator);
            write_frame(
                &mut write,
                &DaemonFrame {
                    kind: "command_accepted".into(),
                    command_id: Some(request.command_id),
                    event: None,
                    receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                    receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                    cursor: Some(event_id),
                    error: None,
                },
            )
            .await?;
            cursor = event_id.saturating_sub(1);
            drop(journal);
            push_events(&mut write, &daemon, &mut cursor).await?;
        }
        if frame.resume_after.is_some() {
            cursor = frame.resume_after.unwrap_or(cursor).max(0);
            push_events(&mut write, &daemon, &mut cursor).await?;
        }
    }
}

async fn push_events(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    daemon: &Daemon,
    cursor: &mut i64,
) -> io::Result<()> {
    let journal = daemon.journal.lock().await;
    let events = journal.events_after(*cursor).map_err(io::Error::other)?;
    for event in events {
        let receipt = journal
            .event_receipt(event.event_id)
            .map_err(io::Error::other)?;
        write_frame(
            write,
            &DaemonFrame {
                kind: "durable_event".into(),
                command_id: None,
                cursor: Some(event.event_id),
                event: Some(event.clone()),
                receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                error: None,
            },
        )
        .await?;
        *cursor = event.event_id;
    }
    Ok(())
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    write: &mut W,
    frame: &DaemonFrame,
) -> io::Result<()> {
    write_json_line(write, frame).await
}

async fn write_json_line<W: AsyncWriteExt + Unpin, T: serde::Serialize>(
    write: &mut W,
    value: &T,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write.write_all(&bytes).await
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

    #[test]
    fn rejects_synced_storage_path_names() {
        assert!(refuse_synced_path(Path::new("/Users/test/Library/Mobile Documents/db")).is_err());
        assert!(refuse_synced_path(Path::new("/var/lib/shardrail")).is_ok());
    }
}
