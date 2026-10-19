use clap::Parser;
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, fstat};
use nix::sys::statfs::statfs;
use serde_json::json;
use shardrail_coordinator::{Coordinator, DaemonDataDir};
use shardrail_journal::Journal;
use shardrail_protocol::{ClientFrame, ClientHello, ClientScope, DaemonFrame, ServerHello};
use shardrail_session_core::Session;
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
use tokio::time::{Duration, MissedTickBehavior, interval};
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
    command_lock: Mutex<()>,
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
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon data directory must be a real directory, not a symlink",
        )
        .into());
    }
    fs::create_dir_all(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    let data = DaemonDataDir::open(&path)?;
    let _journal_fd = open_journal_entry_nofollow(&path)?;
    let mut journal = data.open_journal()?;
    let epoch = journal.journal_epoch()?;
    let host_id = journal.host_id()?;
    Coordinator::recover_journal(&mut journal)?;
    let socket = path.join("shardraild.sock");
    let listener = bind_private_socket(&socket)?;
    let daemon = Arc::new(Daemon {
        data,
        journal: Mutex::new(journal),
        command_lock: Mutex::new(()),
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
    let (epoch, host_id, high_water) = {
        let journal = daemon.journal.lock().await;
        (
            journal.journal_epoch().map_err(io::Error::other)?,
            journal.host_id().map_err(io::Error::other)?,
            journal.high_water().map_err(io::Error::other)?,
        )
    };
    if hello.min_version > PROTOCOL_VERSION || hello.max_version < PROTOCOL_VERSION {
        return write_frame(
            &mut write,
            &DaemonFrame::Error {
                error: "no mutually supported protocol version".into(),
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
    let server = ServerHello {
        protocol_version: PROTOCOL_VERSION,
        host_id,
        journal_epoch: epoch,
        scope: ClientScope::Limited,
        cursor: high_water,
        resync_required: mismatch,
    };
    write_json_line(&mut write, &server).await?;
    push_events(&mut write, &daemon, &mut cursor).await?;
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
            push_events(&mut write, &daemon, &mut cursor).await?;
            continue;
        }
        let frame: ClientFrame = serde_json::from_str(&line).map_err(io::Error::other)?;
        let (request, resume_after) = match frame {
            ClientFrame::Request { request } => (Some(request), None),
            ClientFrame::Resume { resume_after } => (None, Some(resume_after)),
        };
        if let Some(request) = request {
            if request.kind != "session.create"
                || request.expected_state.is_some()
                || request.expected_turn_id.is_some()
            {
                write_frame(
                    &mut write,
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
                .created_session_for_command(hello.client_id, request.command_id)
                .map_err(io::Error::other)?;
            if let Some((_session_id, event_id)) = existing {
                let receipt = daemon
                    .journal
                    .lock()
                    .await
                    .event_receipt(event_id)
                    .map_err(io::Error::other)?;
                write_frame(
                    &mut write,
                    &DaemonFrame::CommandAccepted {
                        command_id: request.command_id,
                        receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                        receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                        cursor: event_id,
                    },
                )
                .await?;
                push_events(&mut write, &daemon, &mut cursor).await?;
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
                    json!({"session_id": session_id, "client_id": hello.client_id, "command_id": request.command_id}),
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
                &mut write,
                &DaemonFrame::CommandAccepted {
                    command_id: request.command_id,
                    receipt_monotonic_ns: receipt.map(|stamp| stamp.0.to_string()),
                    receipt_wall_clock_ms: receipt.map(|stamp| stamp.1),
                    cursor: event_id,
                },
            )
            .await?;
            drop(journal);
            push_events(&mut write, &daemon, &mut cursor).await?;
        }
        if let Some(resume_after) = resume_after {
            cursor = cursor.max(resume_after).max(0);
            push_events(&mut write, &daemon, &mut cursor).await?;
        }
    }
}

async fn push_events(
    write: &mut tokio::net::unix::OwnedWriteHalf,
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
            write,
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
