# Approval protection

An approval decision is accepted only when it matches the pending request still held by this daemon process. The client echoes `approval_id`, `session_id`, `turn_id`, `provider_request_id`, and `content_hash`. The daemon also checks the journal epoch and that the session's current turn is still the bound turn. A mismatch journals `approval_rejected` and does not answer the provider. The pending approval stays pending, so a corrected echo can still be sent once.

`content_hash` is the SHA-256 of a compact JSON object whose keys are sorted: `argv`, `command`, `cwd`, `diff`, `kind`, `network`, `paths`, `raw_input`, `reason`. Paths are sorted and de-duplicated. `command` is the provider's command text, unchanged. `argv` is used only when the provider supplies argv; command text is not split on whitespace. `network` is the canonical JSON of the network context, including host, protocol, and port. `raw_input` is the canonical JSON of the command actions or raw input shown on the card. The journal keeps a 64-character hex digest in a `content_hash` field. Other high-entropy strings are still redacted.

A second decision for the same approval is rejected, including a race between two clients. The provider is answered at most once. A decision for another session, another turn, another provider request id, another epoch, or an approval that is already expired, withdrawn, or abandoned is rejected. A rejection for an approval that is no longer pending is journaled on that approval's session, not on a session id supplied in the request. A resolution frame that names an approval id but does not parse is journaled as `approval_rejected` with reason `missing_binding` or `malformed_binding`, and the provider is not answered. Restart withdraws pending approvals with reason `abandoned`, stored on the approval row. Ending a turn stores `turn_end`, so a later decision is `stale_turn` rather than `abandoned`. The provider child from the previous process is gone.

`acceptForSession` is rejected for every scope. A limited client may choose an option whose class is `once`. A client record with `allow_approvals: false` cannot decide. Missing `allow_approvals` on an older client file still means the client can decide. Timeout expires the approval and sends a decline or cancel, never an allow. The request, the decision (client id, label, scope, option, hash), and every rejection are journaled. The daemon log names the same ids and the rejection reason, and it does not include the command text. The log redacts the 64-hex content hash; the journal keeps it.

## Checklist

Each row is a `session_check_result` event: `item_id`, `status` (`verified`, `failed`, `not_verified`), `source`, `checked_at_ms`, `detail`. The screen uses the latest event for that session and item. No event is `not verified` with source `no daemon evidence`. A `verified` row without a source and a check time is shown as not verified. Status text is never “protected”.

The host banner always says “approvals not protected”. With no session selected it says that no session is selected. With a session and no verified row it names sandbox, approval routing, containment, and the other checks. When any row is verified it keeps that warning and names the failed and not-verified rows. It does not add the rows up into a count, and it never says the session is protected. Fake ACP journals all twelve as `not_verified`, so its banner stays on the original sentence.

| Item | Codex status | Evidence |
| --- | --- | --- |
| sandbox | verified after a good `thread/start` echo | `workspace-write` or `read-only`, network closed. A bad echo is `failed` and later turns are refused. |
| bypass | verified after the same echo | `approvalPolicy` is `untrusted`, the spawn has no `--dangerously` flag, and `acceptForSession` is not a decision. A weaker policy is `failed`. |
| routing | verified after the same echo | `approvalsReviewer` is `user`. An unknown server request is refused. A non-user reviewer is `failed`. |
| config | verified after the session-start snapshot | SHA-256 of the effective Codex approval config from `config/read` (session cwd). Journaled at session start, before every turn, and at session end. A later read that drifts, fails, times out, or does not parse fails the row, does not send `turn/start`, and stops the session. An extension failure before admission is still `failed` on a nil session id and no session is created. |
| widening | verified when the session is ready | `thread/settings/updated` is refused, the row becomes `failed`, and the session stops. None is accepted. A startup echo failure leaves this row `not_verified` because settings monitoring has not started. |
| mcp | verified when extension verification reports `mcp_servers: 0` | The label is “MCP servers disabled”. Any `mcpServer` activity journals `failed` and stops the session. |
| containment | not verified | The child is in its own process group. A macOS sandbox helper and a descendant snapshot are not checked. |
| credentials | not verified | Spawn removes `SSH_AUTH_SOCK`, `SSH_AGENT_PID`, and `GPG_AGENT_INFO`. Whether those sockets stay reachable by path is not checked. |
| writes | not verified | Codex `workspace-write` is the provider sandbox. Writes to Shardrail data, binaries, and provider install trees are not checked. |
| identity | not verified | Reachability of the Shardrail socket, host identity, ControlMaster sockets, and CLI token helpers is not checked. |
| boundary | not verified | The journal is not scanned for a boundary-breaking allow since protection was last established. |
| gaps | not verified | Codex can run a built-in read-only command without an approval card. A file change that skips approval is detected only after it completes. |

