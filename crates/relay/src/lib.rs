use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Copy, Hash, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionRole {
    Daemon,
    Browser,
}

impl ConnectionRole {
    pub fn from_value(value: &str) -> Result<Self, RelayError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "daemon" => Ok(Self::Daemon),
            "browser" => Ok(Self::Browser),
            _ => Err(RelayError::InvalidRole(value.to_owned())),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Browser => "browser",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionHello {
    #[serde(default = "default_hello_type")]
    pub r#type: String,
    pub role: String,
    pub machine_id: String,
    pub client_id: String,
    #[serde(default)]
    pub token: Option<String>,
}

fn default_hello_type() -> String {
    "hello".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayEnvelope {
    #[serde(default = "default_relay_type")]
    pub r#type: String,
    pub machine_id: String,
    pub client_id: String,
    #[serde(default)]
    pub target_client_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub iv: Option<String>,
    #[serde(default)]
    pub ciphertext: Option<String>,
}

fn default_relay_type() -> String {
    "relay".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayStatus {
    pub r#type: String,
    pub machine_id: String,
    pub online: bool,
    pub clients: usize,
}

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("invalid relay frame: {0}")]
    InvalidFrame(String),
    #[error("invalid connection role: {0}")]
    InvalidRole(String),
    #[error("invalid connection hello: {0}")]
    InvalidHello(String),
    #[error("unauthorized connection")]
    Unauthorized,
}

#[derive(Debug, Clone)]
struct RouteConnection {
    role: ConnectionRole,
    machine_id: String,
    #[allow(dead_code)]
    client_id: String,
    tx: UnboundedSender<Message>,
}

type RouteMap = Arc<Mutex<HashMap<(ConnectionRole, String, String), RouteConnection>>>;

#[derive(Debug, Clone)]
pub struct RelayRuntime {
    routes: RouteMap,
    status_tx: broadcast::Sender<RelayStatus>,
    auth_token: Option<String>,
}

impl RelayRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_auth_token(None)
    }

    #[must_use]
    pub fn with_auth_token(auth_token: Option<String>) -> Self {
        let (status_tx, _) = broadcast::channel(256);
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            status_tx,
            auth_token,
        }
    }

    pub fn subscribe_status(&self) -> broadcast::Receiver<RelayStatus> {
        self.status_tx.subscribe()
    }

    pub fn register_connection(
        &self,
        role: ConnectionRole,
        machine_id: &str,
        client_id: &str,
        tx: UnboundedSender<Message>,
    ) {
        let key = (role, machine_id.to_string(), client_id.to_string());
        let mut routes = self.routes.lock().unwrap();
        routes.insert(
            key,
            RouteConnection {
                role,
                machine_id: machine_id.to_string(),
                client_id: client_id.to_string(),
                tx,
            },
        );
        drop(routes);
        let status = self.status(machine_id);
        let _ = self.status_tx.send(status.clone());

        // When daemon connects, inform all connected browsers for this machine
        if role == ConnectionRole::Daemon {
            let status_msg =
                Message::Text(serde_json::to_string(&status).unwrap_or_default().into());
            let routes = self.routes.lock().unwrap();
            for ((r, m, _), conn) in routes.iter() {
                if *r == ConnectionRole::Browser && m == machine_id {
                    let _ = conn.tx.send(status_msg.clone());
                }
            }
        }
    }

    pub fn unregister_connection(&self, role: ConnectionRole, machine_id: &str, client_id: &str) {
        let key = (role, machine_id.to_string(), client_id.to_string());
        let mut routes = self.routes.lock().unwrap();
        routes.remove(&key);
        drop(routes);
        let status = self.status(machine_id);
        let _ = self.status_tx.send(status.clone());

        // When daemon disconnects, inform connected browsers that it is offline
        if role == ConnectionRole::Daemon {
            let status_msg =
                Message::Text(serde_json::to_string(&status).unwrap_or_default().into());
            let routes = self.routes.lock().unwrap();
            for ((r, m, _), conn) in routes.iter() {
                if *r == ConnectionRole::Browser && m == machine_id {
                    let _ = conn.tx.send(status_msg.clone());
                }
            }
        }
    }

    #[must_use]
    pub fn status(&self, machine_id: &str) -> RelayStatus {
        let routes = self.routes.lock().unwrap();
        let online = routes.values().any(|connection| {
            connection.machine_id == machine_id && connection.role == ConnectionRole::Daemon
        });
        let clients = routes
            .values()
            .filter(|connection| {
                connection.machine_id == machine_id && connection.role == ConnectionRole::Browser
            })
            .count();
        RelayStatus {
            r#type: "status".to_string(),
            online,
            machine_id: machine_id.to_string(),
            clients,
        }
    }

    pub fn route_message(
        &self,
        sender_role: ConnectionRole,
        machine_id: &str,
        message: Message,
    ) -> usize {
        let target_role = match sender_role {
            ConnectionRole::Daemon => ConnectionRole::Browser,
            ConnectionRole::Browser => ConnectionRole::Daemon,
        };

        let targets: Vec<RouteConnection> = {
            let routes = self.routes.lock().unwrap();
            routes
                .iter()
                .filter_map(|((role, m_id, _), conn)| {
                    if *role == target_role && m_id == machine_id {
                        Some(conn.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        let mut sent = 0usize;
        for target in targets {
            if target.tx.send(message.clone()).is_ok() {
                sent += 1;
            }
        }
        sent
    }

    pub fn revoke_clients(&self, machine_id: &str, target_client_id: Option<&str>) -> usize {
        let mut routes = self.routes.lock().unwrap();
        let keys_to_remove: Vec<(ConnectionRole, String, String)> = routes
            .keys()
            .filter(|(role, m, c)| {
                *role == ConnectionRole::Browser
                    && m == machine_id
                    && target_client_id.is_none_or(|target| c == target)
            })
            .cloned()
            .collect();

        let count = keys_to_remove.len();
        for key in keys_to_remove {
            routes.remove(&key);
        }
        drop(routes);

        let status = self.status(machine_id);
        let _ = self.status_tx.send(status);
        count
    }

    pub async fn handle_websocket(
        &self,
        stream: WebSocketStream<TcpStream>,
    ) -> Result<(), RelayError> {
        let (mut ws_write, mut ws_read) = stream.split();

        // Expect first message to be ConnectionHello
        let first_msg = match ws_read.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(RelayError::WebSocket(e)),
            None => {
                return Err(RelayError::InvalidHello(
                    "connection closed before hello".into(),
                ));
            }
        };

        let hello_str = match &first_msg {
            Message::Text(s) => s.as_str(),
            _ => return Err(RelayError::InvalidHello("expected text hello frame".into())),
        };

        let hello: ConnectionHello =
            serde_json::from_str(hello_str).map_err(|e| RelayError::InvalidHello(e.to_string()))?;

        let role = ConnectionRole::from_value(&hello.role)?;

        // Check authentication token if configured
        if let Some(expected_token) = &self.auth_token
            && role == ConnectionRole::Daemon
            && hello.token.as_deref() != Some(expected_token)
        {
            return Err(RelayError::Unauthorized);
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        self.register_connection(role, &hello.machine_id, &hello.client_id, tx);

        // If a browser connects, send initial status
        if role == ConnectionRole::Browser {
            let status = self.status(&hello.machine_id);
            let status_json = serde_json::to_string(&status).unwrap_or_default();
            ws_write.send(Message::Text(status_json.into())).await?;
        }

        let mut writer = ws_write;
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if writer.send(msg).await.is_err() {
                    break;
                }
            }
        });

        while let Some(item) = ws_read.next().await {
            match item {
                Ok(msg) => {
                    if msg.is_close() {
                        break;
                    }
                    if let Message::Text(ref text) = msg {
                        // Check for daemon control command (e.g. revoke_client)
                        if role == ConnectionRole::Daemon {
                            let is_revoke =
                                serde_json::from_str::<Value>(text).ok().and_then(|val| {
                                    if val.get("type").and_then(Value::as_str)
                                        == Some("control:revoke_client")
                                    {
                                        Some(
                                            val.get("client_id")
                                                .and_then(Value::as_str)
                                                .map(str::to_string),
                                        )
                                    } else {
                                        None
                                    }
                                });
                            if let Some(target) = is_revoke {
                                self.revoke_clients(&hello.machine_id, target.as_deref());
                                continue;
                            }
                        }
                    }
                    // Forward blindly to counterpart
                    self.route_message(role, &hello.machine_id, msg);
                }
                Err(e) => {
                    match e {
                        tokio_tungstenite::tungstenite::Error::ConnectionClosed
                        | tokio_tungstenite::tungstenite::Error::AlreadyClosed => {}
                        tokio_tungstenite::tungstenite::Error::Protocol(
                            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                        ) => {}
                        tokio_tungstenite::tungstenite::Error::Io(ref io_err)
                            if io_err.kind() == std::io::ErrorKind::ConnectionReset
                                || io_err.kind() == std::io::ErrorKind::UnexpectedEof => {}
                        _ => {
                            eprintln!("WebSocket read error: {e}");
                        }
                    }
                    break;
                }
            }
        }

        writer_task.abort();
        self.unregister_connection(role, &hello.machine_id, &hello.client_id);
        Ok(())
    }

    pub async fn serve(&self, listen: SocketAddr) -> Result<(), RelayError> {
        let listener = TcpListener::bind(listen).await?;
        println!("Shardrail Relay listening on ws://{listen}");
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let runtime = self.clone();
            tokio::spawn(async move {
                match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => {
                        if let Err(err) = runtime.handle_websocket(ws).await {
                            eprintln!("peer {peer_addr} relay session error: {err}");
                        }
                    }
                    Err(err) => {
                        eprintln!("peer {peer_addr} websocket handshake error: {err}");
                    }
                }
            });
        }
    }
}

