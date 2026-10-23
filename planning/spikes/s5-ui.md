# S5 — UI stack under load

- **Spike:** S5 (10 §2) · **Measurement date:** 2026-10-21 · **Machine:** MacBook Pro Mac17,2, Apple M5, 32 GB unified memory, macOS 27.0.1, arm64
- **Code:** `apps/s5-prototype/` · **Status:** B3 measured; W4-d B5 50k×4 recheck meets target; W4-h B4 event loss and hidden-window sampling fixed and verified in one follow-up smoke, with p95 still above target

## 1. Goal and scope

Measure the React 19 / TypeScript / Vite prototype in the real Tauri 2 macOS release app (system WKWebView) and in Playwright WebKit and Chromium. The workload includes an empty transcript, 5,000 or 50,000 events, and 1, 4, or 8 mock streams at 75 tokens/s per stream. No provider account, credentials, model, or model API was used; streams are deterministic local fixtures.

This note records B3 and B5 results and the W4-f live readiness and incomplete B4 attempts from the S5 prototype. It does not change D3 or select a stack fallback. The B4 endpoint was corrected to a second rAF callback after React commits a live-tail row; the prior callback-only measurements ended before paint. The local event-creation follow-up metric remains a separate render proxy.

## 2. Method

### Controlled composer and B5

The composer is a controlled React input. Each keystroke updates React state, parses the draft for a slash command, and commits a draft preview. This puts React rendering on the measured path. The Playwright harness uses keyboard typing. The Tauri harness dispatches synthetic `InputEvent`s to the focused composer; it does not claim to measure a physical keyboard path.

The raw B5 timer starts in the input handler and ends in the first `requestAnimationFrame` callback after React commits the preview, using `performance.now()` at callback entry. This is an **input-to-rAF-callback paint proxy**, not a measurement of completed paint, pixels, or display scanout. The separate first-frame measure requires the committed preview `data-draft` to match and the callback to fall within one nominal frame interval. It uses the rAF frame timestamp when that timestamp is at or after input; if the timestamp precedes input (input and commit occurred later in that same frame), callback time is used to verify the 16.67 ms limit. Both callback deltas and raw rAF timestamp deltas are retained. A match means the DOM was committed by the first qualifying rAF callback; it does not prove the user saw painted pixels.

Each scenario has five repetitions in a cold group and five in a warm group. Reported B5 `median/p95` is the median and nearest-rank p95 of the five per-run raw input-to-rAF p95 values. With five runs, nearest-rank p95 is the maximum of those five values. `min–max` is the range across the per-run individual-sample minimums and maximums. The first-frame columns count DOM matches by the first qualifying rAF callback, not visible pixels. No outlier was discarded. Playwright input text starts with `/` and includes command arguments, so slash-command parsing runs on every typed character.

**Interpretation of 16 ms:** a strict input-to-paint limit of `≤16 ms` sits at the nominal 60 Hz frame interval of 16.67 ms. It is not a device-independent guarantee: refresh rate and frame boundaries matter. Proposed budget wording for the owner to decide: **“At least 95% of draft updates match the committed DOM by the first rAF callback within the nominal 60 Hz frame interval (16.67 ms); report raw input-to-rAF median, p95, maximum, and callback/frame timestamps.”** This is a proposal, not a decision or an approved budget.

### Build, launch groups, and workload

Playwright runs build the production Vite bundle before starting `vite preview`; they do not use the dev server. Runs are headful; Chromium uses `--no-sandbox`. “Cold” means a fresh browser process per Playwright repetition. Playwright “warm” reuses one browser process and creates a fresh context for each repetition. Native cold and warm groups both launch fresh Tauri app processes; warm runs follow earlier launches with OS/file caches in place. Caches were not flushed, so “cold” does not mean a clean machine boot.

