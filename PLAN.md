# Shardrail — Master Plan (DRAFT v2, 2026-10-30)

Status: structure approved by the owner on 2026-10-30; open questions in §8 are waiting for answers.
This is the **single** living plan. Older documents (`planning/*`, `docs/dev/*-status.md`) are archive/reference only.
The plan is written for the executing agent: one task = one iteration, and every iteration follows the template in §4.
Language: English only (the owner and agents work best this way). The owner may talk to you in Russian; the repository stays English.

## 1. Product

One place to control all AI coding agents on all of the owner's machines. Not an observer: read, write, enter sessions,
and do everything the agent itself can do.

- Agents: **Codex** and **Claude Code** first. Others (Antigravity, Grok, Hermes, ...) are added later, one task each.
- Every agent has two forms: a **terminal (CLI)** form and a **desktop app (GUI)** form. Access to both is required.
- Mandatory: **reading, full synchronization, writing, continuing existing sessions**.
- Machines today: MacBook M5, MacBook M1, and a purchased server. A new machine = install Core and pair it. The system must stay extensible.
- N1 interface: web. Native and mobile apps are N2.
- One user (the owner); internal tool.

Navigation hierarchy: `Machines → Agents → Sessions → Messages`, plus returning to an existing session and continuing it.

Priorities: **N1** (core) is mandatory and comes first. **N2** (nice to have) starts only after N1 is accepted. N2 must never reduce N1 reliability.

## 2. Repositories (workspace map: `../CLAUDE.md`, code graph: `../graphify-out/`)

| Repository | Role |
|---|---|
| `Shardrail_Core` | daemon, journal, agent adapters, contracts, client library (installed on every machine) |
| `Shardrail_Web` | web UI, relay/hub, deploy, **this plan** |
| `Shardrail_Android` | Android client (N2; created when the mobile stage begins) |
| `Shardrail_iOS` | iOS client (N2; created when the mobile stage begins) |

The workspace root `Shardrail/` is not a repository. It holds `CLAUDE.md` (map of repositories and their relations for agents) and `graphify-out/` (code graph across repositories). When repositories appear or change, the agent updates `CLAUDE.md` and rebuilds the graph (`graphify update .`).

## 3. Architecture (short)

- **Shardrail_Core** is installed on each machine: the `shardraild` daemon, journal, agent adapters, protocol and contracts, client library.
- **Shardrail_Web** is the web UI plus the relay/hub (server side) plus deploy assets.
- Each daemon keeps an outbound connection to the relay; browsers talk to a machine through the relay. End-to-end encryption and key pairing (archive decisions D67/D69/D70) are kept.
- Extensibility: an agent adapter is a module behind a common interface (list sessions, read, live sync, send, interrupt, approvals). A new agent or a new machine must not change the core.

## 4. Iteration template (one task = one pass)

Every task below is executed by the agent in exactly these steps. A task description lists only what is specific to it.

1. **Research.** Read the current code, this plan, and the rules (§7). Find and verify official documentation and the real behavior of the agent (version, storage format, API). Write findings to the task note (`docs/tasks/<id>.md`): what is established, what is unknown.
2. **Design.** A short list of changes and acceptance criteria. If a decision belongs to the owner, stop and ask; do not guess.
3. **Implementation.** The minimum necessary code on a dedicated branch `task/<id>-<slug>`. Rust contract types are the source of truth; TypeScript is generated.
4. **Tests.** Unit and integration tests (with a fake agent). For real agents, a live run against the test repository (§7.4). New behavior without a test is not accepted.
5. **Self-review.** Re-check the acceptance criteria, run the full check set (fmt, clippy, test, deny, typecheck, schema drift), re-read your own diff as a reviewer (security, error handling, excess code). List separately what was **not** verified.
6. **Deploy.** Build and install on the target machines (locally; on others when connected). Run a smoke check on the **installed** build, not only from source.
7. **Git and docs.** Meaningful commits, PR, green CI, merge to `main`, a tag at the end of a stage. Update this `PLAN.md` (task status) and `docs/tasks/<id>.md` in the same PR. Code and docs must not drift apart.
8. **Report to the owner.** What was done, what was verified and how, what was not verified, which decisions are needed.

Definition of done: acceptance criteria met, CI green, smoke on the installed build passed, docs updated, PR merged.

