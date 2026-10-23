# 08 — Interface and Visual System
> 2026-10-20 (owner): D49–D54 recorded — React retained on partial S5 evidence; journal opens atomically without following symlinks; daemon refuses network/synced data filesystems; Codex is first in P1a; tokio/clap/nix are allowed daemon dependencies; contracts remain generated with client helpers in `packages/client`.
> 2026-10-23 (owner, D66): the React UI first runs in a normal browser; Tauri-specific isolation/capabilities apply only to the later desktop port.

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Product design is a first-class module developed alongside the backend, not decoration added afterwards. Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): agent-authored approval text box (D38), per-session bypass indicators, new inbox items. 2026-10-18 (owner): U8 answered — React 19 + TS strict + Vite SPA + React Aria + TanStack Virtual (D3); all code is written by AI coding agents (D44); §6 UI-stack rules for AI agents added. 2026-10-19 (owner): U21 answered — v1 never shows a 'protected' status; each session shows a verification checklist (D47); S7 gates any future 'protected' status, which needs a separate decision. 2026-10-19 (owner): U2 brand book deferred.

## 1. Principles

- Session state is visible without reading text: derived badges (05 §7), the host-level "approvals not protected" overlay, and a per-session verification checklist (06 §3b). No surface uses the word "protected" as a status in v1 (D47).
- Approval is its own modality, not a chat line: monospace command with preserved line breaks, cwd, both sandbox and approval axes, option scope labels, hidden characters made visible (06 §5). Agent-written explanations (`title`, `reason`, `description`) appear only in a secondary box labelled "agent's description"; "action not disclosed by provider" is a distinct, more cautious card (D38). The approval card carries the session's checklist summary, with failed and unverified items listed first, and opens the full checklist; it never says "protected" (D47).
- Each session shows what bypasses Shardrail's approvals for that provider and profile (06 §3a), e.g. "Hermes: file edits without approval", "pre-approved by Claude settings: 4 rules".
- Model reasoning is collapsed by default, easy to open, never styled as an error.
- Diffs read as diffs, not JSON.
- Protocol errors are written in human language and keep the provider's code on one line for support.
- Keyboard: composer, send, cancel turn, allow, deny, next approval, command palette for every action; visible focus.
- Motion is short and disabled by `prefers-reduced-motion`; no per-token animation; tokens append to text; tool cards update in place.
- Desktop density; light and dark themes; text and state contrast at WCAG 2.2 AA (N7). Accessibility is checked on the stream, approval, composer and tree flows, with a linter in CI only as a cheap filter.
- The macOS window keeps native chrome (traffic lights, normal title bar); no full-screen "cockpit".

## 2. Information architecture

| Surface | Purpose |
| --- | --- |
| Attention inbox (default view) | everything needing the human across hosts: approvals, failures, delivery unknown, approval outcome unconfirmed, queued instruction after an abnormal turn end, drain blockers before an upgrade, journal unavailable, completions; keyboard next/prev, allow/deny |
| Session list | grouped host → workspace; derived badge, provider, sandbox indicator, checklist summary (failed/unverified count), write-lease holder |
| Session view | virtualized transcript, in-place tool cards, approval modality, composer with queue indicator, session indicator opening the verification checklist (item, status, source, last checked) |
| Workspace pane | read-only tree, Git status ("unverified" entries for filtered files), diff for the session's real cwd/worktree; "unknown cwd" shown as such |
| Command palette | all actions |
| Diagnostics | provider versions and compatibility class, binary paths and signing identity, connection state, per-process footprint, "not protected" causes per host and the checklist history per session (D47) |

Layouts: single session, split (2–4 sessions tiled), inbox + session; density setting. Layout state is per-client view state. Phase 1a ships inbox + single session view + palette; tiling and density follow.

## 3. Rendering pipeline

Coalesced deltas from the daemon; at most one repaint per frame per item; incremental Markdown; lazy code highlighting; virtualized long outputs and diffs, collapsed beyond a threshold with "open full"; transcript items outside the virtual window held as lightweight records; a WebView memory watchdog that transparently reloads the view during multi-day use. Untrusted content per 06 §5.

## 4. Visual identity

The Fork Horizon brand book is deferred (U2, owner 2026-10-19); until then: neutral dark base, one accent for focus, one for approval-wait, a distinct colour for deny/degraded; system UI and monospace fonts; a small icon sprite rather than an icon package. Initial primitives: button, field, virtualized list, tool card, approval card, tree, diff, state banner, sandbox indicator, verification checklist — a floor for Phase 1a, extended when a scenario cannot be built from them. No heavy UI kit (Material, full Chakra). Interactive primitives come from React Aria Components; styling with design tokens (Tailwind allowed), never a second primitive library such as Radix/shadcn (D3).

## 5. Mobile

Same data model and derived states; one column; inbox and approvals first. Approval cards show command, cwd, sandbox and the checklist summary in full — truncation never hides the executable part. Remote (`limited`) clients see only once/deny options. Notification payloads contain no secrets or command text by default.

## 6. UI stack and rules for AI agents (D3, D44)

Stack (owner-approved 2026-10-18; retained 2026-10-23 under D49): React 19, TypeScript strict, Vite plain SPA (no Next.js/SSR/RSC), React Aria Components, TanStack Virtual, streaming Markdown through one sanitizing module; React Compiler enabled. W4-m found similar B4 latency growth in Tauri and Swift, pointing to the daemon-to-UI delivery path; optimize that path before reopening the framework choice. Research and sources: 16. General agent rules: 11 §3.

1. **Modern React only:** function components, React 19 APIs, React Compiler on; no class components, no `forwardRef`, no Next.js/RSC/server functions.
2. **One primitive layer:** every interactive element (button, menu, listbox, tree, grid, dialog, tooltip) is a React Aria component; hand-written `role`/`aria-*` and custom key handling need review; no Radix/shadcn mixing.
3. **Streaming:** token deltas never go through per-token `useState`; they land in an external store (`useSyncExternalStore` or one Zustand store) and are coalesced per `requestAnimationFrame`; Markdown is memoized per block; long lists and history use TanStack Virtual only; a test drives N agents × M tokens/s against B4.
4. **Untrusted text (06 §5, D22, D38, R44):** agent text renders only through the `untrusted-render` module (Streamdown or equivalent with sanitize/harden): no raw HTML, no data or remote images, links by protocol allowlist with confirmation; ESLint bans `dangerouslySetInnerHTML`, `innerHTML`, `eval` outside that module; agent text never reaches `href`/`src`/`style` without a validator; bidi and zero-width characters in approval commands are made visible; CSP, Tauri isolation pattern, minimal capabilities.
5. **State:** the daemon is the source of truth; the client holds a bounded projection; local UI state in React state or one Zustand store; no Redux-style second store.
6. **Lint and tests as gates:** ESLint (flat) with `react-hooks`, `react-compiler`, `jsx-a11y` and the bans above; Prettier; `tsc --noEmit`; Vitest + Testing Library; **headless Playwright in the normal browser engine, run during the day**; axe-core on every screen; rendering fixtures T1.10. No physical-device or overnight browser checks are a Phase 1 gate.
7. **Docs for agents:** React Aria MCP server and llms.txt, pinned in `AGENTS.md` (11 §3).
