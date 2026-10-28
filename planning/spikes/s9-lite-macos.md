# S9-lite — macOS sentinel-fd descendant tracking & containment

- **Spike:** S9-lite (10 §2, D43, 06 §8, 15 C9) · **Date(s):** 2026-10-28 · **Machine:** Apple Silicon (arm64, 32 GB, macOS 27.0.1 build 26A434)
- **Code:** `spikes/s9-lite-macos/` (not committed) · **Status:** done

## 1. Goal
Answer the questions from `10-roadmap-and-backlog.md` §2 for S9-lite:
1. Does an inherited sentinel file descriptor (open without `FD_CLOEXEC`) survive across `fork()` / `exec()` boundaries into descendant processes?
2. Can `proc_listpidspath` (macOS `libproc.dylib`) reliably discover descendant processes holding the sentinel path, including when processes detach via `setsid()` or daemon double-fork?
3. Can the supervisor terminate all discovered descendants (`SIGTERM` followed by `SIGKILL`) using only the discovered PID list?
4. What is the empirical behavior and residual risk of active evasion via `fork → exit → setsid → closefrom → sleep`?
5. What is the query overhead and latency of `proc_listpidspath` on macOS?

**Done when:** All test scenarios are exercised on live macOS Darwin kernel, detection reliability and evasion limits are measured with exact PIDs, and containment guarantees and residuals are documented.

## 2. Setup
- OS: macOS 27.0.1 (Darwin 27.0.0 arm64).
- Runtime: Python 3.9.6 via macOS CommandLineTools (`/usr/lib/libproc.dylib` C dynamic library binding).
- Tools: `libproc` symbols `proc_listpidspath(PROC_ALL_PIDS, 0, path, 0, buf, size)`.
- Test harness: `spikes/s9-lite-macos/run_s9_lite_spike.py`.
- No network, no credentials, no real secrets.

## 3. What was run
Ran `spikes/s9-lite-macos/run_s9_lite_spike.py` covering six scenarios:
1. **Baseline inheritance:** Parent creates a sentinel file with `FD_CLOEXEC` cleared; child verifies inherited descriptor and parent queries `proc_listpidspath`.
2. **Session detachment (`setsid`):** Child calls `os.setsid()` to create a new session and process group (breaking process group tracking). Parent queries `proc_listpidspath`.
3. **Double-fork daemonization:** Child spawns grandchild via `setsid()`, intermediate parent exits immediately, reparenting the grandchild to `launchd` (PID 1). Parent queries `proc_listpidspath`.
4. **Tree discovery & termination:** 5 child/grandchild processes spawned with mixed `setsid()` calls. Supervisor queries `proc_listpidspath`, sends `SIGTERM` / `SIGKILL` to all discovered PIDs, and verifies all terminate.
5. **Active evasion (`closefrom`):** Rogue descendant executes `fork → exit → setsid → closefrom(3) → sleep`. Supervisor queries `proc_listpidspath`.
6. **Performance benchmark:** 25 consecutive `proc_listpidspath` queries across the full process tree to measure latency distribution (min, median, p95, max).

## 4. Results with evidence

| Question / Test | Observed behavior | Evidence / Measurement | Result |
|---|---|---|---|
| 1. Baseline inheritance | Child inherits sentinel fd; `proc_listpidspath` returns supervisor and child PIDs | PIDs found: `[59120, 59121]` | **Verified** |
| 2. `setsid()` detachment | Detached process retains sentinel fd; `proc_listpidspath` finds child despite new session/pgrp | Child PID `59123` found in `[59120, 59123]` | **Verified** |
| 3. Double-fork / daemon | Reparented grandchild retains sentinel fd; found despite orphan adoption by PID 1 | Grandchild PID `59130` found in `[59120, 59130]` | **Verified** |
| 4. Tree termination | Supervisor discovers all 5 tree descendants and kills them; all release sentinel fd immediately | All 5 PIDs `[59131..59135]` discovered; dead=True | **Verified** |
| 5. Active evasion (`closefrom`) | Process closes non-stdio descriptors (`closefrom(3)`); `proc_listpidspath` completely loses track | Evader PID `59143` alive=True, in sentinel list=False | **Verified** |
| 6. Query overhead | Full system process table vnode scan takes ~170–230 ms per query | 25 queries: min 169.7 ms, p50 171.6 ms, p95 226.7 ms, max 231.1 ms | **Verified** |

### Detailed observations
1. **Immediate descriptor cleanup on exit:** When a descendant process terminates (even if it remains an unreaped zombie waiting for parent `waitpid`), the macOS XNU kernel drops all file descriptor references in `exit1()`. Thus, `proc_listpidspath` drops the PID immediately upon process death without lingering false positives.
2. **Scan overhead:** Because `proc_listpidspath(PROC_ALL_PIDS)` inspects open file tables across every process in the operating system, each call consumes ~170 ms. Consequently, `proc_listpidspath` cannot be called on a high-frequency polling loop; it is appropriate for on-demand termination sweeps, session cleanup, or low-frequency health checks (e.g. every 5–10 seconds).
3. **Confirmed residual risk:** As predicted in `15-independent-review.md` (C9) and `06-security.md` §8, any child process that explicitly executes `closefrom(3)` (or loops `close(fd)`) before detaching with `setsid()` successfully evades both process-group containment and sentinel-fd tracking.

## 5. Verdict
- **Sentinel-fd inheritance:** Works reliably across normal fork/exec and survives `setsid` and double-fork orphan adoption by `launchd`.
- **Termination sweep:** Discovers and terminates all un-evaded descendants in a single pass.
- **Evasion boundary:** Confirmed empirically that `fork → exit → setsid → closefrom → sleep` escapes sentinel tracking.
- **"Done when" status:** Met. Containment guarantees and residuals documented with empirical proof.

## 6. Impact on the plan
- **D43 (macOS containment):** Confirmed as documented. Containment on macOS must remain officially classified as **best-effort** with residual disclosures. A malicious agent tool running `closefrom()` cannot be caught by sentinel-fd alone without entitlements (coalition/endpoint security APIs).
- **06 §8 & 11 M05 (macOS containment helper):** The helper should combine process-group killing (`killpg`) with a post-termination `proc_listpidspath` sweep against the sentinel path to catch detaching processes (`nohup`, `setsid`) that did not close their descriptors.
- **Checklist item (Containment):** The per-session checklist row for macOS containment must continue to state `not_verified (best-effort: macOS process group + sentinel fd; closefrom bypass exists)` as mandated by D47.
