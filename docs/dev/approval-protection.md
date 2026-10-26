# Approval protection

An approval decision is accepted only when it matches the pending request still held by this daemon process. The client echoes `approval_id`, `session_id`, `turn_id`, `provider_request_id`, and `content_hash`. The daemon also checks the journal epoch and that the session's current turn is still the bound turn. A mismatch journals `approval_rejected` and does not answer the provider. The pending approval stays pending, so a corrected echo can still be sent once.

`content_hash` is the SHA-256 of a compact JSON object whose keys are sorted: `argv`, `cwd`, `diff`, `kind`, `paths`, `reason`. Paths are sorted and de-duplicated. Command text is split on whitespace. The journal keeps a 64-character hex digest in a `content_hash` field. Other high-entropy strings are still redacted.

A second decision for the same approval is rejected, including a race between two clients. The provider is answered at most once. A decision for another session, another turn, another provider request id, another epoch, or an approval that is already expired, withdrawn, or abandoned is rejected. Restart withdraws pending approvals with reason `abandoned` before any client can answer. The provider child from the previous process is gone.

`acceptForSession` is rejected for every scope. A limited client may choose an option whose class is `once`. A client record with `allow_approvals: false` cannot decide. Missing `allow_approvals` on an older client file still means the client can decide. Timeout expires the approval and sends a decline or cancel, never an allow. The request, the decision (client id, label, scope, option, hash), and every rejection are journaled. The daemon log names the same ids and the rejection reason, and it does not include the command text. The log redacts the 64-hex content hash; the journal keeps it.

## Checklist

Each row is a `session_check_result` event: `item_id`, `status` (`verified`, `failed`, `not_verified`), `source`, `checked_at_ms`, `detail`. The screen uses the latest event for that session and item. No event is `not verified` with source `no daemon evidence`. A `verified` row without a source and a check time is shown as not verified. Status text is never “protected”.

The banner keeps “approvals not protected” until the selected session has `sandbox`, `bypass`, and `routing` verified. It then says how many of the twelve rows are verified and names the failed and not-verified ids. Fake ACP journals all twelve as `not_verified`, so its banner stays on the original sentence.

| Item | Codex status | Evidence |
| --- | --- | --- |
| sandbox | verified after a good `thread/start` echo | `workspace-write` or `read-only`, network closed. A bad echo is `failed` and later turns are refused. |
| bypass | verified after the same echo | `approvalPolicy` is `untrusted`, the spawn has no `--dangerously` flag, and `acceptForSession` is not a decision. A weaker policy is `failed`. |
| routing | verified after the same echo | `approvalsReviewer` is `user`. An unknown server request is refused. A non-user reviewer is `failed`. |
| config | verified after `codex_extensions_verified` | Per-run `-c` only. Config writes are not issued. An extension failure is `failed` on a nil session id and no session is created. |
| widening | verified when the session is ready | `thread/settings/updated` is refused, the row becomes `failed`, and the session stops. None is accepted. |
| mcp | verified when extension verification reports `mcp_servers: 0` | The label is “MCP servers disabled”. Any `mcpServer` activity journals `failed` and stops the session. |
| containment | not verified | The child is in its own process group. A macOS sandbox helper and a descendant snapshot are not checked. |
| credentials | not verified | Spawn removes `SSH_AUTH_SOCK`, `SSH_AGENT_PID`, and `GPG_AGENT_INFO`. Whether those sockets stay reachable by path is not checked. |
| writes | not verified | Codex `workspace-write` is the provider sandbox. Writes to Shardrail data, binaries, and provider install trees are not checked. |
| identity | not verified | Reachability of the Shardrail socket, host identity, ControlMaster sockets, and CLI token helpers is not checked. |
| boundary | not verified | The journal is not scanned for a boundary-breaking allow since protection was last established. |
| gaps | not verified | Codex can run a built-in read-only command without an approval card. A file change that skips approval is detected only after it completes. |

A failed check blocks later `turn.start` for that session. Extension failure before admission does not create a session. Fake ACP does not echo a sandbox, a policy, a reviewer, a config pin, a mode change, or an MCP server list, so those six rows are `not_verified` there as well.
