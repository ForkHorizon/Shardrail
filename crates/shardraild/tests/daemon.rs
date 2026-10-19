use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use uuid::Uuid;

fn start_daemon(path: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn connect(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let socket = path.join("shardraild.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            return (stream, reader);
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not create its socket"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(
        !line.is_empty(),
        "daemon closed the socket before sending a frame"
    );
    serde_json::from_str(&line).unwrap()
}

fn handshake(
    path: &Path,
    epoch: Option<Uuid>,
    cursor: i64,
) -> (UnixStream, BufReader<UnixStream>, Value) {
    let (mut stream, mut reader) = connect(path);
    writeln!(
        stream,
        "{}",
        json!({
            "min_version": 1, "max_version": 1, "client_id": Uuid::now_v7(),
            "public_key": null, "proof": null, "resume_after": cursor,
            "journal_epoch": epoch,
        })
    )
    .unwrap();
    let hello = read_json(&mut reader);
    (stream, reader, hello)
}

#[test]
fn real_daemon_creates_streams_recovers_and_rejects_second_instance() {
    let dir = tempdir().unwrap();
    let mut daemon = start_daemon(dir.path());
    let (mut client, mut reader, hello) = handshake(dir.path(), None, 0);
    let epoch = Uuid::parse_str(hello["journal_epoch"].as_str().unwrap()).unwrap();
    assert_eq!(hello["protocol_version"], 1);
    assert_eq!(hello["scope"], "limited");

    let mut second = start_daemon(dir.path());
    assert!(second.wait().unwrap().code().is_some_and(|code| code != 0));

    writeln!(
        client,
        "{}",
        json!({
            "request": {
                "command_id": Uuid::now_v7(), "kind": "session.create",
                "expected_state": null, "expected_turn_id": null, "payload": {}
            },
            "resume_after": null
        })
    )
    .unwrap();
    let accepted = read_json(&mut reader);
    assert_eq!(accepted["kind"], "command_accepted");
    assert!(accepted["receipt_monotonic_ns"].as_str().is_some());
    assert!(accepted["receipt_wall_clock_ms"].as_i64().is_some());
    let event = read_json(&mut reader);
    assert_eq!(event["kind"], "durable_event");
    assert_eq!(event["event"]["kind"], "session_created");

    daemon.kill().unwrap(); // Child::kill is SIGKILL on Unix.
    daemon.wait().unwrap();
    let mut daemon = start_daemon(dir.path());
    let (mut resumed, mut resumed_reader, resumed_hello) = handshake(dir.path(), Some(epoch), 0);
    assert_eq!(resumed_hello["resync_required"], false);
    writeln!(resumed, "{}", json!({"request": null, "resume_after": 0})).unwrap();
    let replayed = read_json(&mut resumed_reader);
    assert_eq!(replayed["event"]["kind"], "session_created");
    assert_eq!(replayed["kind"], "durable_event");
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn journal_symlink_is_refused_without_touching_target() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let external = dir.path().join("outside.db");
    std::fs::write(&external, b"leave me alone").unwrap();
    symlink(&external, dir.path().join("journal.sqlite3")).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(std::fs::read(external).unwrap(), b"leave me alone");
}
