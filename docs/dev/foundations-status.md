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
- Cross-model review fixes (N1): held approvals persist their hold choice and require reconfirmation after 15 minutes; process exit interrupts active turns; queued instructions advance on turn completion; `turn_started` requires a live process; invalid cancel and unknown transitions are rejected; process exit and turn completion resolve pending approvals and mark unacknowledged responses as unconfirmed. Approval operations enforce session ownership; invalid acknowledgements do not fail-close the journal. Provider effect errors record desync without misreporting journal health; timeout cancel state matches its wire decision; lifecycle event timestamps use the supplied or recovery clock; journal payload redaction runs before persistence; ordinary journal opens preserve append-only event rows; hold choices reload from disk; closing sessions use `session_closing` until process exit. Limited clients can allow only `once` options, pending a trusted gateway and adapter metadata boundary.
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
- The workspace contains M02, M03/M07 logic, M04, M18, and the P1a coordinator core. Other modules remain outside the authorized scope. M09/M10 integration remains deferred: `ApprovalResponseRequest` still receives `option_class` and `client_scope` from its caller. There is no authenticated gateway or trusted provider option catalog in this slice to derive those values; do not treat this API as a secured client boundary until those sources are connected.
- Fail-closed protocol actions use `set_fail_closed_effect`; the fake ACP integration registers the hook, rejects the pending approval, cancels the turn, and observes the child exit. No real provider adapter exists in P1a; future adapters must register equivalent protocol effects and map pending IDs to provider reject options.
- `host_id` is currently generated and retained in the journal metadata row. A separate daemon-installation identity that survives replacing the database remains part of M01/data-directory wiring.
- N2 daemon startup, local Unix socket, schema-backed handshake/command/event frames, receipt timestamps, and real-binary lifecycle checks are documented in [daemon-status.md](daemon-status.md). Review fixes addressed replay, fan-out, idempotency, bounded journal reads, directory path safety, epoch resync, and generated frame types. Asymmetric authentication and daemon-routed fake ACP approval handling remain open there.
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

- `packages/client` exports generated DTO types and a typed stub; a local socket relay is still needed before it can connect.
- N2 implements refuse-on-known-network-or-synced storage detection. Filesystem naming heuristics and platform coverage remain an implementation limit; see [daemon-status.md](daemon-status.md).
- The N2 task authorized `tokio`, `clap`, and `nix`; these are recorded in `AGENTS.md` and `deny.toml`.
- `planning/12-decisions.md` ends at D48; D50–D54 remain absent from the register. N2's requested auth/approval details are not all realized; see [daemon-status.md](daemon-status.md).
- W4-a authorized `ed25519-dalek` and `sha2`; the daemon now authenticates registered client keys, bounds client output queues, checks the data directory through a retained descriptor, and redacts high-entropy token-like runs. The requested review file and D50/D51/D53/D55/D57/D60 entries are missing from this checkout, so no decision rationale was inferred. Pairing UI and a native client transport remain deferred; see [daemon-status.md](daemon-status.md).

## Final summary

P1a foundations, the coordinator core, M01 directory/lock guards, and the N2 `shardraild` development skeleton are present on `p1a/coordinator`. The gateway still lacks asymmetric authentication and daemon-routed approval handling; see [daemon-status.md](daemon-status.md). No real provider executable or model API was used.