Loaded B5 typing begins while mock streams are active. A run is accepted only after the expected minimum token output has arrived and streaming has completed. The matrix has 70 native runs and 140 browser runs: seven scenarios × five repetitions × two launch groups, per native app or browser engine. All three surfaces met the required token minimums, and no matrix run reported an application or page error.

The `events` result field remains the requested fixture size; `eventStoreCount` reports the store size including mock-created stream rows. Each current native run records 30 composer samples; Playwright records 26 empty-turn and 27 loaded samples because its command strings differ in length.

### B3 process-tree memory

For each sample, the release shell and newly spawned WebKit WebContent, GPU, and Networking helpers are captured together. `footprint` physical MB and `ps` RSS MB are summed and labelled separately; pre-existing unrelated WebKit helpers are excluded. Five idle samples wait 61 seconds after launch. Five loaded samples capture the process tree 12 seconds into 50k×8 mock streaming. Report median, nearest-rank p95, and min/max; for five samples, p95 equals the maximum.

## 3. Results

### B5 — Tauri release app / WKWebView

Each cell is **median / nearest-rank p95 ms of the five per-run raw input-to-rAF p95s · per-keystroke min–max ms · DOM matches by first qualifying rAF / input samples**.

| Scenario | Cold | Warm |
|---|---:|---:|
| Empty turn | 3 / 4 · 0–11 · 150/150 | 3 / 3 · 0–10 · 150/150 |
| 5k × 1 stream | 15 / 16 · 1–16 · 150/150 | 15 / 16 · 1–16 · 150/150 |
| 5k × 4 streams | 15 / 16 · 1–16 · 150/150 | 15 / 15 · 1–15 · 150/150 |
| 5k × 8 streams | 14 / 15 · 0–15 · 150/150 | 14 / 14 · 0–15 · 150/150 |
| 50k × 1 stream | 15 / 16 · 0–16 · 150/150 | 15 / 15 · 0–16 · 150/150 |
| 50k × 4 streams | 15 / 15 · 0–15 · 150/150 | 14 / 15 · 1–18 · 149/150 |
| 50k × 8 streams | 14 / 15 · 0–17 · 148/150 | 14 / 15 · 1–17 · 149/150 |

In the original broad matrix, cold 50k×4 had one first-frame miss, cold 50k×8 had two, and warm 50k×8 had one: four misses, 2,096/2,100 matched (99.8%). The focused W4-d rerun below supersedes only the original 50k×4 row. The misses exceeded the nominal interval by a small margin and remain in the original data. Raw minima of 0 ms reflect timestamp resolution and do not mean physical scanout was instantaneous. The original `tauri-cold-*` and `tauri-warm-*` result files contain all 2,100 input samples and frame timestamps.

### B5 — Playwright WebKit

Cell format is the same as the native table.

| Scenario | Cold | Warm |
|---|---:|---:|
| Empty turn | 16 / 17 · 0–25 · 126/130 | 15 / 17 · 0–28 · 128/130 |
| 5k × 1 stream | 14 / 15 · 1–17 · 134/135 | 14 / 15 · 1–16 · 135/135 |
| 5k × 4 streams | 13 / 14 · 2–15 · 135/135 | 14 / 14 · 1–15 · 135/135 |
| 5k × 8 streams | 14 / 14 · 2–14 · 135/135 | 371 / 390 · 2–403 · 29/135 |
| 50k × 1 stream | 15 / 15 · 1–16 · 135/135 | 394 / 399 · 20–406 · 0/135 |
| 50k × 4 streams | 14 / 14 · 1–14 · 135/135 | 383 / 390 · 18–422 · 2/135 |
| 50k × 8 streams | 14 / 14 · 1–14 · 135/135 | 382 / 401 · 7–407 · 4/135 |

### B5 — Playwright Chromium

Cell format is the same.

