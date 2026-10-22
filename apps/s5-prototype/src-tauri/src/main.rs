mod stream_store;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use stream_store::{SharedStreamStore, StreamStore};
use tauri::{ipc::Channel, State};
use tauri::{Emitter, Webview};
use uuid::Uuid;

type RelayControl = Arc<Mutex<Option<UnixStream>>>;

struct RelayContext<'a> {
    webview: Webview,
    store: State<'a, SharedStreamStore>,
    relay: State<'a, RelayControl>,
}

#[tauri::command]
fn b4_trace(stage: String, data: Value) {
    if std::env::var("S5_BENCH_QUERY").is_ok_and(|query| b4_trace_enabled(&query)) {
        eprintln!("S5 B4 trace stage={stage} data={data}");
    }
}

fn data_dir() -> PathBuf {
    std::env::var_os("S5_SHARDRAIL_DATA_DIR")
        .or_else(|| std::env::var_os("SHARDRAIL_DATA_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir)
}

fn default_data_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Shardrail")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("shardrail")
    }
}

fn client_key_dir() -> PathBuf {
    std::env::var_os("S5_CLIENT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/Shardrail S5/client-keys")
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("shardrail-s5/client-keys")
            }
        })
}

fn client_key_path(id: &str) -> PathBuf {
    client_key_dir().join(format!("client-{id}.key"))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
fn transcript(
    client_nonce: &str,
    server_nonce: &str,
    client_key: &str,
    host_key: &str,
) -> Result<Vec<u8>, String> {
    let mut hash = Sha256::new();
    hash.update(b"Shardrail mutual client handshake v1\0");
    for version in [1u32, 1, 1] {
        hash.update(version.to_be_bytes());
    }
    for field in [client_nonce, server_nonce, client_key, host_key] {
        let bytes = decode_hex(field)?;
        hash.update((bytes.len() as u32).to_be_bytes());
        hash.update(bytes);
    }
    Ok(hash.finalize().to_vec())
}
fn read_json(reader: &mut BufReader<UnixStream>) -> Result<Value, String> {
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.is_empty() {
        return Err("daemon closed the socket".into());
    }
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
fn cli(daemon: &str, data: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new(daemon);
    command
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .map_err(|e| e.to_string())
}
fn pair_client(data: &Path) -> Result<(String, String), String> {
    let daemon = std::env::var("SHARDRAIL_DAEMON").unwrap_or_else(|_| "shardraild".into());
    let added = cli(
        &daemon,
        data,
        &["client", "add", "--label", "s5-ui-bench", "--scope", "full"],
    )?;
    if !added.status.success() {
        return Err(String::from_utf8_lossy(&added.stderr).into_owned());
    }
    let output = String::from_utf8_lossy(&added.stdout);
    let fields: std::collections::HashMap<_, _> = output
        .split_whitespace()
        .filter_map(|part| part.split_once('='))
        .collect();
    let id = fields
        .get("client_id")
        .ok_or("client registration returned no id")?
        .to_string();
    let host_key = fields
        .get("host_public_key")
        .ok_or("client registration returned no host key")?
        .to_string();
    Ok((id, host_key))
}

fn pairing_record(data: &Path) -> PathBuf {
    data.join("s5-pairing.json")
}

fn read_pairing_record(data: &Path) -> Result<Option<Value>, String> {
    match fs::read(pairing_record(data)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "invalid pairing record".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("could not read pairing record".into()),
    }
}

fn daemon_identity(data: &Path) -> Result<(String, String), String> {
    let daemon = std::env::var("SHARDRAIL_DAEMON").unwrap_or_else(|_| "shardraild".into());
    let output = cli(&daemon, data, &["identity"])?;
    if !output.status.success() {
        return Err("daemon identity unavailable".into());
    }
    let public_key = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|part| part.strip_prefix("host_public_key="))
        .ok_or("daemon identity unavailable")?
        .to_owned();
    if public_key.len() != 64 || decode_hex(&public_key).map_or(true, |key| key.len() != 32) {
        return Err("invalid daemon identity".into());
    }
    Ok((public_key.clone(), daemon_fingerprint(&public_key)?))
}

fn daemon_fingerprint(public_key: &str) -> Result<String, String> {
    let key = decode_hex(public_key)?;
    if key.len() != 32 {
        return Err("invalid daemon identity".into());
    }
    Ok(format!("SHA256:{}", encode_hex(&Sha256::digest(key))))
}

static PAIRING_LOCK: Mutex<()> = Mutex::new(());

#[tauri::command]
fn pairing_info() -> Result<Value, String> {
    let data = data_dir();
    let (daemon_public_key, fingerprint) = daemon_identity(&data)?;
    let record = read_pairing_record(&data)?;
    let stored_client_id = record.as_ref().and_then(|value| value["clientId"].as_str());
    let client_id = if let Some(stored_client_id) = stored_client_id {
        let daemon = std::env::var("SHARDRAIL_DAEMON").unwrap_or_else(|_| "shardraild".into());
        let output = cli(&daemon, &data, &["client", "list"])
            .map_err(|_| "daemon client registry unavailable")?;
        if !output.status.success() {
            return Err("daemon client registry unavailable".into());
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.split_whitespace().next() == Some(stored_client_id))
            .then_some(stored_client_id)
    } else {
        None
    };
    Ok(json!({
        "daemonPublicKey": daemon_public_key,
        "fingerprint": fingerprint,
        "clientId": client_id,
        "pinnedPublicKey": record.as_ref().and_then(|value| value["daemonPublicKey"].as_str())
    }))
}

fn register_pairing(data: &Path, expected_public_key: &str) -> Result<String, String> {
    let _pairing = PAIRING_LOCK.lock().map_err(|_| "pairing unavailable")?;
    let (daemon_public_key, _) = daemon_identity(data)?;
    if expected_public_key != daemon_public_key {
        return Err("daemon identity changed; review its fingerprint again".into());
    }
    let previous = read_pairing_record(data)?
        .and_then(|record| record["clientId"].as_str().map(str::to_owned));
    let daemon = std::env::var("SHARDRAIL_DAEMON").unwrap_or_else(|_| "shardraild".into());
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut seed))
        .map_err(|_| "could not create client key")?;
    let signing = SigningKey::from_bytes(&seed);
    seed.fill(0);
    let client_public_key = encode_hex(&signing.verifying_key().to_bytes());
    let output = cli(
        &daemon,
        data,
        &[
            "client",
            "add",
            "--label",
            "S5 desktop",
            "--scope",
            "full",
            "--public-key",
            &client_public_key,
        ],
    )
    .map_err(|_| "daemon client registration failed")?;
    if !output.status.success() {
        return Err("daemon client registration failed".into());
    }
    let fields: std::collections::HashMap<_, _> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let client_id = fields
        .get("client_id")
        .ok_or("daemon client registration failed")?
        .clone();
    if fields.get("host_public_key") != Some(&daemon_public_key) {
        let _ = cli(&daemon, data, &["client", "remove", &client_id]);
        return Err("daemon identity changed; review its fingerprint again".into());
    }
    let key_dir = client_key_dir();
    fs::create_dir_all(&key_dir).map_err(|_| "could not save client key")?;
    fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "could not save client key")?;
    let key_path = client_key_path(&client_id);
    let mut key_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key_path)
        .map_err(|_| "could not save client key")?;
    if key_file
        .write_all(&signing.to_bytes())
        .and_then(|_| key_file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&key_path);
        let _ = cli(&daemon, data, &["client", "remove", &client_id]);
        return Err("could not save client key".into());
    }
    if let Some(previous) = previous.as_ref().filter(|previous| *previous != &client_id) {
        if remove_client(&daemon, data, previous).is_err() {
            let _ = cli(&daemon, data, &["client", "remove", &client_id]);
            let _ = fs::remove_file(&key_path);
            return Err("could not replace the existing client registration".into());
        }
    }
    let record_path = pairing_record(data);
    let temporary = data.join(format!(".s5-pairing-{}.tmp", Uuid::new_v4()));
    let bytes =
        serde_json::to_vec(&json!({"clientId":client_id,"daemonPublicKey":daemon_public_key}))
            .map_err(|_| "could not save pairing")?;
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
    {
        Ok(file) => file,
        Err(_) => {
            let _ = cli(&daemon, data, &["client", "remove", &client_id]);
            let _ = fs::remove_file(&key_path);
            return Err("could not save pairing".into());
        }
    };
    if file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .is_err()
        || fs::rename(&temporary, &record_path).is_err()
    {
        let _ = fs::remove_file(temporary);
        let _ = cli(&daemon, data, &["client", "remove", &client_id]);
        let _ = fs::remove_file(&key_path);
        return Err("could not save pairing".into());
    }
    if let Some(previous) = previous.as_deref() {
        let _ = fs::remove_file(client_key_path(previous));
    }
    Ok(client_id)
}

