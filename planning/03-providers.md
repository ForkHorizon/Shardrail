# 03 — Provider Integrations

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Evidence IDs (EV-#) are in [14-evidence](14-evidence.md). Nothing here has been executed against a live provider; Phase 0 spikes verify it.

## 1. Channels (D6)

| Agent | Channel used | Deliberately not used |
| --- | --- | --- |
| Grok Build (1.0.52 on the planning machine) | `grok agent stdio`, ACP v1 | headless `-p` as an interactive channel; parsing `~/.grok/sessions` as an API |
| Hermes Agent (0.21.6) | `hermes acp` / `hermes-acp`, ACP v1 | gateway, cron, kanban, Hermes desktop |
| Codex CLI (0.160.0) | separate child `codex app-server` over stdio | WebSocket transport (experimental, unsupported); `codex exec` as a UI session; shared `app-server daemon`/`proxy` used by the user's TUI |
| Claude Code | ACP adapter `@agentclientprotocol/claude-agent-acp` (Apache-2.0, ACP organization, not Anthropic) **or** a narrow supervisor over the Agent SDK — chosen in S4; subject to usage terms (§6) | `claude -p` as the only interactive channel; capturing the live TUI |

Shardrail has its own session model. ACP is the shared language for Grok and Hermes and a candidate for Claude. Codex stays on its app-server because that protocol is richer than ACP and is documented as the way to embed Codex in a product. Shared capabilities are projected into one model; provider-specific capabilities are kept, not flattened.

## 2. Adapter contract

- Capabilities are negotiated at `initialize`; static per-version tables are hints only. The core never calls a method the capability set lacks.
- Mandatory minimum: `start`, `prompt`, `stream`, `cancel`, `approve`, `health`. Optional: `steer`, `resume` (without replay), `load` (with replay), `fork`, `list_history`, `read_history`, `concurrent_sessions_per_process`.
- Normalized events: `session_started`, `turn_started`, `text`, `thought`, `tool_started`, `tool_updated`, `permission_requested`, `permission_resolved`, `plan`, `usage`, `turn_finished`, `session_degraded`, `notice`; each tagged ephemeral or durable (04). Fields without an equivalent are not invented; provider-specific data goes to `provider_ext`.
- ACP agents emit no "item completed" for message text; the adapter synthesizes completion when the update type changes, a tool call starts, or the `session/prompt` response arrives.
- **The ACP client never declares `fs/*` or `terminal/*` capabilities (D7).** Otherwise the agent could ask Shardrail itself to write files and run commands outside the provider's sandbox. File reading for the UI is done by the workspace service.
- **Unknown requests are refused.** Every unknown server request gets an immediate refusing response (decline / empty grant / JSON-RPC error) plus a `notice`; unknown notifications become `notice`. *Why:* an unanswered request blocks a turn forever, and Codex already has request types (permission grants, MCP elicitation, tool user-input) that the first design did not model.
- **Approval options are classified** (D19): `once`; `widening` (persistent or session-wide rules: ACP `allow_always`/`reject_always`, Hermes `allow_always`, Codex `acceptForSession`, `acceptWithExecpolicyAmendment`, `item/permissions/requestApproval` with `scope: "session"`); `boundary-breaking` (actions outside the sandbox). Unknown kinds fail closed to `widening`, or `boundary-breaking` if they indicate sandbox/permission changes.

## 3. Compatibility policy (D24)

| Class | Behavior |
| --- | --- |
| `tested` | normal |
| `untested-newer` | starts with a warning; strictest supported sandbox; `widening`/`boundary-breaking` options hidden; allow-all disabled; shadow schema validation logs divergences |
| `known-bad` | refused without explicit override |

Product builds never opt into experimental surfaces (Codex `capabilities.experimentalApi`). The Codex JSON Schema generated from the pinned CLI is stored in the repository and checked in CI; generating a schema from the user's binary is optional and diagnostic only. *Why:* the first design refused any unknown version, which would turn every routine provider update into an outage for a solo maintainer.

## 4. Resume and replay (D33)

- Prefer resume without replay (ACP `session/resume` when advertised; Codex `thread/resume`).
- On ACP `session/load` (which must replay the whole conversation, EV-01): if Shardrail's journal has no known gap, the replay is discarded (used only to restore provider state) and one `provider_reloaded` marker is journaled. If a gap is known, the replay is shown as a separate, labelled "provider history" view and never merged.
- *Why:* the first design de-duplicated by provider event IDs, but ACP message IDs are optional, so duplicates could not be ruled out.

## 5. Capability matrix (documentation-based; spikes confirm)

| Capability | Codex | Claude | Grok | Hermes |
| --- | --- | --- | --- | --- |
| Rich-client channel | app-server stdio — Verified docs; local help marks it experimental | Agent SDK (Python/TS); CLI `-p` for scripts — Verified | `grok agent stdio` ACP — Verified (local manual, Zed registry) | `hermes acp` — Verified |
| Stream of text and tools | thread/turn/item; deltas; `item/completed` authoritative — Verified | `stream-json`, SDK partial messages — Verified | `session/update` — Verified | ACP updates incl. diff/terminal — Verified |
| Approvals | `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` (`accept`/`acceptForSession`/`decline`/`cancel`), permission grants, elicitation, user input — Verified | SDK `canUseTool`; bare `-p` without a permission host **denies** — Verified | `session/request_permission`; `_meta.yoloMode` — Verified | allow once / session / always (permanent allowlist) / deny; timeout → deny after `approvals.timeout` (default 300 s) — Verified |
| Pending approvals at turn end | cleared with `serverRequest/resolved` documented for user-input requests; for approvals Inferred — S1 | Unverified | ACP: client answers `cancelled` when it cancels — Verified | same ACP rule |
| Resume | `thread/resume` — Verified | `resume` by session ID — Verified | `session/load` — Verified; `session/resume` Unverified | load/resume/fork after restart — Verified |
| Fork | `thread/fork` — Verified | `forkSession` — Verified | `x.ai/session/fork` — Verified (manual) | Verified |
| History listing | `thread/list`, `thread/read` — Verified | `listSessions`, `getSessionMessages` — Verified | Unverified | Verified |
| Steering an active turn | `turn/steer` with `expectedTurnId` — Verified | Unverified | Unverified | Unverified |
| Sandbox | `read-only` / `workspace-write` / `danger-full-access`, separate from approval policy; per-turn override; `.git` read-only; `.codex`/`.agents` read-only **only if they exist as directories** — Verified | permission modes + Bash sandbox | profiles `off`, `workspace`, `read-only`, `strict`, `devbox`; network-deny is a no-op on macOS — Verified | terminal approval; Docker backend exists, not used |
| Cancel | `turn/interrupt` — Verified | SDK `interrupt()`; SIGTERM on `-p` does not end the turn cleanly (exit 143) — Verified | `session/cancel` — Verified | `session/cancel` — Plausible, S3 |
| Background processes | background terminals with `osPid`, `thread/backgroundTerminals/terminate` — Verified | Unverified | Unverified | Unverified |
| Live TUI attach | Unsupported | Unsupported | Unsupported | Unsupported |
| Dangerous surfaces | `thread/shellCommand` runs **outside the sandbox with full access** — never in normal UI | `-p` without `--bare` loads hooks and MCP even from untrusted folders without a trust dialog | `x.ai/fs/write_file`, `x.ai/terminal/*` privileged — not used | `allow_always` writes a permanent allowlist; hosts that auto-answer approvals make execution unattended |

## 6. Per-provider policy

**Codex.** Separate `codex app-server` child per session until S1 rules otherwise; no shared daemon/proxy. Default policy `workspace-write` + `on-request`; `danger-full-access` with `approval never` requires a separate confirmation phrase. `thread/shellCommand` is not bound to any button. Approval-relevant settings are pinned at launch with documented `-c` overrides. Background terminals are terminated through the protocol on cancel. Shardrail does not link Codex sources; types are generated from the schema. `--analytics-default-enabled` is never passed. `clientInfo.name` is reported to OpenAI compliance logs (disclosed in privacy notes). Whether Shardrail may pre-create a protected `.codex/` directory in trusted workspaces (Codex protects it only if it exists) is owner question U20.

**Claude.** Channel per S4. Starting Claude in an `unknown` workspace is forbidden; in `unknown`/`restricted` the session runs with `settingSources: []`, auto memory and connectors disabled; in all trust states approval-relevant settings (permission mode) are pinned at launch. `bypassPermissions` is never a default. Branding follows the SDK rules: the provider is labelled "Claude Agent" (or "Claude" inside an "Agents" menu), never "Claude Code". **Usage terms:** Anthropic's Agent SDK documentation says that, unless previously approved, third-party developers may not offer claude.ai login or rate limits for their products, including agents built on the SDK, and directs them to API-key authentication (EV-04). Until the owner obtains a policy/legal answer (U9), Claude runs only with user-configured API-key authentication in any build. If the answer is negative and API-key-only is rejected, Claude is excluded without affecting other adapters.

**Grok.** `grok agent stdio`. Default sandbox for a new workspace `workspace`, not `off`; user may tighten to `read-only` or `strict`; `off` requires separate confirmation. Network deny cannot be promised on macOS. Grok's `auto` permission mode is shown only if S2 confirms requests still reach `session/request_permission`. Auto-update is suppressed for the spawned process with the documented `--no-auto-update` / `GROK_DISABLE_AUTOUPDATER`.

**Hermes.** `hermes acp` with the curated `hermes-acp` toolset. Browser tools are not set up by Shardrail. The effective toolset (default includes web/browser tools and `execute_code`) is shown per session; Shardrail does not edit `~/.hermes/config.yaml` and only recommends changes. `allow_always` is labelled as a permanent Hermes rule. Shardrail's approval expiry must not exceed Hermes' `approvals.timeout`.

## 7. Differences that must not be flattened

- Codex sandbox and approval policy are two axes; Grok sandbox profile and permission mode are two different axes. The UI shows both, never one "autonomy" slider.
- Claude `auto` is a classifier; Grok `auto` in a non-interactive session blocks what its classifier rejects without asking. Shardrail, as an ACP client, provides the human channel.
- Cost: Grok reports ticks and may omit cost; Claude reports an estimated `total_cost_usd`; Codex cost fields unconfirmed. Missing data shows "no data", never zero.
- Claude cancel via SIGTERM corrupts the turn: the supervisor sends the protocol cancel, waits, and only then stops the process.

## 8. Provider updates

Shardrail never updates providers. Where an official flag exists (Grok), auto-update is suppressed for spawned processes. Elsewhere the compatibility policy (§3) is the protection. After the user updates a provider, Shardrail re-checks the version.
