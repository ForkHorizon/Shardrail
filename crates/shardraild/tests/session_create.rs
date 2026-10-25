use ed25519_dalek::{Signature, Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shardrail_journal::Journal;
use shardrail_protocol::DurableEvent;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

struct TestClient {
    id: Uuid,
    public_key: String,
    key: SigningKey,
}

struct DaemonGuard(Child);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        let started = Instant::now();
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                    return;
                }
            }
        }
    }
}

fn fake_program() -> PathBuf {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_codex_app_server.py");
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755));
    path
}

fn add_client(path: &Path, scope: &str) -> TestClient {
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
    let client = registered["clients"].as_array().unwrap().last().unwrap();
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

fn connect(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let socket = path.join("shardraild.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
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
    assert!(!line.is_empty(), "daemon closed the socket");
    serde_json::from_str(&line).unwrap()
}

fn read_until(reader: &mut BufReader<UnixStream>, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for {kind}");
        let frame = read_json(reader);
        if frame["kind"] == kind {
            return frame;
        }
    }
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
    client_nonce: &str,
    server_nonce: &str,
    client_key: &str,
    host_key: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"Shardrail mutual client handshake v1\0");
    for version in [1u32, 1, 1] {
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

fn handshake(path: &Path, client: &TestClient) -> (UnixStream, BufReader<UnixStream>) {
    let (mut stream, mut reader) = connect(path);
    let nonce = "11".repeat(32);
    writeln!(
        stream,
        "{}",
        json!({
            "min_version": 1, "max_version": 1, "client_id": client.id,
            "public_key": client.public_key, "nonce": nonce, "resume_after": 0,
            "journal_epoch": null,
        })
    )
    .unwrap();
    let hello = read_json(&mut reader);
    let host_secret: [u8; 32] = std::fs::read(path.join("host.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let host_key = SigningKey::from_bytes(&host_secret).verifying_key();
    let signature =
        Signature::from_slice(&decode_hex(hello["signature"].as_str().unwrap())).unwrap();
    let transcript = transcript_hash(
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
        json!({"kind":"authenticate","signature":encode_hex(&client_signature.to_bytes())})
    )
    .unwrap();
    assert_eq!(read_json(&mut reader)["kind"], "authenticated");
    (stream, reader)
}

fn send_create(stream: &mut UnixStream, payload: Value) -> Uuid {
    let command_id = Uuid::now_v7();
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": command_id, "kind": "session.create", "expected_state": null,
            "expected_turn_id": null, "payload": payload
        }})
    )
    .unwrap();
    command_id
}

fn enroll(path: &Path, allow: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(path)
        .args(["client", "enroll", "--label", "browser"]);
    if allow {
        command.arg("--allow-session-create");
    }
    let issued = command.output().unwrap();
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    String::from_utf8(issued.stdout)
        .unwrap()
        .split_whitespace()
        .find_map(|part| part.strip_prefix("enrollment_code="))
        .unwrap()
        .to_owned()
}

fn redeem(path: &Path, code: &str, key: SigningKey) -> TestClient {
    let public_key = encode_hex(&key.verifying_key().to_bytes());
    let (mut stream, mut reader) = connect(path);
    writeln!(
        stream,
        "{}",
        json!({"kind":"enroll","code":code,"public_key":public_key})
    )
    .unwrap();
    let enrolled = read_json(&mut reader);
    assert_eq!(enrolled["kind"], "enrolled", "{enrolled}");
    TestClient {
        id: Uuid::parse_str(enrolled["client_id"].as_str().unwrap()).unwrap(),
        public_key,
        key,
    }
}

fn spawn_logged(command: &mut Command) -> (DaemonGuard, thread::JoinHandle<String>) {
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        let mut buffer = String::new();
        stderr.read_to_string(&mut buffer).unwrap();
        buffer
    });
    (DaemonGuard(child), reader)
}