#[tauri::command]
fn pair_desktop(expected_public_key: String) -> Result<String, String> {
    register_pairing(&data_dir(), &expected_public_key)
}

#[tauri::command]
fn unpair_desktop(relay: State<'_, RelayControl>) -> Result<(), String> {
    let _pairing = PAIRING_LOCK.lock().map_err(|_| "pairing unavailable")?;
    stop_relay(relay.inner())?;
    let data = data_dir();
    let Some(record) = read_pairing_record(&data)? else {
        return Ok(());
    };
    if let Some(client_id) = record["clientId"].as_str() {
        let daemon = std::env::var("SHARDRAIL_DAEMON").unwrap_or_else(|_| "shardraild".into());
        remove_client(&daemon, &data, client_id)?;
        let _ = fs::remove_file(client_key_path(client_id));
    }
    fs::remove_file(pairing_record(&data)).map_err(|_| "could not clear pairing record".into())
}

fn stop_relay(relay: &RelayControl) -> Result<(), String> {
    if let Some(socket) = relay.lock().map_err(|_| "relay unavailable")?.take() {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
    Ok(())
}

fn remove_client(daemon: &str, data: &Path, client_id: &str) -> Result<(), String> {
    let output = cli(daemon, data, &["client", "remove", client_id])
        .map_err(|_| "daemon client removal failed")?;
    if client_removal_succeeded(output.status.success(), &output.stderr) {
        Ok(())
    } else {
        Err("daemon client removal failed".into())
    }
}

fn client_removal_succeeded(success: bool, stderr: &[u8]) -> bool {
    success || String::from_utf8_lossy(stderr).contains("client key not found")
}
fn connect_socket(
    data: &Path,
    id: &str,
    host_expected: &str,
) -> Result<(Value, UnixStream), String> {
    let seed: [u8; 32] = fs::read(client_key_path(id))
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|_| "invalid client key")?;
    let signing = SigningKey::from_bytes(&seed);
    let client_public = encode_hex(&signing.verifying_key().to_bytes());
    let mut nonce_bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut nonce_bytes))
        .map_err(|e| e.to_string())?;
    let client_nonce = encode_hex(&nonce_bytes);
    let socket = UnixStream::connect(data.join("shardraild.sock")).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|e| e.to_string())?);
    let mut writer = socket;
    writeln!(writer, "{}", json!({"min_version":1,"max_version":1,"client_id":id,"public_key":client_public,"nonce":client_nonce,"resume_after":0,"journal_epoch":null})).map_err(|e| e.to_string())?;
    let hello = read_json(&mut reader)?;
    if hello["public_key"] != host_expected {
        return Err("daemon host key changed".into());
    }
    let digest = transcript(
        &client_nonce,
        hello["nonce"].as_str().ok_or("missing daemon nonce")?,
        &client_public,
        host_expected,
    )?;
    let host_key = VerifyingKey::from_bytes(
        &decode_hex(host_expected)?
            .try_into()
            .map_err(|_| "invalid daemon public key")?,
    )
    .map_err(|e| e.to_string())?;
    let signature = ed25519_dalek::Signature::from_slice(&decode_hex(
        hello["signature"]
            .as_str()
            .ok_or("missing daemon signature")?,
    )?)
    .map_err(|e| e.to_string())?;
    host_key
        .verify(&digest, &signature)
        .map_err(|e| e.to_string())?;
    let signature = signing.sign(&digest);
    writeln!(
        writer,
        "{}",
        json!({"kind":"authenticate","signature":encode_hex(&signature.to_bytes())})
    )
    .map_err(|e| e.to_string())?;
    if read_json(&mut reader)?["kind"] != "authenticated" {
        return Err("daemon authentication failed".into());
    }
    writer.set_read_timeout(None).map_err(|e| e.to_string())?;
    Ok((hello, writer))
}
fn dispatch(webview: &Webview, name: &str, payload: &Value) {
    if let Err(error) = webview.emit(name, payload) {
        eprintln!("S5 event send failed: {error}");
    }
}
fn request_sessions(writer: &mut UnixStream, streams: usize) -> Result<(), String> {
    for stream in 0..streams {
        writeln!(writer, "{}", json!({"kind":"request","request":{"command_id":Uuid::new_v4().to_string(),"kind":"session.create","expected_state":null,"expected_turn_id":null,"payload":{"s5_stream":stream}}})).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn is_b4_query(query: &str) -> bool {
    query.split('&').any(|part| part == "b4=1")
}

fn b4_trace_enabled(query: &str) -> bool {
    is_b4_query(query)
}

#[tauri::command]
fn start_b4_relay(
    webview: Webview,
    store: State<'_, SharedStreamStore>,
    relay: State<'_, RelayControl>,
    on_frames: Channel<Value>,
    streams: usize,
) -> Result<(), String> {
    eprintln!("S5 starting daemon relay");
    let data = data_dir();
    let (id, host_key) = pair_client(&data)?;
    eprintln!("S5 paired client {id}");
    start_relay(
        RelayContext {
            webview,
            store,
            relay,
        },
        on_frames,
        streams,
        id,
        host_key,
        true,
    )
}

#[tauri::command]
fn start_paired_relay(
    webview: Webview,
    store: State<'_, SharedStreamStore>,
    relay: State<'_, RelayControl>,
    on_frames: Channel<Value>,
    streams: usize,
) -> Result<(), String> {
    let data = data_dir();
    let record = read_pairing_record(&data)?.ok_or("client is not paired")?;
    let id = record["clientId"]
        .as_str()
        .ok_or("invalid pairing record")?;
    let (host_key, _) = daemon_identity(&data)?;
    if record["daemonPublicKey"] != host_key {
        return Err("daemon identity changed; re-pair to continue".into());
    }
    start_relay(
        RelayContext {
            webview,
            store,
            relay,
        },
        on_frames,
        streams,
        id.to_owned(),
        host_key,
        false,
    )
}

fn start_relay(
    context: RelayContext<'_>,
    on_frames: Channel<Value>,
    streams: usize,
    id: String,
    host_key: String,
    create_sessions: bool,
) -> Result<(), String> {
    let RelayContext {
        webview,
        store,
        relay,
    } = context;
    let data = data_dir();
    let (hello, mut writer) = connect_socket(&data, &id, &host_key)?;
    let control = writer.try_clone().map_err(|e| e.to_string())?;
    if let Some(previous) = relay
        .lock()
        .map_err(|_| "relay unavailable")?
        .replace(control)
    {
        let _ = previous.shutdown(std::net::Shutdown::Both);
    }
    eprintln!("S5 authenticated daemon socket");
    store
        .lock()
        .map_err(|e| e.to_string())?
        .set_channel(on_frames);
    dispatch(
        &webview,
        "daemon-status",
        &json!({"connected":true,"host":hello["host_id"],"epoch":hello["journal_epoch"]}),
    );
    let reader_stream = writer.try_clone().map_err(|e| e.to_string())?;
    let store = Arc::clone(store.inner());
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader_stream);
        let mut received = 0usize;
        let mut agent_chunks = 0usize;
        loop {
            let frame = match read_json(&mut reader) {
                Ok(frame) => frame,
                Err(error) => {
                    eprintln!("S5 daemon reader ended after {received} frames: {error}");
                    break;
                }
            };
            received += 1;
            if frame["kind"] == "durable_event" && frame["event"]["kind"] == "agent_message_chunk" {
                agent_chunks += 1;
            }
            if received.is_multiple_of(1_000) {
                eprintln!("S5 relay received frames={received} agent_chunks={agent_chunks}");
            }
            if frame["kind"] == "durable_event" {
                let mut store = match store.lock() {
                    Ok(store) => store,
                    Err(_) => break,
                };
                store.append(frame);
                if let Err(error) = store.notify() {
                    eprintln!("S5 channel delivery failed after {received} frames: {error}");
                    break;
                }
            }
        }
        dispatch(&webview, "daemon-status", &json!({"connected":false}));
    });
    if create_sessions {
        request_sessions(&mut writer, streams.clamp(1, 8))?;
        eprintln!("S5 session requests sent");
    }
    Ok(())
}