## 5. Changes versus the old plan (record as owner decisions)

| Before | Now |
|---|---|
| Role "supervisor", only own sessions | Full control, including sessions started outside Shardrail |
| Capturing open terminals and typing into them excluded | Access to CLI and GUI, attach and sync are required; the method is decided by research |
| Codex started with `ephemeral: true` | Sessions are persisted and resumable |
| Reading external history is a late phase, "memory only" | Core of N1 |
| Many safety gates before usability | Keep E2E, pairing, fail-closed approvals. Everything else as needed |
| Agents must not be run without permission | Allowed on the test repository under §7.4 |

## 6. Stages and tasks (detailed)

Markers inside tasks: **[R]** research (result = findings in `docs/tasks/<id>.md` with agent versions, commands and evidence), **[D]** develop, **[T]** test, **[V]** self-review, **[P]** deploy, **[G]** git/docs. All steps of the §4 template are mandatory; this section spells out the specifics. Anything called a "hypothesis" must be verified on the installed version, not taken on faith.

Stage gate: all tasks of the stage accepted by the owner; CI green in every touched repository; `PLAN.md` updated.

---

### Stage 0. Put the project in order

**T0.1 Inventory and split map** [DONE 2026-10-31, see `docs/tasks/T0.1-split-map.md`] (analysis only, no changes)
- [x] [R] Build a table "directory / crate / package → Core | Web | archive | delete-after-confirmation". Cover `crates/*`, `packages/*`, `schema/`, `scripts/`, `test-vectors/`, `apps/*`, `deploy/`, `docs/`, `planning/`, `spikes/`, `archive/`, `.github/workflows`, `deny.toml`, `rust-toolchain.toml`.
- [x] [R] Find dependencies between parts: what `relay` and `shardraild` share; what the UI needs from `protocol` (via `packages/contracts`); where the UI gets `packages/client`; what `s5-prototype` uses from `s5-swift`/`src-tauri`.
- [x] [R] Find every path hard-coded in scripts and CI (`scripts/*`, `.github/workflows/ci.yml`, `dev-try.sh`, `build.rs`, which compiles in a git-root path) that will break after the split.
- [x] [R] List branches (`w7…w12`, `p1a/*`, `s1/*`): which are merged into `main`, which are not, and what unmerged work they carry.
- [x] [D] Write `docs/tasks/T0.1-split-map.md`: the final map, risks, order of actions.
- Acceptance: the owner approved the map. T0.2 does not start without it.

**T0.2 Create Shardrail_Core** [DONE 2026-10-31, see `docs/tasks/T0.2-core-setup.md`]
- [x] [D] Repository `Shardrail_Core` (already created on GitHub, empty): move code per the T0.1 map as a clean import (one initial commit; the message links the source repository and source commit).
- [x] [D] Cargo workspace (protocol, journal, session-core, coordinator, shardraild, fake-acp, and the relay-client part if any), `packages/contracts`, `packages/client`, `schema/`, `rust-toolchain.toml`, `deny.toml`, `Cargo.lock`, npm lockfiles.
- [x] [D] Remove hard-coded absolute paths and the `build.rs` binding to a foreign git root.
- [x] [D] CI: fmt, clippy, test, deny, contract generation + drift check, client typecheck and tests.
- [x] [D] Choose how contracts are published to Web: a versioned dependency (git tag or package). Justify the choice in the task note.
- [x] [T] Fresh clone → build → all checks green. Daemon tests with the fake agent pass.
- [x] [P] Build a release `shardraild` on the M5.
- [x] [G] Protect `main`, PR template, tag `v0.0.1-import`.
- Acceptance: Core builds and tests independently of Web.

**T0.3 Create/clean Shardrail_Web**
- [x] [D] Web keeps: the UI (rename `s5-prototype` to a meaningful name, e.g. `apps/web`), `relay`, `deploy/`, `PLAN.md`, `AGENTS.md`, `docs/`, the document archive (`planning/` marked archive).
- [x] [D] The UI consumes contracts and the client from Core by the method chosen in T0.2, with no copies.
- [x] [D] Take out of the active tree: `spikes/`, `apps/s5-swift`, `src-tauri`, `archive/`, logs, screenshots. Do not delete irreversibly: move to an archive branch or a separate archive repository first; delete only after owner confirmation.
- [x] [D] Review Web CI: UI build, UI tests, relay build; E2E tests that need the daemon take the binary from a Core release.
- [x] [T] Fresh clone of Web → build UI and relay → tests. The Web + Core E2E link works with the fake agent.
- [x] [P] Deploy UI + relay locally per `deploy/README.md` and confirm the instructions work.
- [x] [G] Protect `main`, initial commit, tags.
- Acceptance: Web builds independently; the Web + Core combination passes E2E with the fake agent. (Details: `docs/tasks/T0.3-web-setup.md`)

