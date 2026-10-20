# S5 — UI stack under load

- **Spike:** S5 (10 §2) · **Measurement date:** 2026-10-20 · **Machine:** MacBook Pro Mac17,2, Apple M5, 32 GB unified memory, macOS 27.0.1, arm64
- **Code:** `spikes/s5-ui/` (not committed) · **Status:** B3 and B5 measured; B4 remains unmeasured

## 1. Goal and scope

Measure the React 19 / TypeScript / Vite prototype in the real Tauri 2 macOS release app (system WKWebView) and in Playwright WebKit and Chromium. The workload includes an empty transcript, 5,000 or 50,000 events, and 1, 4, or 8 mock streams at 75 tokens/s per stream. No provider account, credentials, model, or model API was used; streams are deterministic local fixtures.

This note updates B3 and B5 methodology and results. It does not change D3 or select a stack fallback. B4 needs daemon receipt timestamps and a calibrated mapping to the page clock; the local event-creation follow-up metric is not a B4 measurement.

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
| 50k × 4 streams | 15 / 15 · 0–17 · 149/150 | 14 / 16 · 0–16 · 150/150 |
| 50k × 8 streams | 14 / 15 · 0–17 · 148/150 | 14 / 15 · 1–17 · 149/150 |

Cold 50k×4 had one first-frame miss, cold 50k×8 had two, and warm 50k×8 had one: four misses, 2,096/2,100 matched (99.8%). The misses exceeded the nominal interval by a small margin and remain in the data. Raw minima of 0 ms reflect timestamp resolution and do not mean physical scanout was instantaneous. The new `tauri-cold-*` and `tauri-warm-*` result files contain all 2,100 input samples and frame timestamps.

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

### Other observations

The local `mockCreationToMountedRowFollowupP95` remains only a render-follow-up proxy. It ends after a mounted row's next frame and a following task, not at a daemon-received event's painted frame. B4 is therefore **unmeasured**. Scroll, frame, heap, and long-task diagnostics remain in the raw per-run JSON; WebKit does not expose all browser diagnostics available in Chromium.

## 4. Verdict

- **B3 idle:** The whole Tauri/WebKit process tree measures below 180 MB physical footprint in all five new samples, with 61.229 MB of worst-sample headroom. Loaded 50k×8 usage is separately reported and exceeds 180 MB.
- **B5:** This matrix does not establish a pass for the 16 ms budget. Native cold 50k×4 and 50k×8 each have a 17 ms individual sample, with four first-frame misses across 2,100 inputs (2,096/2,100 matched, 99.8%; this meets the D58 ≥95% budget, pending the physical-keyboard check). Note: `spikes/reviews4/n3-final.md` quotes 2,099/2,100 and WebKit 1,734/1,880; the result files give 2,096/2,100 and 1,368/1,880, and the result files are authoritative. WebKit has 512 first-frame misses across 1,880 inputs; warm 5k×8 and 50k scenarios show raw run-level p95s around 371–401 ms. Chromium met the first-frame criterion in all 1,880 inputs and stayed below 10 ms raw maxima. Strict `≤16 ms` is near the nominal 60 Hz frame interval of 16.67 ms, so the owner should decide whether to specify a raw-time limit, a first-frame rate, or both. The owner adopted the first-frame-rate wording as D58 (2026-10-20).
- **B4:** Still unmeasured. No daemon receipt timestamp or calibrated daemon-to-page clock is available.
- **Stack decision:** Consistent with owner-approved D3 and D49, these results do not trigger the Svelte fallback. D49 keeps S5 open until B4 has real daemon receipt timestamps. The native input path is synthetic, so a physical-keyboard confirmation remains useful before treating this as final user-perceived latency evidence. No decision register entry was changed.

## 5. Reproduction

From the repository root:

```sh
cd spikes/s5-ui
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

Raw data and process snapshots are in `spikes/s5-ui/results/`. The result files include per-run launch/latency/frame metrics, callback and rAF timestamp deltas, generated-token counts, and process samples. The B3 capture repeats five idle samples after a 61 s settle and five loaded 50k×8 snapshots after 12 s. Apps and harness-launched servers were closed after collection. The bundle build retains Vite's warning that the JavaScript chunk exceeds 500 kB.
