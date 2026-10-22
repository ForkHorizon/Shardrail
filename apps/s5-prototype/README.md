# S5 prototype

## Compare Tauri and Swift B4 runs

The dependency-free comparison script reads per-run JSON recursively. Repeat `--tauri` to combine Tauri result directories; pass the Swift result directory with `--swift`. A run is valid only when the page was visible, all requested daemon chunks arrived, and the measured rows meet `max(20, 1%)` (capped at the event count). The report shows valid/total runs, measured rows against the aggregate sample floor, pooled sample median/p95, and median/p95 of valid per-run medians.

From the repository root:

```sh
node apps/s5-prototype/scripts/b4-compare.mjs \
  --tauri apps/s5-prototype/bench-results/w4j \
  --swift apps/s5-prototype/bench-results/w4k2-swift \
  --out apps/s5-prototype/bench-results/w4l-comparison/b4
```

This writes paired `.md` and `.json` reports. Use repeated `--tauri DIR` arguments to combine multiple Tauri runs.
