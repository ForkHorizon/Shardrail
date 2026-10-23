# W4-m planning final verification

Date: 2026-10-23
Branch: `main`
Scope: commits after `1890ae0`; W4-m planning records, D64/D65 consistency, the two review reports and fixes, and headless verification.

## State

**Verified with a small documentation correction.** D64 keeps the same B4 start and endpoint: daemon receipt mapped to the page clock through entry to the second `requestAnimationFrame` callback after row commit. The clarified population includes only rows whose layout interval intersects the transcript viewport; overscan-only rows do not count. Headless tests confirm partial intersections qualify, overscan does not, and a row can be sampled after it enters the viewport. No change to the B4 timing semantics was found.

D65 consistently sets the per-run eligibility floor to `min(events, max(20, ceil(events × 0.005)))`, yielding 25 samples at 5,000 events and 250 at 50,000. It does not guarantee pooled p95 precision. In the six 5k Tauri cells, `b4.json` records 51–171 valid pooled samples; the Markdown table's 211–263 `measuredRows` includes all attempts. Those p95 values are provisional. The initial 350–490 row-yield and 200–300 sample-adequacy claims were unsupported and are now marked as withdrawn in the build report.

Review 1's pooled-confidence and unsupported-range findings were sound and have been fixed. The stale W4-j text in `planning/spikes/s5-ui.md` remains an owner-side edit and was not touched. The requested `w4mp-review-2.md` is absent; the neighboring `w4m-review-1.md` is a separate earlier review on another branch, so it cannot substitute for review 2. No second-review findings were available to verify.

D49 remains Tauri 2 + React 19, and no code or B4 data changed. I made a small doc correction to distinguish valid pooled samples from all-attempt row totals in D65/D49 and the review records. The worktree began clean; the final changes are limited to planning and review documents. `planning/spikes/s5-ui.md` has no diff.

## Checks

All checks passed from fresh package installs or rerun commands:

- S5 JavaScript: `npm ci`, 36 Node tests, 6 comparator tests, 8 Vitest tests, ESLint, and production build.
- S5 Tauri Rust: rustfmt, Clippy with warnings denied, tests (8 passed, 1 ignored), and the ignored release throughput test (1 passed). This is a Rust unit test, not the B4 measurement.
- Swift core: 11 tests passed with `DEVELOPER_DIR=/Applications/Xcode-beta.app/Contents/Developer`. The package README confirms these tests do not create windows or run an app loop.
- Coordinator workspace Rust: rustfmt, Clippy with warnings denied, all workspace tests, and `cargo deny check`.
- Tauri dependency policy: `cargo deny check` passed.
- Client/contracts: clean `npm ci` for both packages, client typecheck, 5 client tests, and schema drift check.
- `git diff --check` passed.

Existing warnings: Vite reports a 594.30 kB minified JavaScript chunk over its 500 kB warning threshold. `cargo deny` passes but reports duplicate crate versions in the existing lockfiles. npm reported deprecated packages and install scripts requiring explicit approval; install completed and audits reported zero vulnerabilities.

Only headless checks ran. No real AI provider or model API, live B4 measurement, benchmark script, browser, Playwright, WebDriver, Tauri app, Swift app, or GUI was launched. Vitest and shell harness checks used test doubles/fake bridges.

## Remaining issues, ranked

1. **High — B4 exceeds the proposed target.** Every valid cell is above 50 ms. At 5k events, valid Tauri p95s are 127–422 ms and Swift p95s are 175–368 ms. At 50k events, valid Tauri 1-stream p95s are 2,566–2,915 ms and Swift p95s are 1,723–3,604 ms.
2. **High — Tauri coverage is incomplete.** Only 18 of 60 attempts are valid. The 50k 4/8-stream Tauri cells have 0/5 valid runs each, so a complete cross-stack comparison is unavailable. Similar growth across stacks points toward delivery-path work, but does not isolate the bottleneck conclusively.
3. **Medium — low-count 5k Tauri pooled percentiles.** Some use 51 or 54 valid samples; all six cells use 51–171. Treat those p95s as provisional and keep the pooled sample count beside each result.
4. **Medium — stale owner-controlled S5 note.** `planning/spikes/s5-ui.md` still describes B4 as pending and the 1% floor as open. The review's rejection/defer decision is sound under the explicit instruction to leave that file alone. Daliys must reconcile it when ready.
5. **Review coverage — second review missing.** `w4mp-review-2.md` was not present at the requested path, so its findings could not be checked.

## Decisions for Daliys

- Decide whether to rerun or repair the 42 invalid Tauri attempts before treating the comparison as complete.
- Decide whether the 50 ms p95 target remains a release/stack gate and what delivery-path follow-up should come next.
- Reconcile the stale owner-side S5 note, or provide the second review's location if it exists elsewhere.

D65 remains the delegated decision and D49 remains kept. No further code changes were indicated by this review.