fn wait_for_socket(path: &Path, child: &mut Child) {
    let socket = path.join("shardraild.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline && child.try_wait().unwrap().is_none(),
            "daemon exited before listening"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn audits(path: &Path) -> Vec<DurableEvent> {
    Journal::open(path.join("journal.sqlite3"))
        .unwrap()
        .events_after(0)
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "session_create_audit")
        .collect()
}

fn assert_audit(event: &DurableEvent, outcome: &str, reason: Option<&str>, capability: &str) {
    assert_eq!(event.session_id, Uuid::nil());
    assert_eq!(event.payload["outcome"], outcome);
    assert_eq!(event.payload["capability"], capability);
    match reason {
        Some(reason) => assert_eq!(event.payload["reason"], reason),
        None => assert!(event.payload["reason"].is_null()),
    }
}

#[test]
fn startup_allowlist_rejects_root_home_and_the_shardrail_repo() {
    let data = tempfile::tempdir().unwrap();
    for (path, needle) in [
        ("/", "refusing the filesystem root"),
        (&std::env::var("HOME").unwrap(), "refusing $HOME"),
        (
            "/Users/daliys/Daliys/Shardrail",
            "refusing a path inside the Shardrail repository",
        ),
        (
            "/Users/daliys/Daliys/Shardrail/spikes/wt-w8",
            "refusing a path inside the Shardrail repository",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_shardraild"))
            .arg("--data-dir")
            .arg(data.path())
            .arg("--no-file-log")
            .arg("--session-workdir")
            .arg(path)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{path} was accepted");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "{path}: {stderr}");
    }

    let missing = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg(fake_program())
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("--codex-app-server requires --codex-workdir or --session-workdir")
    );
}

#[test]
fn startup_logs_the_canonical_allowlist_and_codex_workdir_is_first() {
    let data = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let link_dir = tempfile::tempdir().unwrap();
    let link = link_dir.path().join("alias");
    symlink(second.path(), &link).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--codex-workdir")
        .arg(first.path())
        .arg("--session-workdir")
        .arg(&link)
        .arg("--codex-app-server")
        .arg(fake_program());
    let (mut guard, reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    drop(guard);
    let stderr = reader.join().unwrap();
    let first = std::fs::canonicalize(first.path()).unwrap();
    let second = std::fs::canonicalize(second.path()).unwrap();
    let listed = format!(
        "session workdir allowlist: {}, {}",
        first.display(),
        second.display()
    );
    assert!(stderr.contains(&listed), "{stderr}");
}

#[test]
fn enrollment_flag_persists_the_capability() {
    let data = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--session-workdir")
        .arg(data.path());
    let (mut guard, _reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    let code = enroll(data.path(), true);
    let granted = redeem(data.path(), &code, SigningKey::from_bytes(&[0x11; 32]));
    let plain = enroll(data.path(), false);
    let plain_client = redeem(data.path(), &plain, SigningKey::from_bytes(&[0x22; 32]));
    drop(guard);
    let registered: Value = serde_json::from_slice(
        &std::fs::read(data.path().join("authorized-clients.json")).unwrap(),
    )
    .unwrap();
    let clients = registered["clients"].as_array().unwrap();
    let granted_row = clients
        .iter()
        .find(|client| client["client_id"] == granted.id.to_string())
        .unwrap();
    let plain_row = clients
        .iter()
        .find(|client| client["client_id"] == plain_client.id.to_string())
        .unwrap();
    assert_eq!(granted_row["allow_session_create"], true);
    assert_eq!(granted_row["scope"], "limited");
    assert_eq!(plain_row["allow_session_create"], false);
}

#[test]
fn session_create_audits_allowed_and_denied_outcomes() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(work.path()).unwrap();
    let code = enroll(data.path(), true);
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--session-workdir")
        .arg(work.path())
        .arg("--codex-app-server")
        .arg(fake_program());
    let (mut guard, _reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);

    let limited = add_client(data.path(), "limited");
    let full = add_client(data.path(), "full");
    let browser = redeem(data.path(), &code, SigningKey::from_bytes(&[0x33; 32]));

    let (mut limited_stream, mut limited_reader) = handshake(data.path(), &limited);
    send_create(&mut limited_stream, json!({"workdir": canonical}));
    let rejected = read_json(&mut limited_reader);
    assert_eq!(rejected["kind"], "command_rejected");
    assert_eq!(rejected["error"], "limited clients cannot create sessions");

    let (mut browser_stream, mut browser_reader) = handshake(data.path(), &browser);
    let workdirs = read_json(&mut browser_reader);
    assert_eq!(workdirs["kind"], "session_workdirs");
    assert_eq!(
        workdirs["workdirs"][0],
        canonical.to_string_lossy().into_owned()
    );

    send_create(&mut browser_stream, json!({}));
    let missing = read_until(&mut browser_reader, "command_rejected");
    assert_eq!(
        missing["error"],
        "limited clients must name an allowlisted workdir"
    );

    send_create(
        &mut browser_stream,
        json!({"workdir": other.path().canonicalize().unwrap()}),
    );
    let outside = read_until(&mut browser_reader, "command_rejected");
    assert_eq!(outside["error"], "workdir is not allowlisted");

    let link = data.path().join("alias");
    symlink(&canonical, &link).unwrap();
    send_create(&mut browser_stream, json!({"workdir": link}));
    let linked = read_until(&mut browser_reader, "command_rejected");
    assert_eq!(linked["error"], "workdir must not contain a symlink");

    let escaped = format!(
        "{}/../{}",
        canonical.display(),
        canonical.file_name().unwrap().to_string_lossy()
    );
    send_create(&mut browser_stream, json!({"workdir": escaped}));
    let dotted = read_until(&mut browser_reader, "command_rejected");
    assert_eq!(dotted["error"], "workdir must not contain . or ..");

    let command_id = send_create(&mut browser_stream, json!({"workdir": canonical}));
    let accepted = read_until(&mut browser_reader, "command_accepted");
    assert!(accepted["cursor"].as_i64().unwrap() > 0);
    let created = loop {
        let frame = read_json(&mut browser_reader);
        if frame["kind"] == "durable_event" && frame["event"]["kind"] == "session_created" {
            break frame;
        }
    };
    assert_eq!(
        created["event"]["payload"]["workdir"],
        canonical.to_string_lossy().into_owned()
    );
    assert_eq!(created["event"]["payload"]["outcome"], "allowed");
    assert_eq!(created["event"]["payload"]["capability"], "session.create");
    assert_eq!(
        created["event"]["payload"]["command_id"],
        command_id.to_string()
    );

    writeln!(
        browser_stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": command_id, "kind": "session.create", "expected_state": null,
            "expected_turn_id": null, "payload": {"workdir": canonical}
        }})
    )
    .unwrap();
    assert_eq!(
        read_until(&mut browser_reader, "command_accepted")["cursor"],
        accepted["cursor"]
    );

    let (mut full_stream, mut full_reader) = handshake(data.path(), &full);
    let full_id = send_create(&mut full_stream, json!({}));
    assert_eq!(
        read_until(&mut full_reader, "command_accepted")["kind"],
        "command_accepted"
    );
    let full_created = loop {
        let frame = read_json(&mut full_reader);
        if frame["kind"] == "durable_event"
            && frame["event"]["kind"] == "session_created"
            && frame["event"]["payload"]["command_id"] == full_id.to_string()
        {
            break frame;
        }
    };
    assert_eq!(full_created["event"]["payload"]["capability"], "full");
    assert_eq!(
        full_created["event"]["payload"]["workdir"],
        canonical.to_string_lossy().into_owned()
    );
    send_create(
        &mut full_stream,
        json!({"workdir": other.path().canonicalize().unwrap()}),
    );
    let full_outside = read_until(&mut full_reader, "command_rejected");
    assert_eq!(full_outside["error"], "workdir is not allowlisted");

    drop(limited_stream);
    drop(browser_stream);
    drop(full_stream);
    drop(guard);

    let events = audits(data.path());
    assert!(events.iter().any(|event| {
        event.payload["outcome"] == "denied"
            && event.payload["reason"] == "scope"
            && event.payload["capability"] == "none"
    }));
    assert!(events.iter().any(|event| {
        event.payload["outcome"] == "denied" && event.payload["reason"] == "workdir"
    }));
    let allowed: Vec<_> = events
        .iter()
        .filter(|event| event.payload["outcome"] == "allowed")
        .collect();
    assert_eq!(allowed.len(), 2, "retry must not write a second audit");
    assert_audit(allowed[0], "allowed", None, "session.create");
    assert_eq!(
        allowed[0].payload["workdir"],
        canonical.to_string_lossy().into_owned()
    );
    assert_audit(allowed[1], "allowed", None, "full");
}

