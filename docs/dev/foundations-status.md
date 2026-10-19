# P1a foundations status

Date: 2026-10-19  
Branch: `p1a/foundations`

## Done

- T0.9: pinned Rust toolchain, root agent rules, workspace lockfile, CI for formatting, Clippy, tests, cargo-deny, and schema drift.
- M02: Rust contract types, committed schemars JSON Schema, generated TypeScript, and a drift check.
- M03/M07 foundation: provider-free approval, queue, journal-failure, session close/purge, and desync state logic.
- M04 foundation: SQLite WAL journal, immutable event rows, durable approval write-ahead records, and recovery of `responding` as `delivered_unconfirmed`.
- M18 foundation: fake JSON-RPC stdio agent and round-trip coverage for initialize, session/new, streamed session/prompt, and an agent-originated `session/request_permission` with correlated client response.
- Tests cover denial defaults/timeouts, provider timeout caps, two-hour hold expiry, 15-minute freshness, workspace-change reconfirmation, write-ahead ordering, disk-full and corruption failures, queued instructions, desync, and close/purge ordering.
- Review fixes: approval responses now enforce expiry in the state transition; client disconnect preserves approval delivery state; journal creation enforces `0600` on Unix and returns committed event cursors; already-exited sessions close immediately; journal failure only cancels a live active turn.
- CI checks npm registry signatures and the locked contracts package licenses (`0BSD`, Apache-2.0, BSD-2-Clause/3-Clause, ISC, MIT, Python-2.0), alongside the schema drift check.

## Stubbed or deferred

- This is foundations only: no daemon binary, network/client gateway, provider adapter, process supervisor, real agent, UI, workspace service, or application wiring.
- Journal migration/versioning, epoch management, single-daemon lock, salvage UI, retention, and purge/VACUUM are not implemented yet.
- The journal exposes append/read primitives and durable approval intent; there is no complete M03 event-fold/recovery runner wiring session state to those rows yet.
- The fake ACP fixture implements only the requested JSON-RPC subset. It is not a compatibility claim for ACP v1/v2. The official ACP SDK was not added because this test fixture needs only line-based request/notification encoding; the SDK would bring an async runtime and broader protocol machinery without exercising a Shardrail client adapter.
- The workspace currently contains only M02, M03/M07 logic, M04, and M18. Other modules remain outside the authorized scope.
- `AGENTS.md` records the Rust and contract-generation dependencies. The contracts-only npm package has signature and license checks; `cargo-deny` covers the Rust graph.

## Deviations and reversible choices

- Dependency versions are locked by Cargo/npm lockfiles; direct dependency versions are compatibility ranges in manifests as Cargo/npm convention requires. Exact resolved versions are in the lockfiles.
- Rust 1.99.0 is pinned because it is the installed stable toolchain in this environment. CI installs that same version.
- Schema generation is the smallest JSON-Schema-to-TypeScript generator (`json-schema-to-typescript`); no `ts-rs` fallback is needed for the current contract types.
- Fake agent session ids and streamed text are fixed fixture values. They are intentionally deterministic for protocol tests.
- The journal uses SQLite WAL with `synchronous=FULL`; SQLite serializes writes, but a separate process-level data-directory lock is deferred to M01.
- Session and approval policy timers are deterministic pure functions taking caller-supplied timestamps; the daemon scheduler that invokes them is deferred.
- Corruption is tested by replacing a closed test database with invalid bytes. Disk-full is tested with SQLite `max_page_count` and a forced page allocation rather than filling the host filesystem.

## Open questions for Daliys

- Should the generated TypeScript contract package export only schema DTOs, or also include hand-authored client helpers once the gateway is started?
- When M01 is added, should the default journal data directory refuse network filesystems or only emit a diagnostic, as left open in 11 §2?
- Confirm the permitted Rust dependency allowlist before expanding beyond the current foundation crates; `deny.toml` is the initial list for this task.

## Final summary

P1a provider-free foundations and the verified review fixes are implemented on `p1a/foundations`. The scoped crates, regression tests, schema and generated types, lockfiles, agent guidance, and CI configuration are present. No provider executable or model API was used. Final check and commit results are recorded in `spikes/reviews/foundations-fix-report.md`.
