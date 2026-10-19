use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn handles_initialize_new_prompt_and_stream_update() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_shardrail-fake-acp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let mut output = BufReader::new(output);

    for (id, method, params) in [
        (1, "initialize", serde_json::json!({"protocolVersion":1})),
        (2, "session/new", serde_json::json!({"cwd":"/tmp"})),
        (
            3,
            "session/prompt",
            serde_json::json!({"sessionId":"fake-session-1","prompt":[]}),
        ),
    ] {
        writeln!(
            input,
            "{}",
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        if method == "session/prompt" {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["method"],
                "session/update"
            );
            line.clear();
            output.read_line(&mut line).unwrap();
            let permission: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(permission["id"], "permission-1");
            assert_eq!(permission["method"], "session/request_permission");
            assert_eq!(permission["params"]["sessionId"], "fake-session-1");
            assert_eq!(
                permission["params"]["options"][1]["optionId"],
                "reject_once"
            );

            writeln!(
                input,
                "{}",
                serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":"permission-1",
                    "result":{"outcome":{"outcome":"selected","optionId":"reject_once"}}
                })
            )
            .unwrap();
            input.flush().unwrap();

            line.clear();
            output.read_line(&mut line).unwrap();
            let outcome_update: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                outcome_update["params"]["update"]["content"]["text"],
                "fake permission outcome: {\"optionId\":\"reject_once\",\"outcome\":\"selected\"}"
            );
            line.clear();
            output.read_line(&mut line).unwrap();
        }
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        assert!(reply.get("result").is_some());
    }
    drop(input);
    assert!(child.wait().unwrap().success());
}
