# S5 — UI stack under load

- **Spike:** S5 (10 §2) · **Measurement dates:** 2026-10-20 (Phase 0 B3/B5), 2026-10-22 (W4-i B4), 2026-10-23 (W4-m D64/D65 re-gate) · **Machine:** MacBook Pro Mac17,2, Apple M5, 32 GB unified memory, macOS 27.0.1, arm64
- **Code:** Phase 0 prototype in `spikes/s5-ui/`; W4-i/W4-j/W4-l/W4-m Tauri harness in `apps/s5-prototype/` and Swift/AppKit spike in `apps/s5-swift/` on the `p1a/coordinator` worktree · **Status:** B3 passes its budget. The B5 composer matrix meets D58's first-frame-rate requirement; physical-keyboard verification remains pending. B4 measured across Tauri 2 and Swift/AppKit under D64/D65; both stacks miss the 50 ms budget by 2–70×. Available valid results show similar latency growth with event count and point to the daemon→UI delivery path, but do not isolate the cause; Tauri has no valid 50k×4/×8 samples. D49 retains Tauri 2 + React 19. B4 is parked as known performance debt (owner, 2026-10-23); delivery throughput optimization and a night Tauri B4 rerun at 0.5% are later backlog items. Under D66 (2026-10-23), Phase 1 is browser-first (the same React UI in a normal browser over loopback WebSocket; Tauri shell as a later desktop port).

## 1. Goal and scope

Evaluate whether the React 19 / TypeScript / Vite UI and Tauri 2 macOS release app (system WKWebView) satisfy memory (B3 idle ≤180 MB), input responsiveness (B5 ≥95% first-frame match, ≤16.67 ms), and streaming event latency (B4 p95 ≤50 ms) under load. Workloads include an empty transcript, 5,000 or 50,000 events, and 1, 4, or 8 mock streams at 75 tokens/s per stream. Streams are deterministic local fixtures; no real provider account, credentials, model, or model API was used.

Current stack and product alignment:
- **D49 stack confirmation (2026-10-23):** Tauri 2 + React 19 is retained. A native Swift/AppKit list spike (`apps/s5-swift`) was implemented and compared against Tauri under identical workloads and semantics (W4-m re-gate). Both stacks missed B4; available results point toward delivery-path work but do not establish that a UI rewrite would not help. Tauri has no valid 50k×4/×8 samples.
- **D66 browser-first Phase 1 (2026-10-23):** Under [18 — Browser-First Roadmap](../18-browser-first-roadmap.md), the primary Phase 1 client is the React web UI running in a standard web browser connected to the local daemon over an opt-in loopback WebSocket (`127.0.0.1`), with local one-time enrollment codes and transcript-bound Ed25519 pairing. The Tauri desktop shell is deferred as a later desktop port. Current development focuses on the browser pairing ceremony and main transcript screen; S5 results remain authoritative and directly relevant for the shared React SPA rendering pipeline (TanStack Virtual, streaming Markdown, per-frame store coalescing).
- **UI alignment with D67–D70:** UI delivery accounts for standard OS install and data locations (D68; candidate to embed built browser UI assets into `shardraild` to eliminate version skew, data in `~/Library/Application Support/Shardrail`), multi-machine remote access (D67; SSH stdio bridge or later hub relay), hub end-to-end encryption (D69; AEAD between client and daemon), and hub metadata privacy (D70; descriptive metadata such as machine and session names is decrypted client-side in the browser UI, while the hub sees only opaque routing IDs, online/offline status, and connection leakage).

## 2. Method

### Controlled composer and B5

The composer is a controlled React input. Each keystroke updates React state, parses the draft for a slash command, and commits a draft preview. This puts React rendering on the measured path. The Playwright harness uses keyboard typing. The Tauri harness dispatches synthetic `InputEvent`s to the focused composer; it does not claim to measure a physical keyboard path.

The raw B5 timer starts in the input handler and ends in the first `requestAnimationFrame` callback after React commits the preview, using `performance.now()` at callback entry. This is an **input-to-rAF-callback paint proxy**, not a measurement of completed paint, pixels, or display scanout. The separate first-frame measure requires the committed preview `data-draft` to match and the callback to fall within one nominal frame interval. It uses the rAF frame timestamp when that timestamp is at or after input; if the timestamp precedes input (input and commit occurred later in that same frame), callback time is used to verify the 16.67 ms limit. Both callback deltas and raw rAF timestamp deltas are retained. A match means the DOM was committed by the first qualifying rAF callback; it does not prove the user saw painted pixels.