impl Default for RelayRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_registration_sets_online_status() {
        let runtime = RelayRuntime::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        runtime.register_connection(ConnectionRole::Daemon, "m_123", "d_1", tx);
        let status = runtime.status("m_123");
        assert!(status.online);
        assert_eq!(status.machine_id, "m_123");
        assert_eq!(status.clients, 0);
    }

    #[test]
    fn browser_clients_count_toward_status() {
        let runtime = RelayRuntime::new();
        let (daemon_tx, _daemon_rx) = mpsc::unbounded_channel();
        let (browser_tx, _browser_rx) = mpsc::unbounded_channel();
        runtime.register_connection(ConnectionRole::Daemon, "m_123", "d_1", daemon_tx);
        runtime.register_connection(ConnectionRole::Browser, "m_123", "b_1", browser_tx);
        let status = runtime.status("m_123");
        assert!(status.online);
        assert_eq!(status.clients, 1);
    }

    #[test]
    fn unregister_updates_status() {
        let runtime = RelayRuntime::new();
        let (daemon_tx, _daemon_rx) = mpsc::unbounded_channel();
        runtime.register_connection(ConnectionRole::Daemon, "m_123", "d_1", daemon_tx);
        assert!(runtime.status("m_123").online);

        runtime.unregister_connection(ConnectionRole::Daemon, "m_123", "d_1");
        assert!(!runtime.status("m_123").online);
    }

    #[test]
    fn route_message_forwards_between_roles() {
        let runtime = RelayRuntime::new();
        let (daemon_tx, mut daemon_rx) = mpsc::unbounded_channel();
        let (browser_tx, mut browser_rx) = mpsc::unbounded_channel();
        runtime.register_connection(ConnectionRole::Daemon, "m_123", "d_1", daemon_tx);
        runtime.register_connection(ConnectionRole::Browser, "m_123", "b_1", browser_tx);

        let sent = runtime.route_message(
            ConnectionRole::Daemon,
            "m_123",
            Message::Text("encrypted_from_daemon".into()),
        );
        assert_eq!(sent, 1);
        match browser_rx.try_recv().unwrap() {
            Message::Text(txt) => assert_eq!(txt, "encrypted_from_daemon"),
            other => panic!("unexpected msg: {other:?}"),
        }
        assert!(daemon_rx.try_recv().is_err());

        let sent_back = runtime.route_message(
            ConnectionRole::Browser,
            "m_123",
            Message::Text("encrypted_from_browser".into()),
        );
        assert_eq!(sent_back, 1);
        match daemon_rx.try_recv().unwrap() {
            Message::Text(txt) => assert_eq!(txt, "encrypted_from_browser"),
            other => panic!("unexpected msg: {other:?}"),
        }
        assert!(browser_rx.try_recv().is_err());
    }

    #[test]
    fn revoke_clients_removes_browser_connections() {
        let runtime = RelayRuntime::new();
        let (browser_tx1, _rx1) = mpsc::unbounded_channel();
        let (browser_tx2, _rx2) = mpsc::unbounded_channel();
        runtime.register_connection(ConnectionRole::Browser, "m_123", "b_1", browser_tx1);
        runtime.register_connection(ConnectionRole::Browser, "m_123", "b_2", browser_tx2);
        assert_eq!(runtime.status("m_123").clients, 2);

        let removed = runtime.revoke_clients("m_123", Some("b_1"));
        assert_eq!(removed, 1);
        assert_eq!(runtime.status("m_123").clients, 1);

        let removed_all = runtime.revoke_clients("m_123", None);
        assert_eq!(removed_all, 1);
        assert_eq!(runtime.status("m_123").clients, 0);
    }
}