**T0.4 Documentation and rules**
- [x] [D] Keep `PLAN.md` in Web; sync the workspace-root `Shardrail/CLAUDE.md` (repositories and relations) and a `README` in each repository (what it is, how to build, how to run, where the plan is).
- [x] [D] Split rules: `AGENTS.md` in Core (code, security, dependencies) and in Web (UI, relay, deploy); common rules by reference to §7 of this plan. Remove outdated statements ("Stage 0, no code", "do not run providers", "endless gates").
- [x] [D] `planning/*`: add `planning/README.md` saying "ARCHIVE, source of truth is PLAN.md". Keep the decision log and security docs as reference; mark decisions that contradict the new course (§5) as "superseded".
- [x] [D] Add the template `docs/tasks/_template.md` for task notes.
- [x] [T] Link check across documents (script or manual): no link to a non-existent path.
- [x] [G] Rebuild the code graph with `graphify update .` from the workspace root.
- Acceptance: a new agent entering the workspace root finds where everything is and what to do next within 5 minutes. (Details: `docs/tasks/T0.4-docs-and-rules.md`)

**T0.5 Baseline and test infrastructure**
- [R] Versions and installation of Codex and Claude Code on the M5 (and M1 if reachable): `codex --version`, `claude --version`, how login works, where configs and sessions live. Read-only; do not run models.
- [D] Create the test repository `Shardrail_TestRepo` (no secrets) for live runs, plus a reset-to-initial-state script.
- [D] Live-run protocol (who, when, what is allowed/forbidden; logs must not contain secrets). Agree it with the owner.
- [T] Full run of Core and Web checks. Record "green / red / flaky".
- [T] Live smoke of Codex through the existing daemon (after the owner's permission): create a session, send a prompt, receive an approval, interrupt.
- [V] "Day 0 state" report: what really works and what does not.
- Acceptance: the owner has an honest list of "what works today"; all later work starts from it.

---

### Stage 1. Machines

**T1.1 Machine model and registry**
- [R] How a machine, pairing and relay discovery work now (`relay`, `shardraild`, UI code); what is already stored (keys, `machine_id`).
- [D] Machine model: id, name, OS/architecture, daemon version, discovered agents (name, version, forms CLI/GUI, install path).
- [D] Agent discovery on a machine: presence of `codex`, `claude`, and GUI apps (install paths on macOS/Linux), versions.
- [D] Daemon heartbeat → relay; online/offline status with reason and last-contact time.
- [T] Unit and integration tests; scenarios: daemon stopped, relay restarted, network lost.
- Acceptance: the UI shows the machine list with state and discovered agents.

**T1.2 Install Core on a new machine**
- [R] macOS (launchd) and Linux (systemd) requirements; where to keep data and logs; how to update binaries.
- [D] Install script/instructions: download a release, install the service, autostart, pair with the hub using one code.
- [D] In-place Core update command without data loss.
- [T] Install from scratch on a clean macOS profile; on Linux in a container/VM at least until the server is available.
- [P] Install on the M5, M1 and the server. **Needed from the owner: server OS, architecture, access.**
- Acceptance: a new machine is added from short instructions in minutes and appears in the UI.

**T1.3 Relay and web console on the server**
- [R] Current configuration (`deploy/`: nginx, systemd), TLS, domain, what production requires; hub threat model (archive D67/D69/D70).
- [D] Production deployment, autostart, log rotation, zero-downtime update procedure.
- [T] Checks: daemon connection loss, relay restart, several machines at once, several browsers, revoking a paired device.
- Acceptance: M5, M1 and the server are visible on the web from any browser; device revocation works.

---

### Stage 2. Codex: read and sync

**T2.1 Codex research (the result shapes the rest of the stage)**
Answer with evidence (commands, excerpts, version):
- [R] Where and in what format Codex stores sessions (hypothesis: `~/.codex/sessions/.../rollout-*.jsonl`), the record types, whether the format is stable between versions.
- [R] What `codex app-server` offers: list/read/resume of threads (hypothesis: `thread/list`, `thread/read`, `thread/resume`), events, approvals; how it differs from reading files.
- [R] Whether `app-server` sees sessions started from the CLI and from the app; whether the CLI and the Codex GUI app share storage; whether one process can read a session that is active in another.
- [R] What happens when the app/CLI and a third-party client work with one session at once (locks, conflicts, duplicates).
- [R] Whether state can be **changed** (send messages, interrupt, change settings) through CLI modes (`codex exec`, `resume`), through app-server, through the GUI app; which of these are officially supported.
- [R] How to tell that a session is active right now (process, lock, file freshness).
- [R] Limits: sizes, encryption, what is persisted (reasoning, tool calls, diffs).
- [D] Output document: a matrix "capability × method (files / app-server / CLI / GUI)", marked works / partially / impossible, with a recommended method for reading, syncing and writing.
- Acceptance: the owner approved the recommendation; open risks are listed.

**T2.2 Agent adapter interface (shared by Codex and Claude Code)**
- [D] Design from T2.1 (and preliminary Claude Code data from T4.1): list sessions, read history (paged), subscribe to new events, send, interrupt, approvals, session state, adapter capability flags.
- [D] A normalized message/event model sufficient for both systems; extensible for new agents.
- [T] Contract tests against a fake adapter.
- [G] Contracts live in Core (Rust → schema → TS). The drift check is green.
- Acceptance: the interface is approved, with no Codex-specific leakage into the shared layer.

**T2.3 Codex: session list**
- [D] Adapter: all sessions on the machine (from CLI and app), metadata (id, working directory, title, time, model, state).
- [T] Fixtures from real records of several Codex versions; test for a corrupted/partly written file; live test on the test repository.
- Acceptance: the list matches what Codex itself shows.

**T2.4 Codex: full history**
- [D] Paged reading of all messages/events, including tool calls, diffs, command outputs, with no loss and no duplicates. Large sessions are not loaded whole.
- [T] Compare with the source (counts, checksums), a session of thousands of records, unusual characters.
- Acceptance: the UI shows the same messages as Codex, in the same order.

**T2.5 Codex: live sync**
- [D] New events from a session that the CLI or the app is writing to appear in Shardrail in real time; cursors, de-duplication, recovery after a break.
- [T] Scenarios: write from CLI → watch on web; write from app → watch on web; daemon restart in the middle; delay/loss of connection.
- Acceptance: latency and integrity are measured and recorded; no loss, no duplicates.

**T2.6 UI: Machines → Agents → Sessions → Messages (Codex)**
- [D] Navigation across the hierarchy, session search/filters, message rendering (markdown, diffs, commands) in a safe mode (untrusted output).
- [T] E2E (Playwright), accessibility, large lists.
- [P] Deploy on the server and check from a phone and a laptop.
- Acceptance: any Codex session on any connected machine can be reached and read in full.

---

### Stage 3. Codex: write and continue

**T3.1 Resume a session**
- [R] From T2.1 pick the resume method (app-server or CLI); what is preserved and lost; how to remove `ephemeral` without side effects; effect on the session file.
- [D] Continue an existing session keeping its context; new sessions are persisted too.
- [T] A session started in the CLI → continued from Shardrail → opened in the CLI: one history, context kept (verify that the model "remembers").
- Acceptance: the end-to-end scenario above passes on live Codex.

**T3.2 Send messages and commands, interrupt**
- [D] Send a prompt to a session, instruction queue, interrupt, turn status (running / waiting / finished).
- [T] Send during a running turn, retry of a command after a break (idempotency by `command_id`), interrupt.

**T3.3 Approvals**
- [D] Show confirmation requests (commands, edits), decisions allow-once/deny; deny by default on timeout, disconnect, journal error; write intent to the journal before answering the provider.
- [T] Failure and fault scenarios, like the existing daemon tests; a live run.

**T3.4 Concurrent-writer policy**
- [R] From T2.1: what really happens with two writers.
- [D] Decision (the owner approves): lock/indicator "session is active elsewhere", warning, a "take over" mode.
- [T] Conflict scenarios CLI/GUI + Shardrail.

**T3.5 Close the stage**
- [T] Regression: reading (stage 2) still works; a load run; repeat on the M1 and the server.
- [P] Deploy to all machines.
- Acceptance: Codex is fully controllable from the web on three machines.

---

### Stage 4. Claude Code

**T4.1 Claude Code research (the result shapes the rest of the stage)**
Answer with evidence (commands, excerpts, version):
- [R] Session storage (hypothesis: `~/.claude/projects/<encoded path>/<id>.jsonl`): format, record types, stability across versions, how subagent/sidechain records and compaction appear.
- [R] Programmatic interfaces: the Agent SDK (`listSessions`, `getSessionMessages`, resume), headless `claude -p` with `stream-json` input/output, hooks. For each: officially supported or not, what it can read and write.
- [R] Terms of use: which authentication the owner's usage is allowed under (own subscription or API key) for programmatic control; record the conclusion and any restrictions.
- [R] Whether a message can be sent to an **already running** interactive process; what hooks can and cannot do; behavior when a session is active in another process.
- [R] Whether the CLI, the desktop app and IDE extensions share storage and whether they see each other's sessions.
- [R] Permissions/approvals: how they work in the SDK and in the CLI; how to route them to an external client; settings that must be pinned for safety.
- [R] How to detect an active session; locks; what happens to a session file when it is resumed from a second process.
- [D] Output document: the same capability matrix as in T2.1 and a recommendation; list where Claude Code does not fit the interface from T2.2.
- Acceptance: the owner approved; differences versus Codex are listed.

**T4.2 Interface adjustment (only if T4.1 requires it)**
- [D] Propose the minimum change to the T2.2 interface as a separate owner decision. Keep Codex working.
- [T] Contract tests plus full Codex regression after the change.
- Acceptance: both agents fit the shared interface without agent-specific leakage; skipped if nothing needs changing.

**T4.3 Claude Code adapter: session list and full history**
- [D] All sessions on the machine (CLI, desktop app and IDE where they share storage), metadata (id, working directory, title, time, model, state), paged history including tool calls, diffs and outputs.
- [T] Fixtures from real files of several versions; corrupted/partly written file; very large session; live check on the test repository.
- Acceptance: list and history match what Claude Code itself shows.

**T4.4 Claude Code adapter: live sync**
- [D] New records from a session written by the CLI/app appear in Shardrail in real time; cursors, de-duplication, recovery after a break (mirrors T2.5).
- [T] Write in CLI → see on web; write in app → see on web; daemon restart; connection loss.
- Acceptance: latency and integrity measured and recorded; no loss, no duplicates.

**T4.5 Claude Code adapter: resume, send, interrupt**
- [D] Resume an existing session without losing context, send messages/commands, queue, interrupt, turn status (mirrors T3.1–T3.2).
- [T] Session started in the CLI → continued from Shardrail → opened in the CLI with one history; send during a running turn; command retry idempotency; interrupt.
- Acceptance: the end-to-end scenario passes on live Claude Code.

**T4.6 Claude Code approvals and permissions**
- [D] Route permission requests to the UI with allow-once/deny; deny by default on timeout, disconnect or journal error; journal intent before answering; pin safe permission settings.
- [T] Failure scenarios, live run on the test repository; verify no path produces an implicit allow.
- Acceptance: behavior matches Codex approvals (T3.3).

**T4.7 Concurrent-writer policy for Claude Code**
- [D] Apply the T3.4 policy (lock/indicator/take-over) on the data from T4.1.
- [T] Conflict scenarios CLI/GUI + Shardrail.

**T4.8 UI for Claude Code and mixed use**
- [D] Same hierarchy UI; agent differences (capabilities, formats) shown honestly via capability flags.
- [T] E2E with both agents at once, several machines.
- [P] Deploy to all machines.
- Acceptance: the end-to-end scenario (machine → agent → session → messages → continue) passes for Claude Code exactly as for Codex.

---

### Stage 5. Terminal and desktop apps (research and owner decision first)

**T5.1 Research: how to launch and attach to an agent in a terminal**
- [R] Options: (a) Shardrail launches the agent itself in a managed PTY (the daemon holds the process; the browser attaches and re-attaches); (b) a wrapper `shardrail run <agent>` launches the agent with attach support; (c) attaching to a process the owner started by hand in an ordinary terminal (tmux/iTerm/Terminal).
- [R] For each option: what is possible on macOS and Linux (permissions, TIOCSTI/input injection limits, tmux integration, `reptyr`-like tools), what the user sees in the original terminal, how screen sync and input work, what breaks, security impact.
- [R] Compatibility with the "session is a file" model (stages 2–4): can read-from-files plus managed launch for writing be enough.
- [R] What "stop and resume under Shardrail" costs for option (c): what is lost (the running turn), what is kept (context from the session file), and how to make this explicit and safe in the UI.
- [D] A document with a recommendation; prototypes of the two most likely options (agree with the owner).
- Acceptance: **owner decision**. T5.3 does not start before it.

**T5.2 Research: desktop (GUI) apps**
- [R] For the Codex app and Claude desktop (later Antigravity): where the data lives, whether the backend/storage is shared with the CLI, whether a local API/IPC/protocol exists, whether a message can be sent programmatically, how the app reacts to external changes of session files.
- [R] What is possible without hacks (official interfaces) versus only via UI automation (Accessibility). For each item rate reliability and risk and propose the acceptable boundary.
- [D] A table "app × (read / live / write / launch)" with ratings and a recommendation.
- Acceptance: owner decision for each app.

**T5.3 Terminal access implementation** (per the T5.1 decision)
- [D] PTY layer in the daemon, output streamed to the browser (terminal in the web), input, resize, reconnect, permission limits, replay of recent output on attach.
- [D] Link a terminal session to its agent session (stages 2–4) so the chat view and the terminal view show the same session.
- [T] Long sessions, network breaks, several browsers attached at once, control sequences treated as untrusted input, large outputs.
- [P] All three machines.
- Acceptance: the owner can launch (or attach to) a Codex and a Claude Code terminal session from the web, type, close the browser, return, and continue.

**T5.4 Desktop app access implementation** (per the T5.2 decision)
- [D] Scope is defined by the findings: at minimum read and live sync; write only if there is a reliable method.
- [T] Scenario "I write in the app → I see it in Shardrail" and back, where possible; behavior when the app is closed or updated.
- [P] All three machines.
- Acceptance: documented per-app support level matching the owner decision.

---

### Stage 6. Reliability and N1 acceptance

**T6.1 Fault tolerance**
- [T] Scenarios: daemon restart in the middle of a turn, relay restart, machine reboot, browser tab closed, network change, disk full on a machine, clock skew. Pass condition for each: journal intact, nothing lost or duplicated, statuses honest, pending approvals fail closed.
- [D] Fix everything found; add a regression test for each fix.
- Acceptance: a written fault matrix with results, all rows pass.

**T6.2 Sync correctness under stress**
- [T] Simultaneous writers (CLI + Shardrail + app) on one session, rapid message bursts, very long sessions, reconnect storms. Verify ordering, no duplicates, correct conflict indication (T3.4/T4.7).
- Acceptance: no data corruption; behavior matches the written policy.

**T6.3 Performance and long runs**
- [T] Measure first, then set targets with the owner: event latency from agent to browser (p50/p95), time to open a large session, daemon memory and CPU idle and loaded, relay load with several machines, a 24-hour soak with real use.
- [D] Fix regressions; record the numbers in `docs/tasks/T6.3.md` as the baseline for future work (use `planning/07-performance.md` only as a reference).
- Acceptance: numbers recorded and accepted by the owner.

**T6.4 Security review**
- [T] Re-check pairing, Host/Origin, E2E, device revocation, relay compromise assumptions, secrets absent from logs/journal/URLs, untrusted agent output rendering, terminal escape handling, loss of a paired device.
- [D] Fix findings; document the remaining accepted risks.
- Acceptance: a short security report; no open high-severity finding.

**T6.5 Install, update and recovery drills**
- [T] Fresh install on a clean machine from the instructions; in-place update of Core and Web; restore after losing a machine; adding a fourth machine as a dry run.
- Acceptance: each drill passes from the written instructions alone.

**T6.6 N1 acceptance**
- [V] Report against the N1 criteria (§1): for Codex and Claude Code on the M5, M1 and the server: open the machine → agent → session → read history → continue work → result visible everywhere. Includes terminal and GUI support level per T5.
- [G] Tag the N1 release in every repository; update the root `CLAUDE.md` and rebuild the code graph.
- Acceptance: the owner signs off. Only then is N2 planned.

---

### N2 (after N1 acceptance; every item is its own stage using §4 and begins with research)

Each N2 item below starts with a research task [R] that produces findings and an owner decision, then development, tests, deploy and docs per the template. Items can be reordered by the owner.

- **N2.1 Agent limits monitoring.** [R] How each agent exposes usage/rate limits (CLI output, app, API, local files) and how often they can be read safely. [D] Collector per adapter, per-agent limit view in real time. [T] Fixtures from real outputs; behavior when limits are unknown.
- **N2.2 Chat statistics.** [R] What metrics each agent records (tokens, context window usage, cost, tool counts). [D] Per-session statistics view. [T] Compare to the agents' own figures.
- **N2.3 Chat data collection and storage.** [R] Retention, privacy, size, encryption-at-rest and what the hub may store (the hub must stay blind). [D] Database storage for collected data and an export path for analysis. [T] Integrity, migration, restore.
- **N2.4 Notifications.** [R] Delivery channels (web push, mobile push, others) compatible with the blind hub (no content in notifications). [D] Triggers: agent finished, agent waiting for decision/input/approval. [T] Delivery, de-duplication, no content leakage.
- **N2.5 Mobile apps.** [R] Choose technology and distribution for Android and iOS; pairing and device revocation on phones. [D] Create `Shardrail_Android` and `Shardrail_iOS`; read, send, approve. [T] Real devices, poor networks. [G] Update the root `CLAUDE.md` and graph.
- **N2.6 Agent lifecycle management.** [R] What each agent allows for start/stop/restart/update. [D] Remote start/stop/restart/update with safe confirmations. [T] Running turns, failures mid-update, rollback.
- **N2.7 Offline workflow and synchronization.** [R] Queue semantics, conflict rules, expiry of stale commands. [D] Local queue of messages/commands when offline; automatic delivery when the connection and machine return; visible queue state. [T] Long offline periods, duplicates, conflicting edits; nothing is lost silently.
- **N2.8 More agents.** One task series per agent (Antigravity, Grok, Hermes, ...): research (T2.1/T4.1 style), adapter, UI, tests.
- **N2.9 More machines.** Add a machine using the T1.2 procedure; fix whatever the dry run exposes.

## 7. Rules for the executing agent

1. **Priority:** N1 before N2; N2 never reduces N1 reliability.
2. **Keep from the old `AGENTS.md`:** Rust contract types are the source of truth (JSON Schema → TS; never hand-edit generated files); the journal is append-only; approvals never default to `allow` and fail closed on error/timeout/disconnect; dependency allowlist (a new dependency needs owner approval and a record); `unsafe_code = forbid`; never commit a red tree.
3. **Do not guess.** Do not rely on memory of how Codex or Claude Code work: verify against documentation and the real behavior of the installed version; record the version in the task note.
4. **Live agents.** Allowed only on the test repository without secrets, using the owner's own login; API keys never enter code or logs. Any run that touches the owner's real projects needs explicit owner consent.
5. **Security stays:** E2E, pairing, Host/Origin checks; secrets never in the journal or URLs.
6. **Git.** A branch per task, clear commits, PR, green CI, merge to `main`. Update docs and the plan in the same PR. No force-push to `main`; no deleting branches or history without confirmation.
7. **Honest reporting:** failing is reported as failing; unverified is named unverified.
8. **Change the plan only in this file**, and only with the owner's knowledge.
9. **Scope.** Do exactly the task assigned. Out-of-scope findings go into the task note or the report; do not fix them silently.

## 8. Open questions for the owner

- ~~Repository history~~ — **decided 2026-10-30: clean import** (one initial commit in each of Core and Web; old history stays in the source repository `Shardrail`).
- ~~Where `PLAN.md` lives~~ — **decided: `Shardrail_Web`**.
- Server OS, architecture and access; how Core is installed on a new machine (needed in Stage 1).
- How agents are launched (Stage 5) — after research T5.1.
- Concurrent-writer policy for one session (T3.4, T4.7).
- Permission for live runs of Codex and Claude Code on the test repository (§7.4); needed in T0.5.
- Targets for performance numbers (T6.3) after measurement.
