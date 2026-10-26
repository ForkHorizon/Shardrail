//! SHA-256 of the effective Codex approval config.
//!
//! The hash input is only the canonical object below. MCP `env`, headers,
//! tokens, API keys, commands, and other secret-bearing fields are not copied.
//!
//! Canonical groups, always present, with sorted object keys and explicit nulls:
//! - `approval_policy` — string or granular bools, else null
//! - `approvals_reviewer` — string or null
//! - `sandbox` — `mode`, `network_access`, `exclude_slash_tmp`, and
//!   `exclude_tmpdir_env_var`
//! - `workspace_roots` — sorted `sandbox_workspace_write.writable_roots`
//! - `model` and `model_provider`
//! - `extensions` — apps, connectors, hooks, and plugins as name → enabled;
//!   skills as `bundled_enabled`, `include_instructions`, and `{name, enabled}`
//! - `mcp_servers` — name → `{enabled}` only
//! - `features` — name → bool
//! - `web_search` — `disabled`, `cached`, `indexed`, `live`, or null
//!
//! `config/read` is the source. `mcpServerStatus/list` is not called.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const INVALID: &str = "config/read response is invalid";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) hash: String,
    pub(crate) canonical: String,
}

pub(crate) fn snapshot_config(result: &Value) -> Result<Snapshot, &'static str> {
    let config = result
        .get("config")
        .and_then(Value::as_object)
        .ok_or(INVALID)?;
    let canonical = canonical_config(config)?;
    let canonical = crate::approval_binding::canonical_json(&canonical);
    let hash = encode_hex(&Sha256::digest(canonical.as_bytes()));
    Ok(Snapshot { hash, canonical })
}

/// Group names whose canonical values differ.
///
/// Empty only when the digests match. A digest mismatch that does not land on
/// a named group is `unknown`, so an empty result is never a mismatch.
pub(crate) fn changed_groups(before: &Snapshot, after: &Snapshot) -> Vec<&'static str> {
    if before.hash == after.hash {
        return Vec::new();
    }
    let left: Value = serde_json::from_str(&before.canonical).unwrap_or(Value::Null);
    let right: Value = serde_json::from_str(&after.canonical).unwrap_or(Value::Null);
    let mut changed: Vec<&'static str> = GROUPS
        .into_iter()
        .filter(|group| left.get(*group) != right.get(*group))
        .collect();
    if changed.is_empty() {
        changed.push("unknown");
    }
    changed
}

const GROUPS: [&str; 10] = [
    "approval_policy",
    "approvals_reviewer",
    "extensions",
    "features",
    "mcp_servers",
    "model",
    "model_provider",
    "sandbox",
    "web_search",
    "workspace_roots",
];

fn canonical_config(config: &Map<String, Value>) -> Result<Value, &'static str> {
    let sandbox_write = optional_object(config, "sandbox_workspace_write")?;
    let network = sandbox_bool(sandbox_write, "network_access")?;
    let exclude_slash_tmp = sandbox_bool(sandbox_write, "exclude_slash_tmp")?;
    let exclude_tmpdir_env_var = sandbox_bool(sandbox_write, "exclude_tmpdir_env_var")?;
    let roots = workspace_roots(sandbox_write)?;
    Ok(json!({
        "approval_policy": approval_policy(config.get("approval_policy"))?,
        "approvals_reviewer": short_string(config.get("approvals_reviewer"), 64)?,
        "extensions": {
            "apps": enabled_flags(optional_object(config, "apps")?, true)?,
            "connectors": enabled_flags(optional_object(config, "connectors")?, false)?,
            "hooks": hook_flags(config.get("hooks"))?,
            "plugins": enabled_flags(optional_object(config, "plugins")?, false)?,
            "skills": skill_flags(config.get("skills"))?,
        },
        "features": feature_flags(config.get("features"))?,
        "mcp_servers": mcp_servers(config)?,
        "model": short_string(config.get("model"), 128)?,
        "model_provider": short_string(config.get("model_provider"), 128)?,
        "sandbox": {
            "exclude_slash_tmp": exclude_slash_tmp,
            "exclude_tmpdir_env_var": exclude_tmpdir_env_var,
            "mode": short_string(config.get("sandbox_mode"), 64)?,
            "network_access": network,
        },
        "web_search": web_search(config.get("web_search"))?,
        "workspace_roots": roots,
    }))
}

