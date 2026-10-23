# W4-m planning report

Date: 2026-10-23
Branch: `main`

## Decisions recorded

- D64 now consistently defines B4 as daemon receipt time mapped to the page clock through entry to the second `requestAnimationFrame` callback after row commit, sampling only rows whose layout interval intersects the transcript viewport. Overscan-only rows are excluded; the measured-row floor is D65.
- D65 sets the floor to `min(events, max(20, ceil(events × 0.005)))`: 25 rows for 5,000 events and 250 for 50,000. The previous 1% requirement was arbitrary. AI Platform made this decision under Daliys's explicit delegation on 2026-10-23. The initial draft also cited 350–490 rows and 200–300 samples as adequate; review found those claims unsupported, and they were withdrawn. For 5k Tauri, the re-gate has 51–171 valid pooled samples while the 211–263 measured-row totals include all attempts.
- The re-gated night data misses the 50 ms target in every valid cell. At 5,000 events, valid Tauri p95 values are 127–422 ms and Swift values are 175–368 ms. At 50,000, valid Tauri 1-stream p95 values are 2,566–2,915 ms and Swift values are 1,723–3,604 ms. Tauri 4/8-stream 50,000-event cells have no valid per-row samples. Both implementations show similar latency growth with event count, which suggests a delivery-path bottleneck but does not isolate it conclusively.
- D49 retains Tauri 2 + React 19. Swift/AppKit remains a reference spike; the next work is the delivery path.

## Planning files updated

Updated `planning/00-INDEX.md`, `02-architecture.md`, `07-performance.md`, `08-ui-ux.md`, `10-roadmap-and-backlog.md`, `12-decisions.md`, `13-risks-and-open-questions.md`, `15-independent-review.md`, and `17-owner-decisions-2026-10-19.md` to align decision summaries, the UI fallback guidance, current performance status, and remaining questions.

The owner-side `planning/spikes/s5-ui.md` was left untouched as instructed. No tests were run because this change only updates planning documents.

## Source material

- [W4-m final verification](w4m-final.md)
- [W4-m D65 re-gate table](../wt-coordinator/apps/s5-prototype/bench-results/w4m-reeval/b4.md)
- [W4-l comparison table](../wt-coordinator/apps/s5-prototype/bench-results/w4l-comparison/b4.md)
