# 01 — Product and Requirements

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. See [00-INDEX](00-INDEX.md).

## 1. Product

Shardrail (Fork Horizon) is an independent workspace for running and supervising several AI coding agents — OpenAI Codex, Anthropic Claude Code, xAI Grok Build and Nous Hermes Agent — on a local Mac and on remote Linux machines, from a desktop app, a browser, and later a phone.

It starts and observes agent sessions, sends instructions, shows tool activity, routes approvals to a human, shows the file tree and Git diff of the directory a session really works in, keeps durable history, and transfers **explicit** task context between agents. It is not a model, not an API proxy, and not a wrapper around a terminal UI.

Existing agent managers were studied only as a source of constraints; their code is not reused.

**Guiding properties, in priority order:** responsiveness; low CPU/memory overhead; visual and interaction quality; reliable execution and recovery; robust security boundaries; extensibility; maintainability; efficient local/remote communication; practical complexity for a primarily solo developer; future commercial options.

## 2. Users and scenarios

One person or a small Fork Horizon team runs several agents on a Mac and on an always-on Linux server. The agents are already installed and logged in by their own tools; Shardrail does not install or bundle them.

1. Open a workspace, see installed agents and versions, start a session with explicit cwd, model and sandbox/approval policy.
2. Watch the stream: text, reasoning, tool calls, diffs, commands, usage where the provider reports it.
3. Answer approvals: allow once, deny, or (from the desktop only) provider-native wider scopes.
4. Queue the next instruction while a turn runs; cancel the current turn.
5. Read the file tree and diff of the directory the session actually uses, including worktrees.
6. After network loss, process kill or reboot, see each session as recoverable or interrupted — never a fabricated success.
7. Hand a task to another agent through a user-reviewed bundle.
8. From a phone: watch progress, answer approvals, send a short instruction (phone never executes tools).
9. See everything that needs attention, across all sessions and hosts, in one inbox.
10. Browse, read-only, sessions that were started outside Shardrail, and continue them as new Shardrail sessions through official provider APIs.

## 3. Scope

**In product:** local daemon + macOS desktop app; browser client to the same daemon; headless Linux daemon (systemd user service); Android client (later phase); four adapters (Grok, Hermes, Codex, Claude); event journal, instruction queue, approvals, read-only files and Git; host management over SSH; attention inbox; handoff bundles; recovery within what providers officially support.

**Out of product (and why):**

| Excluded | Reason |
| --- | --- |
| Own model, API proxy, token billing | Shardrail only shows what providers report |
| Packaging or auto-updating provider CLIs | Their licenses and update channels; user updates them, Shardrail checks versions |
| Capturing an already open TUI, IDE extension or another desktop app; screen scraping; Accessibility automation; sending keystrokes to terminals | No provider offers an official attach; scraping breaks on every UI change |
| Hermes messaging gateway, cron, kanban; wake word; computer use | Provider features, not session management |
| Codex Cloud as an execution environment in the MVP | Not needed for local/remote supervision |
| Human file editing in the UI (MVP) | Two writers on one tree race; view and diff only |
| MCP bus between agents | MCP stays "agent → tools"; Shardrail shows MCP calls agents make |
| Microservices, message brokers, CRDTs, search clusters, Kubernetes | Unnecessary for one owner per host |
| Running agents on Android/Termux | Phone is a client only |
| Multi-tenancy and team ACLs | v1 is single-owner per machine |
| Merging several hosts' journals into one daemon-side store | Unified view is built client-side instead (see 02) |

**Lessons taken from other agent shells (as constraints only):** a common sub-protocol covers turns, streaming and permissions, but sandbox, worktree, fork and cost stay provider-specific; driving an agent through its TUI breaks on the first UI change; "shared context" between agents is a document the user sees, not shared model memory.

## 4. Functional requirements