#[test]
fn session_create_without_codex_is_rejected_when_a_workdir_is_resolved() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(work.path()).unwrap();
    let code = enroll(data.path(), true);
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--session-workdir")
        .arg(work.path());
    let (mut guard, _reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    let browser = redeem(data.path(), &code, SigningKey::from_bytes(&[0x44; 32]));
    let full = add_client(data.path(), "full");
    let (mut browser_stream, mut browser_reader) = handshake(data.path(), &browser);
    let _workdirs = read_json(&mut browser_reader);
    send_create(&mut browser_stream, json!({"workdir": canonical}));
    let rejected = read_until(&mut browser_reader, "command_rejected");
    assert_eq!(rejected["error"], "codex is not configured");
    let (mut full_stream, mut full_reader) = handshake(data.path(), &full);
    send_create(&mut full_stream, json!({}));
    let full_rejected = read_until(&mut full_reader, "command_rejected");
    assert_eq!(full_rejected["error"], "codex is not configured");
    drop(browser_stream);
    drop(full_stream);
    drop(guard);
    let events = audits(data.path());
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.payload["reason"] == "codex" && event.payload["outcome"] == "denied"));
}

#[test]
fn full_client_without_an_allowlist_still_audits_a_fake_provider_session() {
    let data = tempfile::tempdir().unwrap();
    let client = add_client(data.path(), "full");
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log");
    let (mut guard, reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    let (mut stream, mut reader_stream) = handshake(data.path(), &client);
    send_create(&mut stream, json!({}));
    assert_eq!(
        read_until(&mut reader_stream, "command_accepted")["kind"],
        "command_accepted"
    );
    let created = loop {
        let frame = read_json(&mut reader_stream);
        if frame["kind"] == "durable_event" && frame["event"]["kind"] == "session_created" {
            break frame;
        }
    };
    assert!(created["event"]["payload"]["workdir"].is_null());
    assert_eq!(created["event"]["payload"]["outcome"], "allowed");
    drop(stream);
    drop(guard);
    let stderr = reader.join().unwrap();
    assert!(stderr.contains("session workdir allowlist: (empty)"));
    assert!(stderr.contains("outcome=allowed"));
    let events = audits(data.path());
    assert_eq!(events.len(), 1);
    assert_audit(&events[0], "allowed", None, "full");
    assert!(events[0].payload["workdir"].is_null());
}

#[test]
fn session_create_preconditions_are_audited_and_do_not_create() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(work.path()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--session-workdir")
        .arg(work.path())
        .arg("--codex-app-server")
        .arg(fake_program());
    let (mut guard, _reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    let full = add_client(data.path(), "full");
    let (mut stream, mut reader) = handshake(data.path(), &full);
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": Uuid::now_v7(), "kind": "session.create",
            "expected_state": "1", "expected_turn_id": null,
            "payload": {"workdir": canonical}
        }})
    )
    .unwrap();
    let rejected = read_until(&mut reader, "command_rejected");
    assert_eq!(
        rejected["error"],
        "unsupported command or invalid preconditions"
    );
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": Uuid::now_v7(), "kind": "session.create",
            "expected_state": null, "expected_turn_id": Uuid::now_v7(),
            "payload": {"workdir": format!("bad\n{canonical:?}")}
        }})
    )
    .unwrap();
    assert_eq!(
        read_until(&mut reader, "command_rejected")["error"],
        "unsupported command or invalid preconditions"
    );
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": Uuid::now_v7(), "kind": "turn.start",
            "expected_state": "1", "expected_turn_id": null, "payload": {}
        }})
    )
    .unwrap();
    assert_eq!(
        read_until(&mut reader, "command_rejected")["error"],
        "unsupported command or invalid preconditions"
    );
    drop(stream);
    drop(guard);
    let events = Journal::open(data.path().join("journal.sqlite3"))
        .unwrap()
        .events_after(0)
        .unwrap();
    assert!(events.iter().all(|event| event.kind != "session_created"));
    let audits: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "session_create_audit")
        .collect();
    assert_eq!(
        audits.len(),
        2,
        "a non-create command is not a create audit"
    );
    assert_eq!(audits[0].payload["outcome"], "denied");
    assert_eq!(audits[0].payload["reason"], "invalid_preconditions");
    assert_eq!(
        audits[0].payload["workdir"],
        canonical.to_string_lossy().into_owned()
    );
    assert_eq!(audits[1].payload["reason"], "invalid_preconditions");
    assert!(audits[1].payload["workdir"].is_null());
}

