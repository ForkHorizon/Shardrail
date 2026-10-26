//! Binds an approval decision to the exact pending request.

use sha2::{Digest, Sha256};
use shardrail_protocol::{ApprovalBinding, ClientScope, OptionClass};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalContent {
    pub(crate) kind: String,
    /// Provider command text, not a whitespace split of that text.
    pub(crate) command: String,
    /// Argv the provider supplied. Empty when the provider sent command text only.
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) paths: Vec<String>,
    pub(crate) diff: String,
    /// Canonical JSON of the raw input or command actions shown on the card.
    pub(crate) raw_input: String,
    pub(crate) reason: String,
    /// Canonical JSON of the network context, including host, protocol, and port.
    pub(crate) network: String,
}

pub(crate) fn content_hash(content: &ApprovalContent) -> String {
    let mut paths = content.paths.clone();
    paths.sort();
    paths.dedup();
    let canonical = serde_json::json!({
        "argv": content.argv,
        "command": content.command,
        "cwd": content.cwd,
        "diff": content.diff,
        "kind": content.kind,
        "network": content.network,
        "paths": paths,
        "raw_input": content.raw_input,
        "reason": content.reason,
    });
    let bytes = serde_json::to_vec(&canonical).expect("canonical approval content serializes");
    encode_hex(&Sha256::digest(bytes))
}

/// Compact JSON with object keys sorted at every level. Array order is kept.
pub(crate) fn canonical_json(value: &serde_json::Value) -> String {
    serde_json::to_string(&sorted_json(value)).unwrap_or_default()
}

fn sorted_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), sorted_json(&map[key]));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sorted_json).collect())
        }
        other => other.clone(),
    }
}

pub(crate) fn provider_request_id(id: &serde_json::Value) -> String {
    match id {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Null => "null".to_owned(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "invalid".to_owned()),
    }
}

pub(crate) fn binding_mismatch(
    stored: &ApprovalBinding,
    session_id: Uuid,
    turn_id: &str,
    provider_request_id: &str,
    content_hash: &str,
    current_epoch: Uuid,
    current_turn: Option<&str>,
) -> Option<&'static str> {
    if content_hash != stored.content_hash {
        return Some("content_hash");
    }
    if session_id != stored.session_id {
        return Some("session");
    }
    if turn_id != stored.turn_id {
        return Some("turn");
    }
    if provider_request_id != stored.provider_request_id {
        return Some("provider_request");
    }
    if stored.journal_epoch != current_epoch {
        return Some("epoch");
    }
    if current_turn != Some(stored.turn_id.as_str()) {
        return Some("stale_turn");
    }
    None
}

pub(crate) fn decision_rejection(
    allow_approvals: bool,
    scope: ClientScope,
    option_id: &str,
    class: Option<OptionClass>,
) -> Option<&'static str> {
    if !allow_approvals {
        return Some("no_approval_rights");
    }
    if option_id == "acceptForSession" || option_id == "accept_for_session" {
        return Some("accept_for_session");
    }
    let Some(class) = class else {
        return Some("option_not_offered");
    };
    if scope == ClientScope::Limited && class != OptionClass::Once {
        return Some("limited_scope");
    }
    None
}

