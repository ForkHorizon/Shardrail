# Shardrail — Architecture Plan (Index)

**Organization:** Fork Horizon · **Date:** 2026-10-15 · **Revised:** 2026-10-23 (D66 browser-first Phase 1)
**Status:** consolidated candidate — **not owner-approved** (exceptions, owner-approved 2026-10-18: D1 daemon language Rust; D3 UI stack React; D44 code written by AI agents; 2026-10-19: D27 contract types Rust → JSON Schema → TypeScript; D29 Claude with the user's own API key; D45 internal-use scope; D46 single-user scope; D47 no "protected" status in v1, verification checklist; D48 Hermes not in Phase 1; D49 React retained after W4-m Tauri/Swift comparison; D50 atomic no-follow journal open; D51 refuse network/synced data filesystems; D52 Codex first in P1a; D53 daemon dependencies tokio/clap/nix allowed; D54 generated contracts and separate client package; D55 client key auth; D56 approvals through the daemon; D57 high-entropy redaction; D58 B5 budget; D59 tsc dev dependency; D60 data-dir race and slow clients; D61 docs sync; D62 @noble/ed25519; D63 scoped S5 cargo-deny exception; D64 B4 callback-latency proxy with viewport-intersecting rows only; D65 B4 0.5% minimum measured rows, delegated to AI Platform; D66 browser-first Phase 1; all remaining owner questions answered in bulk — 17).
**Product scope (owner, 2026-10-19):** internal tool for the owner and Fork Horizon — not commercial, hosted or distributed to third parties (D45); a scope change reopens U9/U25 before release (R46). Until the product fully works, the only user is Daliys on his own machines (D46); distribution items are parked in 09 §5 until a first other user appears (R47).
**Development model (owner constraint):** all Shardrail code is written by AI coding agents; technology choices are optimized for AI-codegen reliability (N13, D44; rules in 11 §3 and 08 §6). **Authorized (owner, 2026-10-19): the core Phase 0 spikes S1-lite, S2, S5, S9-lite only.** S7, S8, full S9 and all product code (Phase 1a onward) are not authorized.
**Language:** English for all planning documents.

## What Shardrail is (one paragraph)

A workspace for supervising several AI coding agents (Codex, Claude Code, Grok Build, Hermes Agent) on a local Mac and remote Linux hosts. **One Rust daemon per host** (D1, owner-approved) owns agent processes and a SQLite event journal; **the React + TypeScript web UI in a normal browser is the primary Phase 1 client**, connecting to the local daemon over an opt-in loopback WebSocket (D66). Tauri, mobile and other platforms are later ports of the same UI. Agents are driven only through their official protocols (ACP, Codex app-server, Claude Agent SDK). Every risky action goes through **approvals that never default to allow** and are protected, as far as the OS allows, from the agents themselves. Files and Git are shown read-only without ever executing repository-configured programs.

## Documents

| # | Document | Contents |
| --- | --- | --- |
| 01 | [Product and requirements](01-product-and-requirements.md) | vision, scenarios, scope, F1–F25, N1–N13 |
| 02 | [Architecture](02-architecture.md) | system shape, components, macOS/Linux processes, topology, client communication, data ownership, technology choices |
| 03 | [Providers](03-providers.md) | channels, adapter contract, compatibility, replay, capability matrix, per-provider policy, Claude terms |
| 04 | [Sessions, events, data](04-sessions-events-data.md) | identifiers, event tiers, commands, journal storage, recovery, handoff |
| 05 | [State machine](05-state-machine.md) | normative transition tables, timers, badges, invariants |
| 06 | [Security](06-security.md) | threat model, trust boundaries, approval protection, client identity, rendering, safe Git, containment, secrets |
| 07 | [Performance](07-performance.md) | budgets B1–B16; S5 B3/B5 measurements and W4-m B4 re-gate; measurement rules |
| 08 | [UI/UX](08-ui-ux.md) | principles, information architecture, rendering pipeline, visual identity, mobile |
| 09 | [Release and licensing](09-release-and-licensing.md) | targets, licensing, privacy, name; §5 items deferred until other users |
| 10 | [Roadmap and backlog](10-roadmap-and-backlog.md) | phases P0–P4 (P1a slim slice, P1e hardening), spikes S1–S9, epics, candidate tasks, dependencies |
| 11 | [Modules](11-modules.md) | module table M01–M18, outlines of the foundational modules, engineering rules for AI agents |
| 12 | [Decisions](12-decisions.md) | D1–D66 with rationale and what earlier versions got wrong |
| 13 | [Risks and open questions](13-risks-and-open-questions.md) | initial owner questions U1–U26 (U5/U7 deferred until other users); open S5 event-store retention question; risks R1–R49, tensions, unverified assumptions |
| 14 | [Evidence](14-evidence.md) | sources with access dates and confidence labels (EV-01–EV-31) |
| 15 | [Independent review](15-independent-review.md) | 2026-10-18 review by a different model family (in Russian) with a resolution-status table showing where each finding was applied |
| 16 | [UI technology research](16-ui-technology-research.md) | 2026-10-18 research for U8 (in Russian): frameworks compared for AI-codegen reliability, sources, rules for AI agents |
| 17 | [Owner decisions 2026-10-19 onward](17-owner-decisions-2026-10-19.md) | bulk answers, later owner decisions and spike authorization, in Russian |
| 18 | [Browser-first roadmap](18-browser-first-roadmap.md) | W5-a listener and browser pairing, prototype reuse, new work, acceptance criteria and headless Playwright checks |
| spikes/ | [Spike notes](spikes/README.md) | note template; one committed note per spike (`<id>.md`) |

**Reading paths.** Quick overview: 00 → 02 → 12. Security review: 06 → 05 → 03. Planning work: 10 → 11 → 13.

## Conventions

- IDs: F (functional), N (non-functional), B (budget), D (decision), R (risk), U (owner question), S (spike), E/T (epic/task), M (module), EV (evidence), TB (trust boundary), P (phase).
- Labels: **Verified / Inferred / Unverified / Requires experiment**. Numbers in 07 are budgets, not measurements.
- "Planning Stage 0" (documents) is not "Delivery Phase 0" (spikes; core four authorized 2026-10-19, the rest still need authorization).

## How to continue this plan

1. Treat every statement as a hypothesis; verify important claims against primary sources and record them in 14.
2. Change the plan **in place**: edit the relevant document, update 12 (add a "Revised because" line), 13 and cross-references. Do not create parallel versions or long review logs.
3. Keep owner decisions explicit; never mark anything approved — only the owner approves.
4. Do not write application code, install dependencies, run providers or commit without the owner's explicit authorization.
5. Spikes (authorized: S1-lite, S2, S5, S9-lite): code in the gitignored `spikes/<id>/` folder of the repository, live-agent test repos without real secrets in `spikes/<id>/workdir/`; the written note in `planning/spikes/<id>.md` (template: [spikes/README.md](spikes/README.md)) is committed (10 §2).
6. Highest-value next steps: W5-a browser listener and pairing, then run the **authorized** core spikes S1-lite, S2, S5, S9-lite (10 §2, 18); S1-lite verifies the selected Codex app-server path and S2 informs the Grok P1b adapter (D52). Then ask the owner to authorize P1a (S7, S8, full S9 before Phase 1e, not yet authorized). Owner questions: all answered (17) except U5/U7, deferred until other users. The independent review is done (15); re-check provider protocols before each phase gate.

## History (for context only)

The first draft was written by Grok in Russian; three Claude review rounds then challenged and refined it. All superseded drafts, translations and review reports are archived in `../archive/stage0-iterations-2026-10-15.tar.gz` and are **not needed** to work on this plan.

**Revision log.**
- 2026-10-15 — consolidated candidate.
- 2026-10-18 — independent review (15) applied in place: approval write-ahead, ACP v1/v2, Codex `approvalsReviewer` and no-experimental rule, Grok sandbox caveats, provider bypass table, agent-authored approval text, drain-then-upgrade, journal fail-closed, instruction queue and session lifecycle, macOS containment mechanism, slim P1a + P1e, Codex terms (U25); D34–D43, R32–R43, U25–U26, EV-22–EV-31, F23–F25 added. Status unchanged: candidate, not owner-approved.
- 2026-10-18 — owner answered U13: daemon language Rust; D1 owner-approved; Go and C#/.NET NativeAOT recorded as rejected.
- 2026-10-18 — owner answered U8: React 19 + TS strict + Vite SPA + React Aria + TanStack Virtual (D3), initial Svelte 5 fallback conditioned on S5 (superseded by D49 on 2026-10-23); owner context recorded: all code written by AI agents (N13, D44); D27 generator fixed (Rust → JSON Schema → TS); R44–R45, T0.9, T1.16, 08 §6, 11 §3 added; research filed as 16.
- 2026-10-19 — owner approved D27 (Rust types → committed `schemars` JSON Schema → TypeScript, `ts-rs` fallback, CI drift check); 02, 10 and 11 wording aligned.
- 2026-10-19 — owner answered U9/U25: internal-use scope (D45); Claude via SDK/adapter with the user's own API key (D29 approved); Codex app-server with the user's own login; terms gate removed from P1d/E1.15/M08; reopen trigger R46; R3/R43 downgraded.
- 2026-10-19 — owner decided single-user scope (D46): distribution items parked in 09 §5 (Developer ID, notarization, updater, `NOTICE`, SBOM, signed artifacts, bug template, privacy notes, distro packages, store track, trademark, bundling); U3/U14 answered (private, all rights reserved); U5/U7, R28/R29 deferred; code-signing peer admission uses Daliys's own identity; trigger R47.
- 2026-10-19 — owner answered U21 (D47): v1 never shows a "protected" status; each session shows a verification checklist (06 §3b); "approvals not protected" banner stays; S7 gates any future "protected" status via a separate decision. 05 §7, 06, 08, 10, 11, 12 D20, 13 R5, 01 N10 aligned.
- 2026-10-19 — owner accepted all remaining recommendations (17): U1, U2, U4, U6, U10, U11, U12, U15–U20, U22–U24, U26 answered (U2 deferred; U4 to Phase 3; U11 to Phase 2); Hermes not in Phase 1 (D48); P1a runtime checklist kept; core spikes S1-lite, S2, S5, S9-lite authorized. 01–08, 10–13, 15 aligned.
- 2026-10-19 — owner set spike rules: code in gitignored `spikes/<id>/` (test repos in `spikes/<id>/workdir/`), notes committed in `planning/spikes/<id>.md` with the template in `planning/spikes/README.md` (10 §2); plan committed.
- 2026-10-20 — owner decisions D49–D54 synced across 00–13 (T1.3a daemon skeleton added); then D55–D61 accepted (17 §Г): client key auth, approvals through the daemon, high-entropy redaction, B5 budget, tsc dev dependency, data-dir race and slow clients, docs sync. B3 updated to the final S5 run (idle 113.7 MB, ≈396 MB loaded); T1.15 moved under E1.13; Grok → budget-report link added; Python wording in 07 clarified.
- 2026-10-22 — D64 records B4's callback-latency proxy; W4-i results and W4-j history synced across 07, 12, 13, 17 and S5.
- 2026-10-23 — D64 clarified to use viewport-intersecting rows; D65 sets the 0.5% sample floor; W4-m re-gated night data shows Tauri and Swift miss B4 similarly, so D49 retains Tauri + React and delivery-path work comes next.
- 2026-10-23 — D66 makes the normal-browser React UI the primary Phase 1 client; loopback listener and pairing move into Phase 1, Tauri shell/mobile/other platforms become later ports; W5-a roadmap added in 18.
