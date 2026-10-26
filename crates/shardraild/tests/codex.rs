use ed25519_dalek::{Signature, Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shardrail_journal::Journal;
use std::io::{BufRead, BufReader, Write};
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
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

impl Deref for DaemonGuard {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DaemonGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

struct KillThese(Vec<u32>);

impl Drop for KillThese {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .stderr(Stdio::null())
                .status();
        }
    }
}

struct RemoveDir(PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let pid = self.0.id().to_string();
        let _ = Command::new("kill")
            .args(["-TERM", &pid])
            .stderr(Stdio::null())
            .status();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            match self.0.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let group = format!("-{}", self.0.id());
        let _ = Command::new("kill")
            .args(["-KILL", &group])
            .stderr(Stdio::null())
            .status();
        let _ = self.0.wait();
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
    start_codex_with(dirs, mode, timeout_secs, &[])
}

fn start_codex_with(
    dirs: &CodexDirs,
    mode: &str,
    timeout_secs: u64,
    extra: &[&str],
) -> DaemonGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(dirs.data.path())
        .arg("--exit-with-stdin")
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
        .env("SHARDRAIL_CODEX_PID_OUT", pid_path(dirs))
        .env("SHARDRAIL_CODEX_FD_OUT", fd_path(dirs))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(dirs.data.path().join("daemon.stderr")).unwrap(),
        ));
    for arg in extra {
        command.arg(arg);
    }
    command.process_group(0);
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
        "features.plugins=false",
        "features.hooks=false",
        "features.apps=false",
        "skills.bundled.enabled=false",
        "skills.include_instructions=false",
        "skills.config=[]",
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
    assert!(thread["params"]["ephemeral"].as_bool().unwrap());
    assert_eq!(thread["params"]["sandbox"], "workspace-write");
    assert_eq!(thread["params"]["approvalPolicy"], "untrusted");
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
    assert_eq!(turn["params"]["approvalPolicy"], "untrusted");
    assert_eq!(turn["params"]["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(turn["params"]["input"][0]["type"], "text");
    let made = decisions(&trace);
    assert_eq!(made, vec!["decline".to_owned(), "accept".to_owned()]);
    assert!(
        !trace
            .iter()
            .any(|entry| entry.to_string().contains("acceptForSession"))
    );
    assert_codex_trace(&trace);
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
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut trace = trace_messages(&dirs.trace);
    while Instant::now() < deadline
        && !trace
            .iter()
            .any(|entry| entry["direction"] == "in" && entry["message"]["error"]["code"] == -32601)
    {
        thread::sleep(Duration::from_millis(20));
        trace = trace_messages(&dirs.trace);
    }
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
        .arg("--exit-with-stdin")
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(dirs.work.path())
        .env("SHARDRAIL_CODEX_FAKE_MODE", "idle")
        .env("SHARDRAIL_CODEX_TRACE", &dirs.trace)
        .env("SHARDRAIL_CODEX_ARGV_OUT", &dirs.argv)
        .env("SHARDRAIL_CODEX_PID_OUT", &pid_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command.process_group(0);
    let mut daemon = DaemonGuard(command.spawn().unwrap());
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
            .stderr(Stdio::null())
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

#[test]
fn codex_weaker_policy_echo_blocks_turns() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "weak-policy", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let rejected = next_event(&mut reader, "approval_routing_rejected");
    assert!(
        rejected["payload"]["reason"]
            .as_str()
            .unwrap()
            .contains("weaker than untrusted")
    );
    let session_id = rejected["session_id"].as_str().unwrap().to_owned();
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hi"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_rejected");
}

#[test]
fn codex_file_change_without_approval_fails_closed() {
    let fixture = schema_root()
        .parent()
        .unwrap()
        .join("codex-0.160.0-on-request-autowrite.jsonl");
    for line in std::fs::read_to_string(&fixture).unwrap().lines() {
        let frame: Value = serde_json::from_str(&line.replace("TURN_ID", "turn-1")).unwrap();
        assert_valid("ServerNotification.json", &frame);
    }
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "autowrite", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = next_event(&mut reader, "process_started")["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "add sub"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let rejected = next_event(&mut reader, "approval_routing_rejected");
    assert!(
        rejected["payload"]["reason"]
            .as_str()
            .unwrap()
            .contains("without an approval request")
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let trace = trace_messages(&dirs.trace);
        if trace.iter().any(|entry| {
            entry["direction"] == "in" && entry["message"]["method"] == "turn/interrupt"
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no turn/interrupt sent"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    next_event(&mut reader, "turn_finished");
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "again"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_rejected");
}

#[test]
fn codex_readonly_network_echo_blocks_turns() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "readonly-network", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let rejected = next_event(&mut reader, "approval_routing_rejected");
    assert!(
        rejected["payload"]["reason"]
            .as_str()
            .unwrap()
            .contains("sandbox")
    );
    let session_id = rejected["session_id"].as_str().unwrap().to_owned();
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "hi"}),
    );
    let reply = command_reply(&mut reader);
    assert_eq!(reply["kind"], "command_rejected");
    let trace = trace_messages(&dirs.trace);
    let echoed = trace.iter().find(|entry| {
        entry["direction"] == "out" && entry["message"]["result"]["sandbox"]["type"] == "readOnly"
    });
    let echoed = echoed.expect("missing readOnly echo");
    assert_eq!(
        echoed["message"]["result"]["sandbox"]["networkAccess"],
        true
    );
    assert_valid("v2/ThreadStartResponse.json", &echoed["message"]["result"]);
}

#[test]
fn codex_mcp_config_override_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(data.path())
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(data.path())
        .arg("--codex-config")
        .arg("mcp_servers.other.enabled=true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn codex_invalid_turn_session_rejects_without_dropping_or_stalling() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "slow-turn", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    let other = add_client(dirs.data.path(), "full");
    let (mut other_stream, mut other_reader) = handshake(dirs.data.path(), &other);
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
        json!({"session_id": session_id, "prompt": "slow"}),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if trace_messages(&dirs.trace)
            .iter()
            .any(|entry| entry["direction"] == "in" && entry["message"]["method"] == "turn/start")
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    send_command(
        &mut other_stream,
        "turn.start",
        json!({"session_id": "not-a-uuid", "prompt": "nope"}),
    );
    let rejected = command_reply(&mut other_reader);
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "invalid turn command stalled for {:?}",
        started.elapsed()
    );
    assert_eq!(rejected["kind"], "command_rejected");
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("session_id is invalid")
    );
    send_command(
        &mut other_stream,
        "turn.interrupt",
        json!({"session_id": "still-not-a-uuid"}),
    );
    let still_open = command_reply(&mut other_reader);
    assert_eq!(still_open["kind"], "command_rejected");
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
}