fn approval_policy(value: Option<&Value>) -> Result<Value, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) => {
            if !safe_token(text, 64) {
                return Err(INVALID);
            }
            Ok(Value::String(text.clone()))
        }
        Some(Value::Object(object)) => {
            let granular = object
                .get("granular")
                .and_then(Value::as_object)
                .ok_or(INVALID)?;
            Ok(json!({
                "granular": {
                    "mcp_elicitations": required_bool(granular, "mcp_elicitations")?,
                    "request_permissions": optional_bool(granular, "request_permissions")?,
                    "rules": required_bool(granular, "rules")?,
                    "sandbox_approval": required_bool(granular, "sandbox_approval")?,
                    "skill_approval": optional_bool(granular, "skill_approval")?,
                }
            }))
        }
        Some(_) => Err(INVALID),
    }
}

fn web_search(value: Option<&Value>) -> Result<Value, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text))
            if matches!(text.as_str(), "disabled" | "cached" | "indexed" | "live") =>
        {
            Ok(Value::String(text.clone()))
        }
        Some(_) => Err(INVALID),
    }
}

fn feature_flags(value: Option<&Value>) -> Result<Value, &'static str> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    if value.is_null() {
        return Ok(Value::Null);
    }
    let map = value.as_object().ok_or(INVALID)?;
    let mut flags = Map::new();
    for (name, flag) in map {
        if !safe_token(name, 64) {
            return Err(INVALID);
        }
        let Some(flag) = flag.as_bool() else {
            return Err(INVALID);
        };
        flags.insert(name.clone(), json!(flag));
    }
    Ok(Value::Object(flags))
}

fn mcp_servers(config: &Map<String, Value>) -> Result<Value, &'static str> {
    let servers = config
        .get("mcp_servers")
        .and_then(Value::as_object)
        .ok_or(INVALID)?;
    let mut canonical = Map::new();
    for (name, server) in servers {
        if !safe_name(name) {
            return Err(INVALID);
        }
        let enabled = server
            .as_object()
            .and_then(|entry| entry.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or(INVALID)?;
        canonical.insert(name.clone(), json!({"enabled": enabled}));
    }
    Ok(Value::Object(canonical))
}

fn enabled_flags(
    map: Option<&Map<String, Value>>,
    skip_default: bool,
) -> Result<Value, &'static str> {
    let Some(map) = map else {
        return Ok(Value::Null);
    };
    let mut flags = Map::new();
    for (name, entry) in map {
        if skip_default
            && name == "_default"
            && entry.get("enabled").and_then(Value::as_bool).is_none()
        {
            continue;
        }
        if !safe_name(name) {
            return Err(INVALID);
        }
        let enabled = entry
            .as_object()
            .and_then(|object| object.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or(INVALID)?;
        flags.insert(name.clone(), json!(enabled));
    }
    Ok(Value::Object(flags))
}

fn hook_flags(value: Option<&Value>) -> Result<Value, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::Object(map)) => {
            if map.values().all(Value::is_boolean) {
                let mut flags = Map::new();
                for (name, flag) in map {
                    if !safe_name(name) {
                        return Err(INVALID);
                    }
                    flags.insert(name.clone(), json!(flag.as_bool().unwrap_or(false)));
                }
                return Ok(Value::Object(flags));
            }
            enabled_flags(Some(map), false)
        }
        Some(Value::Array(items)) if items.is_empty() => Ok(json!({})),
        Some(Value::Array(items)) => {
            let mut flags = Map::new();
            for item in items {
                let object = item.as_object().ok_or(INVALID)?;
                let name = object
                    .get("name")
                    .or_else(|| object.get("eventName"))
                    .or_else(|| object.get("key"))
                    .and_then(Value::as_str)
                    .filter(|name| safe_name(name))
                    .ok_or(INVALID)?;
                let enabled = object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or(INVALID)?;
                flags.insert(name.to_owned(), json!(enabled));
            }
            Ok(Value::Object(flags))
        }
        Some(_) => Err(INVALID),
    }
}

