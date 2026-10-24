use ed25519_dalek::{Signature, Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shardrail_journal::Journal;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
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

struct CodexDirs {
    data: TempDir,
    trace: PathBuf,
    argv: PathBuf,
    work: TempDir,
}

fn codex_dirs() -> CodexDirs {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    CodexDirs {
        trace: data.path().join("trace.jsonl"),
        argv: data.path().join("argv.json"),
        work,
        data,
    }
}

fn start_codex(dirs: &CodexDirs, mode: &str, timeout_secs: u64) -> DaemonGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(dirs.data.path())
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(dirs.work.path())
        .arg("--codex-config")
        .arg("model_reasoning_effort=\"low\"")
        .arg("--approval-timeout-secs")
        .arg(timeout_secs.to_string())
        .env("SHARDRAIL_CODEX_FAKE_MODE", mode)
        .env("SHARDRAIL_CODEX_TRACE", &dirs.trace)
        .env("SHARDRAIL_CODEX_ARGV_OUT", &dirs.argv)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    DaemonGuard(command.spawn().unwrap())
}

fn connect(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let socket = path.join("shardraild.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            let reader_stream = stream.try_clone().unwrap();
            reader_stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            return (stream, BufReader::new(reader_stream));
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
    reader
        .read_line(&mut line)
        .unwrap_or_else(|error| panic!("daemon read failed: {error}"));
    assert!(!line.is_empty(), "daemon closed the socket");
    serde_json::from_str(&line).unwrap()
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

fn send_command(stream: &mut UnixStream, kind: &str, payload: Value) -> Uuid {
    let command_id = Uuid::now_v7();
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": command_id, "kind": kind, "expected_state": null,
            "expected_turn_id": null, "payload": payload
        }})
    )
    .unwrap();
    command_id
}

fn command_reply(reader: &mut BufReader<UnixStream>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a command reply"
        );
        let frame = read_json(reader);
        if matches!(
            frame["kind"].as_str(),
            Some("command_accepted" | "command_rejected" | "error")
        ) {
            return frame;
        }
    }
}

fn next_event(reader: &mut BufReader<UnixStream>, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for {kind}");
        let frame = read_json(reader);
        if frame["kind"] == "durable_event" && frame["event"]["kind"] == kind {
            return frame["event"].clone();
        }
    }
}

fn resolve(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    approval_id: &Value,
    option_id: &str,
) {
    writeln!(
        stream,
        "{}",
        json!({"kind":"approval_resolution","request":{"approval_id": approval_id, "option_id": option_id}})
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for approval resolution"
        );
        let frame = read_json(reader);
        if frame["kind"] == "approval_resolved" || frame["kind"] == "error" {
            assert_eq!(frame["kind"], "approval_resolved");
            return;
        }
    }
}

