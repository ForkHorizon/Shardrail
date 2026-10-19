use serde_json::{Value, json};
use shardrail_coordinator::{
    ApprovalResponseRequest, ApprovalTimeoutRequest, Coordinator, DaemonDataDir,
};
use shardrail_journal::Journal;
use shardrail_protocol::ApprovalDecision;
use shardrail_session_core::Approval;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::tempdir;
use uuid::Uuid;

#[derive(Debug)]
struct Agent {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Agent {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_shardrail-fake-acp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut agent = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        };
        for (id, method, params) in [
            (1, "initialize", json!({"protocolVersion":1})),
            (2, "session/new", json!({"cwd":"/tmp"})),
            (
                3,
                "session/prompt",
                json!({"sessionId":"fake-session-1","prompt":[]}),
            ),
        ] {
            writeln!(
                agent.input,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .unwrap();
            agent.input.flush().unwrap();
            if method != "session/prompt" {
                let reply = agent.read();
                assert_eq!(reply["id"], id);
            }
        }
        assert_eq!(agent.read()["method"], "session/update");
        let permission = agent.read();
        assert_eq!(permission["method"], "session/request_permission");
        assert_eq!(permission["id"], "permission-1");
        agent
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn send_permission(&mut self, option_id: &str) {
        writeln!(
            self.input,
            "{}",
            json!({
                "jsonrpc":"2.0",
                "id":"permission-1",
                "result":{"outcome":{"outcome":"selected","optionId":option_id}}
            })
        )
        .unwrap();
        self.input.flush().unwrap();
    }

    fn send_cancel(&mut self) {
        writeln!(self.input, "{}", json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"fake-session-1"}})).unwrap();
        self.input.flush().unwrap();
    }

    fn finish_prompt(mut self, expected_option: &str) {
        let outcome = self.read();
        assert_eq!(
            outcome["params"]["update"]["content"]["text"],
            format!(
                "fake permission outcome: {{\"optionId\":\"{expected_option}\",\"outcome\":\"selected\"}}"
            )
        );
        let reply = self.read();
        assert_eq!(reply["id"], 3);
        assert_eq!(reply["result"]["stopReason"], "end_turn");
        drop(self.input);
        assert!(self.child.wait().unwrap().success());
    }
}

fn open_coordinator(path: &std::path::Path) -> (Coordinator, Uuid) {
    let id = Uuid::now_v7();
    let mut journal = Journal::open(path).unwrap();
    Coordinator::recover_journal(&mut journal).unwrap();
    (Coordinator::open(journal, id).unwrap(), id)
}

#[test]
fn happy_path_commits_session_transition_and_approval_before_provider_effect() {
    let dir = tempdir().unwrap();
    let mut agent = Agent::start();
    let journal_path = dir.path().join("journal.db");
    let (mut coordinator, session_id) = open_coordinator(&journal_path);
    coordinator
        .transition(10, "turn_started", json!({"source":"fake-acp"}), || Ok(()))
        .unwrap();
    let mut approval = Approval::new(11);
    coordinator.create_approval(approval.clone()).unwrap();
    let approval_id = approval.id;
    coordinator
        .respond_approval(
            &mut approval,
            ApprovalResponseRequest {
                decision: ApprovalDecision::Allow,
                authorized: true,
                now_ms: 12,
                provider_limit_secs: None,
                option_id: Some("allow_once".into()),
                responder: "local-client",
                option_class: shardrail_protocol::OptionClass::Once,
                client_scope: shardrail_protocol::ClientScope::Full,
            },
            |decision, option| {
                let journal = Journal::open(&journal_path).unwrap();
                assert_eq!(
                    journal.approval_state(approval_id).unwrap().as_deref(),
                    Some("responding")
                );
                assert_eq!(decision, &ApprovalDecision::Allow);
                agent.send_permission(option.unwrap());
                Ok(())
            },
        )
        .unwrap();
    agent.finish_prompt("allow_once");
    coordinator.acknowledge_approval(approval_id).unwrap();
    let events = Journal::open(&journal_path)
        .unwrap()
        .events_for_session(session_id)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        [
            "turn_started",
            "approval_pending",
            "approval_responding",
            "approval_responded_unacked",
            "allowed"
        ]
    );
    assert_eq!(
        Journal::open(&journal_path)
            .unwrap()
            .approval_state(approval_id)
            .unwrap()
            .as_deref(),
        Some("allowed")
    );
}

#[test]
fn explicit_deny_is_written_to_fake_agent() {
    approval_path("deny_once", false);
}

#[test]
fn timeout_uses_injected_clock_and_sends_only_deny() {
    approval_path("reject_once", true);
}