#[test]
fn failed_codex_start_retry_stays_rejected() {
    let dirs = codex_dirs();
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(dirs.data.path())
        .arg("--exit-with-stdin")
        .arg("--no-file-log")
        .arg("--codex-app-server")
        .arg("/usr/bin/false")
        .arg("--codex-workdir")
        .arg(dirs.work.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.stdin(Stdio::piped());
    command.process_group(0);
    let _daemon = DaemonGuard(command.spawn().unwrap());
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    let command_id = send_command(&mut stream, "session.create", json!({}));
    let first = command_reply(&mut reader);
    assert_eq!(first["kind"], "command_rejected");
    writeln!(
        stream,
        "{}",
        json!({"kind":"request","request":{
            "command_id": command_id, "kind": "session.create", "expected_state": null,
            "expected_turn_id": null, "payload": {}
        }})
    )
    .unwrap();
    let second = command_reply(&mut reader);
    assert_eq!(second["kind"], "command_rejected");
    let events = journal_events(dirs.data.path());
    assert!(events.iter().all(|event| event.kind != "session_created"));
    let audits: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "session_create_audit")
        .collect();
    assert_eq!(audits.len(), 2, "each failed attempt is its own denial");
    assert!(audits.iter().all(|event| {
        event.payload["outcome"] == "denied" && event.payload["reason"] == "extensions"
    }));
}

#[test]
fn pinned_turn_schema_rejects_a_start_without_thread_id() {
    let missing = json!({"input": [{"type": "text", "text": "hi"}]});
    assert!(validate_schema("v2/TurnStartParams.json", &missing).is_err());
    let present = json!({
        "threadId": "thread-1",
        "input": [{"type": "text", "text": "hi"}],
        "approvalsReviewer": "user",
        "approvalPolicy": "on-request",
        "cwd": "/tmp/work",
        "sandboxPolicy": {"type": "workspaceWrite"}
    });
    assert_valid("v2/TurnStartParams.json", &present);
    let networked = json!({"type": "readOnly", "networkAccess": true});
    let response = load_schema("v2/ThreadStartResponse.json");
    validate(
        &response["definitions"]["SandboxPolicy"],
        &networked,
        &response,
        0,
    )
    .expect("readOnly networkAccess true is a legal sandbox echo");
}

fn config_values(argv: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut expect_value = false;
    let mut started = false;
    for arg in argv {
        if !started {
            if arg == "app-server" {
                started = true;
            }
            continue;
        }
        if expect_value {
            values.push(arg.clone());
            expect_value = false;
            continue;
        }
        assert_eq!(arg, "-c");
        expect_value = true;
    }
    assert!(!expect_value);
    values
}

