# 04 — Sessions, Events, Data and Recovery

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. The normative state machine is in [05-state-machine](05-state-machine.md).

## 1. Identifiers

| ID | Meaning |
| --- | --- |
| `HostId` | stable per daemon installation; regenerated if a data dir copied to another machine is detected |
| `journal_epoch` | random per database creation; part of every cursor; mismatch forces a full client resync (after salvage or a new DB) |
| `WorkspaceId` | stable id of a directory (realpath + Git common dir if any) |
| `SessionId` | Shardrail UUIDv7; never equal to the provider's id |
| `ProviderSessionRef` | (provider, external id) — never replaced by Shardrail ids, or resume breaks after updates |
| `TurnId`, `ItemId`, `ToolCallId`, `ApprovalId` | Shardrail ids linked to external ids where present |
| `CommandId` | client-generated UUID on every mutating request |
| `EventId` | per-host monotonic integer; the replication cursor (with `journal_epoch`) |

## 2. Event tiers (D17)

| Tier | Examples | Stored | Delivered |
| --- | --- | --- | --- |
| Ephemeral | text/thought deltas, command output deltas, plan deltas | memory only (in-progress buffer per item) | coalesced per item at the client flush interval (≤ 1 per frame); delta frames carry provisional item metadata so a client can render before `item_started` is committed |
| Durable | session/turn lifecycle, completed items with full content, approval records and resolutions, commands, notices, state transitions, `provider_reloaded`, `provider_spawned`, leases, append-only checkpoint chunks of in-progress items (every 2 s or 64 KiB, proposed) | journal | by cursor; replayable |

Checkpoint chunks are append-only and joined into the completed item on completion, so writes stay linear in content size. A crash loses at most the text since the last chunk; the provider still holds it.

*Why tiers:* providers stream token-level deltas (thousands per long turn). Journaling every delta inflates the database and the write path; journaling none loses partial output on a crash. The first design assumed ~200 events per turn.

**Envelope:** `event_id`, `journal_epoch`, `host_id`, `session_id`, `schema_version`, `kind`, `occurred_at` (daemon clock), `provider_ref`, `causation` (`command_id` or provider request id), `payload`, redaction flags.

**Journal rules:** events are immutable; corrections are new events; projections are rebuildable folds. Never stored: environment variables, `auth.json` contents, `Authorization` headers, cookies, values matching the secret denylist and format/entropy detectors (redacted copy marked `redacted`; best effort). Per-item stored content is capped at 256 KiB with a truncation marker; full output stays with the provider.

## 3. Projections and paging

The daemon keeps per-session item projections (completed items + live buffers). History views request pages of items by position or `ItemId`; raw event ranges by cursor remain for catch-up. Proposed budget: first visible page of a 50 000-event session in ≤ 300 ms.

## 4. Commands (D30)

- Every mutating request carries `command_id` and optional preconditions (`expected_turn_id`, `expected_state`); stale preconditions are rejected with the current state (Codex `turn/steer`'s `expectedTurnId` is the precedent).
- The daemon journals `command_accepted` **before** side effects and `command_dispatched` after; provider confirmation, errors and crashes move the command through the delivery table (05 §5). Nothing is resent automatically.
- Session commands keep their outcomes for the session's lifetime; host-level commands (create session, pairing, host operations) for 24 h (proposed).
- Approval responses are tracked by the approval record (05 §4), not the delivery table.

## 5. Concurrency between clients

All mutations go through the command router. Approvals resolve exactly once (first authorized response wins; later ones get the final state). Clients see which other clients are attached (presence, no locking). A single writer per host serializes everything; there is no CRDT and no multi-writer merge.

## 6. Journal storage (D5, D26)

- SQLite WAL, one file per daemon, single writer; a second daemon on the same file is blocked by a lock and exits with a clear error.
- Readers use short transactions; explicit checkpoint policy; WAL size alarm.
- **Schema evolution:** each durable event carries `schema_version`; upgrades forward-migrate stored rows after a temporary pre-migration copy, which is deleted after verification. (*Why not perpetual upcasting:* it doubles the mechanisms a solo maintainer carries forever.)
- **Backups:** no automatic journal backups by default; user-initiated export only, with a warning that purge cannot reach exports. The data directory is excluded from Time Machine by default with a visible setting (U23). APFS local snapshots and user copies are a documented residual.
- **Purge (F17):** `PRAGMA secure_delete=ON` on every connection → delete → full `VACUUM` → `wal_checkpoint(TRUNCATE)`; also deletes any pre-migration copy. (*Why this order:* SQLite only zeroes pages if `secure_delete` is on before the delete, and `incremental_vacuum` is a no-op unless auto-vacuum was configured — EV-20.)
- **Corruption:** open read-only, run an integrity check, offer export/salvage; a new database is created only after explicit user choice (new `journal_epoch`). Provider directories are never touched.
- **Retention:** keep durable events by default; compact checkpoint chunks after completion; per-session purge; optional age-based retention (owner setting).

## 7. Recovery

| Failure | Behavior |
| --- | --- |
| `kill -9` / crash of the daemon | WAL recovers committed events. On restart, recorded provider processes are checked by identity (pid + start time + boot id): alive → `orphan_alive` (turn `detached`); dead → `exited(lost)`; Linux cgroup killed by unit stop → `killed_by_restart`. Accepted-but-unconfirmed commands → `delivery unknown`; open tools → `outcome unknown`; pending approvals → withdrawn; delivered-but-unacknowledged approvals → "outcome not confirmed". Nothing is resent. |
| Provider crash / stdio break | process `exited` (or `leader_exited` while descendants run); open tools `outcome unknown`; resume offered per capability, preferring resume without replay; never while the old process or descendants live |
| Protocol desync with process alive | turn `detached`; termination user-confirmed while a client is attached, automatic after a grace period when none is (U22) |
| Hang without break | turn `stalled` after 15 min without events (excluding turns waiting on approvals); no automatic kill |
| Client drops mid-approval | approval stays pending; resolved only by a client, provider withdrawal or policy expiry (never allow) |
| Client reconnects | catch-up by cursor; snapshot + cursor if the gap exceeds retention, the queue overflowed, or `journal_epoch` changed |
| SSH link drops | remote sessions continue; host shown disconnected; reconnect via the bridge |
| Repeated command after network error | same `command_id` returns the stored result |
| Reboot (Linux or Mac) | daemon restarts (Linux via systemd); turns never continue automatically; manual resume |
| Disk full | sessions pause new turns with a notice; durable events are never dropped silently |

## 8. Handoff and tasks (D10)

- A `Task` is a Shardrail object, not a provider session: goal and constraints written by the human, done criteria, links to `SessionId` and `EventId` ranges, file paths and a short diff taken by the daemon, decisions the human marked transferable.
- Transfer creates a new session at the target provider and sends the bundle as the first prompt after the human has seen and edited it. Hermes memory, Grok memory, CLAUDE.md and skills are never read into the bundle automatically; the human may paste quotes.
- Provider fork (`thread/fork`, session fork) is a different operation — a branch of the same agent — labelled Fork, not Transfer.
- A **manual handoff MVP** (edited text bundle → first prompt) is recommended at Phase 1 close (U19); the full Task model is Phase 4.
- No task queues, kanban, or multiple executors per task. Sub-agent activity (Claude `parent_tool_use_id`, Grok subagents) is displayed when it appears in the stream.

## 9. External history (F15)

Sessions created outside Shardrail are viewed read-through via official APIs and cached in memory only; they are never written to the journal. *Why:* journaling them would copy provider transcripts (possibly with secrets) into Shardrail's store, which the data-ownership rule forbids.
