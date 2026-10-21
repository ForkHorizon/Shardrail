# N2 daemon status

Date: 2026-10-21
Branch: `p1a/coordinator`

## Implemented

- Added the `shardraild` binary with a single startup function ordered as requested: resolve path, refuse known synced paths and network filesystem names, descriptor-relative data-dir create/open and permission check, acquire the process lock, descriptor-relative `openat(O_NOFOLLOW)` journal probe, open SQLite with `SQLITE_OPEN_NOFOLLOW`, verify epoch/host metadata, recover global approval state once, then bind the Unix listener (`0600`).
- The socket protocol negotiates protocol v1 and journal epoch, accepts a host cursor, emits durable events after resume, and supports `session.create` with a UUID command ID. Events carry journal-persisted daemon monotonic and wall-clock receipt stamps. Wire frames use tagged Rust enums and generated TypeScript contracts.
- Real-binary tests cover session creation, event delivery and fan-out, cursor replay after SIGKILL, duplicate-daemon refusal, command retries, epoch mismatch replay, and symlink refusal without modifying the target. Filesystem-name checks have unit coverage.
- `packages/client` now opens the Unix socket in Node, verifies the Ed25519 handshake and daemon host key, sends command IDs, tracks durable event cursors, resumes after reconnect, and exposes typed daemon frames. It uses generated contracts only. `@noble/ed25519` is pinned to exactly `3.1.0` per D62; TypeScript `5.9.3` and Node types `26.1.0` are pinned in the lockfile. `tsc --noEmit` and a Node test against the real daemon and fake ACP are wired into CI.

## Limits and open decisions

- The decision register in `planning/12-decisions.md` stops at D48; the owner decision file `planning/17-owner-decisions-2026-10-19.md` contains sections В and Г with D49–D62.
- Authentication uses Ed25519 keys. `authorized-clients.json` stores each registered public key, label and scope; the daemon verifies a transcript digest covering both nonces, version range/selection and both public keys, signs it with the host key and verifies the client's signature before sending events or handling commands. `shardraild client add|list|remove` manages key registration. Host and client private keys are `0600` files. Interactive pairing UI remains deferred.
- The S5 prototype connects the release app to the daemon through a Tauri shell relay. The relay authenticates with a registered Ed25519 key in an isolated temporary data directory; the production pairing screen remains deferred.
- SQLite's public rusqlite API opens paths, not adopted descriptors. Startup performs the no-follow `openat` probe and the SQLite open itself uses `SQLITE_OPEN_NOFOLLOW`; the private directory and retained daemon lock prevent other users and cooperating daemons from swapping the file. This does not make a file-backed database tamper-proof against a malicious process running as the same UID; D20 already classifies such a host as unprotected.
- The first receipt's monotonic timestamp is relative to the first journal append after process start. Wall-clock receipts persist across restarts. Historical rows created before this change have no stored receipt stamp.

## Verification

Repository checks passed: formatting, Clippy, all workspace tests, cargo-deny, npm CI installs for contracts and client, client `tsc --noEmit` and Node integration test, and schema drift. Provider executables and model APIs are not invoked.

## Review fixes (2026-10-20)

- Data-dir symlinks are rejected before chmod; relative paths resolve against the process working directory. Linux uses `statfs` magic values while macOS uses filesystem type names.
- Durable event reads use bounded 16-event pages joined with receipt stamps. The journal mutex is released before socket writes; idle clients poll for committed events. Hello resumes replay history immediately, and an epoch mismatch keeps the connection open and streams a full replay from cursor zero.
- Session-create retries are deduplicated by `(client_id, command_id)` using the committed `session_created` event; create requests with unsupported preconditions are rejected. Historical coordinators are no longer opened at startup.
- Client and daemon frames are tagged enums; JSON Schema and TypeScript were regenerated. The client package now declares and imports the local contracts package.

## W4-a hardening (2026-10-20)