fn fixture_enabled_skill_names() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex-0.160.0-extensions-default.trimmed.jsonl");
    let mut names = Vec::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let item: Value = serde_json::from_str(line).unwrap();
        let Some(skills) = item["frame"]["result"]["data"][0]["skills"].as_array() else {
            continue;
        };
        for skill in skills {
            if skill["enabled"].as_bool() != Some(true) {
                continue;
            }
            let name = skill["name"].as_str().unwrap().to_owned();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn extension_indexes(trace: &[Value], method: &str) -> Vec<usize> {
    trace
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry["direction"] == "in" && entry["message"]["method"] == method)
        .map(|(index, _)| index)
        .collect()
}

#[test]
fn codex_recorded_extensions_are_disabled_before_thread_start() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "recorded", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let trace = trace_messages(&dirs.trace);
    let argv: Vec<String> = serde_json::from_slice(&std::fs::read(&dirs.argv).unwrap()).unwrap();
    let values = config_values(&argv);
    let names = fixture_enabled_skill_names();
    assert_eq!(names.len(), 8);
    let mut expected = vec![
        "model_reasoning_effort=\"low\"".to_owned(),
        "mcp_servers.node_repl.enabled=false".into(),
        "mcp_servers.cua_repl.command=\"/usr/bin/false\"".into(),
        "mcp_servers.cua_repl.enabled=false".into(),
        "mcp_servers.computer-use.enabled=false".into(),
        "mcp_servers.nexus-unity.enabled=false".into(),
        "mcp_servers.unityMCP.enabled=false".into(),
        "plugins.\"browser@openai-bundled\".enabled=false".into(),
        "plugins.\"computer-use@openai-bundled\".enabled=false".into(),
        "plugins.\"unified-computer-use@openai-bundled\".enabled=false".into(),
        "plugins.\"codex-app-tools@openai-bundled\".enabled=false".into(),
        "plugins.\"visualize@openai-bundled\".enabled=false".into(),
        "features.js_repl=false".into(),
        "web_search=\"disabled\"".into(),
        "features.plugins=false".into(),
        "features.hooks=false".into(),
        "features.apps=false".into(),
        "skills.bundled.enabled=false".into(),
        "skills.include_instructions=false".into(),
    ];
    let disabled = names
        .iter()
        .map(|name| format!("{{name=\"{name}\",enabled=false}}"))
        .collect::<Vec<_>>()
        .join(",");
    expected.push(format!("skills.config=[{disabled}]"));
    expected.push("mcp_servers.node_repl.enabled=false".into());
    expected.push("mcp_servers.\"odd.server\".enabled=false".into());
    expected.push("mcp_servers.soma.enabled=false".into());
    assert_eq!(values, expected);
    let skills = extension_indexes(&trace, "skills/list");
    let hooks = extension_indexes(&trace, "hooks/list");
    let plugins = extension_indexes(&trace, "plugin/installed");
    let apps = extension_indexes(&trace, "app/list");
    let configs = extension_indexes(&trace, "config/read");
    let threads = extension_indexes(&trace, "thread/start");
    assert_eq!(skills.len(), 2, "preflight and session both list skills");
    assert_eq!(hooks.len(), 1);
    assert_eq!(plugins.len(), 1);
    assert_eq!(apps.len(), 1);
    assert_eq!(configs.len(), 2, "preflight and session both read config");
    assert_eq!(threads.len(), 1);
    assert!(
        skills[1] < hooks[0]
            && hooks[0] < plugins[0]
            && plugins[0] < apps[0]
            && apps[0] < configs[1]
            && configs[1] < threads[0]
    );
    assert_extension_exchange(&trace);
    let verified = journal_events(dirs.data.path())
        .into_iter()
        .find(|event| event.kind == "codex_extensions_verified")
        .expect("verification was not journaled");
    assert!(verified.payload.get("session_snapshot").is_some());
    assert_eq!(verified.payload["enabled_skills"], 0);
    assert_eq!(verified.payload["hooks"], 0);
    assert_eq!(verified.payload["enabled_plugins"], 0);
    assert_eq!(verified.payload["apps"], 0);
    assert_eq!(verified.payload["mcp_servers"], 0);
    let decoy = "mcp-decoy-secret-SHOULD-NOT-LEAK-9c1e4b";
    assert!(!verified.payload.to_string().contains(decoy));
    assert_no_decoy(&dirs, decoy);
}

#[test]
fn codex_extensions_left_on_fail_closed() {
    assert_extension_rejected(&codex_dirs(), "extensions-on", "enabled plugins remain");
}

#[test]
fn codex_skill_left_enabled_fails_closed() {
    assert_extension_rejected(&codex_dirs(), "skill-enabled", "enabled skills remain");
}

#[test]
fn codex_hook_left_present_fails_closed() {
    assert_extension_rejected(&codex_dirs(), "hook-present", "hooks remain");
}

#[test]
fn codex_extension_list_error_fails_closed() {
    assert_extension_rejected(&codex_dirs(), "list-error", "skills/list failed");
}

