# 05 — Session State Machine (D18)
> 2026-10-20 (owner): D49–D54 recorded — React retained on partial S5 evidence; journal opens atomically without following symlinks; daemon refuses network/synced data filesystems; Codex is first in P1a; tokio/clap/nix are allowed daemon dependencies; contracts remain generated with client helpers in `packages/client`.

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**; normative for detailed planning of the session engine once approved. 2026-10-19 (owner): U21 answered — v1 never shows a 'protected' status; each session shows a verification checklist (D47); S7 gates any future 'protected' status, which needs a separate decision. 2026-10-19 (owner): remaining owner questions answered in bulk (17); Hermes not in Phase 1 (D48); core spikes S1-lite, S2, S5, S9-lite authorized. U22: desynced processes are never terminated automatically.
> Why separate dimensions: a single linear state (`starting → running → waiting_approval → idle`) could not represent parallel approvals, cancellation in progress, a live process with a broken protocol link, or survivors after a crash.
> All timers are PROPOSED defaults (§6). "Tracked descendants" = processes contained per D32 (Linux: cgroup, a guarantee; macOS: best effort).
> Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): write-ahead approval responses (D34), queued instructions (§5a), session lifecycle (§9), journal-unavailable behaviour (§10), planned restart / drain (D39), ACP v2 acknowledgement rules (D35).

## 0. Cell kinds and conventions

