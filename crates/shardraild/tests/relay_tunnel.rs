use ed25519_dalek::{Signature, Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shardrail_relay::RelayRuntime;
use std::ops::{Deref, DerefMut};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

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

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let group = format!("-{}", self.0.id());
        let _ = Command::new("kill")
            .args(["-KILL", &group])
            .stderr(Stdio::null())
            .status();
        let _ = self.0.wait();
    }
}

#[derive(Clone)]
struct TestClient {
    id: Uuid,
    public_key: String,
    key: SigningKey,
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

fn add_client(path: &Path) -> TestClient {
    let result = Command::new(env!("CARGO_BIN_EXE_shardraild"))
        .arg("--data-dir")
        .arg(path)
        .args(["client", "add", "--label", "test", "--scope", "full"])
        .output()
        .unwrap();
    assert!(result.status.success());
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

fn start_daemon_with_relay(
    path: &Path,
    relay_url: &str,
    machine_id: &str,
    auth_token: Option<&str>,
) -> DaemonGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shardraild"));
    command
        .arg("--data-dir")
        .arg(path)
        .arg("--exit-with-stdin")
        .arg("--no-file-log")
        .arg("--relay-url")
        .arg(relay_url)
        .arg("--relay-machine-id")
        .arg(machine_id);
    if let Some(token) = auth_token {
        command.arg("--relay-auth-token").arg(token);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command.process_group(0);
    DaemonGuard(command.spawn().unwrap())
}

#[tokio::test]
async fn daemon_dials_relay_and_tunnels_client_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let relay_url = format!("ws://{}", relay_addr);

    let relay = RelayRuntime::with_auth_token(Some("secret123".to_string()));

    let server_relay = relay.clone();
    let relay_handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let r = server_relay.clone();
            tokio::spawn(async move {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = r.handle_websocket(ws).await;
                }
            });
        }
    });

    let temp = tempdir().unwrap();
    let client = add_client(temp.path());

    let _daemon =
        start_daemon_with_relay(temp.path(), &relay_url, "machine_alpha", Some("secret123"));

    // Give daemon 500ms to dial outbound to the relay
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect browser to relay
    let (mut browser_ws, _) = connect_async(&relay_url).await.unwrap();

    // Browser registers hello
    let hello = json!({
        "type": "hello",
        "role": "browser",
        "machine_id": "machine_alpha",
        "client_id": "browser_client_1"
    });
    browser_ws
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();

    // Browser receives status: machine is online
    let status_msg = browser_ws.next().await.unwrap().unwrap();
    let status: Value = serde_json::from_str(&status_msg.into_text().unwrap()).unwrap();
    assert_eq!(status["type"], "status");
    assert_eq!(status["machine_id"], "machine_alpha");
    assert_eq!(status["online"], true);

    // Browser initiates protocol ClientHello through relay envelope
    let nonce = "11".repeat(32);
    let hs_req = json!({
        "min_version": 1,
        "max_version": 1,
        "client_id": client.id,
        "public_key": client.public_key,
        "nonce": nonce,
        "resume_after": 0,
        "journal_epoch": null
    });
    let envelope = json!({
        "type": "envelope",
        "machine_id": "machine_alpha",
        "client_id": "browser_client_1",
        "payload": hs_req.to_string()
    });
    browser_ws
        .send(Message::Text(envelope.to_string().into()))
        .await
        .unwrap();

    // Read response envelope from daemon (ServerHello)
    let resp_msg = tokio::time::timeout(Duration::from_secs(5), browser_ws.next())
        .await
        .expect("timed out waiting for response envelope")
        .unwrap()
        .unwrap();

    let resp_envelope: Value = serde_json::from_str(&resp_msg.into_text().unwrap()).unwrap();
    assert_eq!(resp_envelope["type"], "relay");
    assert_eq!(resp_envelope["machine_id"], "machine_alpha");
    assert_eq!(resp_envelope["client_id"], "browser_client_1");

    let server_hello = &resp_envelope["payload"];
    assert_eq!(server_hello["protocol_version"], 1);

    // Verify server signature
    let host_secret: [u8; 32] = std::fs::read(temp.path().join("host.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let host_key = SigningKey::from_bytes(&host_secret).verifying_key();
    assert_eq!(server_hello["public_key"], encode_hex(&host_key.to_bytes()));

    let signature =
        Signature::from_slice(&decode_hex(server_hello["signature"].as_str().unwrap())).unwrap();
    let transcript = transcript_hash(
        [1, 1, 1],
        &nonce,
        server_hello["nonce"].as_str().unwrap(),
        &client.public_key,
        server_hello["public_key"].as_str().unwrap(),
    );
    host_key.verify_strict(&transcript, &signature).unwrap();

    // Browser signs transcript and authenticates through envelope
    let client_signature = client.key.sign(&transcript);
    let auth_cmd = json!({
        "kind": "authenticate",
        "signature": encode_hex(&client_signature.to_bytes())
    });
    let env_auth = json!({
        "type": "relay",
        "machine_id": "machine_alpha",
        "client_id": "browser_client_1",
        "payload": auth_cmd
    });
    browser_ws
        .send(Message::Text(env_auth.to_string().into()))
        .await
        .unwrap();

    // Receive authenticated confirmation
    let auth_resp = tokio::time::timeout(Duration::from_secs(5), browser_ws.next())
        .await
        .expect("timed out waiting for authenticated response envelope")
        .unwrap()
        .unwrap();
    let ok_envelope: Value = serde_json::from_str(&auth_resp.into_text().unwrap()).unwrap();
    assert_eq!(ok_envelope["type"], "relay");
    let auth_ok = &ok_envelope["payload"];
    assert_eq!(auth_ok["kind"], "authenticated");

    // Send session.create command via relay envelope
    let cmd_id = Uuid::now_v7();
    let session_create_req = json!({
        "kind": "request",
        "request": {
            "command_id": cmd_id,
            "kind": "session.create",
            "expected_state": null,
            "expected_turn_id": null,
            "payload": {}
        }
    });
    let env_cmd = json!({
        "type": "relay",
        "machine_id": "machine_alpha",
        "client_id": "browser_client_1",
        "payload": session_create_req
    });
    browser_ws
        .send(Message::Text(env_cmd.to_string().into()))
        .await
        .unwrap();

    // Verify command_accepted envelope
    let cmd_resp = tokio::time::timeout(Duration::from_secs(5), browser_ws.next())
        .await
        .expect("timed out waiting for command_accepted envelope")
        .unwrap()
        .unwrap();
    let cmd_envelope: Value = serde_json::from_str(&cmd_resp.into_text().unwrap()).unwrap();
    assert_eq!(cmd_envelope["type"], "relay");
    assert_eq!(cmd_envelope["payload"]["kind"], "command_accepted");

    // Verify durable_event: session_created envelope
    let event_resp = tokio::time::timeout(Duration::from_secs(5), browser_ws.next())
        .await
        .expect("timed out waiting for session_created event envelope")
        .unwrap()
        .unwrap();
    let event_envelope: Value = serde_json::from_str(&event_resp.into_text().unwrap()).unwrap();
    assert_eq!(event_envelope["type"], "relay");
    assert_eq!(event_envelope["payload"]["kind"], "durable_event");
    assert_eq!(
        event_envelope["payload"]["event"]["kind"],
        "session_created"
    );

    relay_handle.abort();
}