- Data-dir creation/open uses `mkdirat` and `openat(O_NOFOLLOW)`, then checks ownership and mode with `fstat` on the retained descriptor and applies `0700` with `fchmod`. The process uses descriptor-relative access for the lock and journal probe. On macOS it changes the daemon process working directory with `fchdir` on the checked descriptor, so SQLite and socket APIs that accept only paths resolve relative to that pinned directory; SQLite still independently uses `SQLITE_OPEN_NOFOLLOW` because rusqlite cannot adopt the already-open descriptor.
- Each connection has a 32-frame outbound queue. Producers wait up to one second for queue space, then disconnect a stalled client with a reconnect/resume reason in the daemon log; the slow-client integration test confirms a second authenticated client can catch up and continue.
- Journal redaction additionally detects base64-like runs of at least 32 characters with Shannon entropy ≥4 bits/character and hex runs of at least 32 characters with entropy ≥3.0. `/`, `-`, and whitespace split candidate runs, which keeps common paths, URLs, branch names, and diff headers readable; recognized Git hash contexts are also preserved. Long underscore-delimited identifiers may still be redacted, while short or low-entropy secrets may evade this best-effort heuristic. Event kind, provider reference, causation, and payload strings are redacted before persistence.
- Client registry updates use a separate advisory lock across each read-modify-write. Handshake reads time out after five seconds; pre-authentication errors are flushed before close; limited clients cannot create sessions; and a connected client can replay from an earlier cursor. Session creation still opens a new `Journal` connection and reruns idempotent schema setup; sharing the coordinator-owned connection is deferred until the gateway owns broader session commands.

## W4-b daemon approvals (2026-10-20)

- Authenticated clients receive durable `approval_requested` events only after authentication. The event keeps provider-authored `title`, `description`, and `reason` inside a separate `agent_description` field; the provider's structured tool-call kind and options are journaled alongside it.
- The client resolution frame contains only `approval_id` and `option_id`. The daemon maps the offered provider option to allow/deny, derives `once`/`widening`/`boundary-breaking` from the provider option kind and local policy, then applies the authenticated client's registered scope. Strict frame deserialization rejects extra client-supplied kind or scope fields. Limited clients can deny or choose an `once` allow option only.
- The daemon serializes competing responses through one live session coordinator. A winner receives `approval_resolved`; later responses receive `already resolved`. Provider completion acknowledges the journal record. Expiry uses the reject-once option when present and otherwise cancels the fake turn. Recovery after a committed response intent reports `approval_delivered_unconfirmed`.
- Review fixes preserve `expired` after the no-reject cancel path, classify unknown provider option kinds as widening (sandbox/permission kinds as boundary-breaking), and reject unknown client-frame fields at the daemon boundary. The coordinator marks a response sent only after the fake ACP stdin write completes; a true two-client race, limited `allow_once`, expiry terminal states, and early provider exit have regression coverage. Completed approval entries and timeout tasks are removed, and provider errors finalize the interrupted turn/process lifecycle.
- The injected write-ahead delay is asynchronous and remains between the durable intent and provider stdin write. Provider-write completion is synchronized with the request handler before `responded_unacked` is journaled; no SQLite polling loop is used.
- Real-binary integration tests use only `shardrail-fake-acp` via `--fake-acp-path`. `--approval-timeout-secs` controls the daemon policy timer; `--test-write-ahead-delay-ms` is a fault-injection pause used to kill the daemon after the durable response intent and before the fake provider write. No real provider or model API is invoked.
- This remains a fake-provider slice: production provider supervision, ACP option normalization for real adapters, restart/reconnect of live providers, UI rendering, and durable reconstruction of provider option catalogs remain deferred to M05/M06/M09/M14. The task authorization names D55/D56 in section Г of `planning/17-owner-decisions-2026-10-19.md`; the separate decision register ends at D48.

## W4-c client and UI measurement (2026-10-21)