#[tauri::command]
fn get_stream_window(
    store: State<'_, SharedStreamStore>,
    start: usize,
    limit: usize,
    session_id: Option<String>,
) -> Result<Value, String> {
    store
        .lock()
        .map(|store| match session_id {
            Some(session_id) => store.session_window(&session_id, start, limit),
            None => store.window(start, limit),
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn acknowledge_stream_view(
    store: State<'_, SharedStreamStore>,
    version: u64,
) -> Result<(), String> {
    store
        .lock()
        .map_err(|e| e.to_string())?
        .acknowledge(version)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let process_started = Instant::now();
    let query = std::env::var("S5_BENCH_QUERY").ok();
    eprintln!("S5_BENCH_QUERY={query:?}");
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(StreamStore::default())))
        .manage(Arc::new(Mutex::new(None)) as RelayControl)
        .invoke_handler(tauri::generate_handler![
            b4_trace,
            pairing_info,
            pair_desktop,
            unpair_desktop,
            start_b4_relay,
            start_paired_relay,
            get_stream_window,
            acknowledge_stream_view
        ])
        .on_page_load(move |webview, payload| {
            eprintln!(
                "S5 page event {:?} elapsed_ms={} url={:?}",
                payload.event(),
                process_started.elapsed().as_secs_f64() * 1_000.0,
                webview.url()
            );
            eprintln!(
                "page load {:?} {}",
                payload.event(),
                webview.url().map(|url| url.to_string()).unwrap_or_default()
            );
            if let Some(query) = query.as_ref() {
                let desired = format!("?{query}");
                let query_matches = webview.url().ok().is_some_and(|url| {
                    url.query().map(|q| format!("?{q}")).as_deref() == Some(desired.as_str())
                });
                if !query_matches {
                    if payload.event() == tauri::webview::PageLoadEvent::Finished {
                        let _ = webview.eval(format!(
                            "if (location.search !== {desired:?}) location.search = {desired:?};"
                        ));
                    }
                    return;
                }
                if is_b4_query(query) && payload.event() == tauri::webview::PageLoadEvent::Started {
                    let window = webview.window();
                    let shown = window.show();
                    let focused = window.set_focus();
                    eprintln!("S5 B4 window activation show={shown:?} focus={focused:?}");
                }
                let _ = process_started;
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

fn main() {
    run();
}

#[cfg(test)]
mod tests {
    use super::{
        b4_trace_enabled, client_removal_succeeded, daemon_fingerprint, encode_hex,
        register_pairing, stop_relay, RelayControl,
    };
    use ed25519_dalek::SigningKey;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tracing_is_disabled_outside_b4() {
        assert!(b4_trace_enabled("events=5000&b4=1"));
        assert!(!b4_trace_enabled("events=5000"));
    }

    #[test]
    fn pairing_fingerprint_is_sha256_of_daemon_public_key() {
        let seed = [7u8; 32];
        let public_key = encode_hex(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
        let expected_key = encode_hex(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
        assert_eq!(public_key, expected_key);
        assert_eq!(daemon_fingerprint(&public_key).unwrap().len(), 71);
    }

    #[test]
    fn missing_client_registration_is_already_removed() {
        assert!(client_removal_succeeded(false, b"client key not found"));
        assert!(!client_removal_succeeded(false, b"permission denied"));
        assert!(client_removal_succeeded(true, b""));
    }

    #[test]
    fn stopping_relay_closes_the_reader_socket() {
        let (writer, mut reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let relay: RelayControl = std::sync::Arc::new(std::sync::Mutex::new(Some(writer)));
        stop_relay(&relay).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(std::io::Read::read(&mut reader, &mut byte).unwrap(), 0);
        assert!(relay.lock().unwrap().is_none());
    }

    #[test]
    fn concurrent_pair_requests_leave_one_registered_client() {
        let root = std::env::temp_dir().join(format!("s5-pair-race-{}", uuid::Uuid::new_v4()));
        let data = root.join("daemon");
        let keys = root.join("client-keys");
        std::fs::create_dir_all(&data).unwrap();
        let script = root.join("fake-daemon");
        std::fs::write(
            &script,
            r##"#!/bin/sh
shift 2
if [ "$1" = identity ]; then echo "host_public_key=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; exit 0; fi
command="$1"
if [ "$command" != client ]; then exit 2; fi
shift
action="$1"
shift
registry="$(dirname "$0")/registry"
case "$action" in
  add)
    id=1
    [ ! -f "$(dirname "$0")/counter" ] || id=$(( $(cat "$(dirname "$0")/counter") + 1 ))
    echo "$id" > "$(dirname "$0")/counter"
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --public-key ]; then key="$2"; shift 2; else shift; fi
    done
    echo "$id $key" >> "$registry"
    echo "client_id=$id public_key=$key scope=full host_public_key=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    ;;
  remove)
    id="$1"
    grep -q "^$id " "$registry" || { echo "client key not found" >&2; exit 1; }
    grep -v "^$id " "$registry" > "$registry.tmp"
    mv "$registry.tmp" "$registry"
    ;;
  list)
    [ ! -f "$registry" ] || while read -r id key; do echo "$id S5-desktop full $key"; done < "$registry"
    ;;
  *) exit 2 ;;
esac
"##,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        std::env::set_var("SHARDRAIL_DAEMON", &script);
        std::env::set_var("S5_CLIENT_DATA_DIR", &keys);

        let paths = [data.clone(), data.clone()];
        let joins = paths.map(|path| {
            std::thread::spawn(move || register_pairing(&path, &"a".repeat(64)).unwrap())
        });
        let ids = joins.map(|join| join.join().unwrap());
        let registrations = std::fs::read_to_string(root.join("registry")).unwrap();
        assert_eq!(registrations.lines().count(), 1);
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(data.join("s5-pairing.json")).unwrap()).unwrap();
        assert!(ids.iter().any(|id| record["clientId"] == *id));
        assert_eq!(std::fs::read_dir(&keys).unwrap().count(), 1);

        std::env::remove_var("SHARDRAIL_DAEMON");
        std::env::remove_var("S5_CLIENT_DATA_DIR");
        std::fs::remove_dir_all(root).unwrap();
    }
}
