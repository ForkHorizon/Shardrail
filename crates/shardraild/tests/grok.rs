use ed25519_dalek::{Signature, Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shardrail_journal::Journal;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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

fn fake_acp_binary() -> PathBuf {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/shardrail-fake-acp");
    assert!(
        path.exists(),
        "shardrail-fake-acp must be built before running grok tests"
    );
    path
}

fn add_client(path: &Path) -> TestClient {
    let result = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(path)
        .args(["client", "add", "--label", "test", "--scope", "full"])
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

fn start_grok_daemon(path: &Path, workdir: &Path, sandbox: &str, timeout_secs: u64) -> DaemonGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(path)
        .arg("--exit-with-stdin")
        .arg("--grok-agent-path")
        .arg(fake_acp_binary())
        .arg("--grok-workdir")
        .arg(workdir)
        .arg("--grok-sandbox")
        .arg(sandbox)
        .arg("--session-workdir")
        .arg(workdir)
        .arg("--approval-timeout-secs")
        .arg(timeout_secs.to_string())
        .arg("--no-file-log")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command.process_group(0);
    DaemonGuard(command.spawn().unwrap())
}

fn connect(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let socket = path.join("shardraild.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            let reader = BufReader::new(stream.try_clone().unwrap());
            return (stream, reader);
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for shardraild socket");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).unwrap();
    assert!(read > 0, "unexpected EOF from daemon");
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
    (stream, reader)
}

fn next_approval(reader: &mut BufReader<UnixStream>) -> Value {
    loop {
        let frame = read_json(reader);
        if frame["kind"] == "durable_event" && frame["event"]["kind"] == "approval_requested" {
            return frame["event"]["payload"].clone();
        }
    }
}

fn send_resolution(stream: &mut UnixStream, approval: &Value, option_id: &str) {
    let binding = &approval["binding"];
    let request = json!({
        "approval_id": approval["approval_id"],
        "option_id": option_id,
        "content_hash": binding["content_hash"],
        "session_id": binding["session_id"],
        "turn_id": binding["turn_id"],
        "provider_request_id": binding["provider_request_id"],
    });
    writeln!(
        stream,
        "{}",
        json!({"kind": "approval_resolution", "request": request})
    )
    .unwrap();
}

#[test]
fn grok_session_spawns_runs_checks_and_resolves_approval() {
    let dir = tempdir().unwrap();
    let workdir = tempdir().unwrap();
    let client = add_client(dir.path());
    let mut _daemon = start_grok_daemon(dir.path(), workdir.path(), "seatbelt", 30);

    let (mut stream, mut reader) = handshake(dir.path(), &client);

    // Request session creation
    writeln!(
        stream,
        "{}",
        json!({
            "kind": "request",
            "request": {
                "command_id": Uuid::now_v7(),
                "kind": "session.create",
                "expected_state": null,
                "expected_turn_id": null,
                "payload": {
                    "workdir": workdir.path().canonicalize().unwrap().to_str().unwrap()
                }
            }
        })
    )
    .unwrap();
    assert_eq!(read_json(&mut reader)["kind"], "command_accepted");

    // Verify durable journal records grok checklist items
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = Journal::open(dir.path().join("journal.sqlite3"))
            .unwrap()
            .events_after_with_receipts(0, 100)
            .unwrap();

        let has_sandbox_check = events.iter().any(|item| {
            item.event.kind == "session_check_result"
                && item.event.payload.get("item_id") == Some(&json!("sandbox"))
                && item.event.payload.get("status") == Some(&json!("verified"))
        });
        let has_bypass_check = events.iter().any(|item| {
            item.event.kind == "session_check_result"
                && item.event.payload.get("item_id") == Some(&json!("bypass"))
                && item.event.payload.get("status") == Some(&json!("verified"))
        });
        let has_routing_check = events.iter().any(|item| {
            item.event.kind == "session_check_result"
                && item.event.payload.get("item_id") == Some(&json!("routing"))
                && item.event.payload.get("status") == Some(&json!("verified"))
        });

        if has_sandbox_check && has_bypass_check && has_routing_check {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "grok session checks were not journaled in time"
        );
        thread::sleep(Duration::from_millis(30));
    }

    // Wait for approval requested from Grok ACP agent
    let approval = next_approval(&mut reader);
    let approval_id = approval["approval_id"].as_str().unwrap();

    // Client resolves approval with allow_once
    send_resolution(&mut stream, &approval, "allow_once");
    let res = read_json(&mut reader);
    assert_eq!(res["kind"], "approval_resolved", "response was: {res:?}");

    // Verify journal reaches terminal acknowledged state
    let ack_deadline = Instant::now() + Duration::from_secs(5);
    let parsed_approval_id = Uuid::parse_str(approval_id).unwrap();
    loop {
        let state = Journal::open(dir.path().join("journal.sqlite3"))
            .unwrap()
            .approval_state(parsed_approval_id)
            .unwrap();
        if state.as_deref() == Some("allowed") {
            break;
        }
        assert!(
            Instant::now() < ack_deadline,
            "grok approval was not acknowledged in journal"
        );
        thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn grok_workdir_must_be_allowlisted() {
    let dir = tempdir().unwrap();
    let workdir = tempdir().unwrap();

    // Run daemon with grok-workdir that is NOT allowlisted in session-workdir
    let output = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--exit-with-stdin")
        .arg("--grok-agent-path")
        .arg(fake_acp_binary())
        .arg("--grok-workdir")
        .arg(workdir.path())
        .arg("--no-file-log")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "daemon should fail when grok workdir is not allowlisted"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("grok-workdir")
            || stderr.contains("allowlist")
            || stderr.contains("session-workdir"),
        "stderr should mention grok workdir allowlist failure: {stderr}"
    );
}

#[test]
fn grok_cannot_coexist_with_fake_acp_or_codex() {
    let dir = tempdir().unwrap();
    let workdir = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--exit-with-stdin")
        .arg("--grok-agent-path")
        .arg(fake_acp_binary())
        .arg("--fake-acp-path")
        .arg(fake_acp_binary())
        .arg("--grok-workdir")
        .arg(workdir.path())
        .arg("--session-workdir")
        .arg(workdir.path())
        .arg("--no-file-log")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "daemon should fail when multiple provider flags are supplied"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("choose only one of"),
        "stderr should mention provider mutual exclusion: {stderr}"
    );
}