- The client integration test uses the real `shardraild` binary and `shardrail-fake-acp`; fake ACP `session/update` chunks are journaled as durable events with daemon receipt stamps. Event delivery pages are sent without a 100 ms pause between full pages, so the 50k-event benchmark workload can be delivered.
- The M5 data-directory path-swap race is closed by descriptor-relative access and no-follow opens. Regression coverage checks a path swap after open, a symlink swap between `mkdirat` and `openat`, and daemon operations after a path swap.
- The original B4 run matrix used 60 packaged Tauri/WKWebView fake-agent runs, but its measurements are invalid: rows mounted later during automated scrolling were measured against old receipt timestamps, and daemon delivery polled every 100 ms. A follow-up replaces scrolling-based samples with tail-follow samples and wakes subscribers on durable appends. The 60-run matrix and B4 verdict need to be rerun; the previous values and target conclusion are withdrawn.
- B5 was rechecked from the same runs. First-frame matches were cold/warm: 5k×1 99.33%/100%, 5k×4 100%/100%, 5k×8 99.33%/99.33%; 50k×1 96.67%/96%, 50k×4 94%/94%, and 50k×8 97.33%/95.33%. Both 50k×4 cases fall below D58's 95% first-frame target.
- DEFERRED: interactive pairing screen. The spike has no reviewed key-confirmation UI; benchmark pairing uses `shardraild client add` in an isolated temporary data directory.
- Approval-card visual review was completed in the packaged app and saved to the ignored S5 results directory. No provider or model API was used.
- Accepted for now: high-entropy redaction may mask harmless long strings and may miss short secrets until real providers are integrated. Slow-client disconnect reasons appear only in the daemon log, which is sufficient for now.
- Flaky-test reruns after replacing the daemon's cancellable `read_line` with cancellation-safe `Lines::next_line`: `epoch_mismatch_signals_resync_and_streams_history_on_same_connection` passed 50/50; `daemon_file_access_stays_in_opened_directory_after_path_swap` passed 50/50.

## W4-d follow-up (2026-10-21)

- The TypeScript client requires a pinned host key, caps its pre-listener replay queue at 128 frames, reports overflow as an explicit reconnect/resync error, and dispatches one terminal error per connection. Socket closure rejects a reader waiting for a frame consumer. Real-binary tests cover wrong and unauthorized client identities, approval allow/deny, replay with exact cursor membership across two listeners, overflow, and one disconnect notification. CI builds the daemon and fake ACP before those tests.
- The S5 shell source in the local spike checkout uses `host_public_key` from `shardraild client add` and structured Tauri events for daemon frames. The source is ignored and is not part of this branch's tracked tree.
- B4's previous first-rAF callback endpoint was before the browser's paint. A second rAF callback after the live-tail row commits is a valid paint-opportunity proxy: the row's frame had an opportunity to paint before that callback. It does not prove completed paint or scanout, so it cannot by itself establish the literal event-to-painted-frame budget. A packaged-release smoke with the corrected endpoint still produced no samples: the frontend-ready event did not reach the Rust relay listener, and no result was written. Median/p95 and a budget verdict remain unavailable. Raw attempts are under `spikes/s5-ui/results-w4d/b4-smoke/`.
- Fixed the B5 harness gate so only runs explicitly marked `b4=1` wait for daemon frames. The packaged-release 50k×4 recheck completed five cold and five warm repetitions without changing workload or thresholds: cold 150/150 (100%); warm 149/150 (99.33%); combined 299/300 (99.67%). Both cases meet D58's ≥95% budget. Per-run raw p95 and first-frame counts are in `spikes/s5-ui/results-w4d/b5-recheck/`.
- `daemon_file_access_stays_in_opened_directory_after_path_swap` passed 50/50 in this follow-up.

## W4-e headless verification (2026-10-21)

