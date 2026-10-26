//! SHA-256 of the effective Codex approval config.
//!
//! The hash input is only the canonical object below. MCP `env`, headers,
//! tokens, API keys, commands, arguments, and hook `trusted_hash` values are
//! not copied. Top-level keys other than the groups below are ignored, not
//! hashed. That includes `agents`, `memories`, `desktop`, `cloud`, `history`,
//! `permissions`, and `allow_login_shell`. An edit to an ignored key, including
//! the body of the permissions profile selected by `default_permissions`, does
//! not change the digest.
//!
//! Canonical groups, always present, with sorted object keys and explicit nulls:
//! - `approval_policy` — string or granular bools, else null
//! - `approvals_reviewer` — string or null
//! - `default_permissions` — profile id or null. This id selects the permissions
//!   profile, including the sandbox, when `sandbox_mode` is null
//! - `sandbox` — `mode`, `network_access`, `exclude_slash_tmp`, and
//!   `exclude_tmpdir_env_var`
//! - `workspace_roots` — sorted `sandbox_workspace_write.writable_roots`
//! - `model` and `model_provider` — a token, or null. `/` is allowed
//! - `extensions` — apps, connectors, and plugins as name → enabled; skills as
//!   `bundled_enabled`, `include_instructions`, and `{name, enabled}`; hooks as
//!   event lists plus `state` id → enabled
//! - `mcp_servers` — name → `{enabled}` only
//! - `features` — name → bool or null. Null is kept. `features.hooks` is this
//!   bool and is not used to rewrite the hooks table
//! - `web_search` — `disabled`, `cached`, `indexed`, `live`, or null
//!
//! `config/read` is the source. `mcpServerStatus/list` is not called.
//! An invalid body names the first canonical group that failed (`features`,
//! `extensions.hooks`, …) and includes no value.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvalidRead {
    pub(crate) group: &'static str,
}

impl InvalidRead {
    pub(crate) fn reason(self) -> String {
        format!("config/read response is invalid: {}", self.group)
    }
}

fn invalid(group: &'static str) -> InvalidRead {
    InvalidRead { group }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) hash: String,
    pub(crate) canonical: String,
}

pub(crate) fn snapshot_config(result: &Value) -> Result<Snapshot, InvalidRead> {
    let config = result
        .get("config")
        .and_then(Value::as_object)
        .ok_or(invalid("config"))?;
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

const GROUPS: [&str; 11] = [
    "approval_policy",
    "approvals_reviewer",
    "default_permissions",
    "extensions",
    "features",
    "mcp_servers",
    "model",
    "model_provider",
    "sandbox",
    "web_search",
    "workspace_roots",
];

fn canonical_config(config: &Map<String, Value>) -> Result<Value, InvalidRead> {
    let approval_policy = approval_policy(config.get("approval_policy"))?;
    let approvals_reviewer =
        short_string(config.get("approvals_reviewer"), 64, "approvals_reviewer")?;
    let default_permissions = permission_profile(config.get("default_permissions"))?;
    let extensions = extensions(config)?;
    let features = feature_flags(config.get("features"))?;
    let mcp_servers = mcp_servers(config)?;
    let model = model_string(config.get("model"), "model")?;
    let model_provider = model_string(config.get("model_provider"), "model_provider")?;
    let sandbox_write = optional_object(config, "sandbox_workspace_write")?;
    let network = sandbox_bool(sandbox_write, "network_access")?;
    let exclude_slash_tmp = sandbox_bool(sandbox_write, "exclude_slash_tmp")?;
    let exclude_tmpdir_env_var = sandbox_bool(sandbox_write, "exclude_tmpdir_env_var")?;
    let roots = workspace_roots(sandbox_write)?;
    let web_search = web_search(config.get("web_search"))?;
    Ok(json!({
        "approval_policy": approval_policy,
        "approvals_reviewer": approvals_reviewer,
        "default_permissions": default_permissions,
        "extensions": extensions,
        "features": features,
        "mcp_servers": mcp_servers,
        "model": model,
        "model_provider": model_provider,
        "sandbox": {
            "exclude_slash_tmp": exclude_slash_tmp,
            "exclude_tmpdir_env_var": exclude_tmpdir_env_var,
            "mode": short_string(config.get("sandbox_mode"), 64, "sandbox")?,
            "network_access": network,
        },
        "web_search": web_search,
        "workspace_roots": roots,
    }))
}

fn extensions(config: &Map<String, Value>) -> Result<Value, InvalidRead> {
    Ok(json!({
        "apps": enabled_flags(optional_object(config, "apps")?, "extensions.apps", true)?,
        "connectors": enabled_flags(optional_object(config, "connectors")?, "extensions.connectors", false)?,
        "hooks": hook_flags(config.get("hooks"))?,
        "plugins": enabled_flags(optional_object(config, "plugins")?, "extensions.plugins", false)?,
        "skills": skill_flags(config.get("skills"))?,
    }))
}

fn approval_policy(value: Option<&Value>) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) => {
            if !safe_token(text, 64) {
                return Err(invalid("approval_policy"));
            }
            Ok(Value::String(text.clone()))
        }
        Some(Value::Object(object)) => {
            let granular = object
                .get("granular")
                .and_then(Value::as_object)
                .ok_or(invalid("approval_policy"))?;
            Ok(json!({
                "granular": {
                    "mcp_elicitations": required_bool(granular, "mcp_elicitations", "approval_policy")?,
                    "request_permissions": optional_bool(granular, "request_permissions", "approval_policy")?,
                    "rules": required_bool(granular, "rules", "approval_policy")?,
                    "sandbox_approval": required_bool(granular, "sandbox_approval", "approval_policy")?,
                    "skill_approval": optional_bool(granular, "skill_approval", "approval_policy")?,
                }
            }))
        }
        Some(_) => Err(invalid("approval_policy")),
    }
}