#[test]
fn codex_mcp_left_enabled_fails_closed() {
    assert_extension_rejected(&codex_dirs(), "mcp-enabled", "enabled mcp servers remain");
}

#[test]
fn codex_config_read_error_fails_closed() {
    assert_extension_rejected(&codex_dirs(), "config-read-error", "config/read failed");
}

#[test]
fn codex_mcp_startup_after_thread_start_stops_the_session() {
    assert_mcp_activity_stops(&codex_dirs(), "mcp-startup", false);
}

#[test]
fn codex_mcp_request_after_thread_start_stops_the_session() {
    assert_mcp_activity_stops(&codex_dirs(), "mcp-request", false);
}

#[test]
fn codex_mcp_startup_during_a_turn_interrupts_and_stops() {
    assert_mcp_activity_stops(&codex_dirs(), "mcp-during-turn", true);
}

#[test]
fn codex_mcp_item_started_stops_without_journaling_item() {
    assert_mcp_item_stops("mcp-item-started");
}

#[test]
fn codex_mcp_item_updated_stops_without_journaling_item() {
    assert_mcp_item_stops("mcp-item-updated");
}

#[test]
fn codex_mcp_item_completed_stops_without_journaling_item() {
    assert_mcp_item_stops("mcp-item-completed");
}

#[test]
fn codex_mcp_tool_call_method_stops() {
    assert_mcp_item_stops("mcp-item-method");
}

#[test]
fn codex_dynamic_tool_item_stops() {
    assert_mcp_item_stops("dynamic-tool-item");
}

#[test]
fn codex_mcp_auto_approval_review_stops() {
    assert_mcp_item_stops("mcp-auto-review-started");
    assert_mcp_item_stops("mcp-auto-review-completed");
}

#[test]
fn codex_mcp_turn_items_stop() {
    assert_mcp_item_stops("mcp-turn-started-items");
    assert_mcp_item_stops("mcp-turn-completed-items");
}

#[test]
fn codex_dynamic_tool_request_stops_without_journaling_arguments() {
    assert_mcp_item_stops("dynamic-tool-request");
}

#[test]
fn codex_mcp_item_during_turn_interrupts_and_stops() {
    assert_mcp_item_stops("mcp-item-during-turn");
}

#[test]
fn codex_thread_rollout_path_fails_closed() {
    assert_thread_echo_rejected("thread-path", "rollout path");
}

#[test]
fn codex_thread_ephemeral_echo_must_be_true() {
    assert_thread_echo_rejected("bad-ephemeral", "not ephemeral");
}

#[test]
fn codex_thread_started_notification_must_not_report_rollout_path() {
    assert_thread_echo_rejected("notification-path", "rollout path");
}

#[test]
fn codex_thread_started_notification_must_echo_ephemeral() {
    assert_thread_echo_rejected("notification-ephemeral", "not ephemeral");
}

#[test]
fn codex_replaced_path_after_pin_refuses_to_start() {
    let dirs = codex_dirs();
    let replaced = replaced_workdir(&dirs);
    let _cleanup = RemoveDir(replaced);
    let _daemon = start_codex_with(&dirs, "happy", 30, &["--test-replace-workdir-after-pin"]);
    expect_extension_rejection(
        &dirs,
        "chdir-fd: directory identity does not match the pinned workdir",
    );
}

#[test]
fn codex_child_does_not_inherit_the_workdir_directory_fd() {
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "happy", 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let workdir = std::fs::canonicalize(dirs.work.path()).unwrap();
    let metadata = std::fs::metadata(&workdir).unwrap();
    let report = std::fs::read_to_string(fd_path(&dirs)).expect("codex fd report");
    let mut rows = 0usize;
    for line in report.lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        let fds = value["fds"].as_array().expect("fd rows");
        rows += fds.len();
        for fd in fds {
            let target = fd["target"].as_str().unwrap_or("");
            let path = Path::new(target);
            let same_path = !target.is_empty()
                && (path == workdir
                    || std::fs::canonicalize(path).ok().as_deref() == Some(workdir.as_path()));
            let same_dir = fd["dir"] == true
                && fd["dev"].as_u64() == Some(metadata.dev())
                && fd["ino"].as_u64() == Some(metadata.ino());
            assert!(
                !same_path && !same_dir,
                "fd {} points at the workdir ({target}): {fd}",
                fd["fd"]
            );
        }
    }
    assert!(rows > 0, "codex reported no open fds: {report}");
}

