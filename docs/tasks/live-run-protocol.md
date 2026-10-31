# Shardrail Live-Run Protocol

This protocol defines the rules and constraints for live runs of AI coding agents (Codex CLI, Claude Code) through Shardrail daemons and adapters, pursuant to `PLAN.md` §7.4.

## 1. Scope & Authorized Target
- **Permitted Repository**: Live runs of Codex CLI or Claude Code are authorized **only** within `Shardrail_TestRepo` (or an ephemeral sandbox clone derived from it).
- **Prohibited Repositories**: Live agents must **never** be spawned with working directories set to the owner's personal repositories, home directory, or the active Shardrail repositories (`Shardrail_Core`, `Shardrail_Web`), unless the owner provides explicit, documented consent for a specific run.

## 2. Authentication & Secret Hygiene
- **Ambient Authentication**: Live runs must utilize the host machine's existing ambient user login sessions (`~/.codex/auth.json`, `~/.claude/.claude.json`).
- **No Secret Leaks**:
  - API keys, bearer tokens, and session credentials must never appear in CLI command arguments, environment dumps, journal events, test logs, or PR summaries.
  - Sanitization filters must mask any credential patterns before persisting logs or journals.

## 3. Operational Safety & Approvals
- **Fail-Closed Execution**: If a connection drops, times out, or encounters an unrecognized state, the daemon and adapter must evaluate pending approvals to `deny`.
- **Harmless Prompts**: Prompts during automated tests must be strictly bounded (e.g., "Add an integer square root function to src/calc.py" or "Check syntax of tests/test_calc.py"). Destructive shell commands (e.g. `rm`, `curl`, network writes) are forbidden during automated test passes.
- **Interruption Testing**: When verifying turn interruption, agents must verify that running child processes are cleanly killed (`SIGTERM` -> `SIGKILL`) with no zombie processes left running.

## 4. Reset & Clean State
- **Deterministic Cleanup**: Following any live run session, the target sandbox must be reset using `./reset.sh` to restore git state to `initial-state` and eliminate untracked artifacts.
- **Orphan Process Check**: Run `./scripts/check-daemon-test-orphans.sh` after live runs to confirm no background agent processes remain orphaned.