fn skill_flags(value: Option<&Value>) -> Result<Value, &'static str> {
    let empty = json!({
        "bundled_enabled": null,
        "config": null,
        "include_instructions": null,
    });
    let Some(value) = value else {
        return Ok(empty);
    };
    if value.is_null() {
        return Ok(empty);
    }
    let skills = value.as_object().ok_or(INVALID)?;
    let bundled_enabled = match skills.get("bundled") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Object(bundled)) => optional_bool(bundled, "enabled")?,
        Some(_) => return Err(INVALID),
    };
    let include_instructions = match skills.get("include_instructions") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Bool(flag)) => json!(flag),
        Some(_) => return Err(INVALID),
    };
    let config = match skills.get("config") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Array(items)) => {
            let mut rows = Vec::with_capacity(items.len());
            for item in items {
                let object = item.as_object().ok_or(INVALID)?;
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| safe_name(name))
                    .ok_or(INVALID)?;
                let enabled = object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or(INVALID)?;
                rows.push(json!({"enabled": enabled, "name": name}));
            }
            rows.sort_by(|left, right| {
                let left_name = left.get("name").and_then(Value::as_str).unwrap_or("");
                let right_name = right.get("name").and_then(Value::as_str).unwrap_or("");
                left_name
                    .cmp(right_name)
                    .then_with(|| left.to_string().cmp(&right.to_string()))
            });
            Value::Array(rows)
        }
        Some(_) => return Err(INVALID),
    };
    Ok(json!({
        "bundled_enabled": bundled_enabled,
        "config": config,
        "include_instructions": include_instructions,
    }))
}

fn workspace_roots(write: Option<&Map<String, Value>>) -> Result<Value, &'static str> {
    let Some(write) = write else {
        return Ok(Value::Null);
    };
    let Some(roots) = write.get("writable_roots") else {
        return Ok(json!([]));
    };
    if roots.is_null() {
        return Ok(Value::Null);
    }
    let roots = roots.as_array().ok_or(INVALID)?;
    let mut paths = Vec::with_capacity(roots.len());
    for root in roots {
        let Some(path) = root.as_str() else {
            return Err(INVALID);
        };
        if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err(INVALID);
        }
        paths.push(path.to_owned());
    }
    paths.sort();
    paths.dedup();
    Ok(json!(paths))
}

fn optional_object<'a>(
    parent: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, &'static str> {
    match parent.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err(INVALID),
    }
}

fn sandbox_bool(write: Option<&Map<String, Value>>, key: &str) -> Result<Value, &'static str> {
    match write {
        None => Ok(Value::Null),
        Some(write) => optional_bool(write, key),
    }
}

fn optional_bool(map: &Map<String, Value>, key: &str) -> Result<Value, &'static str> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::Bool(flag)) => Ok(json!(flag)),
        Some(_) => Err(INVALID),
    }
}

fn required_bool(map: &Map<String, Value>, key: &str) -> Result<bool, &'static str> {
    map.get(key).and_then(Value::as_bool).ok_or(INVALID)
}

fn short_string(value: Option<&Value>, max_chars: usize) -> Result<Value, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) if safe_token(text, max_chars) => Ok(Value::String(text.clone())),
        Some(_) => Err(INVALID),
    }
}

fn safe_token(text: &str, max_chars: usize) -> bool {
    let count = text.chars().count();
    (1..=max_chars).contains(&count)
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '@')
        })
}

