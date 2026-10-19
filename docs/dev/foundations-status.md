# P1a foundations status

Date: 2026-10-20
Branch: `p1a/coordinator`

## Done

- T0.9: pinned Rust toolchain, root agent rules, workspace lockfile, CI for formatting, Clippy, tests, cargo-deny, and schema drift.
- M02: Rust contract types, committed schemars JSON Schema, generated TypeScript, and a drift check. Durable events now include `journal_epoch`, `host_id`, `provider_ref`, `causation`, and `redaction_flags`; approval responses carry option class and client scope.
- M03/M07 foundation: provider-free approval, queue, journal-failure, session close/purge, and desync state logic.
- M04 foundation: SQLite WAL journal, immutable event rows, a stored database `journal_epoch` validated when appending and reading events, durable approval write-ahead records, and recovery of `responding` as `delivered_unconfirmed`.
- M18 foundation: fake JSON-RPC stdio agent and round-trip coverage for initialize, session/new, streamed session/prompt, and an agent-originated `session/request_permission` with correlated client response.
- Tests cover denial defaults/timeouts, provider timeout caps, two-hour hold expiry, 15-minute freshness, workspace-change reconfirmation, write-ahead ordering, disk-full and corruption failures, queued instructions, desync, and close/purge ordering.
- Cross-model review fixes (N1): held approvals persist their hold choice and require reconfirmation after 15 minutes; process exit interrupts active turns; queued instructions advance on turn completion; desynced or closed sessions cannot start turns/processes; approval operations require an open session with an active turn; rejected close transitions are not journaled; provider effect errors record desync instead of a journal failure; cancel resolves as `withdrawn(cancelled)`; the per-response WAL checkpoint was removed. Limited clients can allow only `once` options.
- CI checks npm registry signatures and the locked contracts package licenses (`0BSD`, Apache-2.0, BSD-2-Clause/3-Clause, ISC, MIT, Python-2.0), alongside the schema drift check.
- P1a coordinator: durable session-transition snapshots replay into M03 state; approval responses commit write-ahead intent before ACP replies; acknowledgement and provider rejection operations finalize approval state. Approval rows store `session_id`, creation/confirmation timestamps, reconfirmation and hold flags, and can be loaded per session after restart. Approval lifecycle events are also included in the session event stream. Global approval recovery is an explicit daemon-startup operation; opening a per-session coordinator does not mutate other sessions' approvals. Journal failures pause the session and invoke an optional fail-closed effect hook with pending approval IDs and the active-turn cancellation requirement. Provider effect errors persist a desynced/unknown snapshot when possible.
- Approval timeout without a reject option journals turn cancellation and the `expired` outcome before sending the cancelled response and turn-cancel effect. Acknowledging that response preserves `expired` rather than mapping it to `denied`.
- Recovery preserves process state across normal turn completion and marks every unverified live process desynced, including idle sessions; active turns for live processes become detached. A process-exit event moves active turns to idle with an `interrupted` outcome.
- M01 data-directory guard: create or harden the directory to `0700`, the daemon lock to `0600`, and the SQLite journal to `0600`; Rust's advisory file lock rejects a concurrent daemon and releases automatically when a crashed process closes its file handles. Journal symlinks and non-regular journal paths are rejected before permission changes/open.
- Fake ACP integration coverage exercises turn journaling, allow, deny, injected-clock timeout, approval journal failure before provider write, crash/restart recovery, and concurrent-daemon refusal. Unit regressions cover timeout cancellation without a reject option, acknowledgement outcomes, cross-session startup recovery isolation, effect failure, process recovery, and symlink refusal. Tests are deterministic and use no real providers or sleeps.

## Stubbed or deferred

- This is foundations plus the coordinator core: no daemon binary, network/client gateway, provider adapter, process supervisor, real agent, UI, workspace service, or production application wiring.
- Journal schema migration/versioning beyond additive column upgrades, salvage UI, retention, and purge/VACUUM are not implemented yet. Journal epoch creation and validation are implemented.
- Provider process discovery and restart resolution still belong to M05; until then, a replayed active turn is conservatively marked detached/desynced and an idle process is marked desynced so the coordinator cannot assume a process is usable.
- The fake ACP fixture implements only the requested JSON-RPC subset. It is not a compatibility claim for ACP v1/v2. The official ACP SDK was not added because this test fixture needs only line-based request/notification encoding; the SDK would bring an async runtime and broader protocol machinery without exercising a Shardrail client adapter.
- The workspace contains M02, M03/M07 logic, M04, M18, and the P1a coordinator core. Other modules remain outside the authorized scope.
- Fail-closed protocol actions use `set_fail_closed_effect`; the fake ACP integration registers the hook, rejects the pending approval, cancels the turn, and observes the child exit. No real provider adapter exists in P1a; future adapters must register equivalent protocol effects and map pending IDs to provider reject options.
- `host_id` is currently generated and retained in the journal metadata row. A separate daemon-installation identity that survives replacing the database remains part of M01/data-directory wiring.
- `AGENTS.md` records the Rust and contract-generation dependencies. The contracts-only npm package has signature and license checks; `cargo-deny` covers the Rust graph.

## Deviations and reversible choices

- Dependency versions are locked by Cargo/npm lockfiles; direct dependency versions are compatibility ranges in manifests as Cargo/npm convention requires. Exact resolved versions are in the lockfiles.
- Rust 1.99.0 is pinned because it is the installed stable toolchain in this environment. CI installs that same version.
- Schema generation is the smallest JSON-Schema-to-TypeScript generator (`json-schema-to-typescript`); no `ts-rs` fallback is needed for the current contract types.
- Fake agent session ids and streamed text are fixed fixture values. They are intentionally deterministic for protocol tests.
- The journal uses SQLite WAL with `synchronous=FULL`; M01 now retains an OS advisory lock for the daemon lifetime. Stale lock files are harmless because ownership comes from the kernel lock, not file contents.
- Session and approval policy timers are deterministic pure functions taking caller-supplied timestamps; the daemon scheduler that invokes them is deferred.
- Corruption is tested by replacing a closed test database with invalid bytes. Disk-full is tested with SQLite `max_page_count` and a forced page allocation rather than filling the host filesystem.
- The review's M5 symlink TOCTOU finding was not in the authorized finding list and remains open for a later M01 hardening pass.

## Open questions for Daliys

- Should the generated TypeScript contract package export only schema DTOs, or also include hand-authored client helpers once the gateway is started?
- Should M01 refuse network filesystems for the journal data directory or only emit a diagnostic, as left open in 11 §2? This implementation does not attempt filesystem-type detection.
- Confirm the permitted Rust dependency allowlist before expanding beyond the current foundation crates; `deny.toml` is the initial list for this task.
- `planning/12-decisions.md` ends at D48; D50 and D51 do not currently exist in the decision register. No decisions were inferred or added as part of this implementation.

## Final summary

P1a provider-free foundations, the coordinator core, and M01 directory/lock guards are implemented on `p1a/coordinator`. The scoped crates, regression tests, schema and generated types, lockfiles, agent guidance, and CI configuration are present. No provider executable or model API was used.
