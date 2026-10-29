use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use shardrail_relay::RelayRuntime;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn test_full_relay_websocket_flow() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let runtime = RelayRuntime::new();

    let server_runtime = runtime.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let r = server_runtime.clone();
            tokio::spawn(async move {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = r.handle_websocket(ws).await;
                }
            });
        }
    });

    let ws_url = format!("ws://{addr}");

    // 1. Browser connects first
    let (mut browser_ws, _) = connect_async(&ws_url).await.unwrap();
    let browser_hello = json!({
        "type": "hello",
        "role": "browser",
        "machine_id": "m_test_machine",
        "client_id": "b_client_1"
    });
    browser_ws
        .send(Message::Text(browser_hello.to_string().into()))
        .await
        .unwrap();

    // Browser receives status: daemon is offline
    let status_frame = browser_ws.next().await.unwrap().unwrap();
    let status_val: serde_json::Value =
        serde_json::from_str(&status_frame.into_text().unwrap()).unwrap();
    assert_eq!(status_val["type"], "status");
    assert_eq!(status_val["online"], false);
    assert_eq!(status_val["machine_id"], "m_test_machine");
    assert_eq!(status_val["clients"], 1);

    // 2. Daemon connects
    let (mut daemon_ws, _) = connect_async(&ws_url).await.unwrap();
    let daemon_hello = json!({
        "type": "hello",
        "role": "daemon",
        "machine_id": "m_test_machine",
        "client_id": "d_daemon_1"
    });
    daemon_ws
        .send(Message::Text(daemon_hello.to_string().into()))
        .await
        .unwrap();

    // Browser receives status update: daemon is online
    let online_frame = browser_ws.next().await.unwrap().unwrap();
    let online_val: serde_json::Value =
        serde_json::from_str(&online_frame.into_text().unwrap()).unwrap();
    assert_eq!(online_val["type"], "status");
    assert_eq!(online_val["online"], true);

    // 3. Daemon sends encrypted payload frame
    let encrypted_event = json!({
        "type": "relay",
        "machine_id": "m_test_machine",
        "client_id": "d_daemon_1",
        "iv": "base64_iv_123",
        "ciphertext": "base64_encrypted_bytes_xyz"
    });
    daemon_ws
        .send(Message::Text(encrypted_event.to_string().into()))
        .await
        .unwrap();

    // Browser receives the encrypted event
    let event_frame = browser_ws.next().await.unwrap().unwrap();
    let event_val: serde_json::Value =
        serde_json::from_str(&event_frame.into_text().unwrap()).unwrap();
    assert_eq!(event_val["ciphertext"], "base64_encrypted_bytes_xyz");
    assert_eq!(event_val["iv"], "base64_iv_123");

    // 4. Browser sends reply frame
    let client_reply = json!({
        "type": "relay",
        "machine_id": "m_test_machine",
        "client_id": "b_client_1",
        "iv": "base64_iv_456",
        "ciphertext": "base64_client_reply_abc"
    });
    browser_ws
        .send(Message::Text(client_reply.to_string().into()))
        .await
        .unwrap();

    // Daemon receives client reply
    let reply_frame = daemon_ws.next().await.unwrap().unwrap();
    let reply_val: serde_json::Value =
        serde_json::from_str(&reply_frame.into_text().unwrap()).unwrap();
    assert_eq!(reply_val["ciphertext"], "base64_client_reply_abc");

    // 5. Daemon disconnects -> browser receives offline status
    drop(daemon_ws);
    let offline_frame = browser_ws.next().await.unwrap().unwrap();
    let offline_val: serde_json::Value =
        serde_json::from_str(&offline_frame.into_text().unwrap()).unwrap();
    assert_eq!(offline_val["type"], "status");
    assert_eq!(offline_val["online"], false);
}

#[tokio::test]
async fn test_relay_revocation_control() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let runtime = RelayRuntime::new();

    let server_runtime = runtime.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let r = server_runtime.clone();
            tokio::spawn(async move {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = r.handle_websocket(ws).await;
                }
            });
        }
    });

    let ws_url = format!("ws://{addr}");

    // Connect browser
    let (mut browser_ws, _) = connect_async(&ws_url).await.unwrap();
    browser_ws
        .send(Message::Text(
            json!({
                "type": "hello",
                "role": "browser",
                "machine_id": "m_rev_machine",
                "client_id": "b_rev_client"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let _ = browser_ws.next().await.unwrap().unwrap(); // initial status

    // Connect daemon
    let (mut daemon_ws, _) = connect_async(&ws_url).await.unwrap();
    daemon_ws
        .send(Message::Text(
            json!({
                "type": "hello",
                "role": "daemon",
                "machine_id": "m_rev_machine",
                "client_id": "d_rev_daemon"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    // Allow daemon hello to be processed
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify online in runtime
    assert!(runtime.status("m_rev_machine").online);
    assert_eq!(runtime.status("m_rev_machine").clients, 1);

    // Daemon revokes the client
    daemon_ws
        .send(Message::Text(
            json!({
                "type": "control:revoke_client",
                "machine_id": "m_rev_machine",
                "client_id": "b_rev_client"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    // Wait a tick for revocation to process
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    assert_eq!(runtime.status("m_rev_machine").clients, 0);
}

#[tokio::test]
async fn test_relay_machine_discovery() {
    let runtime = RelayRuntime::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}");

    let rt = runtime.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let rt_conn = rt.clone();
            tokio::spawn(async move {
                let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let _ = rt_conn.handle_websocket(ws).await;
            });
        }
    });

    // 1. Daemon connects with machine "machine-alpha"
    let (mut daemon_ws, _) = connect_async(&ws_url).await.unwrap();
    daemon_ws
        .send(Message::Text(
            json!({
                "type": "hello",
                "role": "daemon",
                "machine_id": "machine-alpha",
                "client_id": "daemon-1"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // 2. Browser connects for discovery (empty machine_id)
    let (mut browser_ws, _) = connect_async(&ws_url).await.unwrap();
    browser_ws
        .send(Message::Text(
            json!({
                "type": "hello",
                "role": "browser",
                "machine_id": "",
                "client_id": "browser-disc"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let disc_frame = browser_ws.next().await.unwrap().unwrap();
    let disc_val: serde_json::Value =
        serde_json::from_str(&disc_frame.into_text().unwrap()).unwrap();
    assert_eq!(disc_val["type"], "machines");
    let machines = disc_val["machines"].as_array().unwrap();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0]["machine_id"], "machine-alpha");
    assert_eq!(machines[0]["online"], true);

    // 3. Browser asks for machines dynamically
    browser_ws
        .send(Message::Text(
            json!({ "type": "list_machines" }).to_string().into(),
        ))
        .await
        .unwrap();

    let list_frame = browser_ws.next().await.unwrap().unwrap();
    let list_val: serde_json::Value =
        serde_json::from_str(&list_frame.into_text().unwrap()).unwrap();
    assert_eq!(list_val["type"], "machines");
    let machines_req = list_val["machines"].as_array().unwrap();
    assert_eq!(machines_req.len(), 1);
    assert_eq!(machines_req[0]["machine_id"], "machine-alpha");
}