fn trace_messages(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn inbound(trace: &[Value], method: &str) -> Value {
    trace
        .iter()
        .find(|entry| entry["direction"] == "in" && entry["message"]["method"] == method)
        .unwrap_or_else(|| panic!("missing inbound {method}"))["message"]
        .clone()
}

fn decisions(trace: &[Value]) -> Vec<String> {
    trace
        .iter()
        .filter(|entry| entry["direction"] == "in" && entry["message"].get("result").is_some())
        .filter_map(|entry| {
            entry["message"]["result"]["decision"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn journal_events(path: &Path) -> Vec<shardrail_protocol::DurableEvent> {
    Journal::open(path.join("journal.sqlite3"))
        .unwrap()
        .events_after(0)
        .unwrap()
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

#[test]
fn codex_flags_are_mutually_exclusive() {
    let data = tempfile::tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--fake-acp-path")
        .arg(fake_program())
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(data.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn codex_streams_approvals_redacts_and_interrupts() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "happy", 30);
    let full = add_client(dirs.data.path(), "full");
    let limited = add_client(dirs.data.path(), "limited");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &full);
    send_command(&mut stream, "session.create", json!({}));
    let created = command_reply(&mut reader);
    assert_eq!(created["kind"], "command_accepted");
    let trace = trace_messages(&dirs.trace);
    let argv: Vec<String> = serde_json::from_slice(&std::fs::read(&dirs.argv).unwrap()).unwrap();
    assert!(
        argv.iter()
            .any(|arg| arg.contains("model_reasoning_effort"))
    );
    for required in [
        "mcp_servers.node_repl.enabled=false",
        "mcp_servers.cua_repl.command=\"/usr/bin/false\"",
        "mcp_servers.computer-use.enabled=false",
        "plugins.\"browser@openai-bundled\".enabled=false",
        "plugins.\"computer-use@openai-bundled\".enabled=false",
        "plugins.\"unified-computer-use@openai-bundled\".enabled=false",
        "plugins.\"codex-app-tools@openai-bundled\".enabled=false",
        "plugins.\"visualize@openai-bundled\".enabled=false",
        "features.js_repl=false",
        "web_search=\"disabled\"",
    ] {
        assert!(argv.iter().any(|arg| arg == required), "missing {required}");
    }
    let initialize = inbound(&trace, "initialize");
    assert!(initialize["params"].get("experimentalApi").is_none());
    assert!(
        initialize["params"]["capabilities"]
            .get("experimentalApi")
            .is_none()
    );
    let thread = inbound(&trace, "thread/start");
    assert_eq!(thread["params"]["approvalsReviewer"], "user");
    assert_eq!(thread["params"]["sandbox"], "workspace-write");
    assert_eq!(thread["params"]["approvalPolicy"], "on-request");
    let cwd = dirs.work.path().canonicalize().unwrap();
    assert_eq!(thread["params"]["cwd"], cwd.to_string_lossy().into_owned());

    let (mut limited_stream, mut limited_reader) = handshake(dirs.data.path(), &limited);
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(&mut limited_stream, "session.create", json!({}));
    let rejected = command_reply(&mut limited_reader);
    assert_eq!(rejected["error"], "limited clients cannot create sessions");

    let prompt = "Add a function sub";
    send_command(
        &mut limited_stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": prompt}),
    );
    assert_eq!(
        command_reply(&mut limited_reader)["kind"],
        "command_accepted"
    );
    let chunk = next_event(&mut limited_reader, "agent_message_chunk");
    let text = chunk["payload"]["content"]["text"].as_str().unwrap();
    assert!(text.contains("[REDACTED]"));
    assert!(!text.contains("Qm9tV2l6a0p4Y2xRbFh2N1N4bE5vV2V5WjRrU2J1R3A"));
    let approval = next_event(&mut limited_reader, "approval_requested");
    assert_eq!(approval["payload"]["command"], "python3 -m unittest");
    assert_eq!(approval["payload"]["approval_kind"], "command_execution");
    resolve(
        &mut limited_stream,
        &mut limited_reader,
        &approval["payload"]["approval_id"],
        "decline",
    );
    let file_approval = next_event(&mut limited_reader, "approval_requested");
    assert_eq!(file_approval["payload"]["approval_kind"], "file_change");
    assert!(
        file_approval["payload"]["changes"]
            .to_string()
            .contains("calc.py")
    );
    resolve(
        &mut limited_stream,
        &mut limited_reader,
        &file_approval["payload"]["approval_id"],
        "accept",
    );
    let finished = next_event(&mut limited_reader, "turn_finished");
    assert_eq!(finished["payload"]["provider_status"], "completed");
    assert_eq!(
        finished["payload"]["session_snapshot"]["last_turn_outcome"],
        "Provider"
    );

    send_command(
        &mut limited_stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "Explain calc.py in detail, step by step"}),
    );
    assert_eq!(
        command_reply(&mut limited_reader)["kind"],
        "command_accepted"
    );
    let _delta = next_event(&mut limited_reader, "agent_message_chunk");
    send_command(
        &mut limited_stream,
        "turn.interrupt",
        json!({"session_id": session_id}),
    );
    assert_eq!(
        command_reply(&mut limited_reader)["kind"],
        "command_accepted"
    );
    let interrupted = next_event(&mut limited_reader, "turn_finished");
    assert_eq!(interrupted["payload"]["provider_status"], "interrupted");
    assert_eq!(
        interrupted["payload"]["session_snapshot"]["last_turn_outcome"],
        "Interrupted"
    );

    let trace = trace_messages(&dirs.trace);
    let turn = inbound(&trace, "turn/start");
    assert_eq!(turn["params"]["approvalsReviewer"], "user");
    assert_eq!(turn["params"]["approvalPolicy"], "on-request");
    assert_eq!(turn["params"]["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(turn["params"]["input"][0]["type"], "text");
    let made = decisions(&trace);
    assert_eq!(made, vec!["decline".to_owned(), "accept".to_owned()]);
    assert!(
        !trace
            .iter()
            .any(|entry| entry.to_string().contains("acceptForSession"))
    );
    drop(stream);
    drop(limited_stream);
}

#[test]
fn codex_bad_reviewer_blocks_turns() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "bad-reviewer", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let rejected = next_event(&mut reader, "approval_routing_rejected");
    assert!(
        rejected["payload"]["reason"]
            .as_str()
            .unwrap()
            .contains("routed")
    );
    let session_id = rejected["session_id"].as_str().unwrap().to_owned();
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hi"}),
    );
    let reply = command_reply(&mut reader);
    assert_eq!(reply["kind"], "command_rejected");
    assert!(reply["error"].as_str().unwrap().contains("routed"));
}

#[test]
fn codex_omitted_reviewer_blocks_turns() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "omit-reviewer", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = next_event(&mut reader, "approval_routing_rejected")["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hi"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_rejected");
}

#[test]
fn codex_bad_turn_reviewer_stops_the_turn() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "bad-turn-reviewer", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hi"}),
    );
    let reply = command_reply(&mut reader);
    assert_eq!(reply["kind"], "command_rejected");
    assert!(reply["error"].as_str().unwrap().contains("routed"));
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "again"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_rejected");
}

#[test]
fn codex_unknown_request_is_refused() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "unknown", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "ask"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let refused = next_event(&mut reader, "provider_request_refused");
    assert_eq!(refused["payload"]["method"], "item/tool/requestUserInput");
    let trace = trace_messages(&dirs.trace);
    let error = trace
        .iter()
        .find(|entry| entry["direction"] == "in" && entry["message"]["error"]["code"] == -32601);
    assert!(error.is_some(), "missing JSON-RPC refusal: {trace:?}");
    assert!(
        !decisions(&trace)
            .iter()
            .any(|decision| decision == "accept")
    );
}

