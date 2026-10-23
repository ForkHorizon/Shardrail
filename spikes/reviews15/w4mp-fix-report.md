# W4-m planning review fix report

Date: 2026-10-23
Branch: `main`
Scope: findings in `w4mp-review-1.md`; `w4mp-review-2.md` was not present at the requested path.

## Findings

1. **Fixed — pooled p95 confidence.** D65 now explicitly defines the 0.5% rule as a per-run eligibility floor, not a precision guarantee. D65 and the D49 result summaries identify 5k Tauri pooled p95 values based on 51–263 valid rows as provisional and retain the counts for interpretation. No aggregate sample threshold was invented.
2. **Deferred — stale S5 planning note.** The review correctly identifies obsolete W4-j “pending” and 1% text in `planning/spikes/s5-ui.md`. That file is owner-controlled and explicitly out of scope; it was not edited or staged. It remains to be reconciled by the owner. Its `assume-unchanged` Git flag is set, and the worktree reported no visible changes for it.
3. **Fixed — unsupported 350–490 range.** Removed the untraceable row-yield and 200–300 sample-adequacy claims from D65 in `planning/12-decisions.md` and `planning/17-owner-decisions-2026-10-19.md`. The replacement describes the actual per-cell counts from the re-gate table.

No findings were rejected. No code changed, so no code regression tests apply; documentation assertions were checked against the cited re-gate table.

## Validation

- Confirmed the 5k Tauri pooled row counts in the re-gate table range from 51 to 263 for valid cells.
- Confirmed the cited planning files contain no 350–490 or 200–300 adequacy assertion.
- `git diff --check` passed.
- Second review unavailable: no findings could be verified from it.
