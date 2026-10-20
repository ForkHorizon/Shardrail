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
            "initialize" => Some(reply(
                id,
                json!({"protocolVersion":1,"agentInfo":{"name":"shardrail-fake-acp","version":"0.1.0"},"capabilities":{"loadSession":false}}),
            )),
            "session/new" => {
                session_id = "fake-session-1";
                Some(reply(id, json!({"sessionId":session_id})))
            }
            "session/prompt" => {
                let notification = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"fake streamed response"}}}});
                writeln!(out, "{}", notification)?;
                let mut permission_request = json!({
                    "jsonrpc":"2.0",
                    "id":"permission-1",
                    "method":"session/request_permission",
                    "params":{
                        "sessionId":session_id,
                        "toolCall":{"toolCallId":"fake-tool-call","title":"Run fixture command","kind":"execute"},
                        "options":[
                            {"optionId":"allow_once","name":"Allow once","kind":"allow_once"},
                            {"optionId":"allow_always","name":"Allow always","kind":"allow_always"},
                            {"optionId":"allow_boundary","name":"Allow permission change","kind":"allow_boundary_breaking"},
                            {"optionId":"reject_once","name":"Reject once","kind":"reject_once"},
                            {"optionId":"grant_access","name":"Grant access","kind":"grant_access"},
                            {"optionId":"grant_sandbox_bypass","name":"Grant sandbox bypass","kind":"grant_sandbox_bypass"}
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
                writeln!(
                    out,
                    "{}",
                    json!({
                        "jsonrpc":"2.0",
                        "method":"session/update",
                        "params":{"sessionId":session_id,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("fake permission outcome: {outcome}")}}}
                    })
                )?;
                Some(reply(id, json!({"stopReason":"end_turn"})))
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