fn web_search(value: Option<&Value>) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text))
            if matches!(text.as_str(), "disabled" | "cached" | "indexed" | "live") =>
        {
            Ok(Value::String(text.clone()))
        }
        Some(_) => Err(invalid("web_search")),
    }
}

fn feature_flags(value: Option<&Value>) -> Result<Value, InvalidRead> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    if value.is_null() {
        return Ok(Value::Null);
    }
    let map = value.as_object().ok_or(invalid("features"))?;
    let mut flags = Map::new();
    for (name, flag) in map {
        if !safe_token(name, 64) {
            return Err(invalid("features"));
        }
        let flag = match flag {
            Value::Bool(flag) => json!(flag),
            Value::Null => Value::Null,
            _ => return Err(invalid("features")),
        };
        flags.insert(name.clone(), flag);
    }
    Ok(Value::Object(flags))
}

/// Profile id such as `:workspace`. Codex uses it instead of `sandbox_mode`.
fn permission_profile(value: Option<&Value>) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) if model_token(text) => Ok(Value::String(text.clone())),
        Some(_) => Err(invalid("default_permissions")),
    }
}

fn mcp_servers(config: &Map<String, Value>) -> Result<Value, InvalidRead> {
    let servers = config
        .get("mcp_servers")
        .and_then(Value::as_object)
        .ok_or(invalid("mcp_servers"))?;
    let mut canonical = Map::new();
    for (name, server) in servers {
        if !safe_name(name) {
            return Err(invalid("mcp_servers"));
        }
        let enabled = server
            .as_object()
            .and_then(|entry| entry.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or(invalid("mcp_servers"))?;
        canonical.insert(name.clone(), json!({"enabled": enabled}));
    }
    Ok(Value::Object(canonical))
}

fn enabled_flags(
    map: Option<&Map<String, Value>>,
    group: &'static str,
    skip_default: bool,
) -> Result<Value, InvalidRead> {
    let Some(map) = map else {
        return Ok(Value::Null);
    };
    let mut flags = Map::new();
    for (name, entry) in map {
        if skip_default && name == "_default" {
            match entry {
                Value::Null => continue,
                Value::Object(object) => match object.get("enabled") {
                    None | Some(Value::Null) => continue,
                    Some(Value::Bool(_)) => {}
                    Some(_) => return Err(invalid(group)),
                },
                _ => return Err(invalid(group)),
            }
        }
        if !safe_name(name) {
            return Err(invalid(group));
        }
        let enabled = entry
            .as_object()
            .and_then(|object| object.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or(invalid(group))?;
        flags.insert(name.clone(), json!(enabled));
    }
    Ok(Value::Object(flags))
}

/// Event name → `{count, entries}` plus `state` id → enabled.
///
/// `trusted_hash` is not copied. A hook file edit that does not change the
/// executable name, matcher, count, or `enabled` flag is not a digest change.
/// `features.hooks=false` is hashed on `features` and does not clear `state`.
fn hook_flags(value: Option<&Value>) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::Object(map)) => hook_object(map),
        Some(_) => Err(invalid("extensions.hooks")),
    }
}