| Scenario | Cold | Warm |
|---|---:|---:|
| Empty turn | 1.5 / 1.7 · 0.2–5.9 · 130/130 | 1.7 / 2.3 · 0.7–6.7 · 130/130 |
| 5k × 1 stream | 7.5 / 8 · 0.4–8.5 · 135/135 | 6.4 / 7.5 · 0.7–8.6 · 135/135 |
| 5k × 4 streams | 7.6 / 7.6 · 0.4–7.9 · 135/135 | 6.8 / 8.3 · 0.6–8.7 · 135/135 |
| 5k × 8 streams | 7.2 / 8 · 0.5–9 · 135/135 | 7.4 / 7.6 · 0.4–7.8 · 135/135 |
| 50k × 1 stream | 7.9 / 8.2 · 0.7–8.7 · 135/135 | 7.5 / 7.9 · 0.6–8.4 · 135/135 |
| 50k × 4 streams | 7.6 / 7.9 · 0.3–8 · 135/135 | 6.7 / 7.5 · 0.4–9.2 · 135/135 |
| 50k × 8 streams | 7.7 / 7.9 · 0.3–9.3 · 135/135 | 7.2 / 8 · 0.5–9 · 135/135 |

Across 1,880 WebKit input samples, 1,368 met the first-frame criterion; WebKit's warm 5k×8 and 50k scenarios include multi-hundred-millisecond outliers retained in the run records. Their cause was not isolated. All 1,880 Chromium input samples met the criterion. Browser cold-launch times are separate from B5 in `results/playwright.json`; WebKit does not expose the Long Tasks API in this harness. These browser figures are comparison data, not the native acceptance result.

### B3 — Tauri release process tree

| Workload | Physical footprint MB: median / p95 · min–max | RSS MB: median / p95 · min–max |
|---|---:|---:|
| Idle | 113.708 / 114.986 · 112.135–114.986 | 233.570 / 234.373 · 231.801–234.373 |
| 50k events × 8 streams | 396.087 / 460.198 · 198.807–460.198 | 453.378 / 481.149 · 398.623–481.149 |

Against the proposed 180 MB idle physical-footprint ceiling, the worst observed idle sample leaves **65.014 MB headroom**. Loaded physical footprint rises substantially; the idle ceiling should not be described as a loaded-memory pass. The loaded physical-footprint range was 198.807–460.198 MB; the RSS range was 398.623–481.149 MB. Repetition 4 had the lowest loaded physical-footprint value (198.807 MB) despite 398.623 MB RSS; the discrepancy is unexplained and needs review before treating the loaded range as stable. The largest loaded sample occurred in repetition 3. RSS and physical footprint are different measures and must not be compared as if interchangeable.

### B4 — Tauri release app / WKWebView

The intended native method connects the release app to the daemon through a Tauri shell relay over its Unix socket. The relay uses an Ed25519 client key registered in a per-run temporary data directory. Durable fake-agent events carry daemon wall-clock receipt timestamps; five `Date.now()`/`performance.now()` samples map daemon time to the page clock. The corrected endpoint is entry to a second rAF callback after the row commits. That callback is after the frame containing the row had a paint opportunity, so it is a valid conservative paint-opportunity proxy. It does not measure completed paint or display scanout and cannot alone prove the literal painted-frame budget. Clock mapping uncertainty is emitted in the result JSON; with no completed sample result, no run-specific uncertainty is available.

The original cold/warm × 5k/50k events × 1/4/8 streams × five repetitions matrix is withdrawn: automated scrolling mounted old virtualized rows seconds after their daemon receipt, and those scroll-time mounts were counted as B4 latency. Daemon subscribers also polled every 100 ms. The implementation now follows the live tail and wakes subscribers after appends.

W4-f produced 16 cold 5k×1 smoke/diagnostic attempts in total; every per-run JSON explicitly reports a timeout. The initial attempts failed before readiness. A CSP violation captured during diagnosis identified the duplicate static `index.html` policy blocking Tauri's per-launch isolation iframe. After removing that meta policy and rebuilding, the packaged release app launched only through `apps/s5-prototype/b4-bench.sh` and logged the full live chain: frontend readiness received, daemon relay started, client paired, socket authenticated, session requests sent, and 5,000 daemon frames received. The smoke trace counted 5,000 durable agent chunks.