pub(crate) fn rejection_message(reason: &str) -> String {
    match reason {
        "limited_scope" => {
            "limited clients cannot choose widening or boundary-breaking options".to_owned()
        }
        "accept_for_session" => "acceptForSession is not offered".to_owned(),
        "no_approval_rights" => "client cannot decide approvals".to_owned(),
        "option_not_offered" => "option is not offered for this approval".to_owned(),
        "already_decided" | "unavailable" => "approval is already resolved".to_owned(),
        "expired" => "approval is expired".to_owned(),
        "abandoned" => "approval was abandoned".to_owned(),
        "stale_turn" => "approval is from an earlier turn".to_owned(),
        "unknown_approval" => "approval is already resolved or unavailable".to_owned(),
        "missing_binding" | "malformed_binding" => {
            "approval binding is missing or malformed".to_owned()
        }
        _ => "approval binding does not match".to_owned(),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ApprovalContent {
        ApprovalContent {
            kind: "command_execution".into(),
            command: String::new(),
            argv: vec!["python3".into(), "-m".into(), "unittest".into()],
            cwd: "/work".into(),
            paths: vec![],
            diff: String::new(),
            raw_input: String::new(),
            reason: "run tests".into(),
            network: String::new(),
        }
    }

    #[test]
    fn content_hash_is_sha256_of_the_canonical_json() {
        let raw = br#"{"argv":["python3","-m","unittest"],"command":"","cwd":"/work","diff":"","kind":"command_execution","network":"","paths":[],"raw_input":"","reason":"run tests"}"#;
        let expected = encode_hex(&Sha256::digest(raw));
        assert_eq!(content_hash(&sample()), expected);
        let mut changed = sample();
        changed.reason = "other".into();
        assert_ne!(content_hash(&changed), expected);
    }

    #[test]
    fn quoted_commands_and_network_endpoints_do_not_collide() {
        let mut spaced = sample();
        spaced.argv.clear();
        spaced.command = "echo \"a b\"".into();
        let mut wider = spaced.clone();
        wider.command = "echo \"a  b\"".into();
        assert_eq!(
            spaced.command.split_whitespace().collect::<Vec<_>>(),
            wider.command.split_whitespace().collect::<Vec<_>>()
        );
        assert_ne!(content_hash(&spaced), content_hash(&wider));

        let mut actions = spaced.clone();
        actions.command = "echo hi".into();
        actions.raw_input =
            canonical_json(&serde_json::json!([{"command":"echo hi","type":"unknown"}]));
        let mut other_actions = actions.clone();
        other_actions.raw_input =
            canonical_json(&serde_json::json!([{"type":"unknown","command":"echo hi "}]));
        assert_ne!(content_hash(&actions), content_hash(&other_actions));

        let https = canonical_json(
            &serde_json::json!({"protocol":"https","host":"example.com","port":443}),
        );
        let reordered = canonical_json(
            &serde_json::json!({"host":"example.com","port":443,"protocol":"https"}),
        );
        assert_eq!(https, reordered);
        let mut endpoint = sample();
        endpoint.network = https;
        let mut other_port = endpoint.clone();
        other_port.network =
            canonical_json(&serde_json::json!({"host":"example.com","port":80,"protocol":"https"}));
        let mut other_host = endpoint.clone();
        other_host.network = canonical_json(
            &serde_json::json!({"host":"example.org","port":443,"protocol":"https"}),
        );
        let mut no_port = endpoint.clone();
        no_port.network =
            canonical_json(&serde_json::json!({"host":"example.com","protocol":"https"}));
        assert_ne!(content_hash(&endpoint), content_hash(&other_port));
        assert_ne!(content_hash(&endpoint), content_hash(&other_host));
        assert_ne!(content_hash(&endpoint), content_hash(&no_port));
        assert_ne!(content_hash(&endpoint), content_hash(&sample()));
    }

    #[test]
    fn binding_rejects_each_mismatched_field_and_a_moved_turn() {
        let stored = ApprovalBinding {
            approval_id: Uuid::nil(),
            session_id: Uuid::from_u128(1),
            turn_id: "turn-1".into(),
            provider_request_id: "11".into(),
            content_hash: "abc".into(),
            journal_epoch: Uuid::from_u128(2),
        };
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-1",
                "11",
                "nope",
                stored.journal_epoch,
                Some("turn-1"),
            ),
            Some("content_hash")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                Uuid::from_u128(9),
                "turn-1",
                "11",
                "abc",
                stored.journal_epoch,
                Some("turn-1"),
            ),
            Some("session")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-0",
                "11",
                "abc",
                stored.journal_epoch,
                Some("turn-1"),
            ),
            Some("turn")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-1",
                "10",
                "abc",
                stored.journal_epoch,
                Some("turn-1"),
            ),
            Some("provider_request")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-1",
                "11",
                "abc",
                Uuid::from_u128(3),
                Some("turn-1"),
            ),
            Some("epoch")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-1",
                "11",
                "abc",
                stored.journal_epoch,
                Some("turn-2"),
            ),
            Some("stale_turn")
        );
        assert_eq!(
            binding_mismatch(
                &stored,
                stored.session_id,
                "turn-1",
                "11",
                "abc",
                stored.journal_epoch,
                Some("turn-1"),
            ),
            None
        );
    }

    #[test]
    fn scope_rejects_observers_session_allows_and_limited_widening() {
        assert_eq!(
            decision_rejection(false, ClientScope::Full, "accept", Some(OptionClass::Once)),
            Some("no_approval_rights")
        );
        assert_eq!(
            decision_rejection(
                true,
                ClientScope::Full,
                "acceptForSession",
                Some(OptionClass::Widening)
            ),
            Some("accept_for_session")
        );
        assert_eq!(
            decision_rejection(
                true,
                ClientScope::Limited,
                "allow_always",
                Some(OptionClass::Widening)
            ),
            Some("limited_scope")
        );
        assert_eq!(
            decision_rejection(
                true,
                ClientScope::Limited,
                "decline",
                Some(OptionClass::Once)
            ),
            None
        );
        assert_eq!(
            decision_rejection(true, ClientScope::Full, "missing", None),
            Some("option_not_offered")
        );
    }
}
