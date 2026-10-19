# Spike notes

> Part of the Shardrail architecture plan. Rules (owner, 2026-10-19; 10 §2): spike **code** lives in the repository's gitignored `spikes/<id>/` folder (live-agent test repos in `spikes/<id>/workdir/`, no real secrets) and is never committed. The **note** — the written output of each spike — lives here as `planning/spikes/<id>.md` and is committed.

Authorized: S1-lite (`s1-lite-codex`), S2 (`s2-grok`), S5 (`s5-ui`), S9-lite (`s9-lite-macos`). Not yet authorized: S3, S4, S6, S7, S8, full S9.

## Note template

Copy into `planning/spikes/<id>.md`:

```markdown
# <ID> — <title>

- **Spike:** <ID> (10 §2) · **Date(s):** YYYY-MM-DD · **Machine:** <model, chip, memory, OS version>
- **Code:** `spikes/<id>/` (not committed) · **Status:** done | partial | abandoned

## 1. Goal
The questions from 10 §2 this spike answers, and its "done when".

## 2. Setup
- Tool and provider versions (exact): e.g. `codex --version`, `grok --version`, Rust toolchain, Node, Tauri, macOS build.
- Accounts/logins used (no secrets), sandbox profiles, flags, config pinned.
- Test workspace: `spikes/<id>/workdir/…` (what it contains; confirm no real secrets).

## 3. What was run
Steps, commands and scenarios, in order, enough to reproduce.

## 4. Results with evidence
Per question: observed behaviour, measurements (with units and method), and evidence — log excerpts, protocol frames, screenshots (paths), links. Label each result Verified / Inferred / Unverified.

## 5. Verdict
Answer to each question in one line; "done when" met or not.

## 6. Impact on the plan
Decisions, risks, requirements and other IDs affected (D…, R…, U…, F…, N…, B…, EV…), with the proposed in-place change; new evidence entries for 14; follow-up spikes or open issues.
```