Each scenario has five repetitions in a cold group and five in a warm group. Reported B5 `median/p95` is the median and nearest-rank p95 of the five per-run raw input-to-rAF p95 values. With five runs, nearest-rank p95 is the maximum of those five values. `min–max` is the range across the per-run individual-sample minimums and maximums. The first-frame columns count DOM matches by the first qualifying rAF callback, not visible pixels. No outlier was discarded. Playwright input text starts with `/` and includes command arguments, so slash-command parsing runs on every typed character.

**Interpretation of 16 ms:** a strict input-to-paint limit of `≤16 ms` sits at the nominal 60 Hz frame interval of 16.67 ms. It is not a device-independent guarantee: refresh rate and frame boundaries matter. D58 budget: **“At least 95% of draft updates match the committed DOM by the first rAF callback within the nominal 60 Hz frame interval (16.67 ms); report raw input-to-rAF median, p95, maximum, and callback/frame timestamps.”** This wording was adopted as D58 on 2026-10-20.

### Build, launch groups, and workload

Playwright runs build the production Vite bundle before starting `vite preview`; they do not use the dev server. Runs are headful; Chromium uses `--no-sandbox`. “Cold” means a fresh browser process per Playwright repetition. Playwright “warm” reuses one browser process and creates a fresh context for each repetition. Native cold and warm groups both launch fresh Tauri app processes; warm runs follow earlier launches with OS/file caches in place. Caches were not flushed, so “cold” does not mean a clean machine boot.

Loaded B5 typing begins while mock streams are active. A run is accepted only after the expected minimum token output has arrived and streaming has completed. The matrix has 70 native runs and 140 browser runs: seven scenarios × five repetitions × two launch groups, per native app or browser engine. All three surfaces met the required token minimums, and no matrix run reported an application or page error.

The `events` result field remains the requested fixture size; `eventStoreCount` reports the store size including mock-created stream rows. Each current native run records 30 composer samples; Playwright records 26 empty-turn and 27 loaded samples because its command strings differ in length.

### B3 process-tree memory

For each sample, the release shell and newly spawned WebKit WebContent, GPU, and Networking helpers are captured together. `footprint` physical MB and `ps` RSS MB are summed and labelled separately; pre-existing unrelated WebKit helpers are excluded. Five idle samples wait 61 seconds after launch. Five loaded samples capture the process tree 12 seconds into 50k×8 mock streaming. Report median, nearest-rank p95, and min/max; for five samples, p95 equals the maximum.

### B4 streaming latency method and gates

The B4 benchmark measures streaming event delivery from daemon receipt to a client paint-opportunity callback:
- **Client connection and clock sync:** The client connects to the daemon through a local relay (or socket) using an Ed25519 key registered in an isolated per-run temporary data directory. Durable fake-agent events carry daemon wall-clock receipt timestamps. The page calibrates `Date.now()` against `performance.now()` five times per run and uses the mean offset for conversion (uncertainty ≤ ±1 ms).
- **D64 B4 semantics (owner-approved 2026-10-22, clarified 2026-10-23):** The timer starts at the durable daemon receipt timestamp mapped to the page clock. It ends at `performance.now()` on entry to the second `requestAnimationFrame` callback after the event row is committed to the DOM and layout-visible. This measures callback latency (a paint-opportunity proxy), not completed paint or display scanout.
- **Viewport intersection filter (W4-l overscan fix):** Virtualized rows (TanStack Virtual in React, `table.rows(in: table.visibleRect)` in Swift) are strictly filtered by layout interval (`start`/`size`) against the transcript viewport (`scrollTop`/`clientHeight`). Rows mounted only in overscan are excluded from B4 measurement; they become eligible only once they actually scroll into the viewport. Partial overlaps qualify; edge-only contact and zero-height viewports do not.
- **D65 sample gate (delegated to AI Platform, 2026-10-23):** Minimum measured-row floor is `min(events, max(20, ceil(events × 0.005)))`. For 5,000 events, the gate requires at least 25 measured visible rows (125 across 5 repetitions). For 50,000 events, it requires at least 250 measured visible rows (1,250 across 5 repetitions). A run with fewer verified per-row samples is marked invalid and excluded from pooled latency statistics.

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

Cold 50k×4 had one first-frame miss, cold 50k×8 had two, and warm 50k×8 had one: four misses, 2,096/2,100 matched (99.8%). The misses exceeded the nominal interval by a small margin and remain in the data. Raw minima of 0 ms reflect timestamp resolution and do not mean physical scanout was instantaneous. The `tauri-cold-*` and `tauri-warm-*` result files contain all 2,100 input samples and frame timestamps.

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

### B4 — Latency results and cross-stack comparison