fn hook_object(map: &Map<String, Value>) -> Result<Value, InvalidRead> {
    let mut events = Map::new();
    let mut state = Value::Null;
    for (name, value) in map {
        if name == "state" {
            state = hook_state(value)?;
            continue;
        }
        if !safe_token(name, 64) {
            return Err(invalid("extensions.hooks"));
        }
        let list = value.as_array().ok_or(invalid("extensions.hooks"))?;
        let mut entries = Vec::with_capacity(list.len());
        for item in list {
            entries.push(hook_entry(item)?);
        }
        events.insert(
            name.clone(),
            json!({"count": list.len(), "entries": entries}),
        );
    }
    Ok(json!({"events": events, "state": state}))
}

fn hook_state(value: &Value) -> Result<Value, InvalidRead> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let map = value.as_object().ok_or(invalid("extensions.hooks"))?;
    let mut flags = Map::new();
    for (id, entry) in map {
        if !bounded_text(id, 4096) {
            return Err(invalid("extensions.hooks"));
        }
        let object = entry.as_object().ok_or(invalid("extensions.hooks"))?;
        let enabled = match object.get("enabled") {
            None | Some(Value::Null) => Value::Null,
            Some(Value::Bool(flag)) => json!(flag),
            Some(_) => return Err(invalid("extensions.hooks")),
        };
        flags.insert(id.clone(), enabled);
    }
    Ok(Value::Object(flags))
}

fn hook_entry(value: &Value) -> Result<Value, InvalidRead> {
    let object = value.as_object().ok_or(invalid("extensions.hooks"))?;
    let program = match object.get("command") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(text)) => {
            Value::String(program_name(text).ok_or(invalid("extensions.hooks"))?)
        }
        Some(_) => return Err(invalid("extensions.hooks")),
    };
    let matcher = match object.get("matcher") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(text)) if bounded_text(text, 256) => Value::String(text.clone()),
        Some(_) => return Err(invalid("extensions.hooks")),
    };
    Ok(json!({"matcher": matcher, "program": program}))
}

/// Executable name only. The path and arguments are dropped.
fn program_name(command: &str) -> Option<String> {
    let token = command.split_whitespace().next()?;
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    bounded_text(name, 256).then(|| name.to_owned())
}

fn skill_flags(value: Option<&Value>) -> Result<Value, InvalidRead> {
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
    let skills = value.as_object().ok_or(invalid("extensions.skills"))?;
    let bundled_enabled = match skills.get("bundled") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Object(bundled)) => optional_bool(bundled, "enabled", "extensions.skills")?,
        Some(_) => return Err(invalid("extensions.skills")),
    };
    let include_instructions = match skills.get("include_instructions") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Bool(flag)) => json!(flag),
        Some(_) => return Err(invalid("extensions.skills")),
    };
    let config = match skills.get("config") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Array(items)) => {
            let mut rows = Vec::with_capacity(items.len());
            for item in items {
                let object = item.as_object().ok_or(invalid("extensions.skills"))?;
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| safe_name(name))
                    .ok_or(invalid("extensions.skills"))?;
                let enabled = object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or(invalid("extensions.skills"))?;
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
        Some(_) => return Err(invalid("extensions.skills")),
    };
    Ok(json!({
        "bundled_enabled": bundled_enabled,
        "config": config,
        "include_instructions": include_instructions,
    }))
}

fn workspace_roots(write: Option<&Map<String, Value>>) -> Result<Value, InvalidRead> {
    let Some(write) = write else {
        return Ok(Value::Null);
    };
    let Some(roots) = write.get("writable_roots") else {
        return Ok(json!([]));
    };
    if roots.is_null() {
        return Ok(Value::Null);
    }
    let roots = roots.as_array().ok_or(invalid("workspace_roots"))?;
    let mut paths = Vec::with_capacity(roots.len());
    for root in roots {
        let Some(path) = root.as_str() else {
            return Err(invalid("workspace_roots"));
        };
        if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err(invalid("workspace_roots"));
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
) -> Result<Option<&'a Map<String, Value>>, InvalidRead> {
    match parent.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err(invalid(object_group(key))),
    }
}