| ID | Requirement | Acceptance (future) |
| --- | --- | --- |
| F1 | Discover provider binaries, show path and version, classify each version as `tested`, `untested-newer` or `known-bad`. `known-bad` does not start without override; `untested-newer` starts with a warning in the strictest mode (strictest sandbox, no widening or boundary-breaking options, no allow-all). | Fixture with substituted `--version` per class |
| F2 | Create a session with cwd, model and permission policy. | Protocol fixture per adapter |
| F3 | Send an instruction and show the stream until the stop reason. | Fixture streams; UI renders the final frame |
| F4 | Show tool calls: name, target, status, truncated input and result; raw input on expand. | Tool card fixture |
| F5 | Route approvals to the provider. An approval resolves exactly once; client disconnect or window close never resolves it; expiry follows policy and never means allow. | Fake agent never receives allow after expiry; concurrent responders |
| F6 | Cancel the current turn through the provider's own mechanism. | Fixture + live spike |
| F7 | One pending instruction per session when the provider does not accept a second turn. | Second instruction not sent until the turn closes |
| F8 | Read-only tree and diff of the session directory; unknown directory shown as unknown. Viewing never executes any program named by Git configuration, attributes or environment; discovery stays inside the workspace; symlinks out of the workspace are not followed; LFS compared natively; never "modified" from an unfiltered comparison. | Malicious-repo fixture set (see 06 §6) |
| F9 | Journal survives `kill -9` of the daemon; session list and last confirmed cursor are intact. | SQLite integration test |
| F10 | Provider process death marks the session degraded; resume offered only when the capability exists. | stdio break fixture |
| F11 | A handoff bundle opens as the first prompt of another session; the user sees and edits it before sending. | Bundle test; denylisted secrets absent |
| F12 | A client disconnected for 30 s catches up from its cursor without duplicates. | Cursor test |
| F13 | Workspaces have trust states `unknown` / `restricted` / `trusted`. In `unknown`/`restricted`, no session starts that loads project-level executable configuration (hooks, MCP server definitions, project rules) for **any** provider. For Claude this means SDK `settingSources: []` plus auto memory and claude.ai connectors disabled (exact switches confirmed in spike S4). | Fixture repo with hooks, CLAUDE.md and a memory-writing prompt: no hook runs, no memory written |
| F14 | Attention inbox across sessions and hosts (pending approvals, failures, delivery-unknown, completions); OS notifications when the window is hidden. | Approval on host B appears while viewing host A |
| F15 | Read-through viewing of external session history via official APIs (memory cache only, never journaled); continue as a new session via official resume/fork. | Codex `thread/list`/`read`, Claude `listSessions`/`getSessionMessages`, Hermes list/load |
| F16 | Host management: add/remove remote hosts through the user's system SSH configuration; per-host health. | Bridge through SSH config with a jump host |
| F17 | Per-session purge: content unrecoverable from the database, WAL and Shardrail-managed copies. | Forensic check after purge |
| F18 | Delivery uncertainty is visible (`delivery unknown`, `outcome unknown`); nothing is resent automatically. | Crash between accept and dispatch |
| F19 | One write-capable session per worktree by default; the lease is visible and journaled. | Second session refused or read-only |
| F20 | Agent processes and all their descendants are contained; after a daemon restart, survivors are detected and a session is never resumed while its previous process lives. | Kill daemon during a detached tool |
| F21 | Approval outcomes are never misreported: a delivered decision shows as allowed/denied or "outcome not confirmed", never as withdrawn. | Crash between response and acknowledgement |
| F22 | Approvals that act outside the session sandbox are labelled boundary-breaking, desktop-only, and end host approval protection when allowed. | Card and state test |

## 5. Non-functional requirements

| ID | Requirement |
| --- | --- |
| N1 | Performance budgets (07) are measured on the reference machine before a phase is declared closed. |
| N2 | At rest, Shardrail-owned code runs no periodic timer more often than every 15 s; stdio breaks, FSEvents/inotify and explicit refresh replace polling. Timers may be armed only while data flows. |
| N3 | Provider credential stores are never read. The journal is classified sensitive because tool output can contain secrets; redaction is best effort; retention and purge exist. |
| N4 | The daemon listens only on a user-owned Unix socket (`0700` dir, `0600` socket) by default; an optional loopback WebSocket listener is opt-in. Every client authenticates with an asymmetric key and authenticates the daemon. |
| N5 | Failure of one adapter does not stop the others. |
| N6 | Every decision has a reconsideration condition; a provider protocol change is always sufficient. |
| N7 | Core flows (stream, approval, composer, tree) target WCAG 2.2 AA. |
| N8 | Dependency licenses **and usage terms** permit Fork Horizon's intended distribution; copyleft dependencies need a separate legal decision. |
| N9 | Approval correctness and secret isolation are never traded for latency budgets. |
| N10 | Approval protection is stated per host and UID, never claimed beyond what is enforced (06 §3). |
| N11 | Untrusted content renders without active content or remote loads; approval cards reveal hidden characters. |
| N12 | Idle efficiency is measured, including wakeups and energy impact (07). |
