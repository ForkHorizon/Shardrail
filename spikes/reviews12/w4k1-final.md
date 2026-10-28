# W4-k-1 final verification

Date: 2026-10-22
Branch reviewed: `main` at `d3fdd4a` (based on `2f295df`)
Coordinator implementation inspected read-only: `p1a/coordinator` at `617c78d`

## State

**Planning changes are internally consistent and verified.** D64 is recorded as a callback-latency proxy: start with the daemon receipt timestamp mapped to the page clock; stop at `performance.now()` on entry to the second rAF callback after the row is committed and visible. The register, owner-decision note, performance budget and S5 note agree. The definition does not claim completed paint or scanout.

The B4 history matches the W4-i summary: 60/60 valid runs, 200,036 samples, median 900 ms and p95 3,014 ms, over the proposed 50 ms p95 target. W4-j's microbench numbers match its final review and remain explicitly separate from B4 evidence. Its live B4 measurement is still pending.

All review-2 documentation findings are fixed. The review-1 overscan finding is real; deferring its code fix is sound for this main-branch planning scope. The S5 note now blocks treating a future W4-j run as D64 evidence until the sampler excludes overscan-only rows and has a regression test. The index also now distinguishes the answered initial U1–U26 questions from the still-open S5 decisions.

No planning regression was found. A focused assertion pass confirmed D64 wording, B4 history, the open questions, the sampler caveat, the conditional Swift fallback and the 94% B5 result.

## Checks rerun

All checks below were run headlessly from the `p1a/coordinator` checkout, with fresh `npm ci` installs for contracts, client and S5. Heavy Cargo commands used `nice`.

| Check | Result |
|---|---|
| Workspace Rust format | Pass |
| Workspace tests, all targets | Pass, 102 tests |
| Workspace Clippy, warnings denied | Pass |
| Workspace `cargo deny check` | Pass; existing duplicate `syn` warning |
| S5 Tauri Rust format, tests and Clippy | Pass; 4 tests pass, 1 release throughput test ignored by default |
| S5 scoped `cargo deny check` | Pass; duplicate transitive crate warnings |
| S5 ignored release throughput test | Pass; 50,000 append/range pairs and bounded-range assertions |
| Client typecheck and tests | Pass, 4 tests |
| Schema drift check | Pass |
| S5 Node tests and Vitest | Pass, 30 + 8 tests |
| S5 ESLint | Pass |
| S5 production frontend build | Pass; existing 588.33 kB chunk advisory |
| `git diff --check` for `main` changes | Pass |

The broader coordinator history diff-check still reports trailing whitespace in old archived benchmark logs and `spikes/reviews7/w4e-build-report.md`; those files are outside the main planning diff and unchanged here.

No provider/model API, browser, Playwright, WebDriver, Tauri or Swift app, Simulator, or GUI was run. No live B4 measurement was run. The release throughput test measures the Rust event store only; it is not B4.

## Remaining issues, ranked

1. **High — sampler invalidates D64 evidence until fixed.** In `p1a/coordinator`, `main.tsx` passes all virtual items to `markDaemonRowsCommitted`, including overscan rows, and the eligibility check only confirms that the row is mounted. Mounted overscan can be outside the viewport. Fix this in the coordinator implementation and add a fake-viewport regression test before accepting W4-j results as D64 evidence.
2. **High — W4-j B4 status is unknown.** W4-i fails the target; W4-j microbenchmarks do not measure relay cadence, WKWebView rendering or B4 latency. Run the live matrix only after the sampler gate is fixed and the planned run is authorized/scheduled.
3. **Medium — two S5 decisions remain open.** Decide whether 500 samples (1% of a 50k run) stays a hard gate, and what retention cap the Rust event store should use before production long-session use.
4. **Medium — loaded B5 recheck is below D58.** The 50k×4 streaming result is 141/150 (94%) in both cold and warm groups. The W4-j path has not been remeasured under this load; the synthetic-input result also leaves the physical-keyboard check open.
5. **Low — conditional stack comparison remains undecided.** If W4-j still misses B4 after a valid run, Daliys must decide whether to proceed with the documented Swift/AppKit side-by-side spike, then choose the stack after comparing both implementations.

## Daliys decisions

- Confirm whether the 500-sample / 1% B4 floor is mandatory for 50k-event runs.
- Set or defer the event-store history-retention cap.
- After a valid W4-j run, decide whether a B4 miss triggers the Swift comparison; make the stack choice after that comparison.
- Recheck the 50k×4 B5 shortfall on W4-j and complete the physical-keyboard confirmation before treating B5 as settled.

Local commits on `main` remain unpushed. This pass added only the small index summary correction and this verification report; no app or benchmark implementation was changed.