fn approval_path(option_id: &str, timeout: bool) {
    let dir = tempdir().unwrap();
    let mut agent = Agent::start();
    let journal_path = dir.path().join("journal.db");
    let (mut coordinator, _) = open_coordinator(&journal_path);
    coordinator
        .transition(0, "turn_started", json!({}), || Ok(()))
        .unwrap();
    let mut approval = Approval::new(1_000);
    let approval_id = approval.id;
    coordinator.create_approval(approval.clone()).unwrap();
    let send = |decision: &ApprovalDecision, option: Option<&str>| {
        assert_eq!(decision, &ApprovalDecision::Deny);
        agent.send_permission(option.unwrap());
        Ok(())
    };
    if timeout {
        coordinator
            .timeout_approval(
                &mut approval,
                ApprovalTimeoutRequest {
                    now_ms: 181_000,
                    provider_limit_secs: None,
                    option_id: Some(option_id.into()),
                },
                send,
                || Ok(()),
            )
            .unwrap();
    } else {
        coordinator
            .respond_approval(
                &mut approval,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Deny,
                    authorized: true,
                    now_ms: 1_001,
                    provider_limit_secs: None,
                    option_id: Some(option_id.into()),
                    responder: "local-client",
                    option_class: shardrail_protocol::OptionClass::Once,
                    client_scope: shardrail_protocol::ClientScope::Full,
                },
                send,
            )
            .unwrap();
    }
    agent.finish_prompt(option_id);
    coordinator.acknowledge_approval(approval_id).unwrap();
    assert_eq!(
        coordinator.approval_state(approval_id).unwrap().as_deref(),
        Some(if timeout { "expired" } else { "denied" })
    );
}

#[test]
fn journal_failure_mid_session_rejects_pending_approval_and_cancels_turn() {
    let dir = tempdir().unwrap();
    let agent = std::sync::Arc::new(std::sync::Mutex::new(Agent::start()));
    let journal_path = dir.path().join("journal.db");
    let id = Uuid::now_v7();
    let journal = Journal::open(&journal_path).unwrap();
    let mut coordinator = Coordinator::open(journal, id).unwrap();
    coordinator
        .transition(0, "turn_started", json!({}), || Ok(()))
        .unwrap();
    let mut approval = Approval::new(1);
    coordinator.create_approval(approval.clone()).unwrap();
    let hook_agent = agent.clone();
    let approval_id = approval.id;
    coordinator.set_fail_closed_effect(move |plan| {
        assert!(plan.cancel_turn);
        assert_eq!(plan.pending_approvals, [approval_id]);
        let mut agent = hook_agent.lock().unwrap();
        agent.send_permission("reject_once");
        agent.send_cancel();
        Ok(())
    });
    coordinator.fail_approval_writes_for_test().unwrap();
    let called = std::cell::Cell::new(false);
    assert!(
        coordinator
            .respond_approval(
                &mut approval,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Allow,
                    authorized: true,
                    now_ms: 2,
                    provider_limit_secs: None,
                    option_id: Some("allow_once".into()),
                    responder: "local-client",
                    option_class: shardrail_protocol::OptionClass::Once,
                    client_scope: shardrail_protocol::ClientScope::Full,
                },
                |_, _| {
                    called.set(true);
                    Ok(())
                }
            )
            .is_err()
    );
    assert!(!called.get());
    assert!(!coordinator.journal_healthy());
    assert_eq!(
        coordinator.session().turn,
        shardrail_session_core::TurnState::Cancelling
    );
    assert_eq!(
        coordinator.approval_state(approval.id).unwrap().as_deref(),
        Some("pending")
    );
    drop(coordinator);
    let agent = std::sync::Arc::try_unwrap(agent)
        .unwrap()
        .into_inner()
        .unwrap();
    agent.finish_prompt("reject_once");
}

#[test]
fn crash_after_response_write_recovers_as_delivered_unconfirmed() {
    let dir = tempdir().unwrap();
    let mut agent = Agent::start();
    let journal_path = dir.path().join("journal.db");
    let (mut coordinator, _) = open_coordinator(&journal_path);
    coordinator
        .transition(0, "turn_started", json!({}), || Ok(()))
        .unwrap();
    let mut approval = Approval::new(1);
    coordinator.create_approval(approval.clone()).unwrap();
    let approval_id = approval.id;
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _result: Result<(), _> = coordinator.respond_approval(
            &mut approval,
            ApprovalResponseRequest {
                decision: ApprovalDecision::Allow,
                authorized: true,
                now_ms: 2,
                provider_limit_secs: None,
                option_id: Some("allow_once".into()),
                responder: "local-client",
                option_class: shardrail_protocol::OptionClass::Once,
                client_scope: shardrail_protocol::ClientScope::Full,
            },
            |_, option| {
                agent.send_permission(option.unwrap());
                panic!("simulated daemon crash before delivery confirmation");
            },
        );
    }));
    assert!(crash.is_err());
    drop(coordinator);
    let (recovered, _) = open_coordinator(&journal_path);
    assert_eq!(
        recovered.approval_state(approval_id).unwrap().as_deref(),
        Some("delivered_unconfirmed")
    );
    agent.child.kill().unwrap();
    agent.child.wait().unwrap();
}

#[test]
fn second_daemon_is_refused_and_crash_releases_advisory_lock() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let daemon = DaemonDataDir::open(&data_dir).unwrap();
    assert!(DaemonDataDir::open(&data_dir).is_err());
    drop(daemon);
    assert!(DaemonDataDir::open(&data_dir).is_ok());
}
