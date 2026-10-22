# 07 — Performance and Resource Budgets
> 2026-10-22 (owner): D64 records B4 callback-latency semantics; W4-i failed the proposed target and W4-j live remeasurement is pending. Earlier D49–D54 decisions — React retained on partial S5 evidence; journal opens atomically without following symlinks; daemon refuses network/synced data filesystems; Codex is first in P1a; tokio/clap/nix are allowed daemon dependencies; contracts remain generated with client helpers in `packages/client`.

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Budgets remain proposed acceptance limits; S5 provides partial measurements on the reference Mac (Daliys's MacBook Pro M5, 32 GB, macOS 27.0.1): B3 idle median is 113.7 MB / 180 MB (final re-run; ≈396 MB median under 50k×8 load), and B5 native matched 2,096/2,100 keystrokes by the first frame (max 17 ms); B5 budget per D58. W4-i measured B4 above its target; W4-j live remeasurement is pending. See [S5 results](spikes/s5-ui.md), D49 and D64. Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): B16 split warm/cold, B11 a target, measurement load for a solo developer. 2026-10-19 (owner): U1 answered — reference machine is Daliys's own Mac.

## 1. Where the cost is

Memory is dominated by the WebView and the provider processes (Codex native binary and possibly its npm wrapper, Python only if Hermes is added in a later phase (not a Phase 1 dependency, D48), Node + CLI per Claude session, Grok's binary). The daemon language matters less than not embedding Chromium and not keeping extra processes "just in case". Therefore: no Electron, no shared provider process until a spike allows it, no Hermes browser tools set up by Shardrail, and provider memory always reported in its own column, never hidden in the daemon budget. Model network latency is outside Shardrail's budgets.

## 2. Budgets

Reference machine: Daliys's own current Apple Silicon Mac (U1, owner 2026-10-19); its model and memory are recorded with every measurement.

| ID | Budget | Scope | How measured |
| --- | --- | --- | --- |
| B1 | daemon RSS idle ≤ 40 MB after 60 s | `shardraild`, 0 sessions | process sample |
| B2 | ≤ 15 MB per idle session projection in the daemon | daemon only | RSS difference |
| B3 | ≤ 180 MB idle; **S5 final: idle median 113.7 MB (max 115.0) of 180, pass; under 50k×8 load ≈396 MB median, reported separately (D61)** | **total physical footprint** of the app plus all `com.apple.WebKit.*` helper processes, empty transcript | `footprint` / Activity Monitor over the process tree; S5 measurement |
| B4 | p95 ≤ 50 ms from daemon receipt to `performance.now()` at entry to the second rAF callback after the event row is committed and visible (D64); callback-latency proxy, not scanout. **W4-i: 900 ms median / 3,014 ms p95; fails. W4-j live result pending.** | local, 1 active session | daemon receipt timestamp mapped to the page clock; callback mark; see D64 and S5 |
| B5 | **≥ 95% of keystrokes visible in the next frame (first rAF within 16.67 ms), plus one physical-keyboard check (D58)**; S5 native: 2,096/2,100 (99.8%), raw max 17 ms | UI | input trace; S5 used synthetic input and recorded loaded samples separately |
| B6 | cold start to ready window ≤ 1.5 s | shell + daemon listening | timestamps, no provider login |
| B7 | session list visible ≤ 1 s after `kill -9` | daemon + SQLite | integration test |
| B8 | over 8 h, daemon RSS ≤ 1.3× its 10-minute value (1 active + 2 idle sessions); 72 h soak: same, WAL ≤ 64 MiB, WebView footprint ≤ B3 + 150 MB | daemon, WebView | soak harness; provider leaks reported separately |
| B9 | no Shardrail-owned timer more frequent than 15 s at rest | daemon and UI code | code review + failing test on forbidden intervals |
| B10 | batch write ≤ 50 ms | SQLite single writer | bench on reference machine |
| B11 | idle wakeups ≤ 1/s average for the whole process tree incl. WebKit; CPU ≈ 0 %; energy impact "Low" — **target, not a gate**, until S5 shows what WebKit helpers do on their own | app + daemon | `powermetrics`, Activity Monitor |
| B12 | per-client outbound queue ≤ 8 MiB; overflow → resync, never growth | gateway | stalled-client test |
| B13 | first visible page of a 50 000-event session ≤ 300 ms (warm DB) | daemon + UI | fixture DB |
| B14 | approval round trip (tap → provider receives response) ≤ 100 ms local | daemon | `approval.respond` span |
| B15 | durable bytes written per turn ≤ 3× completed item content; retained ≤ 2× | journal | statistics on recorded sessions |
| B16 | Git status ≤ 1 computation in flight per worktree; ≤ 1 s for 100k tracked files **with a warm file cache**; cold-cache time reported separately (fsmonitor is forbidden by D23, so a cold stat walk can take several seconds) | workspace service | fixture repo |
| — | transcript of 5 000 events scrolls without WebView growth > 80 MB above B3 | UI | S5 |

If the WebView alone exceeds B3 on the reference machine, the budget is revised **together with the measurement** and a decision on the shell; it is never raised silently. (B3 originally measured only the shell process, which would have hidden the WebKit helper processes.)

## 3. Measurement rules

- Report physical footprint per process tree, with separate columns for daemon, shell + WebKit, Git worker, macOS containment helper, and each provider; record provider versions.
- No budget closes without a stored trace.
- Phase 1a gates on B4 and B7 only; B1 and B3 are recorded as observations in Phase 1a; the rest at Phase 1 close; B8 72 h soak in Phase 2. B16 applies only once the safe-Git view exists (Phase 1e, D40).
- Measurement load is real work for a solo developer: one scripted harness produces all traces; budgets without a harness stay "targets" rather than gates.

## 4. Design rules that keep budgets reachable

- Daemon-side coalescing of deltas per item to the frame interval; per-client bounded queues (04 §2).
- UI: virtualized transcript (no tens of thousands of DOM nodes); tool cards updated in place; incremental Markdown parsing (only the open tail block re-parsed — re-parsing a whole message per delta is quadratic); code highlighting deferred until a block closes and is visible.
- Files and Git: no timer-driven status. FSEvents/inotify with 200 ms debounce plus a refresh button; watch only expanded directories and Git metadata; respect Linux inotify limits and fall back to manual refresh with a visible notice; tree loads one level at a time; status coalesced to one computation in flight per worktree, showing "stale since T" instead of blocking.
- Timers are armed only while data flows; presence and keepalive are event-driven or ≥ 15 s.
