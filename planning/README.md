# Planning Archive

**ARCHIVE — The single source of truth for all active planning is [`../PLAN.md`](../PLAN.md).**

This folder contains historical planning documentation, early RFCs, architectural research, and design rounds compiled prior to the workspace multi-repo split on 2026-10-31.

## Active Status & Reference Usage

- **Source of Truth**: [`../PLAN.md`](../PLAN.md) is the master plan and defines all active roadmap stages (Stage 0 to N1/N2), tasks, acceptance criteria, and agent operating rules.
- **Decision Log & Security Reference**: Documents such as `06-security.md` and `12-decisions.md` remain valuable historical references for security rationale and design lineage.
- **Superseded Decisions**: Any decision in `12-decisions.md` or earlier planning documents that contradicts §5 ("What was reconsidered") or §6 of `PLAN.md` is **superseded**. Specifically:
  - Multi-repo split: Code is partitioned between `Shardrail_Core` (daemon, journal, coordinator, protocol, client/contracts) and `Shardrail_Web` (UI, relay, deploy), superseding monolithic repository layouts.
  - Browser UI first: Browser SPA (`apps/web`) is canonical, superseding native desktop/Tauri shells as the primary client.
  - Clean import history: History split via clean import initial commits, superseding joint history preservation.
  - Test vectors & fixtures: Fake ACP agent harnesses and decoupled test fixtures supersede provider live accounts for CI.

Do not edit planning documents here for current task planning. Update [`../PLAN.md`](../PLAN.md) instead.
