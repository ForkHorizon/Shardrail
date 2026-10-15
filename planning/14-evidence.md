# 14 — Evidence

> Part of the Shardrail architecture plan. Labels: **Verified** (primary source read on the access date), **Inferred** (reasoned from verified facts or well-known platform behavior), **Unverified**, **Requires experiment**. No provider was executed and nothing was measured.

## 1. External sources

| ID | Source | Finding used | Label | Accessed |
| --- | --- | --- | --- | --- |
| EV-01 | <https://agentclientprotocol.com/protocol/session-setup> | `session/load` must replay the whole conversation; `session/resume` (capability `sessionCapabilities.resume`) must not; `loadSession` must be advertised; message ids optional | Verified | 2026-10-15 |
| EV-02 | <https://agentclientprotocol.com/protocol/overview> | ACP v1 methods: `initialize`, `authenticate`, `session/new`, `session/prompt`, `session/cancel`, optional `session/load`, `session/set_mode`; client `session/request_permission`, optional `fs/*`, `terminal/*` | Verified | 2026-10-15 |
| EV-03 | <https://learn.chatgpt.com/docs/app-server> (redirect from developers.openai.com/codex/app-server) | stdio default; WebSocket experimental/unsupported; `unix://`; thread/turn methods incl. per-turn `sandboxPolicy`/`cwd`, `turn/steer` with `expectedTurnId`; approval methods and decisions; permission grants (`scope: "session"`), MCP elicitation, user input; `serverRequest/resolved`; `item/completed` authoritative; `experimentalApi` opt-in; `thread/shellCommand` outside sandbox; schema generation per version; `clientInfo.name` to compliance logs | Verified | 2026-10-15 |
| EV-04 | <https://code.claude.com/docs/en/agent-sdk/overview> | third parties may not offer claude.ai login or rate limits unless approved; API-key auth; Commercial Terms; branding rules | Verified | 2026-10-15 |
| EV-05 | <https://code.claude.com/docs/en/agent-sdk/sessions> | resume/fork by id; `listSessions`, `getSessionMessages`; transcripts in `~/.claude/projects/` | Verified | 2026-10-15 |
| EV-06 | <https://hermes-agent.nousresearch.com/docs/user-guide/features/acp> | `hermes acp`; allow once/session/always/deny; timeout 300 s → deny; list/load/resume/fork; auto-allow host warning; default toolset contents | Verified | 2026-10-15 |
| EV-07 | <https://git-scm.com/docs/git-config> | `core.fsmonitor` can name a hook command; protected configuration is system/global/command only; `core.hooksPath=/dev/null`; program-valued options (`core.sshCommand`, `core.pager`, `core.editor`, `credential.helper`, `core.askPass`, `diff.external`); `include`/`includeIf`; `config.worktree` | Verified | 2026-10-15 |
| EV-08 | <https://v2.tauri.app/release/> | Tauri 2.12.3 (2026-10-10) is current stable | Verified | 2026-10-15 |
| EV-09 | <https://zed.dev/acp/agent/grok-build> | `grok agent stdio` is the ACP entry point | Verified | 2026-10-15 |
| EV-10 | <https://github.com/agentclientprotocol/claude-agent-acp> | Apache-2.0; ACP organization; wraps the Claude Agent SDK; README silent on auth mode and `settingSources` | Verified | 2026-10-15 |
| EV-11 | <https://agentclientprotocol.com/protocol/tool-calls> | permission outcomes `cancelled`/`selected`; option kinds `allow_once`, `allow_always`, `reject_once`, `reject_always`; a reject option is not required | Verified | 2026-10-15 |
| EV-12 | <https://agentclientprotocol.com/protocol/prompt-turn> | client answers pending permission requests `cancelled` when it cancels; accepts tool updates after cancel; stop reasons | Verified | 2026-10-15 |
| EV-13 | <https://code.claude.com/docs/en/agent-sdk/claude-code-features> | `settingSources` default loads project hooks, CLAUDE.md, skills; `[]` disables them; auto memory, `~/.claude.json`, connectors load regardless | Verified | 2026-10-15 |
| EV-14 | <https://git-scm.com/docs/gitattributes> | filter drivers defined in config, assigned by attributes | Verified (whether `git status` runs clean filters: Inferred) | 2026-10-15 |
| EV-15 | <https://docs.rs/gix-filter/latest/gix_filter/> and `pipeline/struct.Options.html` | gix supports external filter programs via a driver list | Verified | 2026-10-15 |
| EV-16 | <https://docs.rs/gix/latest/gix/open/permissions/struct.Config.html> | repository config always loaded; toggles for system/git/user/env/includes/git_binary | Verified | 2026-10-15 |
| EV-17 | <https://git-scm.com/docs/git> | `GIT_CEILING_DIRECTORIES` limits; `GIT_DIR`, `GIT_EXTERNAL_DIFF`, `GIT_PAGER`, `GIT_SSH_COMMAND`, `GIT_ASKPASS` | Verified | 2026-10-15 |
| EV-18 | <https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html> | parent = creating thread; cleared on setuid exec and credential changes; no signal if parent already gone | Verified | 2026-10-15 |
| EV-19 | <https://man7.org/linux/man-pages/man5/systemd.kill.5.html> | default `KillMode=control-group` kills remaining processes on unit stop | Verified | 2026-10-15 |
| EV-20 | <https://www.sqlite.org/pragma.html> | `secure_delete` must be on before the delete (or VACUUM); `incremental_vacuum` no-op without `auto_vacuum=incremental` | Verified | 2026-10-15 |
| EV-21 | <https://learn.chatgpt.com/docs/llms-full.txt> | Codex `workspace-write`: `.git` read-only; `.codex`/`.agents` read-only only if they exist as directories; project rules load only when trusted; background terminals with `osPid` and terminate | Verified | 2026-10-15 |

## 2. Original draft sources (accessed 2026-10-13 by the drafting agent, not re-read)

OpenAI Codex blog "Codex as a platform", Codex SDK and sandboxing docs, `github.com/openai/codex` (Apache-2.0); Claude Code headless docs (`-p`, `stream-json`, `--bare`, hooks without trust dialog, SIGTERM 143) and permission modes; Grok Build 1.0.52 local manual (headless, agent mode, sessions, sandbox profiles incl. macOS network no-op, permissions, auto-update flags; SDK list incl. Rust `agent-client-protocol`, Go `acp-go-sdk`); `github.com/NousResearch/hermes-agent` (MIT); Tauri distribute docs; `github.com/agentclientprotocol/agent-client-protocol` (protocol version 1, Apache-2.0).

## 3. Local facts on the planning machine (2026-10-13; configurations unchanged)

`codex-cli 0.160.0` (npm wrapper `@openai/codex`; `app-server` and `generate-json-schema` marked experimental; `daemon`/`proxy` subcommands exist); `grok 1.0.52`; Hermes `0.21.6+763.g1fd1827` (git install in `~/.hermes/hermes-agent`, commands `hermes`, `hermes-acp`); Claude Code: `~/.claude` present, `claude` not on PATH.

## 4. Inferred platform facts used

WKWebView renders in separate WebKit helper processes; macOS RSS undercounts compressed memory; same-UID processes can read each other's `0600` files; browsers cannot open Unix sockets; peer credentials via `SO_PEERCRED` / `LOCAL_PEERCRED`; `SMAppService` (macOS 13+) registers bundled launch agents with user approval; App Sandbox blocks spawning arbitrary user binaries.