#[test]
fn codex_parent_stdin_eof_stops_the_daemon_and_the_child() {
    let dirs = codex_dirs();
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(dirs.data.path())
        .arg("--no-file-log")
        .arg("--exit-with-stdin")
        .arg("--codex-app-server")
        .arg(fake_program())
        .arg("--codex-workdir")
        .arg(dirs.work.path())
        .arg("--approval-timeout-secs")
        .arg("30")
        .env("SHARDRAIL_CODEX_FAKE_MODE", "idle")
        .env("SHARDRAIL_CODEX_TRACE", &dirs.trace)
        .env("SHARDRAIL_CODEX_PID_OUT", pid_path(&dirs))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(dirs.data.path().join("daemon.stderr")).unwrap(),
        ));
    command.process_group(0);
    let mut daemon = DaemonGuard(command.spawn().unwrap());
    let mut kill = KillThese(vec![daemon.id()]);
    let stdin = daemon.stdin.take();
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let child_pid = read_pid_file(&pid_path(&dirs));
    kill.0.push(child_pid.trim().parse().expect("codex pid"));
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match daemon.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "daemon exit status {status}");
                break;
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            other => panic!("daemon did not exit after stdin closed: {other:?}"),
        }
    }
    wait_until_pid_gone(&child_pid);
    assert!(
        !pid_alive(daemon.id()),
        "daemon {} was still running",
        daemon.id()
    );
    kill.0.clear();
}

fn assert_mcp_activity_stops(dirs: &CodexDirs, mode: &str, during_turn: bool) {
    let daemon = start_codex(dirs, mode, 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_session_id(dirs.data.path());
    if during_turn {
        send_command(
            &mut stream,
            "turn.start",
            json!({"session_id": session_id, "prompt": "hello"}),
        );
        let started = command_reply(&mut reader);
        assert_eq!(started["kind"], "command_accepted", "{started}");
    }
    let events = wait_for_journal(dirs.data.path(), "codex_extensions_rejected");
    let event = events
        .iter()
        .find(|event| event.kind == "codex_extensions_rejected")
        .unwrap();
    let reason = event.payload["reason"].as_str().unwrap();
    assert!(reason.contains("mcp server activity"), "{reason}");
    assert!(
        events.iter().all(|event| {
            event.kind != "provider_notice"
                || event.payload["method"]
                    .as_str()
                    .is_none_or(|method| !method.starts_with("mcpServer"))
        }),
        "mcp activity was journaled as a notice"
    );
    if during_turn {
        let trace = trace_messages(&dirs.trace);
        assert!(
            extension_indexes(&trace, "turn/interrupt").len() == 1,
            "running turn was not interrupted: {trace:?}"
        );
    }
    let _ = wait_for_journal(dirs.data.path(), "process_exited");
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "again"}),
    );
    let later = command_reply(&mut reader);
    assert_eq!(later["kind"], "command_rejected", "{later}");
    assert!(
        later["error"]
            .as_str()
            .unwrap()
            .contains("mcp server activity"),
        "{}",
        later["error"]
    );
    let child_pid = std::fs::read_to_string(pid_path(dirs)).unwrap_or_default();
    assert!(!child_pid.trim().is_empty(), "codex pid was not recorded");
    wait_until_pid_gone(&child_pid);
    drop(daemon);
    assert_no_decoy(dirs, "mcp-decoy-secret-SHOULD-NOT-LEAK-9c1e4b");
}

fn assert_mcp_item_stops(mode: &str) {
    let dirs = codex_dirs();
    let daemon = start_codex(&dirs, mode, 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let session_id = journal_session_id(dirs.data.path());
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "trigger MCP guard"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let events = wait_for_journal(dirs.data.path(), "codex_extensions_rejected");
    assert!(events.iter().any(|event| {
        event.kind == "codex_extensions_rejected"
            && event.payload["reason"]
                .as_str()
                .unwrap()
                .contains("mcp server activity")
    }));
    assert!(
        events.iter().all(|event| {
            !(matches!(
                event.kind.as_str(),
                "provider_item" | "provider_notice" | "provider_request_refused"
            ) && (event.payload.to_string().contains("mcpToolCall")
                || event.payload.to_string().contains("dynamicToolCall")
                || event.payload.to_string().contains("mcp-item-secret")
                || event.payload.to_string().contains("dynamic-tool-secret")
                || event.payload.to_string().contains("mcp-server-secret")
                || event.payload.to_string().contains("mcp-tool-secret")
                || event.payload.to_string().contains("mcp-turn-item-secret")
                || event.payload.to_string().contains("dynamic-args-secret")))
        }),
        "MCP tool item was journaled"
    );
    let trace = trace_messages(&dirs.trace);
    assert_eq!(extension_indexes(&trace, "turn/start").len(), 1);
    assert_eq!(
        extension_indexes(&trace, "turn/interrupt").len(),
        1,
        "{trace:?}"
    );
    if mode == "dynamic-tool-request" {
        assert!(trace.iter().any(|frame| {
            frame["direction"] == "in"
                && frame["message"]["id"] == "dynamic-call"
                && frame["message"]["error"]["code"] == -32601
        }));
    }
    let child_pid = read_pid_file(&pid_path(&dirs));
    wait_until_pid_gone(&child_pid);
    send_command(
        &mut stream,
        "turn.start",
        json!({"session_id": session_id, "prompt": "must be refused"}),
    );
    assert_eq!(command_reply(&mut reader)["kind"], "command_rejected");
    drop(daemon);
}

fn assert_thread_echo_rejected(mode: &str, reason: &str) {
    let dirs = codex_dirs();
    let daemon = start_codex(&dirs, mode, 30);
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let rejected = next_event(&mut reader, "approval_routing_rejected");
    assert!(
        rejected["payload"]["reason"]
            .as_str()
            .unwrap()
            .contains(reason)
    );
    assert!(extension_indexes(&trace_messages(&dirs.trace), "turn/start").is_empty());
    let child_pid = read_pid_file(&pid_path(&dirs));
    wait_until_pid_gone(&child_pid);
    drop(daemon);
}

fn fd_path(dirs: &CodexDirs) -> PathBuf {
    dirs.data.path().join("codex.fds")
}

fn pid_path(dirs: &CodexDirs) -> PathBuf {
    dirs.data.path().join("codex.pid")
}

fn replaced_workdir(dirs: &CodexDirs) -> PathBuf {
    let path =
        std::fs::canonicalize(dirs.work.path()).unwrap_or_else(|_| dirs.work.path().to_path_buf());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}-replaced"))
}