Every (state, event) cell is one of:
- **`→ state [+ action]`** — transition.
- **`ignore`** — idempotent repeat (already dedup'd by D30 `command_id` where applicable); no notice.
- **`route`** — event belongs to another handler (session-level updates such as available commands; late tool updates attached to an earlier turn); no state change here. **Exception:** agent-initiated mode or config-option changes (ACP `current_mode_update` / `config_option_update`, tool kind `switch_mode`, Claude permission-mode changes) are policy events handled by the approval broker (06 §3a, D35), not silently routed.
- **`impossible → notice`** — any cell not listed in a row. A conformance test suite checks all four kinds.

Event ordering on daemon restart: session lifecycle (§9) → process (§1) → link (§2) → turn (§3) → approvals (§4) → queue (§5a) → delivery (§5) → write lease (D31).

## 1. Process (per provider process; under D8 one process may host several sessions)

States: `none`, `spawning`, `alive`, `leader_exited` (leader gone, tracked descendants alive), `terminating`, `kill_failed` (sticky), `orphan_alive` (found alive after restart), `exited(reason)` with reason ∈ {`code`, `abnormal`, `spawn_error`, `terminated`, `lost`, `killed_by_restart`, `orphan`}. `abnormal` = non-zero/signal exit not requested by Shardrail.

| From | Event | Result |
| --- | --- | --- |
| `none` | spawn requested | journal `provider_spawn_intent` → (macOS: start helper first) → `spawning` |
| `spawning` | spawn ok | `alive` + journal `provider_spawned` (pid, start time, boot id, cgroup/helper id, binary path) |
| `spawning` | spawn failed | `exited(spawn_error)` |
| `spawning` / `alive` | leader exits, no tracked descendants | `exited(code` or `abnormal)` |
| `alive` | leader exits, tracked descendants alive | `leader_exited` |
| `leader_exited` | last tracked descendant exits | `exited(code` or `abnormal)` |
| `spawning` / `alive` / `leader_exited` | terminate requested | guard: if the process hosts other sessions (D8), requires user confirmation listing them, else the request is rejected and the turn stays `cancelling` + notice → `terminating` (escalation below) |
| `terminating` | all tracked processes gone | `exited(terminated)` |
| `terminating` | `T_kill` elapsed after SIGKILL | `kill_failed` + notice |
| `terminating` | terminate requested | `ignore` |
| `kill_failed` | all tracked processes gone | `exited(terminated)` |
| `kill_failed` | terminate requested | `terminating` (retry) |
| `exited` | spawn requested (resume/restart) | guard: no `orphan_alive` / `leader_exited` / `kill_failed` for this provider session → `spawning`; else refused |
| `exited` | terminate requested | `ignore` |
| `orphan_alive` / `leader_exited` / `kill_failed` | spawn requested | refused |
| *any non-`exited` recorded* | daemon restart: identity match (pid + start time + boot id) alive | `orphan_alive` |
| *any non-`exited` recorded* | daemon restart: dead or identity mismatch | `exited(lost)` |
| *any non-`exited` recorded* | daemon restart: Linux cgroup killed by unit stop | `exited(killed_by_restart)` |
| `orphan_alive` | terminate requested | `terminating` **without** protocol-cancel step (no link) |
| `orphan_alive` | orphan and tracked descendants exit (observed via pidfd/kqueue) | `exited(orphan)` |
| `alive` (turn `idle`, no approval open) | planned daemon restart after drain (D39) | protocol `session/close` where supported → `terminating` → `exited(terminated)`; sessions stay resumable |

**Escalation (`terminating`, non-orphan):** if link `ready` and a turn is active → protocol cancel → wait `T_grace` → SIGTERM to all tracked processes → 5 s → SIGKILL (Linux: `cgroup.kill`; macOS: helper per 06 §8) → wait `T_kill`. Under U11 the escalation is performed by the privileged spawner. Codex background terminals are **not** terminated through `thread/backgroundTerminals/terminate` (experimental API, excluded by D24/D36); they are tracked descendants and die with containment.

**Planned restart (D39, drain-then-upgrade):** a daemon upgrade or a version-skew restart is a planned operation, not a crash. The daemon (1) stops accepting new turns and queued dispatch, (2) lists sessions with an active turn or an open approval as blockers and shows them in every attached client, (3) waits until all are `idle` or until `T_drain`, (4) on `T_drain` asks the user (with the blocker list) whether to cancel them or postpone the upgrade — never cancels silently, (5) closes idle provider sessions through the protocol, journals `planned_shutdown`, and restarts. A shell update never restarts a protocol-compatible daemon. On restart, sessions closed this way are `exited(terminated)` with turn `idle`, not `killed_by_restart`.

## 2. Link (protocol session)

States: `none`, `initializing`, `ready`, `resuming`, `desynced`.

| From | Event | Result |
| --- | --- | --- |
| `none` | process `alive` | `initializing` |
| `initializing` | initialize ok | `ready` |
| `initializing` | initialize error / `T_init` | `desynced` |
| `ready` | resume/load requested (D33; only while turn `idle`, preserving I1) | `resuming` |
| `resuming` | resume/load complete | `ready` |
| `resuming` | resume/load failed / `T_resume` | `desynced` |
| `initializing` / `ready` / `resuming` | protocol error (malformed frame, response with unknown non-tombstoned id) | `desynced` |
| `ready` / `resuming` | response with tombstoned (locally timed-out) id | `ignore` + notice |
| any | process leaves `alive`/`spawning` (leader exit, `exited`) | `none` |
| any | daemon restart | `none` |
| `desynced` | (process still alive) | turn → `detached` (§3); termination per U22 (owner 2026-10-19): **never automatic** — the process stays contained, the session is marked `desynced` and goes to the inbox; termination only on user confirmation; the orphan-daemon timer does not kill it |
| any (shared process, D8) | malformed frame on the shared stdio stream | **all** sessions hosted by that process → `desynced` (a framing error is per connection, not per session) |

## 3. Turn (one active turn per session)

States: `idle`, `running`, `stalled`, `cancelling`, `detached` (execution may continue; Shardrail has no channel). On entering `idle` a durable `turn_outcome` is recorded: provider stop reason (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`, Codex equivalents) or Shardrail outcome (`interrupted`, `killed_mid_tool`, `cancelled_by_termination`, `killed_by_restart`, `unknown_after_restart`). On any Shardrail outcome, open tool calls → `outcome_unknown`.

**Dispatch guard (I5):** link `ready` ∧ process `alive` ∧ no `orphan_alive`/`leader_exited`/`kill_failed` for this session ∧ write lease held if the session is write-capable (D31) ∧ host-protection acknowledgement satisfied (D20; failed checklist items acknowledged per host × provider, D47). Otherwise the command is rejected with the current state.

| From | Event | Result |
| --- | --- | --- |
| `idle` | prompt dispatched (guard passed) | `running` |
| `idle` | session-level update | `route` |
| `idle` | late update for a finished turn's tool call | `route` (attach to that item; no state change) |
| `idle` | approval request | refuse immediately (03 §2 refusal rule) + notice. Background work (ACP v2 background activity, Codex background terminals) may legitimately ask while foreground is idle; v1 policy is still refuse + notice, revisited when background work is supported |
| `running` / `stalled` | turn finished (stop reason) | `idle` + provider outcome |
| `running` / `stalled` | provider event for this turn | `running` |
| `running` | no provider event ≥ `T_stall` and no approval `pending`/`responded_unacked` | `stalled` |
| `running` / `stalled` | cancel requested | `cancelling` + protocol cancel; pending approvals answered `cancelled` (§4) |
| `cancelling` | cancel requested | `ignore` |
| `cancelling` | provider event | `cancelling` (ACP: accept tool updates after cancel) |
| `cancelling` | turn finished (stop reason) | `idle` + **provider** stop reason (may be `end_turn` if the turn completed first) |
| `cancelling` | `T_cancel` elapsed | request process termination (§1; shared-process guard applies); stays `cancelling`. If the guard needs a user confirmation and no client is attached: attention item + notice; the orphan-daemon timer (§6) bounds the wait |
| `cancelling` | process `exited(terminated)` | `idle` + `killed_mid_tool` if tool calls open (+ workspace notice: lock files, diff since turn start) else `cancelled_by_termination` |
| `running` / `stalled` / `cancelling` | process `exited` (not requested) | `idle` + `interrupted` |
| `running` / `stalled` / `cancelling` | link `desynced` with process alive, or process `leader_exited` | `detached` |
| `detached` | cancel requested | process terminate requested (user-confirmed) |
| `detached` | process `exited` | `idle` + `killed_mid_tool` (if termination was requested and tools open) else `interrupted` |
| any non-`idle` | daemon restart, process `orphan_alive` | `detached` |
| any non-`idle` | daemon restart, process `exited(lost)` | `idle` + `unknown_after_restart` |
| any non-`idle` | daemon restart, process `exited(killed_by_restart)` | `idle` + `killed_by_restart` |

## 4. Approval record (per request; D19)

States: `pending` (flag `needs_reconfirmation`), `responding(decision)` (write-ahead, D34), `responded_unacked(decision)`, terminal: `allowed`, `denied`, `expired`, `withdrawn(reason)`, `delivered_unconfirmed(decision)`.

**Write-ahead rule (D34).** A response is never written to a provider before `approval_responding` (approval id, decision, option id, responder client) is committed and synced to the journal. Order: CAS `pending → responding` → journal commit → write to provider stdin → `responded_unacked`. A crash anywhere after the commit can therefore only surface as `delivered_unconfirmed`, never as `withdrawn`. *Why:* without it, a crash between sending `allow` and committing would show "withdrawn" for an action the agent already executed (F21). Expiry and deny-mapping responses (`expired`, `cancelled` answers) follow the same write-ahead order.

Option classes (D19): `once`, `widening`, `boundary-breaking`; unknown kinds fail closed to `widening`, or `boundary-breaking` if they indicate sandbox/permission changes.

**Acknowledgement:** Codex — `serverRequest/resolved` for the request after Shardrail's response, or `item/completed` for the item; ACP v1 — `tool_call_update` for the same `toolCallId` after the response, or the `session/prompt` response; ACP v2 (D35) — `tool_call_update` for the subject's `toolCallId` when the request has one, otherwise a `state_update` leaving `requires_action` or reaching `idle`; the v2 `session/prompt` response only acknowledges message insertion and never acknowledges an approval. A permission request without any subject (v2 allows it) is shown as "action not disclosed by provider" and needs a second confirmation to allow (06 §5).

| From | Event | Result |
| --- | --- | --- |
| `pending` | authorized response, class permitted for the client's scope (and second confirmation if `needs_reconfirmation`) | `responding(decision)` (CAS: first wins) + journal `approval_responding`; `boundary-breaking` allow → host protection `unprotected` (D20) |
| `pending` | authorized response but the journal write fails (disk full, corruption) | stays `pending`; responder gets a typed error; §10 fail-closed rule applies |
| `responding` | journal commit ok | write response to provider → `responded_unacked(decision)` |
| `responding` | daemon restart / process leaves `alive` before the write is confirmed | `delivered_unconfirmed(decision)` + notice (the response may or may not have reached the provider) |
| `responding` | another response | rejected: "already responded" |
| `pending` | response from a client without the required scope | rejected (typed error) |
| `pending` | provider withdraws (resolved/terminated before any response) | `withdrawn(provider)` |
| `pending` | turn **cancel requested** | `withdrawn(cancelled)` + answer `cancelled` |
| `pending` | turn finished | `withdrawn(turn_end)` |
| `pending` | process leaves `alive` / link `desynced` | `withdrawn(process_lost)` |
| `pending` | expiry policy fires (`deny_after` or hold maximum) | `expired` + deny mapping: `reject_once`-class option if offered, else cancel the turn and answer `cancelled` |
| `pending` | staleness threshold or workspace changed (HEAD moved or tracked-diff hash changed; D23 "unverified" entries count as changed) | `pending` + `needs_reconfirmation` |
| `pending` | daemon restart | `withdrawn(daemon_restart)` |
| `pending` | orphan-daemon timeout (U18) | `withdrawn(orphan_timeout)` (via turn cancel) |
| `responded_unacked` | provider acknowledges | `allowed` / `denied` |
| `responded_unacked` | JSON-RPC error on the response | `withdrawn(rejected_by_provider)` |
| `responded_unacked` | turn finished / cancel requested / process leaves `alive` / link `desynced` / daemon restart / `T_ack` | `delivered_unconfirmed(decision)` ("Allowed — outcome not confirmed") + notice |
| `responded_unacked` | another response | rejected: "already responded" |
| terminal | any response | rejected with final state |

## 5. Delivery (per mutating command, excluding approval responses which use §4)

States: `accepted`, `dispatched`, `confirmed`, `failed(reason)`, `unknown`, resolved: `resolved_resent`, `resolved_discarded`, `resolved_confirmed`.

| From | Event | Result |
| --- | --- | --- |
| `accepted` | written to provider; journaled | `dispatched` |
| `accepted` / `dispatched` | daemon restart | `unknown` |
| `dispatched` | provider confirms (e.g., turn started, response ok) | `confirmed` |
| `dispatched` | provider error / precondition mismatch (`expectedTurnId`) | `failed(reason)` |
| `dispatched` | process leaves `alive` / link `desynced` before confirmation | `unknown` |
| `unknown` | user: resend (guard: process `exited` ∧ no tracked descendants ∧ no `orphan_alive`) | new command (new `command_id`); old → `resolved_resent` |
| `unknown` | user: discard | `resolved_discarded` |
| `unknown` | user: mark as delivered | `resolved_confirmed` |

## 5a. Instruction queue (F7; D42)

States per queued instruction: `queued`, `dispatching` (handed to the delivery table §5 as a new command), terminal: `sent`, `discarded`, `edited` (replaced by a new queued entry).

| From | Event | Result |
| --- | --- | --- |
| — | user sends an instruction while turn ∉ {`idle`} and the provider does not accept a second turn | journal `instruction_queued` → `queued` (at most one per session, F7; a second one replaces or is refused per UI choice) |
| `queued` | turn → `idle` with a provider outcome and dispatch guard I5 passes | `dispatching` → command `accepted` (§5) |
| `queued` | turn → `idle` with a **Shardrail** outcome (`interrupted`, `killed_mid_tool`, `unknown_after_restart`, …) | stays `queued` + attention item ("previous turn did not finish normally — send anyway?"); never auto-dispatched |
| `queued` | user edits / discards | `edited` / `discarded` |
| `queued` | daemon restart | stays `queued` (it was never written to the provider); **never** `delivery unknown` |
| `queued` | session closing (§9) | `discarded` + notice |

*Why:* without a queue state, an instruction that was accepted but never dispatched surfaced as `delivery unknown` after a restart, which misreports something Shardrail knows for certain.

## 6. Timers (all PROPOSED; configurable)

| Timer | Default | Applies in | On expiry | Owner Q |
| --- | --- | --- | --- | --- |
| `T_init` | 30 s | link `initializing` | `desynced` | — |
| `T_resume` | 120 s | link `resuming` | `desynced` | — |
| `T_grace` | 10 s | escalation (incl. user-confirmed desync termination) | SIGTERM step | U22 |
| `T_cancel` | 30 s | turn `cancelling` | request termination | — |
| `T_kill` | 10 s | process `terminating` after SIGKILL | `kill_failed` | — |
| `T_stall` | 15 min | turn `running`, no approval open | `stalled` | — |
| `T_ack` | 60 s | approval `responded_unacked` | `delivered_unconfirmed` | — |
| `T_drain` | 10 min | planned restart (D39) | ask the user: cancel blockers or postpone | — |
| `deny_after` | min(180 s, provider limit) — the limit is read from the protocol where the provider exposes it (e.g. Hermes `approvals.timeout` when known); when unknown the card says "the provider may decline earlier" and hold cannot exceed a known limit (15 S13) | approval `pending`, default policy | `expired` | U10 |
| hold maximum | 2 h | approval `pending`, opt-in hold policy | `expired` | U17 |
| staleness | 15 min (chosen independently of `T_stall`: maximum context age before re-confirmation) | approval `pending` | flag `needs_reconfirmation` | U17 |
| orphan-daemon | 10 min without any client | whole daemon | cancel turns | U18 |
| journal-unavailable grace | 0 s (immediate) | journal write failure (§10) | fail closed | — |

## 7. Badges

**Host overlay (outside precedence):** `approvals not protected` (with causes) per D20; `journal unavailable` (§10); `draining for upgrade` (D39); client-local `disconnected`. **Session secondary badges:** `instruction queued` (§5a), `lease held` (D31), checklist summary `checks: n verified · k failed · m unverified` (06 §3b). No host or session badge ever reads "protected" in v1 (D47); the host-protection state is internal and only its loss is shown.

**Session precedence:** `orphan / detached / kill_failed` > `needs approval` > `approval outcome unconfirmed` > `degraded` (`exited(abnormal|lost)`, `desynced`) > `delivery unknown` > `terminating` > `resuming` > `stalled` > `cancelling` > `running` > `idle`. A normal `exited(code 0|terminated)` with turn `idle` shows `idle`.

## 8. Cross-dimension invariants

- **I1** turn ∈ {`running`, `stalled`, `cancelling`} ⇒ process `alive` ∧ link `ready`.
- **I2** turn = `idle` ⇒ no tracked process executes for the session (Linux: guaranteed by cgroup; macOS: best effort, residual shown).
- **I3** approval `pending` or `responded_unacked` ⇒ turn ∈ {`running`, `stalled`, `cancelling`}.
- **I4** write lease held while the holder's process is not `exited` or any tracked descendant is alive; `orphan_alive` holds the lease (D31).
- **I5** dispatch guard (§3).
- **I6** at most one live process per provider session reference.
- **I7** an approval response is on the wire only if `approval_responding` is committed (D34).
- **I8** journal unavailable ⇒ no `allow` is sent, no turn is dispatched, no instruction leaves `queued` (§10).
- **I9** session lifecycle `closed`/`purging`/`purged` ⇒ process `exited` (or never spawned) ∧ no tracked descendant alive ∧ write lease released.

## 9. Session lifecycle (D42)

States: `open`, `closing`, `closed`, `purging`, `purged` (tombstone: ids and purge record only). Archive is a UI filter on `closed`, not a state.

| From | Event | Result |
| --- | --- | --- |
| — | session created (F2) | `open` |
| `open` | user: end session | turn `idle` required (else: cancel first, user-confirmed) → queued instruction `discarded` → ACP `session/close` (when advertised) / Codex `thread/unsubscribe` → process terminate per topology (§1) → `closing` |
| `closing` | process `exited` ∧ no tracked descendant | `closed` + write lease released (D31) |
| `closing` | `kill_failed` / `orphan_alive` | stays `closing` + notice; lease stays held (I4) |
| `closed` | user: resume (capability present) | `open` (new process, I6) |
| `closed` | user: purge (F17) | `purging` → purge procedure (04 §6) → `purged` |
| `open` / `closing` | user: purge | refused: "end the session first" |
| `purging` | daemon restart | resume the purge procedure (idempotent) |
| `open` | daemon restart | stays `open`; process/turn per §1–§3 |

The write lease (D31) of an idle session with a live process is released only through "end session"; the session list shows long-held leases so a forgotten idle session does not block a worktree silently.

## 10. Journal unavailable (disk full, corruption, I/O error) (D41)

- **Fail closed:** every `pending` approval is answered with the provider's reject option, or `cancelled` with a turn cancel when none exists; no `allow` can be sent because D34 requires a journal commit first. Running turns are cancelled through the protocol; no new turn or queued instruction is dispatched.
- Decisions taken while the journal is unavailable are kept in a bounded in-memory log and in the diagnostics ring, and are journaled as `recovered_events` once writing works again.
- Clients see a host-level banner "journal unavailable — sessions paused" with the cause.
- **Disk full** clears automatically once a test write succeeds (sessions return to `idle`; nothing is resent). **Corruption** follows 04 §6: read-only open, salvage or new database only by explicit user choice; provider processes are terminated through the protocol after the fail-closed step, so nothing keeps running unobserved.