After the CSP correction, the `smoke-live`, `smoke-trace`, and `smoke-trace2` cold 5k×1 attempts timed out (75–120 seconds) without a valid B4 result. One trace recorded `connected: true`, 5,000 chunks, and zero second-rAF samples; benchmark progress did not pass its daemon-stream-complete point. The W4-f review fix now waits for completion after one successful connection, excludes lifecycle rows from the B4 population, accepts a nonzero rendered-sample count only when all configured agent chunks arrived, and disables mock autostart and synthetic scrolling for B4. Three follow-up cold 5k×1 attempts and one 10×1 diagnostic attempt also timed out without valid latency samples; all failed records are retained under `apps/s5-prototype/bench-results/w4f-20261021/` and `apps/s5-prototype/bench-results/w4f-fix*/`. The browser-side automation/collection hang remains unresolved.

The 60-run cold/warm × 5k/50k × 1/4/8 × five-repetition matrix was not run. This review pass allowed at most 20 remeasurement runs; the 60-run gate remains open. B4 median/p95 values and the ≤50 ms verdict are unavailable; the optional B5 rerun was skipped. The second-rAF endpoint remains a paint-opportunity proxy, not proof of completed paint or display scanout. Rust now records process-start-to-page and process-start-to-readiness monotonic timings; the follow-up 5k readiness logs were 381–450 ms, and the 10-event check was 359 ms. No valid B4 sample or run-specific clock uncertainty was produced.

#### W4-g live smoke follow-up (2026-10-22)

Three cold 5k×1 smokes were run through the packaged release app with `b4=1`, each with a 120-second harness timeout. All three logs confirm frontend readiness, relay authentication, session requests, and receipt of all 5,000 daemon frames. All three timed out without a saved result: **0/3 valid smokes**. Raw timeout records and logs are under `apps/s5-prototype/bench-results/w4g-live/`.

Two fixes were committed while diagnosing the missing B4 results: `a5146f8` checks row presence at layout commit and retains the sample through the second rAF even if virtualization removes the row; `148f427` schedules B4 sampling from each daemon row's layout commit. The S5 test suite passed 31/31 after both changes, lint passed, and the release app was rebuilt. The live smoke still did not save a result, so neither fix is validated end to end.

The matrix was not started: **0/60 valid matrix runs**. The cold 5k×1 smoke cell has no valid sample arrays; per-cell and aggregate median/p95 are unavailable. There is no B4 pass/fail verdict against **≤50 ms**. The endpoint remains entry to a second rAF callback after layout commit, which is a paint-opportunity proxy and does not prove completed paint or display scanout. Do not infer latency from the relay frame receipt logs.

#### W4-h live diagnosis (2026-10-22)

Four fresh-release cold 5k×1 diagnostic smokes were run, the W4-h live-run limit. Tracing from the WKWebView showed fire-and-forget Tauri event batches outran the frontend: Rust dispatched 5,006 frames, while the frontend had received 1,006 frames and counted 995 agent chunks. The relay now waits for a frontend acknowledgement per event batch and uses a bounded 64-frame queue; a Rust test covers the acknowledgement wait.

After all 5,000 agent chunks reached the frontend, the final smoke's stderr trace reported `visibility=hidden`, `focused=false`, `committedRows=0`, and `samples=0`; those fields are absent from the saved result artifact because that packaged bundle predated the source change. The app had been started as a background executable, leaving WKWebView rAF sampling suspended. The attempted `open -a` activation could launch an unmanaged duplicate, so it has been removed; the B4 page load now shows and focuses its own Tauri window. One follow-up smoke verified this activation. B4 skips unrelated synthetic composer typing before waiting for its daemon stream.

