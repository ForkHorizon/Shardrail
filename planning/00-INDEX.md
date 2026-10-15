# Shardrail — Architecture Plan (Index)

**Organization:** Fork Horizon · **Date:** 2026-10-15
**Status:** consolidated candidate — **not owner-approved**. Planning only; **implementation is not authorized**, including Phase 0 spikes.
**Language:** English for all planning documents.

## What Shardrail is (one paragraph)

A workspace for supervising several AI coding agents (Codex, Claude Code, Grok Build, Hermes Agent) on a local Mac and remote Linux hosts. **One Rust daemon per host** owns agent processes and a SQLite event journal; **thin web clients** (a Tauri desktop app, a browser, later a phone) connect to it. Agents are driven only through their official protocols (ACP, Codex app-server, Claude Agent SDK). Every risky action goes through **approvals that never default to allow** and are protected, as far as the OS allows, from the agents themselves. Files and Git are shown read-only without ever executing repository-configured programs.

## Documents

| # | Document | Contents |
| --- | --- | --- |
| 01 | [Product and requirements](01-product-and-requirements.md) | vision, scenarios, scope, F1–F22, N1–N12 |
| 02 | [Architecture](02-architecture.md) | system shape, components, macOS/Linux processes, topology, client communication, data ownership, technology choices |
| 03 | [Providers](03-providers.md) | channels, adapter contract, compatibility, replay, capability matrix, per-provider policy, Claude terms |
| 04 | [Sessions, events, data](04-sessions-events-data.md) | identifiers, event tiers, commands, journal storage, recovery, handoff |
| 05 | [State machine](05-state-machine.md) | normative transition tables, timers, badges, invariants |
| 06 | [Security](06-security.md) | threat model, trust boundaries, approval protection, client identity, rendering, safe Git, containment, secrets |
| 07 | [Performance](07-performance.md) | budgets B1–B16 (proposed, unmeasured), measurement rules |
| 08 | [UI/UX](08-ui-ux.md) | principles, information architecture, rendering pipeline, visual identity, mobile |
| 09 | [Release and licensing](09-release-and-licensing.md) | targets, licensing, privacy, name |
| 10 | [Roadmap and backlog](10-roadmap-and-backlog.md) | phases P0–P4, spikes S1–S9, epics, candidate tasks, dependencies |
| 11 | [Modules](11-modules.md) | module table M01–M18 and outlines of the foundational modules |
| 12 | [Decisions](12-decisions.md) | D1–D33 with rationale and what earlier versions got wrong |
| 13 | [Risks and open questions](13-risks-and-open-questions.md) | owner decisions U1–U24, risks, tensions, unverified assumptions |
| 14 | [Evidence](14-evidence.md) | sources with access dates and confidence labels |

**Reading paths.** Quick overview: 00 → 02 → 12. Security review: 06 → 05 → 03. Planning work: 10 → 11 → 13.

## Conventions

- IDs: F (functional), N (non-functional), B (budget), D (decision), R (risk), U (owner question), S (spike), E/T (epic/task), M (module), EV (evidence), TB (trust boundary), P (phase).
- Labels: **Verified / Inferred / Unverified / Requires experiment**. Numbers in 07 are budgets, not measurements.
- "Planning Stage 0" (now: documents only) is not "Delivery Phase 0" (future spikes that need authorization).

## How to continue this plan

1. Treat every statement as a hypothesis; verify important claims against primary sources and record them in 14.
2. Change the plan **in place**: edit the relevant document, update 12 (add a "Revised because" line), 13 and cross-references. Do not create parallel versions or long review logs.
3. Keep owner decisions explicit; never mark anything approved — only the owner approves.
4. Do not write application code, install dependencies, run providers or commit without the owner's explicit authorization.
5. Highest-value next steps: owner answers to U9, U13, U3/U14, U21; then authorization of spikes S7, S8, S9, S5; an independent review by a different model family.

## History (for context only)

The first draft was written by Grok in Russian; three Claude review rounds then challenged and refined it. All superseded drafts, translations and review reports are archived in `../archive/stage0-iterations-2026-10-15.tar.gz` and are **not needed** to work on this plan.
