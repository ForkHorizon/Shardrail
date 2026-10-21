# Wave 4 step e build report

Date: 2026-10-21  
Branch: `p1a/coordinator`  
Mode: headless only

## Changes

- Copied the S5 UI shell and measurement harness from `/Users/daliys/Daliys/Shardrail/spikes/s5-ui` into tracked `apps/s5-prototype/`. The original directory remains untouched. Generated output, dependencies, Tauri targets, and `results*` are ignored. The B4 daemon binary defaults now resolve from the repository root after the move.
- Fixed readiness-listener registration in `apps/s5-prototype/src-tauri/src/main.rs`: it registers on the matching benchmark page's `Started` event, before React startup effects can emit `s5-ready`. Readiness remains one-shot. The frontend bridge registers daemon listeners before emitting readiness.
- Added Rust readiness-state tests and Node fake-bridge tests. Added test-only `proptest` coverage for resume-frame round trips and handshake transcript binding, plus daemon stress/recovery coverage for 24 concurrent authenticated clients and eight SIGKILL restart/replay cycles. Existing tests cover slow clients and a synchronized competing approval race.
- Updated [daemon-status.md](../../docs/dev/daemon-status.md) with evidence, checks, deferred items, and the exact night B4 command.

## UI-ready root cause and evidence

The ignored W4-d B4 logs under `spikes/s5-ui/results-w4d/b4-smoke/` show the first page load at `tauri://localhost` without a query, followed by a redirected load with the benchmark query. The shell listened for `s5-ready` only in the matching page's `Finished` callback. React's startup effect can run and emit before that callback installs the Rust listener, so the event is lost. The listener was moved to the matching query's `Started` callback; the previous and corrected order are represented in headless Rust and JS tests. No real B4 measurement was run to validate the change.

## Verification results

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `nice cargo clippy --workspace --all-targets -- -D warnings` | pass, 0.64 s |
| `nice cargo test --workspace --all-targets` | pass: 101 Rust tests; daemon integration suite 25/25 in 5.55 s |
| `cargo deny check` | pass; existing duplicate `syn` version warning |
| `npm ci` in contracts and client | pass |
| `npm --prefix packages/client run typecheck` | pass |
| `npm --prefix packages/client test` | pass: 4/4 |
| `scripts/check-schema-drift.sh` | pass |
| `npm ci` in `apps/s5-prototype` | pass |
| S5 `npm test` | pass: 23/23 in 1.59 s |
| S5 `npm run build` | pass in 1.15 s; Vite reports the existing >500 kB bundle advisory |
| `nice cargo test --manifest-path apps/s5-prototype/src-tauri/Cargo.toml` | pass: 2/2 readiness tests; first headless compile 18.23 s |
| `npm run lint` in S5 | pass with one existing React hook dependency warning |
| `bash -n apps/s5-prototype/b4-bench.sh apps/s5-prototype/tauri-bench.sh` | pass |

The first workspace test attempt exposed a test-only ordering assumption: the streamed durable event may arrive before `command_accepted`. The recovery loop now accepts either frame order while requiring both; the full workspace run then passed. The initial S5 production build also caught a callback variance issue in the new bridge helper; its generic frame type fixes it.

`proptest` is the only new Rust dependency, and it is dev-only. It supplies the requested randomized property checks without adding runtime code or a production dependency. No browser, Playwright, Tauri application, provider, or model API was launched.

## Deferred

- Real B4 packaged-app run and its latency verdict. Night command, after release binaries and the packaged S5 app are built:

  ```sh
  S5_B4_EVENTS="5000 50000" S5_B4_STREAMS="1 4 8" S5_B4_TEMPERATURES="cold warm" S5_B4_REPETITIONS=5 nice apps/s5-prototype/b4-bench.sh
  ```

- Physical-keyboard B5 check.
- Reviewed interactive connection and pairing screen.

The UI-ready fix is supported by registration-order tests and existing logs; actual delivery and B4 timing remain unverified until the night run.
