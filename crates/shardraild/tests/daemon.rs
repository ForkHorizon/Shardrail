use ed25519_dalek::{Signature, Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use uuid::Uuid;

struct TestClient {
    id: Uuid,
    public_key: String,
    key: SigningKey,
}

fn add_client(path: &Path) -> TestClient {
    add_client_with_scope(path, "full")
}

fn add_client_with_scope(path: &Path, scope: &str) -> TestClient {
    let result = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(path)
        .args(["client", "add", "--label", "test", "--scope", scope])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let registered: Value =
        serde_json::from_slice(&std::fs::read(path.join("authorized-clients.json")).unwrap())
            .unwrap();
    let client = &registered["clients"][0];
    let id = Uuid::parse_str(client["client_id"].as_str().unwrap()).unwrap();
    let public_key = client["public_key"].as_str().unwrap().to_string();
    let secret: [u8; 32] = std::fs::read(path.join(format!("client-{id}.key")))
        .unwrap()
        .try_into()
        .unwrap();
    TestClient {
        id,
        public_key,
        key: SigningKey::from_bytes(&secret),
    }
}

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
    client: &TestClient,
) -> (UnixStream, BufReader<UnixStream>, Value) {
    let (mut stream, mut reader) = connect(path);
    let nonce = "11".repeat(32);
    writeln!(
        stream,
        "{}",
        json!({
            "min_version": 1, "max_version": 1, "client_id": client.id,
            "public_key": client.public_key, "nonce": nonce, "resume_after": cursor,
            "journal_epoch": epoch,
        })
    )
    .unwrap();
    let hello = read_json(&mut reader);
    let host_secret: [u8; 32] = std::fs::read(path.join("host.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let host_key = SigningKey::from_bytes(&host_secret).verifying_key();
    assert_eq!(hello["public_key"], encode_hex(&host_key.to_bytes()));
    let signature =
        Signature::from_slice(&decode_hex(hello["signature"].as_str().unwrap())).unwrap();
    let transcript = transcript_hash(
        [1, 1, 1],
        &nonce,
        hello["nonce"].as_str().unwrap(),
        &client.public_key,
        hello["public_key"].as_str().unwrap(),
    );
    host_key.verify_strict(&transcript, &signature).unwrap();
    let client_signature = client.key.sign(&transcript);
    writeln!(
        stream,
        "{}",
        json!({"kind":"authenticate", "signature":encode_hex(&client_signature.to_bytes())})
    )
    .unwrap();
    assert_eq!(read_json(&mut reader)["kind"], "authenticated");
    (stream, reader, hello)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn transcript_hash(
    versions: [u32; 3],
    client_nonce: &str,
    server_nonce: &str,
    client_key: &str,
    host_key: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"Shardrail mutual client handshake v1\0");
    for version in versions {
        hasher.update(version.to_be_bytes());
    }
    for field in [
        decode_hex(client_nonce),
        decode_hex(server_nonce),
        decode_hex(client_key),
        decode_hex(host_key),
    ] {
        hasher.update((field.len() as u32).to_be_bytes());
        hasher.update(field);
    }
    hasher.finalize().into()
}

#[test]
fn handshake_rejects_pre_auth_commands_and_cross_session_signature_replay() {
    let dir = tempdir().unwrap();
    let client = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let nonce = "22".repeat(32);
    let hello = json!({"min_version":1,"max_version":1,"client_id":client.id,
        "public_key":client.public_key,"nonce":nonce,"resume_after":0,"journal_epoch":null});

    let (mut first, mut first_reader) = connect(dir.path());
    writeln!(first, "{hello}").unwrap();
    let first_server = read_json(&mut first_reader);
    let first_transcript = transcript_hash(
        [1, 1, 1],
        &nonce,
        first_server["nonce"].as_str().unwrap(),
        &client.public_key,
        first_server["public_key"].as_str().unwrap(),
    );
    let replay_signature = client.key.sign(&first_transcript);
    drop(first);

    let (mut second, mut second_reader) = connect(dir.path());
    writeln!(second, "{hello}").unwrap();
    let second_server = read_json(&mut second_reader);
    assert_ne!(first_server["nonce"], second_server["nonce"]);
    writeln!(
        second,
        "{}",
        json!({"kind":"authenticate","signature":encode_hex(&replay_signature.to_bytes())})
    )
    .unwrap();
    let mut line = String::new();
    assert_eq!(read_json(&mut second_reader)["kind"], "error");
    assert_eq!(second_reader.read_line(&mut line).unwrap(), 0);

    let (mut reordered, mut reordered_reader) = connect(dir.path());
    writeln!(reordered, "{hello}").unwrap();
    let _ = read_json(&mut reordered_reader);
    writeln!(reordered, "{}", json!({"kind":"request","request":{"command_id":Uuid::now_v7(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{}}})).unwrap();
    assert_eq!(read_json(&mut reordered_reader)["kind"], "error");

    let (mut healthy, mut healthy_reader, _) = handshake(dir.path(), None, 0, &client);
    writeln!(healthy, "{}", json!({"kind":"request","request":{"command_id":Uuid::now_v7(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{}}})).unwrap();
    assert_eq!(read_json(&mut healthy_reader)["kind"], "command_accepted");
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn unsupported_versions_and_unauthorized_clients_receive_error_frames() {
    let dir = tempdir().unwrap();
    let registered = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());

    let (mut unsupported, mut unsupported_reader) = connect(dir.path());
    writeln!(unsupported, "{}", json!({"min_version":2,"max_version":3,"client_id":registered.id,"public_key":registered.public_key,"nonce":"11".repeat(32),"resume_after":0,"journal_epoch":null})).unwrap();
    assert_eq!(read_json(&mut unsupported_reader)["kind"], "error");

    let (mut unauthorized, mut unauthorized_reader) = connect(dir.path());
    writeln!(unauthorized, "{}", json!({"min_version":1,"max_version":1,"client_id":Uuid::now_v7(),"public_key":"00".repeat(32),"nonce":"22".repeat(32),"resume_after":0,"journal_epoch":null})).unwrap();
    assert_eq!(read_json(&mut unauthorized_reader)["kind"], "error");

    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn concurrent_client_additions_preserve_every_registration() {
    let dir = tempdir().unwrap();
    let children: Vec<_> = (0..5)
        .map(|index| {
            let child = Command::new(env!("CARGO_BIN_EXE_shardraild"))
                .arg("--data-dir")
                .arg(dir.path())
                .args([
                    "client",
                    "add",
                    "--label",
                    &format!("parallel-{index}"),
                    "--scope",
                    "full",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            (index, child)
        })
        .collect();
    for (index, child) in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "client {index}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let registry: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("authorized-clients.json")).unwrap())
            .unwrap();
    assert_eq!(registry["clients"].as_array().unwrap().len(), 5);
}

#[test]
fn limited_client_cannot_create_sessions() {
    let dir = tempdir().unwrap();
    let client = add_client_with_scope(dir.path(), "limited");
    let mut daemon = start_daemon(dir.path());
    let (mut stream, mut reader, _) = handshake(dir.path(), None, 0, &client);
    writeln!(stream, "{}", json!({"kind":"request","request":{"command_id":Uuid::now_v7(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{}}})).unwrap();
    let response = read_json(&mut reader);
    assert_eq!(response["kind"], "command_rejected");
    assert_eq!(response["error"], "limited clients cannot create sessions");
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn client_can_resume_from_an_earlier_cursor_on_the_same_connection() {
    let dir = tempdir().unwrap();
    let client = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut stream, mut reader, _) = handshake(dir.path(), None, 0, &client);
    writeln!(stream, "{}", json!({"kind":"request","request":{"command_id":Uuid::now_v7(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{}}})).unwrap();
    let accepted = read_json(&mut reader);
    assert_eq!(accepted["kind"], "command_accepted");
    assert!(accepted["cursor"].as_i64().unwrap() > 0);
    let event = read_json(&mut reader);
    assert_eq!(event["kind"], "durable_event");
    let cursor = event["cursor"].as_i64().unwrap();

    writeln!(stream, "{}", json!({"kind":"resume","resume_after":0})).unwrap();
    let replayed = read_json(&mut reader);
    assert_eq!(replayed["kind"], "durable_event");
    assert_eq!(replayed["cursor"], cursor);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn stalled_handshake_is_closed_after_timeout() {
    let dir = tempdir().unwrap();
    let client = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut stream, mut reader) = connect(dir.path());
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    writeln!(stream, "{}", json!({"min_version":1,"max_version":1,"client_id":client.id,"public_key":client.public_key,"nonce":"11".repeat(32),"resume_after":0,"journal_epoch":null})).unwrap();
    assert_eq!(read_json(&mut reader)["protocol_version"], 1);
    let started = Instant::now();
    let mut line = String::new();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0);
    assert!(started.elapsed() >= Duration::from_secs(4));
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn daemon_file_access_stays_in_opened_directory_after_path_swap() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let moved = dir.path().join("data-opened");
    let external = dir.path().join("external");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&external).unwrap();
    let client = add_client(&data);
    let mut daemon = start_daemon(&data);
    let (mut stream, mut reader, _) = handshake(&data, None, 0, &client);

    std::fs::rename(&data, &moved).unwrap();
    symlink(&external, &data).unwrap();
    writeln!(stream, "{}", json!({"kind":"request","request":{"command_id":Uuid::now_v7(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{}}})).unwrap();
    let accepted = read_json(&mut reader);
    assert_eq!(accepted["kind"], "command_accepted");
    assert!(accepted["cursor"].as_i64().unwrap() > 0);
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    assert!(moved.join("journal.sqlite3").exists());
    assert!(!external.join("journal.sqlite3").exists());
}

#[test]
fn client_key_commands_register_list_and_remove_private_keys() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let client = add_client(dir.path());
    for name in [format!("client-{}.key", client.id), "host.key".into()] {
        assert_eq!(
            std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let listed = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["client", "list"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains(&client.id.to_string()));

    let client_id = client.id.to_string();
    let removed = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["client", "remove", &client_id])
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert!(
        !dir.path()
            .join(format!("client-{}.key", client.id))
            .exists()
    );
    let registrations: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("authorized-clients.json")).unwrap())
            .unwrap();
    assert_eq!(registrations["clients"].as_array().unwrap().len(), 0);
}

#[test]
fn real_daemon_creates_streams_recovers_and_rejects_second_instance() {
    let dir = tempdir().unwrap();
    let client_key = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut client, mut reader, hello) = handshake(dir.path(), None, 0, &client_key);
    let epoch = Uuid::parse_str(hello["journal_epoch"].as_str().unwrap()).unwrap();
    assert_eq!(hello["protocol_version"], 1);
    assert_eq!(hello["scope"], "full");

    let mut second = start_daemon(dir.path());
    assert!(second.wait().unwrap().code().is_some_and(|code| code != 0));

    writeln!(
        client,
        "{}",
        json!({
            "kind": "request",
            "request": {
                "command_id": Uuid::now_v7(), "kind": "session.create",
                "expected_state": null, "expected_turn_id": null, "payload": {}
            }
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
    let (mut resumed, mut resumed_reader, resumed_hello) =
        handshake(dir.path(), Some(epoch), 0, &client_key);
    assert_eq!(resumed_hello["resync_required"], false);
    writeln!(resumed, "{}", json!({"kind": "resume", "resume_after": 0})).unwrap();
    let replayed = read_json(&mut resumed_reader);
    assert_eq!(replayed["event"]["kind"], "session_created");
    assert_eq!(replayed["kind"], "durable_event");
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn replay_fanout_and_command_retries_preserve_the_cursor() {
    let dir = tempdir().unwrap();
    let client_key = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut writer, mut writer_reader, _) = handshake(dir.path(), None, 0, &client_key);
    let command_id = Uuid::now_v7();
    let request = json!({
        "kind": "request",
        "request": {"command_id": command_id, "kind": "session.create", "expected_state": null, "expected_turn_id": null, "payload": {}}
    });
    writeln!(writer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(read_json(&mut writer_reader)["kind"], "command_accepted");
    assert_eq!(read_json(&mut writer_reader)["event"]["event_id"], 1);

    let (subscriber, mut subscriber_reader, _) = handshake(dir.path(), None, 0, &client_key);
    assert_eq!(read_json(&mut subscriber_reader)["event"]["event_id"], 1);

    writeln!(writer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    let retry = read_json(&mut writer_reader);
    assert_eq!(retry["kind"], "command_accepted");
    assert_eq!(retry["cursor"], 1);

    let second_request = json!({
        "kind": "request",
        "request": {"command_id": Uuid::now_v7(), "kind": "session.create", "expected_state": null, "expected_turn_id": null, "payload": {}}
    });
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&second_request).unwrap()
    )
    .unwrap();
    assert_eq!(read_json(&mut writer_reader)["kind"], "command_accepted");
    assert_eq!(read_json(&mut writer_reader)["event"]["event_id"], 2);
    assert_eq!(read_json(&mut subscriber_reader)["event"]["event_id"], 2);
    drop(subscriber);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn slow_client_is_disconnected_without_blocking_other_clients_or_journaling() {
    let dir = tempdir().unwrap();
    let client_key = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut slow, mut slow_reader, _) = handshake(dir.path(), None, 0, &client_key);
    slow.set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    for _ in 0..500 {
        let frame = json!({"kind":"request","request":{
            "command_id":Uuid::now_v7(), "kind":"session.create",
            "expected_state":null,"expected_turn_id":null,"payload":{}
        }});
        if writeln!(slow, "{}", serde_json::to_string(&frame).unwrap()).is_err() {
            break;
        }
    }
    let mut closed = false;
    for _ in 0..100 {
        let mut line = String::new();
        if slow_reader.read_line(&mut line).unwrap() == 0 {
            closed = true;
            break;
        }
    }
    assert!(
        closed,
        "slow client connection stayed open after its output queue filled"
    );

    let (mut healthy, mut healthy_reader, hello) = handshake(dir.path(), None, 0, &client_key);
    let mut cursor = 0;
    while cursor < hello["cursor"].as_i64().unwrap() {
        cursor = read_json(&mut healthy_reader)["cursor"].as_i64().unwrap();
    }
    writeln!(
        healthy,
        "{}",
        json!({"kind":"request","request":{
            "command_id":Uuid::now_v7(),"kind":"session.create",
            "expected_state":null,"expected_turn_id":null,"payload":{}
        }})
    )
    .unwrap();
    assert_eq!(read_json(&mut healthy_reader)["kind"], "command_accepted");
    assert_eq!(
        read_json(&mut healthy_reader)["event"]["kind"],
        "session_created"
    );
    drop(slow);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn epoch_mismatch_signals_resync_and_streams_history_on_same_connection() {
    let dir = tempdir().unwrap();
    let client_key = add_client(dir.path());
    let mut daemon = start_daemon(dir.path());
    let (mut client, mut reader, _) = handshake(dir.path(), None, 0, &client_key);
    writeln!(
        client,
        "{}",
        json!({
            "kind": "request",
            "request": {
                "command_id": Uuid::now_v7(), "kind": "session.create",
                "expected_state": null, "expected_turn_id": null, "payload": {}
            }
        })
    )
    .unwrap();
    assert_eq!(read_json(&mut reader)["kind"], "command_accepted");
    let _ = read_json(&mut reader);
    drop(client);

    let (_resumed, mut resumed_reader, hello) =
        handshake(dir.path(), Some(Uuid::now_v7()), 99, &client_key);
    assert_eq!(hello["resync_required"], true);
    assert_eq!(read_json(&mut resumed_reader)["kind"], "durable_event");
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

#[cfg(unix)]
#[test]
fn symlinked_data_directory_is_rejected_without_chmodding_its_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempdir().unwrap();
    let external = dir.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = dir.path().join("data-link");
    symlink(&external, &link).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(&link)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(
        std::fs::metadata(external).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