fn read_pid_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "codex pid was not recorded");
        thread::sleep(Duration::from_millis(20));
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn journal_session_id(path: &Path) -> String {
    journal_events(path)
        .into_iter()
        .find(|event| event.kind == "session_created")
        .expect("session_created")
        .session_id
        .to_string()
}

fn wait_for_journal(path: &Path, kind: &str) -> Vec<shardrail_protocol::DurableEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = journal_events(path);
        if events.iter().any(|event| event.kind == kind) || Instant::now() > deadline {
            assert!(
                events.iter().any(|event| event.kind == kind),
                "missing {kind}"
            );
            return events;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_until_pid_gone(text: &str) {
    let pid = text.trim();
    if pid.is_empty() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("codex child {pid} was still running");
}

fn assert_no_decoy(dirs: &CodexDirs, decoy: &str) {
    let journal = std::fs::read(dirs.data.path().join("journal.sqlite3")).unwrap_or_default();
    let log = std::fs::read(dirs.data.path().join("daemon.stderr")).unwrap_or_default();
    let trace = std::fs::read(&dirs.trace).unwrap_or_default();
    for (name, bytes) in [("journal", journal), ("log", log), ("trace", trace)] {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(decoy),
            "{name} contains the config/read decoy"
        );
    }
}

fn assert_extension_rejected(dirs: &CodexDirs, mode: &str, reason: &str) {
    let _daemon = start_codex(dirs, mode, 30);
    expect_extension_rejection(dirs, reason);
}

fn expect_extension_rejection(dirs: &CodexDirs, reason: &str) {
    let client = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &client);
    send_command(&mut stream, "session.create", json!({}));
    let rejected = command_reply(&mut reader);
    assert_eq!(rejected["kind"], "command_rejected");
    assert!(
        rejected["error"].as_str().unwrap().contains(reason),
        "{}",
        rejected["error"]
    );
    let trace = trace_messages(&dirs.trace);
    assert!(
        extension_indexes(&trace, "thread/start").is_empty(),
        "thread/start ran after a failed extension check: {trace:?}"
    );
    let events = journal_events(dirs.data.path());
    assert!(
        events.iter().all(|event| event.kind != "session_created"),
        "extension failure must not record a session"
    );
    let event = events
        .iter()
        .find(|event| event.kind == "codex_extensions_rejected")
        .expect("rejection was not journaled");
    assert_eq!(event.session_id, Uuid::nil());
    assert_eq!(event.payload["reason"], reason);
    assert!(event.payload["session_snapshot"].is_null());
    let rendered = event.payload.to_string();
    assert!(!rendered.contains("/.codex/"));
    assert!(!rendered.contains("/.agents/"));
    let audit = events
        .iter()
        .find(|event| event.kind == "session_create_audit")
        .expect("denial was not audited");
    assert_eq!(audit.payload["outcome"], "denied");
    assert_eq!(audit.payload["reason"], "extensions");
    assert!(
        events
            .iter()
            .all(|event| event.payload["outcome"] != "allowed")
    );
}

fn schema_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-app-server-0.160.0")
}

fn load_schema(name: &str) -> Value {
    let path = schema_root().join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {path:?}: {error}"))
}

fn assert_valid(name: &str, instance: &Value) {
    validate_schema(name, instance)
        .unwrap_or_else(|error| panic!("{name} rejected {instance}: {error}"));
}