The second attempt was accepted by the former count-only gate with 17 samples, but reported B4 median/p95 of 77,158/77,461 ms and had no visibility fields. It is excluded as an invalid hidden-window measurement. The W4-h harness required a visible page, every requested chunk, and at least `min(events, max(20, 1% of events))` samples; this was the pre-D65 floor, now 0.5%. It records focus but does not gate on it because rAF depends on visibility.

The final diagnostic POST returned HTTP 200, but the saved sample count was zero and the harness rejected it. Valid smokes: **0/4**. The full matrix was not started: **0/60 valid runs**; every cell is 0/5 with median/p95 unavailable. The ≤50 ms verdict stays undetermined. The measurement endpoint remains the second rAF opportunity after row commit; it does not prove paint completion or display scanout. Logs are under `apps/s5-prototype/bench-results/w4h/`.

#### W4-h review-fix smoke (2026-10-22)

One follow-up cold 5k×1 smoke passed with a visible, focused page, all 5,000 chunks, and 751 second-rAF samples (15.02% coverage). Its median/p95 were 579/733 ms, above the proposed ≤50 ms target. This is one valid diagnostic run, not enough to determine the budget verdict; the 60-run matrix remains 0/60. The pre-D65 gate required at least `min(events, max(20, 1% of events))` samples; this run's 751 samples exceed its 50-sample floor. Raw logs and result JSON are under `/Users/daliys/Daliys/Shardrail/spikes/reviews9/w4h-fix-smoke/`.

After the smoke, the relay was changed headlessly to keep up to four 64-frame batches in flight while retaining exact acknowledgements and a bounded queue. The live smoke predates this final relay window and does not verify its latency effect; no second smoke was run under the one-smoke review limit.

#### W4-i full B4 matrix (2026-10-22)

The fresh packaged release app completed all 60 cold/warm × 5k/50k × 1/4/8-stream runs, five repetitions per cell. The D64 endpoint is the second `requestAnimationFrame` callback after daemon-row layout commit, accepted by Daliys; the result remains a paint-opportunity proxy. All 60 runs passed the then-current 1% harness gate (visible page and all requested fake-agent chunks); D65 later lowers the sample floor to 0.5%. Page focus was true in all runs. No failed or missing run records: raw JSON, app/daemon/server logs, timestamps, and per-run sample coverage are under `apps/s5-prototype/bench-results/w4i/`.

Latency percentiles pool the individual row samples across the five valid runs in each cell (median uses the midpoint for even populations; p95 is nearest-rank). The overall population is 200,036 samples from 1,650,000 requested events (12.12% coverage), with median/p95 **900/3,014 ms**. All twelve cell p95 values exceed the proposed **≤50 ms** budget. Per-run sample count and coverage are in `run-coverage.csv`; the full table and verdict are in `/Users/daliys/Daliys/Shardrail/spikes/reviews9/w4i-matrix-report.md`.

Every app log contains non-fatal `ResizeObserver loop completed with undelivered notifications` reports (9,367 total occurrences). The runner also logs an expected initial localhost health-check connection refusal while the results server starts; it recovered before each run. These warnings did not prevent result collection or cleanup. Sampling covers mounted live-tail rows only; it does not establish completed paint or display scanout.

Each one-repetition runner invocation labels its internal query/log `r1`; the outer per-run folder and `run.log` carry the intended repetition number. Result JSON files are stored with the folder's run label. The report documents removal of wrapper-created false missing-result sentinels while retaining all 60 actual result payloads.

B5's 50k×4 recheck completed five repetitions per launch group in the packaged release app, with the original 50k events, four local mock streams, and unchanged 16.67 ms first-frame criterion. Cold repetitions matched 30/30 each (raw input-to-rAF p95: 15, 14, 13, 15, 15 ms; group 150/150, 100%). Warm repetitions matched 30/30, 30/30, 30/30, 30/30, and 29/30 (raw p95: 15, 14, 14, 15, 14 ms; group 149/150, 99.33%). Combined: 299/300 (99.67%), above D58's ≥95% first-frame budget. The five-run median/nearest-rank p95 of per-run raw p95s were 15/15 ms cold and 14/15 ms warm. Raw records are in `spikes/s5-ui/results-w4d/b5-recheck/`.

