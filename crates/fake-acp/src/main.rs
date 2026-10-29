use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};

fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut session_id = "fake-session";
    let mut protocol_version: u64 = 1;

    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "session/cancel" {
            return Ok(());
        }
        let response = match method {
            "initialize" => {
                let forced_version = std::env::var("SHARDRAIL_FAKE_ACP_PROTOCOL_VERSION")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok());
                protocol_version = forced_version.unwrap_or(1);

                // D7 verification: verify client does not advertise fs or terminal capabilities
                if let Some(capabilities) =
                    request.get("params").and_then(|p| p.get("capabilities"))
                {
                    if (capabilities.get("fs").is_some() || capabilities.get("terminal").is_some())
                        && std::env::var_os("SHARDRAIL_FAKE_ACP_REJECT_CLIENT_CAPABILITIES")
                            .is_some()
                    {
                        Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32600,
                                "message": "Client declared forbidden fs or terminal capabilities"
                            }
                        }))
                    } else {
                        Some(reply(
                            id,
                            json!({
                                "protocolVersion": protocol_version,
                                "agentInfo": {
                                    "name": "shardrail-fake-acp",
                                    "version": "0.1.0"
                                },
                                "capabilities": {
                                    "loadSession": false
                                }
                            }),
                        ))
                    }
                } else {
                    Some(reply(
                        id,
                        json!({
                            "protocolVersion": protocol_version,
                            "agentInfo": {
                                "name": "shardrail-fake-acp",
                                "version": "0.1.0"
                            },
                            "capabilities": {
                                "loadSession": false
                            }
                        }),
                    ))
                }
            }
            "session/new" => {
                session_id = "fake-session-1";
                Some(reply(id, json!({"sessionId": session_id})))
            }
            "session/prompt" => {
                // If requested by test hook, emit an unknown agent request to verify client refusal (D7, 03 §2)
                if std::env::var_os("SHARDRAIL_FAKE_ACP_SEND_UNKNOWN_REQUEST").is_some() {
                    let unknown_id = json!("unknown-probe-1");
                    let unknown_req = json!({
                        "jsonrpc": "2.0",
                        "id": unknown_id,
                        "method": "workspace/unsupported_probe",
                        "params": {"path": "/etc/shadow"}
                    });
                    writeln!(out, "{}", unknown_req)?;
                    out.flush()?;

                    let mut resp_line = String::new();
                    input.read_line(&mut resp_line)?;
                    let resp: Value = serde_json::from_str(&resp_line)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    if resp.get("id") != Some(&unknown_id) || resp.get("error").is_none() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "client did not immediately refuse unknown server request",
                        ));
                    }
                }

                // In ACP v2, the prompt request is immediately acknowledged (D35)
                if protocol_version >= 2 {
                    let ack = reply(id.clone(), json!({"sessionId": session_id}));
                    writeln!(out, "{}", ack)?;
                    out.flush()?;
                }

                let count = std::env::var("SHARDRAIL_FAKE_ACP_STREAM_EVENTS")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1)
                    .min(50_000);
                for seq in 0..count {
                    let mut update_obj = json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {
                            "type": "text",
                            "text": format!("fake streamed response {seq}")
                        }
                    });
                    if protocol_version >= 2 {
                        update_obj["messageId"] = json!(format!("msg-{seq}"));
                    }
                    let notification = json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": update_obj
                        }
                    });
                    writeln!(out, "{}", notification)?;
                }
                let mut permission_request = json!({
                    "jsonrpc": "2.0",
                    "id": "permission-1",
                    "method": "session/request_permission",
                    "params": {
                        "sessionId": session_id,
                        "toolCall": {
                            "toolCallId": "fake-tool-call",
                            "title": "Run fixture command",
                            "kind": "execute"
                        },
                        "options": [
                            {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                            {"optionId": "allow_always", "name": "Allow always", "kind": "allow_always"},
                            {"optionId": "allow_boundary", "name": "Allow permission change", "kind": "allow_boundary_breaking"},
                            {"optionId": "reject_once", "name": "Reject once", "kind": "reject_once"},
                            {"optionId": "grant_access", "name": "Grant access", "kind": "grant_access"},
                            {"optionId": "grant_sandbox_bypass", "name": "Grant sandbox bypass", "kind": "grant_sandbox_bypass"}
                        ]
                    }
                });
                if std::env::var_os("SHARDRAIL_FAKE_ACP_NO_REJECT").is_some() {
                    permission_request["params"]["options"]
                        .as_array_mut()
                        .expect("fixture options are an array")
                        .retain(|option| option["kind"] != "reject_once");
                }
                writeln!(out, "{}", permission_request)?;
                out.flush()?;
                if std::env::var_os("SHARDRAIL_FAKE_ACP_DISCONNECT_AFTER_PERMISSION").is_some() {
                    return Ok(());
                }

                let mut response_line = String::new();
                input.read_line(&mut response_line)?;
                let permission_response: Value = serde_json::from_str(&response_line)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                if permission_response.get("method").and_then(Value::as_str)
                    == Some("session/cancel")
                {
                    return Ok(());
                }
                if permission_response.get("id") != Some(&json!("permission-1")) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "permission response id did not match request",
                    ));
                }
                let outcome = permission_response
                    .get("result")
                    .and_then(|result| result.get("outcome"))
                    .cloned()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "permission outcome missing")
                    })?;

                let mut outcome_obj = json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {
                        "type": "text",
                        "text": format!("fake permission outcome: {outcome}")
                    }
                });
                if protocol_version >= 2 {
                    outcome_obj["messageId"] = json!("msg-outcome");
                }
                writeln!(
                    out,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": outcome_obj
                        }
                    })
                )?;

                if protocol_version >= 2 {
                    // In ACP v2, turn finishes via state_update: idle notification (D35)
                    writeln!(
                        out,
                        "{}",
                        json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session_id,
                                "update": {
                                    "sessionUpdate": "state_update",
                                    "state": "idle",
                                    "stopReason": "end_turn"
                                }
                            }
                        })
                    )?;
                    None
                } else {
                    // In ACP v1, turn finishes via session/prompt response
                    Some(reply(id, json!({"stopReason": "end_turn"})))
                }
            }
            _ if id.is_null() => None,
            _ => Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}),
            ),
        };
        if let Some(response) = response {
            writeln!(out, "{}", response)?;
            out.flush()?;
        }
    }
    Ok(())
}