fn validate_schema(name: &str, instance: &Value) -> Result<(), String> {
    let schema = load_schema(name);
    validate(&schema, instance, &schema, 0)
}

fn assert_extension_exchange(trace: &[Value]) {
    let mut methods = std::collections::HashMap::<String, String>::new();
    for entry in trace {
        let message = &entry["message"];
        let direction = entry["direction"].as_str().unwrap_or("");
        if direction == "in"
            && let (Some(method), Some(id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            )
        {
            methods.insert(id.to_string(), method.to_owned());
            match method {
                "skills/list" => assert_valid("v2/SkillsListParams.json", &message["params"]),
                "hooks/list" => assert_valid("v2/HooksListParams.json", &message["params"]),
                "plugin/installed" => {
                    assert_valid("v2/PluginInstalledParams.json", &message["params"]);
                }
                "app/list" => assert_valid("v2/AppsListParams.json", &message["params"]),
                "thread/start" => assert_valid("v2/ThreadStartParams.json", &message["params"]),
                _ => {}
            }
        }
        if direction == "out"
            && message.get("result").is_some()
            && let Some(method) = message
                .get("id")
                .and_then(|id| methods.get(&id.to_string()))
        {
            match method.as_str() {
                "skills/list" => assert_valid("v2/SkillsListResponse.json", &message["result"]),
                "hooks/list" => assert_valid("v2/HooksListResponse.json", &message["result"]),
                "plugin/installed" => {
                    assert_valid("v2/PluginInstalledResponse.json", &message["result"]);
                }
                "app/list" => assert_valid("v2/AppsListResponse.json", &message["result"]),
                _ => {}
            }
        }
    }
}

fn assert_codex_trace(trace: &[Value]) {
    let mut methods = std::collections::HashMap::<String, String>::new();
    for entry in trace {
        let message = &entry["message"];
        let direction = entry["direction"].as_str().unwrap_or("");
        if direction == "in" {
            if let (Some(method), Some(id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            ) {
                methods.insert(id.to_string(), method.to_owned());
                match method {
                    "initialize" => assert_valid("v1/InitializeParams.json", &message["params"]),
                    "skills/list" => assert_valid("v2/SkillsListParams.json", &message["params"]),
                    "hooks/list" => assert_valid("v2/HooksListParams.json", &message["params"]),
                    "plugin/installed" => {
                        assert_valid("v2/PluginInstalledParams.json", &message["params"]);
                    }
                    "app/list" => assert_valid("v2/AppsListParams.json", &message["params"]),
                    "thread/start" => assert_valid("v2/ThreadStartParams.json", &message["params"]),
                    "turn/start" => assert_valid("v2/TurnStartParams.json", &message["params"]),
                    "turn/interrupt" => {
                        assert_valid("v2/TurnInterruptParams.json", &message["params"]);
                    }
                    _ => {}
                }
            }
            if message
                .get("result")
                .and_then(|value| value.get("decision"))
                .is_some()
            {
                let result = &message["result"];
                let command =
                    validate_schema("CommandExecutionRequestApprovalResponse.json", result);
                let file = validate_schema("FileChangeRequestApprovalResponse.json", result);
                assert!(
                    command.is_ok() || file.is_ok(),
                    "approval result {result} command={command:?} file={file:?}"
                );
            }
        }
        if direction == "out" {
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                match method {
                    "item/agentMessage/delta" => {
                        assert_valid("v2/AgentMessageDeltaNotification.json", &message["params"]);
                    }
                    "turn/completed" => {
                        assert_valid("v2/TurnCompletedNotification.json", &message["params"]);
                    }
                    "item/commandExecution/requestApproval" => {
                        assert_valid(
                            "CommandExecutionRequestApprovalParams.json",
                            &message["params"],
                        );
                    }
                    "item/fileChange/requestApproval" => {
                        assert_valid("FileChangeRequestApprovalParams.json", &message["params"]);
                    }
                    _ => {}
                }
            }
            if message.get("result").is_some()
                && message.get("error").is_none()
                && let Some(method) = message
                    .get("id")
                    .and_then(|id| methods.get(&id.to_string()))
            {
                match method.as_str() {
                    "initialize" => {
                        assert_valid("v1/InitializeResponse.json", &message["result"]);
                    }
                    "skills/list" => assert_valid("v2/SkillsListResponse.json", &message["result"]),
                    "hooks/list" => assert_valid("v2/HooksListResponse.json", &message["result"]),
                    "plugin/installed" => {
                        assert_valid("v2/PluginInstalledResponse.json", &message["result"]);
                    }
                    "app/list" => assert_valid("v2/AppsListResponse.json", &message["result"]),
                    "thread/start" => {
                        assert_valid("v2/ThreadStartResponse.json", &message["result"]);
                    }
                    "turn/start" => assert_valid("v2/TurnStartResponse.json", &message["result"]),
                    "turn/interrupt" => {
                        assert_valid("v2/TurnInterruptResponse.json", &message["result"]);
                    }
                    _ => {}
                }
            }
        }
    }
    for method in ["initialize", "thread/start", "turn/start", "turn/interrupt"] {
        assert!(
            methods.values().any(|seen| seen == method),
            "trace never sent {method}"
        );
    }
}

fn validate(schema: &Value, instance: &Value, root: &Value, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("schema nesting is too deep".into());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return validate(resolve_ref(root, reference)?, instance, root, depth + 1);
    }
    if let Some(branches) = schema.get("allOf").and_then(Value::as_array) {
        for branch in branches {
            validate(branch, instance, root, depth + 1)?;
        }
    }
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array)
        && !branches
            .iter()
            .any(|branch| validate(branch, instance, root, depth + 1).is_ok())
    {
        return Err(format!("no anyOf branch matched {instance}"));
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| validate(branch, instance, root, depth + 1).is_ok())
            .count();
        if matches != 1 {
            return Err(format!("oneOf matched {matches} branches for {instance}"));
        }
    }
    if let Some(kinds) = schema.get("type")
        && !type_matches(kinds, instance)
    {
        return Err(format!("type {kinds} rejected {instance}"));
    }
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array)
        && !enum_values.iter().any(|item| item == instance)
    {
        return Err(format!("enum rejected {instance}"));
    }
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
        && instance
            .as_str()
            .is_some_and(|text| text.len() < min as usize)
    {
        return Err("minLength rejected the string".into());
    }
    if let Some(object) = instance.as_object() {
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in object {
                if let Some(property) = properties.get(key) {
                    validate(property, value, root, depth + 1)
                        .map_err(|error| format!("{key}: {error}"))?;
                } else {
                    match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            return Err(format!("unexpected property {key}"));
                        }
                        Some(extra) if extra.is_object() => {
                            validate(extra, value, root, depth + 1)?;
                        }
                        _ => {}
                    }
                }
            }
        } else if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && !object.is_empty()
        {
            return Err("unexpected properties".into());
        }
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required {
                let key = key.as_str().unwrap_or("");
                if !object.contains_key(key) {
                    return Err(format!("missing required property {key}"));
                }
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), instance.as_array()) {
        for value in values {
            validate(items, value, root, depth + 1)?;
        }
    }
    Ok(())
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Result<&'a Value, String> {
    let path = reference
        .strip_prefix("#/")
        .ok_or_else(|| format!("unsupported schema ref {reference}"))?;
    let mut cursor = root;
    for part in path.split('/') {
        let part = part.replace("~1", "/").replace("~0", "~");
        cursor = cursor
            .get(&part)
            .ok_or_else(|| format!("missing schema ref {reference}"))?;
    }
    Ok(cursor)
}

