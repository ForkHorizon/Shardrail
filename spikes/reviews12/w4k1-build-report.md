# W4-k-1 planning build report

Date: 2026-10-22
Branch: `main`

## Changes

- Recorded owner-approved D64 in planning/12 and planning/17. B4 starts at the daemon receipt timestamp mapped to the page clock and ends at `performance.now()` on entry to the second rAF callback after the row is committed and visible. The docs label it a callback-latency proxy, not scanout.
- Updated the S5 note and performance budget with the full B4 history: the original matrix was withdrawn; W4-i completed 60/60 valid runs and measured 200,036 samples (median 900 ms, p95 3,014 ms, over the ≤50 ms target); earlier relay-batch acknowledgement and hidden-window frame issues were fixed; W4-j has only headless microbench results so far, with its live night run pending.
- Recorded the W4-j microbench results without treating them as B4 evidence: 50,000 Rust JSON append/range pairs in 149.724 ms (333,948 pairs/s), and frontend 64-row projection updates at 0.0161 / 0.0191 ms for 5k / 50k rows.
- Added the open event-store retention and 500-row / 1% sample-floor questions in planning/13.
- Added the conditional D49/S5 fallback: if W4-j still misses B4, compare a Swift/AppKit list side by side with Tauri using the same fake daemon, handshake, D64 semantics, and result JSON; Daliys makes the stack decision after comparison. D49's register entry is unchanged.
- Updated 00-INDEX and retained the pre-existing S5 note edits after checking them against `spikes/reviews11/w4j-final.md`, the W4-i matrix report, and the coordinator worktree implementation.

## Evidence and boundaries

- W4-i results: `spikes/reviews9/w4i-matrix-report.md`.
- W4-j implementation review and outstanding gates: `spikes/reviews11/w4j-final.md`.
- Read-only inspection confirmed W4-j uses Tauri channels, a Rust windowed store, per-frame coalescing, memoized Markdown, fixed-height rows, and a two-rAF sample after visibility/commit. Its review says the 500-sample delivery is not established and full history retention remains undecided.
- `apps/s5-swift` was not present in the checked `p1a/coordinator` worktree; it is recorded as the conditional spike path, not as existing implementation.
- D49 itself was not changed. No application code was changed, and the coordinator worktree was left untouched.

## Checks

- `git diff --check` passed.
- No tests or live B4 run were performed; this is a planning-doc change, and W4-j live measurement remains pending the scheduled night run.
