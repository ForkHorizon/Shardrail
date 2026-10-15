# 05 — Session State Machine (D18)

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**; normative for detailed planning of the session engine once approved.
> Why separate dimensions: a single linear state (`starting → running → waiting_approval → idle`) could not represent parallel approvals, cancellation in progress, a live process with a broken protocol link, or survivors after a crash.
> All timers are PROPOSED defaults (§6). "Tracked descendants" = processes contained per D32 (Linux: cgroup, a guarantee; macOS: best effort).

## 0. Cell kinds and conventions

Every (state, event) cell is one of:
- **`→ state [+ action]`** — transition.
- **`ignore`** — idempotent repeat (already dedup'd by D30 `command_id` where applicable); no notice.
- **`route`** — event belongs to another handler (session-level updates such as available commands or mode changes; late tool updates attached to an earlier turn); no state change here.
- **`impossible → notice`** — any cell not listed in a row. A conformance test suite checks all four kinds.

Event ordering on daemon restart: process (§1) → link (§2) → turn (§3) → approvals (§4) → delivery (§5) → write lease (D31).

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

**Escalation (`terminating`, non-orphan):** if link `ready` and a turn is active → protocol cancel (+ Codex `thread/backgroundTerminals/terminate`) → wait `T_grace` → SIGTERM to all tracked processes → 5 s → SIGKILL (Linux: `cgroup.kill`) → wait `T_kill`. Under U11 the escalation is performed by the privileged spawner.

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
| `desynced` | (process still alive) | turn → `detached` (§3); termination per policy: user-confirmed while a client is attached; automatic after `T_grace` when no client is attached (U22) |

## 3. Turn (one active turn per session)

States: `idle`, `running`, `stalled`, `cancelling`, `detached` (execution may continue; Shardrail has no channel). On entering `idle` a durable `turn_outcome` is recorded: provider stop reason (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`, Codex equivalents) or Shardrail outcome (`interrupted`, `killed_mid_tool`, `cancelled_by_termination`, `killed_by_restart`, `unknown_after_restart`). On any Shardrail outcome, open tool calls → `outcome_unknown`.

**Dispatch guard (I5):** link `ready` ∧ process `alive` ∧ no `orphan_alive`/`leader_exited`/`kill_failed` for this session ∧ write lease held if the session is write-capable (D31) ∧ host-protection acknowledgement satisfied (D20). Otherwise the command is rejected with the current state.

| From | Event | Result |
| --- | --- | --- |
| `idle` | prompt dispatched (guard passed) | `running` |
| `idle` | session-level update | `route` |
| `idle` | late update for a finished turn's tool call | `route` (attach to that item; no state change) |
| `idle` | approval request | refuse immediately (D24) + notice |
| `running` / `stalled` | turn finished (stop reason) | `idle` + provider outcome |
| `running` / `stalled` | provider event for this turn | `running` |
| `running` | no provider event ≥ `T_stall` and no approval `pending`/`responded_unacked` | `stalled` |
| `running` / `stalled` | cancel requested | `cancelling` + protocol cancel; pending approvals answered `cancelled` (§4) |
| `cancelling` | cancel requested | `ignore` |
| `cancelling` | provider event | `cancelling` (ACP: accept tool updates after cancel) |
| `cancelling` | turn finished (stop reason) | `idle` + **provider** stop reason (may be `end_turn` if the turn completed first) |
| `cancelling` | `T_cancel` elapsed | request process termination (§1; shared-process guard applies); stays `cancelling` |
| `cancelling` | process `exited(terminated)` | `idle` + `killed_mid_tool` if tool calls open (+ workspace notice: lock files, diff since turn start) else `cancelled_by_termination` |
| `running` / `stalled` / `cancelling` | process `exited` (not requested) | `idle` + `interrupted` |
| `running` / `stalled` / `cancelling` | link `desynced` with process alive, or process `leader_exited` | `detached` |
| `detached` | cancel requested | process terminate requested (user-confirmed) |
| `detached` | process `exited` | `idle` + `killed_mid_tool` (if termination was requested and tools open) else `interrupted` |
| any non-`idle` | daemon restart, process `orphan_alive` | `detached` |
| any non-`idle` | daemon restart, process `exited(lost)` | `idle` + `unknown_after_restart` |
| any non-`idle` | daemon restart, process `exited(killed_by_restart)` | `idle` + `killed_by_restart` |

## 4. Approval record (per request; D19)

States: `pending` (flag `needs_reconfirmation`), `responded_unacked(decision)`, terminal: `allowed`, `denied`, `expired`, `withdrawn(reason)`, `delivered_unconfirmed(decision)`.

Option classes (D19): `once`, `widening`, `boundary-breaking`; unknown kinds fail closed to `widening`, or `boundary-breaking` if they indicate sandbox/permission changes.

**Acknowledgement:** Codex — `serverRequest/resolved` for the request after Shardrail's response, or `item/completed` for the item; ACP — `tool_call_update` for the same `toolCallId` after the response, or the `session/prompt` response.

| From | Event | Result |
| --- | --- | --- |
| `pending` | authorized response, class permitted for the client's scope (and second confirmation if `needs_reconfirmation`) | `responded_unacked(decision)` (CAS: first wins); `boundary-breaking` allow → host protection `unprotected` (D20) |
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

## 6. Timers (all PROPOSED; configurable)

| Timer | Default | Applies in | On expiry | Owner Q |
| --- | --- | --- | --- | --- |
| `T_init` | 30 s | link `initializing` | `desynced` | — |
| `T_resume` | 120 s | link `resuming` | `desynced` | — |
| `T_grace` | 10 s | escalation; unattended desync | SIGTERM step | U22 |
| `T_cancel` | 30 s | turn `cancelling` | request termination | — |
| `T_kill` | 10 s | process `terminating` after SIGKILL | `kill_failed` | — |
| `T_stall` | 15 min | turn `running`, no approval open | `stalled` | — |
| `T_ack` | 60 s | approval `responded_unacked` | `delivered_unconfirmed` | — |
| `deny_after` | 180 s (or a lower provider limit if known, e.g. Hermes `approvals.timeout` when the user tells Shardrail its value) | approval `pending`, default policy | `expired` | U10 |
| hold maximum | 2 h | approval `pending`, opt-in hold policy | `expired` | U17 |
| staleness | 15 min (chosen independently of `T_stall`: maximum context age before re-confirmation) | approval `pending` | flag `needs_reconfirmation` | U17 |
| orphan-daemon | 10 min without any client | whole daemon | cancel turns | U18 |

## 7. Badges

**Host overlay (outside precedence):** `approvals not protected` (with causes) per D20; client-local `disconnected`.

**Session precedence:** `orphan / detached / kill_failed` > `needs approval` > `approval outcome unconfirmed` > `degraded` (`exited(abnormal|lost)`, `desynced`) > `delivery unknown` > `terminating` > `resuming` > `stalled` > `cancelling` > `running` > `idle`. A normal `exited(code 0|terminated)` with turn `idle` shows `idle`.

## 8. Cross-dimension invariants

- **I1** turn ∈ {`running`, `stalled`, `cancelling`} ⇒ process `alive` ∧ link `ready`.
- **I2** turn = `idle` ⇒ no tracked process executes for the session (Linux: guaranteed by cgroup; macOS: best effort, residual shown).
- **I3** approval `pending` or `responded_unacked` ⇒ turn ∈ {`running`, `stalled`, `cancelling`}.
- **I4** write lease held while the holder's process is not `exited` or any tracked descendant is alive; `orphan_alive` holds the lease (D31).
- **I5** dispatch guard (§3).
- **I6** at most one live process per provider session reference.
