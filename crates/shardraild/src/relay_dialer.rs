use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct RelayDialerOptions {
    pub relay_url: String,
    pub machine_id: String,
    pub auth_token: Option<String>,
    pub socket_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct OutboundHello {
    #[serde(rename = "type")]
    msg_type: String,
    role: String,
    machine_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    client_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct InboundRelayFrame {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    machine_id: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    iv: Option<String>,
    #[serde(default)]
    ciphertext: Option<String>,
}

type ClientTunnelMap = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>;

pub async fn run_relay_dialer(options: RelayDialerOptions, mut shutdown: watch::Receiver<()>) {
    let mut retry_delay = Duration::from_secs(1);
    const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

    loop {
        let mut conn_shutdown = shutdown.clone();
        tokio::select! {
            _ = shutdown.changed() => {
                crate::daemon_log::log("INFO", "relay", "relay dialer stopping due to shutdown");
                break;
            }
            res = dial_and_tunnel(&options, &mut conn_shutdown) => {
                match res {
                    Ok(()) => {
                        crate::daemon_log::log("INFO", "relay", "relay connection closed gracefully, reconnecting...");
                        retry_delay = Duration::from_secs(1);
                    }
                    Err(e) => {
                        crate::daemon_log::log("WARN", "relay", &format!("relay error: {e}, retrying in {retry_delay:?}"));
                    }
                }
            }
        }

        tokio::select! {
            _ = shutdown.changed() => {
                break;
            }
            _ = tokio::time::sleep(retry_delay) => {
                retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
            }
        }
    }
}

async fn dial_and_tunnel(
    options: &RelayDialerOptions,
    shutdown: &mut watch::Receiver<()>,
) -> Result<(), io::Error> {
    crate::daemon_log::log(
        "INFO",
        "relay",
        &format!(
            "connecting to relay at {} for machine {}",
            options.relay_url, options.machine_id
        ),
    );

    let (ws_stream, _) = connect_async(&options.relay_url)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // 1. Send Hello registration
    let hello = OutboundHello {
        msg_type: "hello".to_string(),
        role: "daemon".to_string(),
        machine_id: options.machine_id.clone(),
        token: options.auth_token.clone(),
        client_id: format!("daemon-{}", options.machine_id),
    };

    let hello_json = serde_json::to_string(&hello).map_err(io::Error::other)?;
    ws_write
        .send(Message::Text(hello_json.into()))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

    crate::daemon_log::log("INFO", "relay", "registered successfully with relay");

    // Channels for sending frames back to the relay
    let (tx_to_relay, mut rx_to_relay) = mpsc::channel::<Message>(128);

    // Active client tunnels
    let tunnels: ClientTunnelMap = Arc::new(Mutex::new(HashMap::new()));

    // Task to write outgoing messages to the WebSocket
    let mut ws_writer_task = tokio::spawn(async move {
        while let Some(msg) = rx_to_relay.recv().await {
            if ws_write.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut result = Ok(());

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                break;
            }
            _ = &mut ws_writer_task => {
                result = Err(io::Error::new(io::ErrorKind::BrokenPipe, "relay writer failed"));
                break;
            }
            incoming = ws_read.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_incoming_relay_text(
                            &text,
                            options,
                            &tunnels,
                            &tx_to_relay,
                        ).await;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = tx_to_relay.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        crate::daemon_log::log("INFO", "relay", "relay sent close frame");
                        break;
                    }
                    Some(Err(e)) => {
                        result = Err(io::Error::new(io::ErrorKind::ConnectionReset, e));
                        break;
                    }
                    None => {
                        crate::daemon_log::log("INFO", "relay", "relay stream reached EOF");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Clean up all client tunnels on disconnect
    let mut map = tunnels.lock().await;
    map.clear();

    result
}

async fn handle_incoming_relay_text(
    text: &str,
    options: &RelayDialerOptions,
    tunnels: &ClientTunnelMap,
    tx_to_relay: &mpsc::Sender<Message>,
) {
    let frame: InboundRelayFrame = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(_) => return,
    };

    let client_id = match frame.client_id {
        Some(ref cid) if !cid.is_empty() => cid.clone(),
        _ => return,
    };

    // Extract payload text
    let payload_str = if let Some(ref p) = frame.payload {
        match p {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    } else if let (Some(iv), Some(ciphertext)) = (frame.iv, frame.ciphertext) {
        // If it's an encrypted frame, serialize the relay envelope itself
        json!({
            "type": "relay",
            "machine_id": options.machine_id,
            "client_id": client_id,
            "iv": iv,
            "ciphertext": ciphertext
        })
        .to_string()
    } else {
        return;
    };

    // Get or spawn tunnel for this client
    let mut map = tunnels.lock().await;
    let sender = match map.get(&client_id) {
        Some(s) => s.clone(),
        None => {
            let (tx_client, rx_client) = mpsc::channel::<String>(64);
            map.insert(client_id.clone(), tx_client.clone());

            let socket_path = options.socket_path.clone();
            let machine_id = options.machine_id.clone();
            let c_id = client_id.clone();
            let relay_tx = tx_to_relay.clone();
            let tunnels_cleanup = Arc::clone(tunnels);

            tokio::spawn(async move {
                if let Err(e) = spawn_client_unix_tunnel(
                    socket_path,
                    machine_id,
                    c_id.clone(),
                    rx_client,
                    relay_tx,
                )
                .await
                {
                    crate::daemon_log::log(
                        "WARN",
                        "relay",
                        &format!("client {c_id} tunnel ended: {e}"),
                    );
                }
                let mut m = tunnels_cleanup.lock().await;
                m.remove(&c_id);
            });

            tx_client
        }
    };

    let _ = sender.send(payload_str).await;
}

async fn spawn_client_unix_tunnel(
    socket_path: PathBuf,
    machine_id: String,
    client_id: String,
    mut rx_from_relay: mpsc::Receiver<String>,
    tx_to_relay: mpsc::Sender<Message>,
) -> io::Result<()> {
    let unix = UnixStream::connect(&socket_path).await?;
    let (unix_read, mut unix_write) = unix.into_split();

    let write_task = async {
        while let Some(line) = rx_from_relay.recv().await {
            unix_write.write_all(line.as_bytes()).await?;
            unix_write.write_all(b"\n").await?;
            unix_write.flush().await?;
        }
        Ok::<(), io::Error>(())
    };

    let read_task = async {
        let mut lines = BufReader::new(unix_read).lines();
        while let Some(line) = lines.next_line().await? {
            let payload_val: Value =
                serde_json::from_str(&line).unwrap_or_else(|_| Value::String(line.clone()));
            let out_frame = json!({
                "type": "relay",
                "machine_id": machine_id,
                "client_id": client_id,
                "payload": payload_val
            });
            let out_text = serde_json::to_string(&out_frame).map_err(io::Error::other)?;
            if tx_to_relay
                .send(Message::Text(out_text.into()))
                .await
                .is_err()
            {
                break;
            }
        }
        Ok::<(), io::Error>(())
    };

    tokio::select! {
        r = write_task => r,
        r = read_task => r,
    }
}