### Other observations

The local `mockCreationToMountedRowFollowupP95` remains only a render-follow-up proxy. Scroll, frame, heap, and long-task diagnostics remain in the raw per-run JSON; WebKit does not expose all browser diagnostics available in Chromium.

## 4. Verdict

- **B3 idle:** The whole Tauri/WebKit process tree measures below 180 MB physical footprint in all five new samples, with 61.229 MB of worst-sample headroom. Loaded 50k×8 usage is separately reported and exceeds 180 MB.
- **B5:** The original broad matrix did not meet the ≥95% first-frame budget for every native scenario. Its 50k×4 row is superseded by the focused W4-d rerun, which matched 299/300 (99.67%). This does not replace results for the other scenarios. The original matrix's native aggregate was 2,096/2,100; note `spikes/reviews4/n3-final.md` quotes 2,099/2,100 and WebKit 1,734/1,880, while original result files give 2,096/2,100 and 1,368/1,880. WebKit had 512 first-frame misses across 1,880 inputs; warm 5k×8 and 50k scenarios showed raw run-level p95s around 371–401 ms. Chromium met the first-frame criterion in all 1,880 inputs and stayed below 10 ms raw maxima. Strict `≤16 ms` is near the nominal 60 Hz frame interval of 16.67 ms, so the owner should decide whether to specify a raw-time limit, a first-frame rate, or both. The owner adopted the first-frame-rate wording as D58 (2026-10-20).
- **B4:** No matrix verdict. W4-h found event-batch loss without frontend backpressure and hidden-window rAF suspension (`visibility=hidden`, zero committed rows/samples). The relay now waits for matching frontend batch acknowledgements, and the benchmark focuses its own Tauri window. One follow-up 5k×1 smoke passed the corrected collection gates but reported median/p95 of 579/733 ms, above the proposed ≤50 ms target. The 60-run matrix remains 0/60 and the budget verdict is undetermined. The second-rAF endpoint is a paint-opportunity proxy, not proof of completed paint.
- **B4 W4-i matrix (2026-10-22):** The full packaged-release matrix completed: 60/60 runs passed the then-current visibility, chunk-count, and 1% sample-floor gates; D65 later lowers the sample floor to 0.5%. Pooled D64 sample median/p95: 900/3,014 ms (200,036 samples from 1,650,000 requested events; 12.12% coverage), exceeding the proposed ≤50 ms target. Every cell p95 also exceeded 50 ms. Per-run coverage and raw records are in `apps/s5-prototype/bench-results/w4i/`; the full report is `/Users/daliys/Daliys/Shardrail/spikes/reviews9/w4i-matrix-report.md`. The second-rAF endpoint remains a paint-opportunity proxy over mounted live-tail rows, not proof of completed paint or scanout. Repeated non-fatal `ResizeObserver loop completed with undelivered notifications` messages appeared in all 60 app logs.
- **Stack decision:** D49's B4 requirement remains open until the corrected measurement matrix is rerun. No stack decision follows from the withdrawn B4 values. The native input path is synthetic, so a physical-keyboard confirmation remains useful before treating B5 as final user-perceived latency evidence.

## 5. Reproduction

From the repository root:

```sh
cd apps/s5-prototype
npm ci
npx playwright install chromium webkit
npm test
npm run lint
npm run a11y
npm run build
S5_BENCH_SMOKE=1 npm run bench       # smoke writes a timestamped results/playwright-smoke-*.json
npm run bench                  # production vite preview; 5 cold + 5 warm per scenario/engine
npm run tauri -- build --bundles app
bash tauri-bench.sh            # release .app; 5 cold + 5 warm per scenario
python3 b3-bench.py            # 5 idle and 5 loaded process-tree snapshots
```

