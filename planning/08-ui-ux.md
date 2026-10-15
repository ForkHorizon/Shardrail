# 08 — Interface and Visual System

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Product design is a first-class module developed alongside the backend, not decoration added afterwards.

## 1. Principles

- Session state is visible without reading text: derived badges (05 §7) and a host-level protection overlay.
- Approval is its own modality, not a chat line: monospace command with preserved line breaks, cwd, both sandbox and approval axes, option scope labels, hidden characters made visible (06 §5).
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
| Attention inbox (default view) | everything needing the human across hosts: approvals, failures, delivery unknown, approval outcome unconfirmed, completions; keyboard next/prev, allow/deny |
| Session list | grouped host → workspace; derived badge, provider, sandbox indicator, write-lease holder |
| Session view | virtualized transcript, in-place tool cards, approval modality, composer with queue indicator |
| Workspace pane | read-only tree, Git status ("unverified" entries for filtered files), diff for the session's real cwd/worktree; "unknown cwd" shown as such |
| Command palette | all actions |
| Diagnostics | provider versions and compatibility class, binary paths and signing identity, connection state, per-process footprint, host protection state and its causes |

Layouts: single session, split (2–4 sessions tiled), inbox + session; density setting. Layout state is per-client view state. Phase 1a ships inbox + single session view + palette; tiling and density follow.

## 3. Rendering pipeline

Coalesced deltas from the daemon; at most one repaint per frame per item; incremental Markdown; lazy code highlighting; virtualized long outputs and diffs, collapsed beyond a threshold with "open full"; transcript items outside the virtual window held as lightweight records; a WebView memory watchdog that transparently reloads the view during multi-day use. Untrusted content per 06 §5.

## 4. Visual identity

Until the Fork Horizon brand book exists (U2): neutral dark base, one accent for focus, one for approval-wait, a distinct colour for deny/degraded; system UI and monospace fonts; a small icon sprite rather than an icon package. Initial primitives: button, field, virtualized list, tool card, approval card, tree, diff, state banner, sandbox indicator — a floor for Phase 1a, extended when a scenario cannot be built from them. No heavy UI kit (Material, full Chakra).

## 5. Mobile

Same data model and derived states; one column; inbox and approvals first. Approval cards show command, cwd and sandbox in full — truncation never hides the executable part. Remote (`limited`) clients see only once/deny options. Notification payloads contain no secrets or command text by default.