#[test]
fn codex_child_crash_interrupts_the_turn() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "crash", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "crash"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let finished = next_event(&mut reader, "turn_finished");
    assert_eq!(
        finished["payload"]["session_snapshot"]["last_turn_outcome"],
        "Interrupted"
    );
    let _ = next_event(&mut reader, "process_exited");
}

#[test]
fn codex_approval_timeout_never_accepts() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "hold", 1);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hold"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let _ = next_event(&mut reader, "approval_requested");
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut made = Vec::new();
    while Instant::now() < deadline {
        made = decisions(&trace_messages(&dirs.trace));
        if !made.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(made, vec!["decline".to_owned()]);
}

#[test]
fn codex_disconnect_during_approval_never_accepts() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "disconnect", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "session_created")
        .unwrap()
        .session_id;
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "drop"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let _ = next_event(&mut reader, "approval_requested");
    let finished = next_event(&mut reader, "turn_finished");
    assert_eq!(
        finished["payload"]["session_snapshot"]["last_turn_outcome"],
        "Interrupted"
    );
    thread::sleep(Duration::from_millis(200));
    let made = decisions(&trace_messages(&dirs.trace));
    assert!(
        !made.iter().any(|decision| decision == "accept"),
        "{made:?}"
    );
}

#[test]
fn codex_daemon_exit_kills_the_child_group() {
    let dirs = codex_dirs();
    let pid_path = dirs.data.path().join("child.pid");
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(dirs.data.path())
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(dirs.work.path())
        .env("SHARDRAIL_CODEX_FAKE_MODE", "idle")
        .env("SHARDRAIL_CODEX_TRACE", &dirs.trace)
        .env("SHARDRAIL_CODEX_ARGV_OUT", &dirs.argv)
        .env("SHARDRAIL_CODEX_PID_OUT", &pid_path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut daemon = command.spawn().unwrap();
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let child_pid = std::fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .to_owned();
    let status = Command::new("kill")
        .args(["-TERM", &daemon.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let _ = daemon.wait();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut alive = true;
    while Instant::now() < deadline {
        let probe = Command::new("kill")
            .args(["-0", &child_pid])
            .status()
            .unwrap();
        if !probe.success() {
            alive = false;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive, "codex child {child_pid} survived daemon exit");
}