#### Evolution of B4 measurement

1. **Original Phase 0 matrix (withdrawn):** Automated scrolling mounted virtualized rows seconds after daemon receipt, polluting latency calculations; daemon subscribers also polled durable events every 100 ms. Discarded as invalid.
2. **W4-i corrected matrix (historical):** All 60/60 planned runs were valid under the initial 1% sample gate without overscan exclusion. Overall pooled median/p95 was **900 / 3,014 ms** across 200,036 sampled rows, failing the proposed p95 ≤50 ms target. At 5k events, cell medians were ~100–230 ms; at 50k × 1 stream they reached ~2 seconds.
3. **W4-j rework:** Introduced Tauri channels, a Rust-side windowed event store, per-frame coalescing, memoized Markdown, and fixed row heights. In-memory microbenchmarks showed the Rust store completing 50,000 JSON append/range pairs in 149.7 ms (333,948 pairs/s) and frontend projection updates taking 0.016–0.019 ms per 64-row update. These microbenchmarks confirmed in-memory efficiency but did not measure IPC/relay throughput, WKWebView rendering, or end-to-end B4 latency.
4. **W4-l overscan correction:** Filtered virtualized rows strictly by layout interval intersection with the transcript viewport, ensuring overscan rows are not sampled until visible. Swift AppKit visible-rect sampling was confirmed. A dependency-free cross-stack comparator CLI was introduced.
5. **W4-m re-gate and Tauri vs. Swift comparison:** Re-evaluated the matrix under D64 viewport filtering and D65 0.5% minimum sample floor.

#### W4-m re-gate results (Tauri 2 vs. Swift/AppKit)

The W4-m evaluation re-gated 60 Tauri runs and 60 Swift/AppKit runs from identical workloads against D64 and D65. Swift completed 60/60 valid runs (307,160 measured rows against the 8,250 floor). Tauri produced 18/60 valid runs (15,000 saved measured rows against 8,250 floor across all attempts, but 42 runs lacked per-row sample arrays and remained invalid).

Latency columns below show pooled valid-row median / p95 in milliseconds.

| Events | Streams | Temp | Tauri valid/total | Tauri rows/floor | Tauri med/p95 ms | Swift valid/total | Swift rows/floor | Swift med/p95 ms |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 5,000 | 1 | cold | 3/5 | 248/125 | 172 / 422 | 5/5 | 5,236/125 | 256.2 / 368.0 |
| 5,000 | 1 | warm | 2/5 | 263/125 | 189 / 325 | 5/5 | 5,166/125 | 241.1 / 360.1 |
| 5,000 | 4 | cold | 3/5 | 246/125 | 121 / 293 | 5/5 | 4,168/125 | 193.4 / 284.4 |
| 5,000 | 4 | warm | 1/5 | 237/125 | 103 / 198 | 5/5 | 4,233/125 | 187.6 / 268.4 |
| 5,000 | 8 | cold | 1/5 | 211/125 | 76 / 127 | 5/5 | 4,072/125 | 134.2 / 207.3 |
| 5,000 | 8 | warm | 1/5 | 216/125 | 105 / 145 | 5/5 | 4,036/125 | 120.8 / 174.5 |
| 50,000 | 1 | cold | 3/5 | 2,485/1,250 | 1,361 / 2,566 | 5/5 | 53,532/1,250 | 2,447.6 / 3,592.2 |
| 50,000 | 1 | warm | 4/5 | 2,815/1,250 | 1,649 / 2,915 | 5/5 | 53,726/1,250 | 2,425.8 / 3,604.5 |
| 50,000 | 4 | cold | 0/5 | 2,061/1,250 | — | 5/5 | 44,559/1,250 | 1,622.8 / 2,355.6 |
| 50,000 | 4 | warm | 0/5 | 2,214/1,250 | — | 5/5 | 45,511/1,250 | 1,563.5 / 2,374.1 |
| 50,000 | 8 | cold | 0/5 | 2,048/1,250 | — | 5/5 | 41,265/1,250 | 1,116.6 / 1,777.2 |
| 50,000 | 8 | warm | 0/5 | 1,956/1,250 | — | 5/5 | 41,656/1,250 | 1,151.9 / 1,722.8 |

#### Key findings