`npm run bench` builds the production bundle before starting `vite preview`, launches headful Playwright WebKit and Chromium, and uses `--no-sandbox` for Chromium. A full run writes `results/playwright.json`; smoke mode defaults to a timestamped file, and `S5_BENCH_OUTPUT` selects a path explicitly. `tauri-bench.sh` starts a loopback-only result receiver, launches and closes the bundled release app for each scenario, captures each JSON result and process snapshots, and stops the receiver. `b3-bench.py` uses the same release bundle and records `footprint` physical MB separately from summed `ps` RSS. Each matrix command keeps outliers in the result files.

Raw data and process snapshots are in `spikes/s5-ui/results/`. The result files include per-run launch/latency/frame metrics, callback and rAF timestamp deltas, generated-token counts, and process samples. The B3 capture repeats five idle samples after a 61 s settle and five loaded 50k×8 snapshots after 12 s. Apps and harness-launched servers were closed after collection. The benchmark does not implement a reviewed interactive pairing screen; setup uses the daemon CLI in an isolated temporary data directory. The approval card was visually checked in the release app; `approval-card-review.png` is the saved capture. The bundle build retains Vite's warning that the JavaScript chunk exceeds 500 kB.

#### W4-j headless streaming rework (2026-10-22)

The S5 B4 data path now uses a Tauri IPC channel for ordered stream notices; Rust retains the durable event list and answers bounded aggregate or per-session range queries (up to 256 rows). Only a channel notice's fetched version releases backpressure; scroll-only range responses cannot acknowledge an unrelated notice. Status remains on Tauri events because it carries only connection lifecycle state. The frontend coalesces notices within one animation frame, preserves a scrolled-up window, and commits a bounded window synchronously after the channel's frame coalescing. B4 uses a 128-row live-tail window and 32-row virtualizer overscan to collect the unchanged minimum sample floor during fast streams; ordinary transcript views keep a 32-row tail and 8-row overscan. Fixed 112 px virtual rows remove row remeasurement; row refs replace selector lookups. Markdown memoization uses `(row id, content hash)` with source equality as a collision guard, and `rehype-sanitize` remains enabled. Repeated range reads cannot resample a row whose D64 sample has completed.

The B4 endpoint is unchanged: page `performance.now()` at entry to the second rAF callback after the row's layout commit, adjusted by the existing page/daemon wall-clock mapping. Visibility, all requested chunk count, and minimum sample gates remain unchanged. Headless fake-channel, per-session window, range, sanitizer, unique-sample, and D64 tests cover the path. The existing W4-i live B4 numbers are historical; no live B4 measurement was run for W4-j or this review. No runner parameter changes are required: the frontend now creates the channel and invokes `start_b4_relay` after registering the status listener. The existing app freshness check covers the changed frontend and Rust sources.

#### W4-m D65 re-gate of existing night data (2026-10-23)

D65 changes only the minimum sample floor to `min(events, max(20, ceil(0.5% of events)))`; D64's second-rAF endpoint and viewport-intersecting-row population are unchanged. No live runs were started. The immutable W4-j Tauri and W4-k2 Swift records were re-evaluated with the saved per-row sample arrays in `apps/s5-prototype/bench-results/w4j/` and `apps/s5-prototype/bench-results/w4k2-swift/`. Results are in `apps/s5-prototype/bench-results/w4m-reeval/b4.md` and `.json`: Tauri 18/60 and Swift 60/60 valid runs. Tauri result JSONs with only a sample count and an error (no per-row sample array) remain invalid and have no recomputed percentiles; saved-array medians and p95s are recomputed for valid runs only. The new per-cell floor is 25 rows at 5k and 250 at 50k events. D49 remains Tauri + React; Swift is reference-only.
