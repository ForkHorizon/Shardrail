mod stream_store;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
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
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join("Library/Application Support/Shardrail")
        })
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
fn connect_socket(
    data: &Path,
    id: &str,
    host_expected: &str,
) -> Result<(Value, UnixStream), String> {
    let seed: [u8; 32] = fs::read(data.join(format!("client-{id}.key")))
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
    on_frames: Channel<Value>,
    streams: usize,
) -> Result<(), String> {
    eprintln!("S5 starting daemon relay");
    let data = data_dir();
    let (id, host_key) = pair_client(&data)?;
    eprintln!("S5 paired client {id}");
    let (hello, mut writer) = connect_socket(&data, &id, &host_key)?;
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
    request_sessions(&mut writer, streams.clamp(1, 8))?;
    eprintln!("S5 session requests sent");
    Ok(())
}

#[tauri::command]
fn get_stream_window(
    store: State<'_, SharedStreamStore>,
    start: usize,
    limit: usize,
) -> Result<Value, String> {
    store
        .lock()
        .map(|store| store.window(start, limit))
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
        .invoke_handler(tauri::generate_handler![
            b4_trace,
            start_b4_relay,
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
    use super::b4_trace_enabled;

    #[test]
    fn tracing_is_disabled_outside_b4() {
        assert!(b4_trace_enabled("events=5000&b4=1"));
        assert!(!b4_trace_enabled("events=5000"));
    }
}