- **Both stacks fail the proposed ≤50 ms target:** At 5k events, valid Tauri medians range ~76–189 ms (p95 127–422 ms, provisional on 51–171 valid row samples per cell; 211–263 measured rows across all attempts); Swift medians range 121–256 ms (specifically 120.8–256.2 ms, p95 174.5–368.0 ms). At 50k×1 stream, Tauri valid medians reach ~1,361–1,649 ms (p95 2,566–2,915 ms) and Swift medians reach ~2,438 ms (2,425.8–2,447.6 ms, p95 3,592.2–3,604.5 ms). For 50k×4/×8 streams, Tauri has no valid data (0/5 runs met the sample floor with valid per-row arrays), while Swift medians are ~1,120–1,620 ms (1,116.6–1,622.8 ms, p95 1,722.8–2,374.1 ms).
- **Results point toward delivery-path work:** For valid 1-stream cells, both stacks' latency grows by roughly an order of magnitude from 5k to 50k events; Swift shows similar growth at 4 and 8 streams. Tauri has no valid 50k×4/×8 data, so those workloads cannot support a cross-stack comparison. The results point toward daemon-to-UI delivery as a bottleneck, but do not isolate its cause. D49 retains Tauri 2 + React 19.
- **Decision impact:** D49 retained Tauri 2 + React 19. Swift/AppKit remains a reference spike only.

### Other observations

The local `mockCreationToMountedRowFollowupP95` remains only a render-follow-up proxy. Scroll, frame, heap, and long-task diagnostics remain in the raw per-run JSON; WebKit does not expose all browser diagnostics available in Chromium. The retention cap for the Rust-side event history remains open (13 §1b); the prototype retains all durable rows so prior ranges remain available.

## 4. Verdict

- **B3 idle (Pass):** The Tauri/WebKit process tree physical footprint measures 113.708 MB median (114.986 MB max) across five idle samples, well below the 180 MB ceiling (65.014 MB headroom). Loaded 50k×8 physical footprint rises to ~396 MB median and is reported separately under D61.
- **B5 keystroke responsiveness (Measured first-frame rate passes):** 2,096/2,100 keystrokes matched the committed DOM by the first qualifying rAF callback within 16.67 ms (99.8% pass rate, raw maximum 17 ms), meeting D58's ≥95% requirement. The D58 physical-keyboard verification remains pending.
- **B4 streaming event latency (Fails budget; parked as deferred debt):** Every valid cell in both Tauri and Swift exceeds the proposed 50 ms p95 budget (by 2–8× at 5k and 20–70× at 50k). On 2026-10-23, the owner parked B4 as known performance debt: it does not block Phase 1 progress (S1-lite and core spikes proceed). Daemon-to-UI throughput optimization and a night Tauri B4 rerun at 0.5% gate are deferred to a later item.
- **UI stack decision (D49 retained):** Tauri 2 + React 19 remains the chosen UI stack. The Swift/AppKit spike confirmed that a native UI rewrite does not fix the delivery-path bottleneck.
- **Client roadmap (D66 browser-first):** Primary Phase 1 client is the React web UI in a standard browser via an opt-in loopback WebSocket (`127.0.0.1`). Tauri is a later desktop port of the same React UI. The browser pairing ceremony and main screen represent the immediate path (see [18 — Browser-First Roadmap](../18-browser-first-roadmap.md)).

## 5. Reproduction

- **Phase 0 prototype (B3/B5 historical matrix):** Committed in `spikes/s5-ui/`.
- **W4-i through W4-m prototype and benchmarks:** Located in `apps/s5-prototype/` (Tauri harness, React prototype, and comparator) and `apps/s5-swift/` (Swift/AppKit spike) on the `p1a/coordinator` worktree.
- **Cross-stack comparison command:**
  ```sh
  node apps/s5-prototype/scripts/b4-compare.mjs \
    --tauri apps/s5-prototype/bench-results/w4j \
    --swift apps/s5-prototype/bench-results/w4k2-swift \
    --out apps/s5-prototype/bench-results/w4m-reeval/b4
  ```
- **Live benchmarking rules:** Do not run B4/B5 benchmarks, Tauri apps, Swift apps, or headed browsers during daytime. Any future Tauri B4 verification must run headless, at night, on the reference Apple Silicon Mac, adhering to D64 viewport filtering and D65 0.5% sample floor.

From the repository root, the older Phase 0 matrix is reproduced with:

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

Raw data and process snapshots are in `spikes/s5-ui/results/`. The result files include per-run launch/latency/frame metrics, callback and rAF timestamp deltas, generated-token counts, and process samples. The B3 capture repeats five idle samples after a 61 s settle and five loaded 50k×8 snapshots after 12 s. Apps and harness-launched servers were closed after collection. The benchmark does not implement a reviewed interactive pairing screen; setup uses the daemon CLI in an isolated temporary data directory. The approval card was visually checked in the release app; `approval-card-review.png` is the saved capture. The bundle build retains Vite's warning that the JavaScript chunk exceeds 500 kB.