fn type_matches(kinds: &Value, instance: &Value) -> bool {
    let mut names = Vec::new();
    if let Some(list) = kinds.as_array() {
        names.extend(list.iter().filter_map(Value::as_str));
    } else if let Some(name) = kinds.as_str() {
        names.push(name);
    }
    names.into_iter().any(|kind| match kind {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "boolean" => instance.is_boolean(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "number" => instance.is_number(),
        "null" => instance.is_null(),
        _ => false,
    })
}

#[test]
fn codex_notices_before_admission_are_journaled_after_session_created() {
    // codex-cli 0.160.0 sends configWarning and remoteControl/status/changed right
    // after initialize, before the session exists in the journal.
    let dirs = codex_dirs();
    let _daemon = start_codex(&dirs, "happy", 30);
    let full = add_client(dirs.data.path(), "full");
    let (mut stream, mut reader) = handshake(dirs.data.path(), &full);
    send_command(&mut stream, "session.create", json!({}));
    assert_eq!(command_reply(&mut reader)["kind"], "command_accepted");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let events = loop {
        let events = journal_events(dirs.data.path());
        if events
            .iter()
            .any(|event| event.kind == "codex_extensions_verified")
            || std::time::Instant::now() > deadline
        {
            break events;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let position = |predicate: &dyn Fn(&shardrail_protocol::DurableEvent) -> bool| {
        events.iter().position(predicate)
    };
    let created = position(&|event| event.kind == "session_created").expect("session_created");
    let early = position(&|event| {
        event.kind == "provider_notice" && event.payload.to_string().contains("configWarning")
    })
    .expect("early provider notice journaled");
    let verified =
        position(&|event| event.kind == "codex_extensions_verified").expect("extensions verified");
    assert!(created < early, "early notice must follow session_created");
    assert!(early < verified, "held notices are flushed at admission");
    assert!(
        !events
            .iter()
            .any(|event| event.kind == "session_create_audit"
                && event.payload.to_string().contains("denied"))
    );
}
