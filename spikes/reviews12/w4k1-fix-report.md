# W4-k-1 review fix report

Date: 2026-10-22
Branch: `main`

## Review 1

1. **Valid; deferred — B4 sampler counts mounted overscan rows as visible.** Verified in the read-only `p1a/coordinator` worktree: `main.tsx` passes all virtual items (including overscan) to `markDaemonRowsCommitted`, and eligibility checks mounting rather than viewport intersection. The requested scope is main-branch planning/docs; implementation and its regression test belong to the separate coordinator code change. The S5 note now explicitly blocks treating W4-j as D64 evidence until the sampler is fixed and regression-tested. No B4 run was performed.
2. **Fixed — stale B4 status and revision date in `00-INDEX.md`.** The index now states that W4-i failed its target and W4-j live remeasurement is pending, and its revision date is 2026-10-22.

## Review 2

1. **Fixed — decision register approval header omitted D49–D63.** It now lists D45–D61 with their approval date range, D62 as owner-delegated, and D63 separately as owner-approved.
2. **Fixed — index omitted D62–D64 in its status and date.** Added all three and advanced the revised date.
3. **Fixed — S5 code pointer and reproduction drift.** The note distinguishes the Phase 0 `spikes/s5-ui/` prototype from the W4-i/W4-j code and B4 harness in `apps/s5-prototype/` on `p1a/coordinator`, and records the build and B4 runner commands. The commands were inspected but not run.
4. **Fixed — obsolete D58 proposal wording.** The B5 wording now identifies D58 as adopted by the owner on 2026-10-20.
5. **Fixed — missing B5 streaming deficit in the verdict.** Added the 50k×4 results (141/150, 94%, cold and warm), below D58's 95% threshold, and states that W4-j has not been measured under this load.
6. **Fixed — stale header in `07-performance.md`.** Updated it to D64 and the current W4-i/W4-j status.
7. **Fixed — ambiguous scope in `13-risks-and-open-questions.md`.** It now limits the U5/U7 statement to initial U1–U26 questions and separately points to the still-open S5 questions in §1b.

## Additional verification

- Confirmed `apps/s5-swift/` is not present in the `p1a/coordinator` worktree; clarified in the S5 and owner-decision notes that this is a future target directory.
- Planning regression assertions passed for the corrected dates, decision statuses, B4/B5 results and explicit measurement caveat.
- `git diff --check` passed.
- No application code or automated app tests were changed or run. No app, browser, provider, model API, or live B4 measurement was launched.