#[test]
fn extensions_rejected_session_is_not_retried() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let client = add_client(data.path(), "full");
    let command_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let journal_path = data.path().join("journal.sqlite3");
    {
        let mut journal = Journal::open(&journal_path).unwrap();
        let epoch = journal.journal_epoch().unwrap();
        let host_id = journal.host_id().unwrap();
        journal
            .append_event(&DurableEvent {
                event_id: 0,
                journal_epoch: epoch,
                host_id,
                session_id,
                schema_version: 1,
                kind: "session_created".into(),
                occurred_at_ms: 1,
                provider_ref: None,
                causation: None,
                redaction_flags: Vec::new(),
                payload: json!({
                    "session_id": session_id,
                    "client_id": client.id,
                    "command_id": command_id
                }),
            })
            .unwrap();
        journal
            .append_event(&DurableEvent {
                event_id: 0,
                journal_epoch: epoch,
                host_id,
                session_id,
                schema_version: 1,
                kind: "codex_extensions_rejected".into(),
                occurred_at_ms: 2,
                provider_ref: None,
                causation: None,
                redaction_flags: Vec::new(),
                payload: json!({"reason": "enabled plugins remain", "session_snapshot": null}),
            })
            .unwrap();
    }
    std::fs::set_permissions(
        &journal_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let argv = data.path().join("codex-argv.json");
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--session-workdir")
        .arg(work.path())
        .arg("--codex-app-server")
        .arg(fake_program())
        .env("SHARDRAIL_CODEX_ARGV_OUT", &argv)
        .env("SHARDRAIL_CODEX_FAKE_MODE", "happy");
    let (mut guard, _reader) = spawn_logged(&mut command);
    wait_for_socket(data.path(), &mut guard.0);
    let (mut stream, mut reader) = handshake(data.path(), &client);
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": command_id, "kind": "session.create",
            "expected_state": null, "expected_turn_id": null, "payload": {}
        }})
    )
    .unwrap();
    let rejected = read_until(&mut reader, "command_rejected");
    assert_eq!(rejected["error"], "codex extensions rejected");
    drop(stream);
    drop(guard);
    assert!(!argv.exists(), "a rejected session must not spawn Codex");
    let events = Journal::open(&journal_path)
        .unwrap()
        .events_after(0)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "session_created")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|event| event.payload["outcome"] != "allowed")
    );
}