A failed check blocks later `turn.start` for that session. Extension failure before admission does not create a session. Fake ACP does not echo a sandbox, a policy, a reviewer, a config pin, a mode change, or an MCP server list, so those six rows are `not_verified` there as well.

## Config snapshot

The `config` row is `verified` only after the session-start snapshot is journaled, and it stays `verified` while each pre-turn snapshot has the same hash. The screen shows that row's source and last-checked time from the latest `session_check_result`. The host banner still starts with “approvals not protected”.

`config/read` on the session child is the source. That read sees the effective config, including the per-run `-c` overrides. The daemon does not merge `config.toml` itself and does not call `mcpServerStatus/list`. The session-start hash reuses the verification `config/read` taken before `thread/start`. Each `turn.start` does a fresh `config/read` and compares it before `turn/start` is sent. At session end the daemon reads once more while the child is still alive. If the child is already gone, or that read times out, it journals `config_snapshot` with `recorded: false` and does not change a `verified` row.

The hash is SHA-256 of one canonical JSON object. Object keys are sorted at every level. Missing values are explicit nulls. The object always has these keys:

- `approval_policy` — a short token, or `{granular: {mcp_elicitations, request_permissions, rules, sandbox_approval, skill_approval}}` with explicit nulls for the optional bools
- `approvals_reviewer` — token or null
- `sandbox` — `mode` from `sandbox_mode`, and `network_access`, `exclude_slash_tmp`, and `exclude_tmpdir_env_var` from `sandbox_workspace_write`. Each bool is null when that object or field is absent
- `workspace_roots` — sorted, de-duplicated `sandbox_workspace_write.writable_roots`
- `model` and `model_provider` — token or null. A token may include `/` so a provider model id such as `openai/gpt-4o` still hashes. Other punctuation, spaces, and values longer than 128 characters make the read invalid. Approval-policy tokens do not allow `/`
- `extensions` — `apps`, `connectors`, `hooks`, and `plugins` as name to `enabled` only; `skills` as `bundled_enabled`, `include_instructions`, and `{name, enabled}` rows
- `mcp_servers` — name to `{enabled}` only
- `features` — name to bool
- `web_search` — `disabled`, `cached`, `indexed`, `live`, or null

`model_reasoning_effort` is not part of the hash. MCP `env`, headers, commands, args, URLs, tokens, API keys, and auth are not copied into the object, the hash, the journal, or the log. A `features` value that is not a bool, or an extension entry with no `enabled` bool, makes the read invalid. `apps._default` is skipped when it has no `enabled` bool.

Each snapshot is a `config_snapshot` event (`point`, `hash`, `matched`, `changed`, `recorded`) plus the `config` check row. `point` is `session-start`, `pre-turn`, or `session-end`. `hash` is `sha256:` plus the 64 hex digits. The `sha256:` label keeps the digest in the journal; a bare 64-hex string would be entropy-redacted. The check source names that point and the same labeled hash. The daemon log prints `sha256:` plus the first 12 hex digits, or `sha256:none` when that snapshot has no digest. A pre-turn or session-end read matches only when its digest equals the session-start digest. On drift the detail is `config drifted:` plus the canonical group names that changed, with no values. A digest mismatch that names no group is `unknown`, and the turn still does not start. A failed read uses a fixed reason: `config/read failed`, `config/read timed out`, or `config/read response is invalid`. A matching session-end snapshot does not turn a failed row back to `verified`.