fn object_group(key: &str) -> &'static str {
    match key {
        "apps" => "extensions.apps",
        "connectors" => "extensions.connectors",
        "plugins" => "extensions.plugins",
        "sandbox_workspace_write" => "sandbox",
        _ => "config",
    }
}

fn sandbox_bool(write: Option<&Map<String, Value>>, key: &str) -> Result<Value, InvalidRead> {
    match write {
        None => Ok(Value::Null),
        Some(write) => optional_bool(write, key, "sandbox"),
    }
}

fn optional_bool(
    map: &Map<String, Value>,
    key: &str,
    group: &'static str,
) -> Result<Value, InvalidRead> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::Bool(flag)) => Ok(json!(flag)),
        Some(_) => Err(invalid(group)),
    }
}

fn required_bool(
    map: &Map<String, Value>,
    key: &str,
    group: &'static str,
) -> Result<bool, InvalidRead> {
    map.get(key).and_then(Value::as_bool).ok_or(invalid(group))
}

fn short_string(
    value: Option<&Value>,
    max_chars: usize,
    group: &'static str,
) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) if safe_token(text, max_chars) => Ok(Value::String(text.clone())),
        Some(_) => Err(invalid(group)),
    }
}

fn model_string(value: Option<&Value>, group: &'static str) -> Result<Value, InvalidRead> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(text)) if model_token(text) => Ok(Value::String(text.clone())),
        Some(_) => Err(invalid(group)),
    }
}

fn model_token(text: &str) -> bool {
    let count = text.chars().count();
    (1..=128).contains(&count)
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '@' | '/')
        })
}

fn safe_token(text: &str, max_chars: usize) -> bool {
    let count = text.chars().count();
    (1..=max_chars).contains(&count)
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '@')
        })
}

fn safe_name(name: &str) -> bool {
    bounded_text(name, 256)
}