- Moved the S5 prototype shell and measurement sources into tracked `apps/s5-prototype/`; kept the original `spikes/s5-ui` checkout untouched. `node_modules`, `dist`, Tauri `target`, and `results*` are ignored. The B4 script now resolves daemon binaries from the repository root.
- Root cause of the missing B4 readiness event: the shell registered its `s5-ready` listener on `PageLoadEvent::Finished`. The B4 logs show the frontend page first loading without the benchmark query, then loading with it; the `Finished` callback registered the listener after React's startup effect could emit the event. Registration now happens on the matching query's `Started` event, before frontend effects. Rust tests cover listener timing and one-shot relay state; fake-bridge Node tests assert frontend listeners register before readiness is emitted.
- Added `proptest` as a test-only dependency for JSON resume-frame round trips and handshake transcript binding/invalid input properties. Existing integration coverage now includes 24 concurrent authenticated clients, eight SIGKILL restart/replay cycles with SQLite journal integrity checks, slow-client isolation, replay, signature-replay rejection, and a synchronized two-client approval race. The SIGKILL test tolerates event and command reply frames arriving in either order.
- Verification: `cargo fmt --all -- --check`, `nice cargo clippy --workspace --all-targets -- -D warnings`, `nice cargo test --workspace --all-targets` (101 Rust tests, including 25 daemon integration tests; 5.55 s for the integration suite), `cargo deny check`, npm CI installs for contracts and client, client typecheck (4/4 client tests), schema drift, S5 `npm ci`, S5 Node tests (23/23, 1.59 s), S5 production build (1.15 s), S5 headless Tauri Rust tests (2/2; clean compile 18.23 s), and shell syntax checks all passed. `cargo deny` reports the existing duplicate `syn` versions. S5 lint has one existing React hook dependency warning and no errors. B4 was not run.
- Deferred: real B4 packaged-app matrix, physical-keyboard B5 check, and reviewed interactive connection/pairing screen. No GUI, browser, Playwright, or provider was run for W4-e.

Ready-to-run night B4 matrix command (after release binaries and the packaged S5 app are built):

```sh
S5_B4_EVENTS="5000 50000" S5_B4_STREAMS="1 4 8" S5_B4_TEMPERATURES="cold warm" S5_B4_REPETITIONS=5 nice apps/s5-prototype/b4-bench.sh
```

## W4-e review fixes (2026-10-21)

- `b4-bench.sh` now passes `b4=1` and rejects results unless `b4.count` equals the requested event count. The shell relay is only registered for matching `autobench=1`, event-bearing B4 URLs, so B5 does not try to connect to a daemon.
- The fake Tauri bridge test covers cleanup after one listener registers and the other fails. Rust headless tests exercise the matching `Started` page decision, deliver a fake `s5-ready` callback, and verify one relay start. The real B4 run remains deferred.
- Protocol properties now round-trip every client frame variant, verify each transcript-bound nonce/key and negotiated version mutation invalidates the signature, and check cursor replay against randomized in-memory journals. Existing daemon tests continue to cover concurrent clients, slow-client isolation, repeated SIGKILL recovery, and the approval race.
- The Tauri shell is an independent Cargo workspace, so root workspace commands and `cargo deny check` do not cover it. Run its checks separately:

  ```sh
  cargo fmt --manifest-path apps/s5-prototype/src-tauri/Cargo.toml -- --check
  nice cargo clippy --manifest-path apps/s5-prototype/src-tauri/Cargo.toml --all-targets -- -D warnings
  nice cargo test --manifest-path apps/s5-prototype/src-tauri/Cargo.toml
  ```

  `cargo deny check` from that workspace currently fails on Tauri transitive dependencies (`MPL-2.0` licenses and the unmaintained `proc-macro-error` advisory). Extending the repository's license/advisory policy for this spike is deferred; the root workspace `cargo deny check` passes.
- Review-fix verification: root workspace 102 Rust tests passed (including 25 daemon integration tests in 5.54 s); standalone Tauri tests 4/4 passed and standalone Tauri Clippy passed. S5 Node tests passed 25/25 (1.66 s), lint passed without warnings, and production build passed (Vite retains the >500 kB chunk advisory). Formatting, contracts/client npm CI, client typecheck (4/4 tests), schema drift, shell syntax, and root `cargo deny check` passed. No GUI, browser, Playwright, B4 measurement, or provider/API was run.

- D63 (2026-10-21, approved by Daliys): scoped cargo-deny exception for the S5 prototype only, in `apps/s5-prototype/src-tauri/deny.toml`. Pinned license exceptions: MPL-2.0 for `cssparser@0.37.0`, `cssparser-macros@0.7.1`, `dtoa-short@0.3.5`, `selectors@0.38.0`, `option-ext@0.2.0`; `Apache-2.0 WITH LLVM-exception` for `target-lexicon@0.12.16` (build-time). Advisory `RUSTSEC-2024-0370` (`proc-macro-error`, unmaintained, build-time only via Tauri) ignored. All come via Tauri; new crates or version bumps fail again. `s5-ui` now declares `LicenseRef-Proprietary`, `publish = false`. Root `deny.toml` unchanged.