fn safe_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=256).contains(&count) && !name.chars().any(char::is_control)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "mcp-decoy-secret-SHOULD-NOT-LEAK-9c1e4b";

    fn sample(policy: &str) -> Value {
        json!({
            "config": {
                "approval_policy": policy,
                "approvals_reviewer": "user",
                "sandbox_mode": "workspace-write",
                "sandbox_workspace_write": {
                    "network_access": false,
                    "writable_roots": ["/tmp/b", "/tmp/a"],
                    "exclude_slash_tmp": false
                },
                "model": "fake",
                "model_provider": "fake",
                "web_search": "disabled",
                "features": {"plugins": false, "hooks": false, "apps": false, "js_repl": false},
                "plugins": {
                    "browser@openai-bundled": {"enabled": false, "env": {"TOKEN": SECRET}, "bearer_token": SECRET}
                },
                "apps": {"demo": {"enabled": false, "api_key": SECRET}, "_default": {"approvals_reviewer": "user"}},
                "connectors": {"gmail": {"enabled": false, "headers": {"Authorization": SECRET}}},
                "hooks": {"sessionStart": {"enabled": false, "command": "node /tmp/hook.js"}},
                "skills": {
                    "bundled": {"enabled": false},
                    "include_instructions": false,
                    "config": [
                        {"name": "zeta", "enabled": false, "path": "/tmp/zeta"},
                        {"name": "alpha", "enabled": false, "path": SECRET}
                    ]
                },
                "mcp_servers": {
                    "soma": {"enabled": false, "command": "soma", "env": {"TOKEN": SECRET}, "headers": {"Authorization": SECRET}},
                    "node_repl": {"enabled": false, "env": {"TOKEN": SECRET}}
                }
            },
            "origins": {"user": {"file": "/Users/owner/.codex/config.toml"}}
        })
    }

    #[test]
    fn secrets_are_omitted_from_the_hash_input_and_do_not_change_it() {
        let snapshot = snapshot_config(&sample("untrusted")).unwrap();
        assert!(!snapshot.canonical.contains(SECRET));
        assert!(!snapshot.canonical.contains("TOKEN"));
        assert!(!snapshot.canonical.contains("Authorization"));
        assert!(!snapshot.canonical.contains("bearer_token"));
        assert!(!snapshot.canonical.contains("api_key"));
        assert!(!snapshot.canonical.contains("command"));
        assert!(!snapshot.canonical.contains("/Users/owner"));
        let mut other = sample("untrusted");
        other["config"]["mcp_servers"]["soma"]["env"] = json!({"TOKEN": "different-secret"});
        other["config"]["plugins"]["browser@openai-bundled"]["bearer_token"] =
            json!("another-secret");
        let again = snapshot_config(&other).unwrap();
        assert_eq!(snapshot.hash, again.hash);
        assert_eq!(snapshot.hash.len(), 64);
        assert!(
            snapshot
                .canonical
                .contains("\"workspace_roots\":[\"/tmp/a\",\"/tmp/b\"]")
        );
        assert!(
            snapshot.canonical.find("\"name\":\"alpha\"").unwrap()
                < snapshot.canonical.find("\"name\":\"zeta\"").unwrap()
        );
    }

    #[test]
    fn each_field_group_is_named_when_it_changes() {
        let base = snapshot_config(&sample("untrusted")).unwrap();
        let cases = [
            ("approval_policy", sample("never")),
            ("sandbox", {
                let mut value = sample("untrusted");
                value["config"]["sandbox_mode"] = json!("danger-full-access");
                value
            }),
            ("sandbox", {
                let mut value = sample("untrusted");
                value["config"]["sandbox_workspace_write"]["exclude_slash_tmp"] = json!(true);
                value
            }),
            ("sandbox", {
                let mut value = sample("untrusted");
                value["config"]["sandbox_workspace_write"]["exclude_tmpdir_env_var"] = json!(true);
                value
            }),
            ("approvals_reviewer", {
                let mut value = sample("untrusted");
                value["config"]["approvals_reviewer"] = json!("auto_review");
                value
            }),
            ("model", {
                let mut value = sample("untrusted");
                value["config"]["model"] = json!("other-model");
                value
            }),
            ("extensions", {
                let mut value = sample("untrusted");
                value["config"]["plugins"]["browser@openai-bundled"]["enabled"] = json!(true);
                value
            }),
            ("mcp_servers", {
                let mut value = sample("untrusted");
                value["config"]["mcp_servers"]["extra"] =
                    json!({"enabled": false, "env": {"TOKEN": SECRET}});
                value
            }),
            ("features", {
                let mut value = sample("untrusted");
                value["config"]["features"]["plugins"] = json!(true);
                value
            }),
            ("workspace_roots", {
                let mut value = sample("untrusted");
                value["config"]["sandbox_workspace_write"]["writable_roots"] = json!(["/tmp/a"]);
                value
            }),
            ("web_search", {
                let mut value = sample("untrusted");
                value["config"]["web_search"] = json!("live");
                value
            }),
        ];
        for (group, value) in cases {
            let next = snapshot_config(&value).unwrap();
            assert_ne!(base.hash, next.hash, "{group}");
            assert_eq!(changed_groups(&base, &next), vec![group], "{group}");
            assert!(!next.canonical.contains(SECRET));
        }
        assert!(changed_groups(&base, &base).is_empty());
    }

    #[test]
    fn a_digest_mismatch_with_no_named_group_is_unknown() {
        let base = snapshot_config(&sample("untrusted")).unwrap();
        let mut same_body = base.clone();
        same_body.hash = "f".repeat(64);
        assert_ne!(base.hash, same_body.hash);
        assert_eq!(changed_groups(&base, &same_body), vec!["unknown"]);

        let broken = Snapshot {
            hash: "a".repeat(64),
            canonical: "not-json".into(),
        };
        let also_broken = Snapshot {
            hash: "b".repeat(64),
            canonical: "{".into(),
        };
        assert_eq!(changed_groups(&broken, &also_broken), vec!["unknown"]);

        let mut with_extra: Value = serde_json::from_str(&base.canonical).unwrap();
        with_extra
            .as_object_mut()
            .unwrap()
            .insert("zz".into(), json!(1));
        let extra = Snapshot {
            hash: "c".repeat(64),
            canonical: with_extra.to_string(),
        };
        assert_eq!(changed_groups(&base, &extra), vec!["unknown"]);

        let mut same_digest = base.clone();
        same_digest.canonical = "{}".into();
        assert!(changed_groups(&base, &same_digest).is_empty());
    }

    #[test]
    fn missing_fields_are_explicit_nulls_and_bad_shapes_fail() {
        let snapshot = snapshot_config(&json!({
            "config": {"mcp_servers": {}},
            "origins": {}
        }))
        .unwrap();
        assert!(snapshot.canonical.contains("\"approval_policy\":null"));
        assert!(snapshot.canonical.contains("\"model\":null"));
        assert!(snapshot.canonical.contains("\"network_access\":null"));
        assert!(snapshot.canonical.contains("\"exclude_slash_tmp\":null"));
        assert!(
            snapshot
                .canonical
                .contains("\"exclude_tmpdir_env_var\":null")
        );
        assert!(snapshot.canonical.contains("\"workspace_roots\":null"));
        assert!(snapshot.canonical.contains("\"features\":null"));
        assert!(snapshot_config(&json!({"origins": {}})).is_err());
        assert!(
            snapshot_config(&json!({"config": {"mcp_servers": {"soma": {"enabled": "false"}}}}))
                .is_err()
        );
        assert!(
            snapshot_config(
                &json!({"config": {"mcp_servers": {}, "features": {"plugins": "false"}}})
            )
            .is_err()
        );
        assert!(
            snapshot_config(&json!({"config": {"mcp_servers": {}, "web_search": SECRET}})).is_err()
        );
        let mut bad_flag = sample("untrusted");
        bad_flag["config"]["sandbox_workspace_write"]["exclude_slash_tmp"] = json!("false");
        assert!(snapshot_config(&bad_flag).is_err());
        bad_flag["config"]["sandbox_workspace_write"]["exclude_slash_tmp"] = json!(false);
        bad_flag["config"]["sandbox_workspace_write"]["exclude_tmpdir_env_var"] = json!(0);
        assert!(snapshot_config(&bad_flag).is_err());
    }
}