fn bounded_text(text: &str, max_chars: usize) -> bool {
    let count = text.chars().count();
    (1..=max_chars).contains(&count) && !text.chars().any(char::is_control)
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
                "features": {"plugins": false, "hooks": false, "apps": false, "js_repl": false, "network_proxy": null},
                "plugins": {
                    "browser@openai-bundled": {"enabled": false, "env": {"TOKEN": SECRET}, "bearer_token": SECRET}
                },
                "apps": {"demo": {"enabled": false, "api_key": SECRET}, "_default": {"approvals_reviewer": "user"}},
                "connectors": {"gmail": {"enabled": false, "headers": {"Authorization": SECRET}}},
                "hooks": {
                    "PreToolUse": [],
                    "SessionStart": [{"command": "node /tmp/hook.js", "matcher": "Bash", "env": {"TOKEN": SECRET}}],
                    "state": {
                        "plugin:SessionStart:0:0": {"trusted_hash": SECRET, "enabled": false}
                    }
                },
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

    fn realistic() -> Value {
        let mut hooks = Map::new();
        for event in [
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "SubagentStart",
            "SubagentStop",
            "Stop",
            "Interrupt",
        ] {
            hooks.insert(event.to_owned(), json!([]));
        }
        hooks.insert(
            "state".into(),
            json!({
                "plugin:SessionStart:0:0": {"trusted_hash": "fixture-hook-hash"},
                "user-hook.py:PreToolUse:0:1": {"enabled": true, "trusted_hash": "other-hook-hash"}
            }),
        );
        json!({
            "config": {
                "approval_policy": "on-request",
                "approvals_reviewer": null,
                "sandbox_mode": null,
                "web_search": null,
                "model": "gpt-5.4",
                "model_provider": null,
                "sandbox_workspace_write": null,
                "connectors": null,
                "skills": null,
                "default_permissions": ":workspace",
                "apps": {
                    "_default": null,
                    "connector_a1b2c3d4": {
                        "approvals_reviewer": "user",
                        "default_tools_approval_mode": "prompt",
                        "default_tools_enabled": false,
                        "destructive_enabled": false,
                        "enabled": true,
                        "links": {"acct": {"approvals_reviewer": "auto_review"}},
                        "omit_tools_from": ["deferred"],
                        "open_world_enabled": false,
                        "tools": {"send": {"enabled": true, "api_key": SECRET}}
                    }
                },
                "plugins": {"browser@openai-bundled": {"enabled": true}},
                "hooks": hooks,
                "features": {
                    "plugins": true,
                    "hooks": false,
                    "apps": true,
                    "network_proxy": null
                },
                "mcp_servers": {
                    "soma": {"enabled": true, "command": "soma", "env": {"TOKEN": SECRET}, "headers": {"Authorization": SECRET}}
                },
                "agents": {"explorer": {"description": SECRET}},
                "memories": {"note": SECRET},
                "desktop": null,
                "cloud": {"token": SECRET},
                "history": {"persist": true},
                "permissions": {"workspace": {"network": "open", "env": SECRET}},
                "allow_login_shell": true
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
        assert!(!snapshot.canonical.contains("trusted_hash"));
        assert!(!snapshot.canonical.contains("/tmp/hook.js"));
        assert!(!snapshot.canonical.contains("/Users/owner"));
        assert!(snapshot.canonical.contains("\"program\":\"node\""));
        assert!(snapshot.canonical.contains("\"matcher\":\"Bash\""));
        assert!(snapshot.canonical.contains("\"network_proxy\":null"));
        let mut other = sample("untrusted");
        other["config"]["mcp_servers"]["soma"]["env"] = json!({"TOKEN": "different-secret"});
        other["config"]["plugins"]["browser@openai-bundled"]["bearer_token"] =
            json!("another-secret");
        other["config"]["hooks"]["state"]["plugin:SessionStart:0:0"]["trusted_hash"] =
            json!("different-hash");
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
            ("default_permissions", {
                let mut value = sample("untrusted");
                value["config"]["default_permissions"] = json!(":workspace");
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
    fn realistic_config_read_is_accepted_and_ignores_unknown_keys() {
        let snapshot = snapshot_config(&realistic()).unwrap();
        assert!(!snapshot.canonical.contains(SECRET));
        assert!(!snapshot.canonical.contains("fixture-hook-hash"));
        assert!(!snapshot.canonical.contains("other-hook-hash"));
        assert!(!snapshot.canonical.contains("trusted_hash"));
        assert!(!snapshot.canonical.contains("allow_login_shell"));
        assert!(!snapshot.canonical.contains("\"permissions\""));
        assert!(!snapshot.canonical.contains("_default"));
        assert!(
            snapshot
                .canonical
                .contains("\"default_permissions\":\":workspace\"")
        );
        assert!(snapshot.canonical.contains("\"network_proxy\":null"));
        assert!(snapshot.canonical.contains("\"hooks\":false"));
        assert!(snapshot.canonical.contains("\"connectors\":null"));
        assert!(!snapshot.canonical.contains("\"sandbox_workspace_write\""));
        assert!(snapshot.canonical.contains("\"network_access\":null"));
        assert!(snapshot.canonical.contains("\"workspace_roots\":null"));
        assert!(snapshot.canonical.contains("\"bundled_enabled\":null"));
        assert!(snapshot.canonical.contains("\"connector_a1b2c3d4\":true"));
        assert!(
            snapshot
                .canonical
                .contains("\"plugin:SessionStart:0:0\":null")
        );
        assert!(
            snapshot
                .canonical
                .contains("\"user-hook.py:PreToolUse:0:1\":true")
        );
        assert!(
            snapshot
                .canonical
                .contains("\"PreToolUse\":{\"count\":0,\"entries\":[]}")
        );
        let mut extra = realistic();
        extra["config"]["agents"] = json!({"other": true});
        extra["config"]["memories"] = json!({"note": "different"});
        extra["config"]["permissions"]["workspace"]["network"] = json!("closed");
        extra["config"]["allow_login_shell"] = json!(false);
        extra["config"]["hooks"]["state"]["plugin:SessionStart:0:0"]["trusted_hash"] =
            json!("rotated-hook-hash");
        assert_eq!(snapshot.hash, snapshot_config(&extra).unwrap().hash);
        assert_eq!(snapshot.hash.len(), 64);
    }

    #[test]
    fn hook_state_and_null_feature_drifts_are_named() {
        let base = snapshot_config(&realistic()).unwrap();
        assert!(base.canonical.contains("\"hooks\":false"));
        assert!(
            base.canonical
                .contains("\"user-hook.py:PreToolUse:0:1\":true")
        );

        let mut enabled = realistic();
        enabled["config"]["hooks"]["state"]["user-hook.py:PreToolUse:0:1"]["enabled"] =
            json!(false);
        let flipped = snapshot_config(&enabled).unwrap();
        assert_ne!(base.hash, flipped.hash);
        assert_eq!(changed_groups(&base, &flipped), vec!["extensions"]);
        assert!(flipped.canonical.contains("\"hooks\":false"));

        let mut feature = realistic();
        feature["config"]["features"]["network_proxy"] = json!(true);
        let turned_on = snapshot_config(&feature).unwrap();
        assert_ne!(base.hash, turned_on.hash);
        assert_eq!(changed_groups(&base, &turned_on), vec!["features"]);
        assert!(turned_on.canonical.contains("\"network_proxy\":true"));

        let mut hooks_on = realistic();
        hooks_on["config"]["features"]["hooks"] = json!(true);
        let hooks = snapshot_config(&hooks_on).unwrap();
        assert_eq!(changed_groups(&base, &hooks), vec!["features"]);
        assert!(
            hooks
                .canonical
                .contains("\"user-hook.py:PreToolUse:0:1\":true")
        );
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
        assert!(snapshot.canonical.contains("\"default_permissions\":null"));
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
        assert!(snapshot.canonical.contains("\"connectors\":null"));
        assert!(snapshot.canonical.contains("\"bundled_enabled\":null"));
        let missing = snapshot_config(&json!({"origins": {}})).unwrap_err();
        assert_eq!(missing.group, "config");
        assert_eq!(missing.reason(), "config/read response is invalid: config");
        let mcp =
            snapshot_config(&json!({"config": {"mcp_servers": {"soma": {"enabled": "false"}}}}))
                .unwrap_err();
        assert_eq!(mcp.group, "mcp_servers");
        assert_eq!(mcp.reason(), "config/read response is invalid: mcp_servers");
        let features = snapshot_config(
            &json!({"config": {"mcp_servers": {}, "features": {"plugins": "false"}}}),
        )
        .unwrap_err();
        assert_eq!(features.group, "features");
        let search = snapshot_config(&json!({"config": {"mcp_servers": {}, "web_search": SECRET}}))
            .unwrap_err();
        assert_eq!(search.group, "web_search");
        let mut bad_flag = sample("untrusted");
        bad_flag["config"]["sandbox_workspace_write"]["exclude_slash_tmp"] = json!("false");
        assert_eq!(snapshot_config(&bad_flag).unwrap_err().group, "sandbox");
        bad_flag["config"]["sandbox_workspace_write"]["exclude_slash_tmp"] = json!(false);
        bad_flag["config"]["sandbox_workspace_write"]["exclude_tmpdir_env_var"] = json!(0);
        assert_eq!(snapshot_config(&bad_flag).unwrap_err().group, "sandbox");
        let mut bad_hooks = sample("untrusted");
        bad_hooks["config"]["hooks"]["SessionStart"][0]["command"] = json!(1);
        assert_eq!(
            snapshot_config(&bad_hooks).unwrap_err().group,
            "extensions.hooks"
        );
        let mut bad_profile = sample("untrusted");
        bad_profile["config"]["default_permissions"] = json!({"profile": ":workspace"});
        assert_eq!(
            snapshot_config(&bad_profile).unwrap_err().group,
            "default_permissions"
        );
    }

    #[test]
    fn slash_model_ids_are_hashed_and_other_punctuation_is_rejected() {
        let mut value = sample("untrusted");
        value["config"]["model"] = json!("openai/gpt-4o");
        value["config"]["model_provider"] = json!("openrouter/openai");
        let snapshot = snapshot_config(&value).unwrap();
        assert!(snapshot.canonical.contains("\"model\":\"openai/gpt-4o\""));
        assert!(
            snapshot
                .canonical
                .contains("\"model_provider\":\"openrouter/openai\"")
        );
        let mut changed = value.clone();
        changed["config"]["model"] = json!("openai/gpt-4o-mini");
        let next = snapshot_config(&changed).unwrap();
        assert_ne!(snapshot.hash, next.hash);
        assert_eq!(changed_groups(&snapshot, &next), vec!["model"]);

        value["config"]["model"] = json!("openai/gpt 4o");
        assert_eq!(snapshot_config(&value).unwrap_err().group, "model");
        value["config"]["model"] = json!("openai/gpt-4o\n");
        assert_eq!(snapshot_config(&value).unwrap_err().group, "model");
        value["config"]["model"] = json!("a".repeat(129));
        assert_eq!(snapshot_config(&value).unwrap_err().group, "model");
        assert_eq!(
            snapshot_config(&sample("on-request/extra"))
                .unwrap_err()
                .group,
            "approval_policy"
        );
    }
}
