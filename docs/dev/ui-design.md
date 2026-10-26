# Shardrail browser UI design system

Status: W12 stage A design direction. This is the working UI specification for the browser prototype; it does not change the product information architecture or security model.

## Design brief

Shardrail is a local, security-sensitive developer console for supervising AI coding sessions. It should feel calm, trustworthy, compact, and readable during long work sessions. Use clear hierarchy and restrained color, with enough density to keep sessions, events, checks, and approvals close at hand.

The interface reports evidence and uncertainty honestly. It never claims approvals are protected, never treats missing information as success, and never presents model-authored text as verified instruction. The permanent host banner retains the phrase **“approvals not protected”** and its existing meaning. Each session retains the complete verification checklist.

This stage documents the design language and screenshot coverage. Screen redesign happens in later W12 stages.

## Principles

1. **State before decoration.** Connection, session, approval, and verification states are legible at a glance and also have text labels.
2. **Trust boundaries are visible.** Structured action facts lead; agent-authored explanation is secondary and labelled untrusted.
3. **Safe actions stay easy.** Deny is at least as visible and easy to reach as Allow. Allow is never the default, primary, autofocus, or Enter-key action.
4. **Dense, not cramped.** Use compact metadata and generous separation between distinct actions. Keep commands and paths readable without truncation.
5. **One responsive product.** Phone layout preserves the same information and safety cues; it changes navigation and stacking, not the data model.
6. **Keyboard and touch are first-class.** Visible focus, predictable order, React Aria keyboard behavior, and 44px phone targets.
7. **Motion explains change.** Use short transitions for state changes only. Respect reduced-motion preferences.
8. **Use plain language.** Human-readable errors first; provider error codes remain available as compact support detail.

## Visual direction

- Quiet neutral surfaces, one cool accent for navigation/focus, amber for pending approval, red for denial/failure, and green only for verified/success evidence.
- Avoid gradients, glowing borders, glass effects, full-bleed saturated panels, and decorative movement.
- Use system UI and system monospace stacks. Load no remote fonts, scripts, images, icons, or stylesheets.
- Keep desktop as an application window, not a full-screen “cockpit”.
- Use in-repository inline SVG icons; no icon package.

## Token model

Use three layers: **primitive** values describe raw color/size choices; **semantic** tokens describe intent; **component** tokens apply semantic values to a named component. Components consume semantic tokens unless a true local exception is documented. Do not copy raw hex/OKLCH values into component rules.

Primitive colors use OKLCH for perceptual palette steps. The values below are initial authoring values; verify actual rendered text/background pairs to WCAG 2.2 AA during implementation. OKLCH improves perceptual adjustment, but does not guarantee contrast. Do not use APCA as a WCAG pass/fail criterion: it is an evolving proposal, while WCAG 2.2 AA uses the specified relative-luminance contrast ratio.

```css
@layer tokens {
  :root {
    color-scheme: light dark;
    --p-neutral-0: oklch(99% 0.004 255);
    --p-neutral-50: oklch(97% 0.006 255);
    --p-neutral-100: oklch(94% 0.008 255);
    --p-neutral-200: oklch(88% 0.012 255);
    --p-neutral-400: oklch(65% 0.018 255);
    --p-neutral-600: oklch(46% 0.02 255);
    --p-neutral-800: oklch(27% 0.02 255);
    --p-neutral-900: oklch(20% 0.02 255);
    --p-neutral-950: oklch(15% 0.018 255);
    --p-blue-600: oklch(49% 0.14 250);
    --p-blue-400: oklch(70% 0.13 245);
    --p-amber-700: oklch(48% 0.12 65);
    --p-amber-300: oklch(82% 0.12 75);
    --p-red-700: oklch(48% 0.17 25);
    --p-red-300: oklch(75% 0.14 25);
    --p-green-700: oklch(45% 0.12 150);
    --p-green-300: oklch(74% 0.12 150);
    --p-purple-700: oklch(48% 0.13 305);
    --p-purple-300: oklch(76% 0.12 305);
  }
}
```

### Semantic color tokens

| Role | Light | Dark | Use |
|---|---|---|---|
| `--color-bg` | neutral 50 | neutral 950 | Application canvas |
| `--color-surface` | neutral 0 | neutral 900 | Main panels, cards |
| `--color-raised` | white | neutral 800 | Popovers, dialogs, raised rows |
| `--color-border` | neutral 200 | neutral 800 | Dividers and control edges |
| `--color-text` | neutral 900 | neutral 50 | Main readable text |
| `--color-muted` | neutral 600 | neutral 400 | Secondary text; verify 4.5:1 for body text |
| `--color-accent` | blue 600 | blue 400 | Links, selected navigation, non-destructive primary action |
| `--color-focus` | blue 600 | blue 400 | Visible focus ring, minimum 3:1 against adjacent colors |
| `--color-approval-wait` | amber 700 | amber 300 | Pending approval label and border, always paired with text/icon |
| `--color-danger-deny` | red 700 | red 300 | Deny, failure, blocked, destructive outcome |
| `--color-success` | green 700 | green 300 | Verified/success with explicit text |
| `--color-warning` | amber 700 | amber 300 | Caution and degraded states |
| `--color-untrusted` | purple 700 | purple 300 | Agent-authored description marker and boundary |

Set a `data-theme="light"|"dark"` override on the root for a manual choice; absent that attribute, follow `prefers-color-scheme`. Set `color-scheme` on the root so native controls match. Prefer paired light/dark values in `light-dark()` where that keeps rules smaller; use root theme scopes for complete palettes and explicit manual overrides. Add `<meta name="color-scheme" content="light dark">` before stylesheet loading.

### Type, spacing, and shape

| Token | Value | Typical use |
|---|---:|---|
| `--font-ui` | system UI stack | Interface text |
| `--font-mono` | system monospace stack | Commands, paths, IDs, support codes |
| `--text-xs` | 0.75rem / 1.1 | Dense timestamps, secondary metadata; never essential alone |
| `--text-sm` | 0.875rem / 1.35 | Controls, labels, metadata |
| `--text-md` | 1rem / 1.5 | Body and default control text |
| `--text-lg` | 1.125rem / 1.35 | Section headings |
| `--text-xl` | 1.375rem / 1.25 | Screen title |
| `--text-2xl` | 1.75rem / 1.2 | Pairing title or major empty state |

Use a 4px base spacing scale: `--space-1: .25rem`, `--space-2: .5rem`, `--space-3: .75rem`, `--space-4: 1rem`, `--space-5: 1.25rem`, `--space-6: 1.5rem`, `--space-8: 2rem`, `--space-10: 2.5rem`, `--space-12: 3rem`.

Radii: `--radius-sm: 4px`, `--radius-md: 8px`, `--radius-lg: 12px`, `--radius-pill: 999px`. Use square corners sparingly for code output where a card radius would reduce usable width.

Shadows: `--shadow-popover: 0 8px 24px oklch(0% 0 0 / .14)` and `--shadow-dialog: 0 20px 60px oklch(0% 0 0 / .24)`. Shadows show elevation; borders still define edges in both themes.

Layer order: `--z-base: 0`, `--z-sticky: 10`, `--z-sidebar: 20`, `--z-popover: 30`, `--z-toast: 40`, `--z-modal: 50`. Do not create arbitrary z-index values in components.

Motion: `--duration-fast: 100ms`, `--duration-normal: 160ms`, `--duration-slow: 220ms`; `--ease-standard: cubic-bezier(.2, 0, 0, 1)`, `--ease-enter: cubic-bezier(0, 0, 0, 1)`. No token-by-token animation. Reduce or remove nonessential transitions under `prefers-reduced-motion: reduce`.

## Layout and responsive behavior

### Desktop: 1200px and wider

- Persistent session navigation column: 260–320px, independently scrollable.
- Main session surface fills remaining width and has a readable max content width around 1100px.
- Header and the host “approvals not protected” banner remain visible while the session body scrolls.
- Transcript, approval area, and composer use clear vertical regions; keep the checklist easy to find and complete.
- Allow denser event metadata, but do not shrink body text below 14px.

### Compact desktop/tablet: 700–1199px

- Navigation column may narrow to 220–260px; wrap secondary metadata before shrinking the transcript.
- Collapse optional diagnostics into labelled disclosures. Do not collapse the host banner, approval facts, checklist summary, or untrusted marker.
- Use container queries for cards that can appear in differently sized panes; use media queries for viewport-level navigation changes.

### Phone: 390px target and below 700px

- One column. Sessions open as a drawer or dedicated list view; keep a labelled way back to sessions.
- Header may wrap. Host banner remains full-width and visible.
- Approval card stays in the primary flow and shows exact command/argv/paths, cwd, sandbox, approval scope, and checklist summary without truncating executable details.
- Put action buttons on separate, full-width or comfortably split rows; touch targets at least 44×44px.
- Composer respects the safe area and remains reachable above the on-screen keyboard. No horizontal page scrolling.
- Session/event rows reflow metadata onto additional lines; IDs and paths wrap safely.

Use logical properties (`margin-inline`, `padding-block`, `inset-inline-start`, etc.). Use CSS Grid/Flexbox before absolute positioning. Container queries are for reusable component adaptation, not a substitute for the small set of viewport layout transitions.

## Component specifications

### Buttons

- **Primary:** one non-destructive next action, solid accent, high contrast. Never Allow.
- **Secondary:** neutral surface and border for routine actions such as Reconnect or Open checklist.
- **Quiet:** low emphasis for navigation or utility actions; still has hover, focus, pressed, and disabled states.
- **Deny:** danger text and clear bordered or filled danger treatment. It is at least as prominent as Allow and no harder to reach.
- **Allow once:** neutral/outlined treatment, never a default action or primary emphasis. Must require the existing first click and second explicit confirmation when the action is undisclosed. No autofocus; Enter never activates Allow.
- Disabled state remains readable and includes nearby explanation if a required permission or connection is missing.
- All interactive buttons use React Aria Components `Button`.

### Field, select, and form

- Visible label; helper text and errors associated with the field. Error text describes cause and next step.
- Focus ring uses `--color-focus` with a visible offset. Invalid state uses text/icon as well as color.
- Select uses React Aria `Select`, `Popover`, and `ListBox`; do not replace with a custom listbox.
- Form submit is explicit; Enter in a field must not accidentally allow an approval.

### Session list item

- Entire row is selectable with React Aria `ListBox` / `ListBoxItem` semantics.
- Show workspace/session label, provider, current lifecycle state, latest activity, and checklist counts when known.
- Selection has a non-color cue (surface/edge and selected indicator). Unknown and unavailable values remain explicit.
- Small-screen row uses two or three lines instead of clipped columns.

### Banners and alerts

- Host banner is always visible in the main product view and always states “approvals not protected” with current causes. Never shorten, hide, collapse by default, or imply a protected state.
- Connection state is distinct from host security state. Use text labels for Connected, Reconnecting, Offline, and Key changed.
- Inline alerts sit next to the action/field that failed. Critical failures use `role="alert"`; nonurgent connection updates use status announcements without repeated noisy announcements.
- Toasts are reserved for reversible, transient confirmations; they never replace durable approval/session state.

### Verification checklist row

- Every required item remains present. Show item label, one of `verified`, `not verified`, `failed`, or `n/a`, source/evidence, detail, and last checked time.
- Failed and not-verified items sort first. Status is expressed in words and a shape/icon, not color alone.
- Never sum rows into one “protected” verdict. A failed or unknown check cannot look successful.

### Tool/event card and transcript

- Tool calls are structured event cards, not chat bubbles. Show event type, time/cursor where useful, and output with preserved whitespace.
- Diffs are rendered as diffs. Long output can collapse behind an explicit control; visible excerpt is labelled as partial.
- Virtualize long event histories with the existing TanStack Virtual usage; keep focusable regions keyboard scrollable.
- Agent prose always goes through the existing `UntrustedMarkdown` module. No raw HTML, remote images, unsafe URL schemes, `dangerouslySetInnerHTML`, `innerHTML`, or `eval`.

### Untrusted text block

- Separate surface/border plus the visible label **“Agent’s description · untrusted”**.
- It is secondary to structured command, paths, network target, cwd, and permission facts.
- Links are not silently opened; preserve the confirmation step and protocol allowlist.
- Make bidi/zero-width/control characters visible in approval action facts and offer a raw-byte view when the underlying UI supports it.

### Approval card

- This is a distinct decision surface, not an ordinary transcript message.
- Lead with exact machine-readable action facts. Preserve line breaks. Show cwd, sandbox axis, approval axis, option scope, and hidden-character indicators.
- If action details are absent, say **“action not disclosed by provider”** and keep the additional second-step Allow confirmation.
- Show the checklist summary, prioritizing failed/unverified counts; the full checklist stays available.
- Pending, responding, responded-unacked, delivered-unconfirmed, denied, allowed, expired, and withdrawn states have explicit labels. Disconnects, timeouts, and delivery uncertainty use degraded/unknown language and never resemble Allow.
- Deny is immediately available. Allow is explicit, never default/primary/autofocused, never keyboard-submitted by Enter, and the second confirmation remains.
- Browser/mobile scope exposes only once and deny options; no widening action appears.

### Composer and interrupt

- Label the prompt field, keep the draft on its session, and preserve it after rejected sends.
- Send is disabled for empty text. Show exact blocked reason after failure/disconnect; never clear a draft on uncertain delivery.
- Interrupt is a separate action and does not erase text. Use explicit “Interrupt” wording rather than an ambiguous stop icon.
- On phones, controls meet 44px targets and remain reachable with the virtual keyboard open.

### Empty, loading, error, and offline states

- **Empty sessions:** say no sessions exist, explain the next action, and keep the host banner visible.
- **First load:** use a small neutral skeleton only when layout is known; do not imply sessions or approvals exist before data arrives. Label waiting state.
- **Streaming:** append content without per-token animation or layout jumps; tool cards update in place.
- **Pairing error:** say whether identity could not be read, enrollment failed, confirmation expired, or key mismatch blocked pairing. Give a safe next action.
- **Create/prompt error:** keep entered form/prompt content; show the failure inline and state whether an action was not sent or its outcome is unknown.
- **Offline/reconnecting:** clearly state reconnection status and keep approval controls disabled or blocked. Never imply a response succeeded.
- **No provider details:** say action is undisclosed rather than guessing from agent text.

### Icon set

Author a small local SVG symbol set and React components for: sessions/list, session/workspace, connection/online, reconnect, offline, warning, error, verified, not verified, failed, approval/pending, deny, allow/confirm, expand/collapse, external link, copy, interrupt, send, close/back, and checklist. Decorative SVGs are hidden from assistive technology; icon-only controls have accessible names. No emoji as state icons.

## Screen-by-screen specification

| Screen/state | Required content and behavior |
|---|---|
| Pairing / enrollment | Expected daemon fingerprint, explicit fingerprint comparison instructions, limited scope wording, one-time code, explicit user confirmation; disclose fallback key storage and require acknowledgement if applicable. |
| Pairing changed-key block | Prominent blocked state; state that the daemon key changed and pairing is blocked; explain trusted recovery steps. No one-click “continue anyway”. |
| Pairing error/expired | Distinguish identity unavailable, invalid code, and expired confirmation; retry/review current fingerprint safely. |
| Main header | Product name, local-daemon context, connection state and reconnect action. |
| Host banner | Always visible and meaning-preserving “approvals not protected” notice plus causes. |
| Session list | Empty and populated states; provider/status/activity/check summary; keyboard selection. |
| New session | Workdir selection, clear permission state, creation action, retained selection and inline create error. |
| Session detail | Selected session identity, event region, prompt composer, interrupt, complete verification checklist, approval area. |
| Streaming transcript | Agent text is untrusted; tool events distinct and structured; live tail follows unless the user has scrolled away. |
| Tool events and diffs | Event type, safe output, preserved whitespace, diff semantics, explicit truncation/collapse affordance. |
| Approval pending | Exact action facts, scope, cwd, sandbox, checklist summary, untrusted description label, Deny and Allow once. |
| Approval two-step Allow | Separate explicit confirmation with undisclosed-action warning; no keyboard default path to allow. |
| Approval answered/stale | Clear terminal/uncertain status; remove active controls when resolved; stale tab rejection stays explicit. |
| Verification checklist | Full per-session rows and evidence; failed/unverified first; no aggregate protection claim. |
| Interrupt | Immediate intent, clear pending/accepted/failure status, composer draft preserved. |
| Prompt blocked | Draft remains; inline reason states no send / uncertain delivery; no false success. |
| Connection lost/reconnecting/offline | Visible connection label and reconnect action; approvals cannot be sent while disconnected. |
| General errors | Human language, field/action proximity, provider code retained as support detail. |

## Accessibility checklist

- WCAG 2.2 AA contrast: normal text 4.5:1, large text 3:1; meaningful component boundaries and focus indicators at least 3:1 where required.
- Do not use color as the only state cue. Include text and, where appropriate, an icon or shape.
- Every interactive primitive comes from React Aria Components when available. Do not hand-roll role/ARIA behavior or keyboard systems.
- Visible `:focus-visible`; focus is never obscured by sticky content, drawers, banners, or dialogs. Restore focus after overlays close.
- Dialogs are named and manage focus. Errors are programmatically associated and announced once without stealing focus unexpectedly.
- Phone targets are at least 44×44 CSS px. Keep controls separated and avoid tiny inline action icons.
- At 200% text zoom, content reflows without loss or horizontal page scroll. Long paths/commands wrap or have a safe horizontal code region.
- Respect `prefers-reduced-motion`; no essential meaning depends on animation.
- Test keyboard-only pairing, list navigation, composer, deny, two-step allow, reconnect, and checklist access.
- Test automated accessibility on both themes and desktop/phone layouts, then manually verify focus, reading order, and touch targets.
- Maintain semantic headings, landmarks, labels, status announcements, and descriptive document language.

## CSS architecture

- Plain modern CSS; no styling framework. Start with cascade layers: `reset`, `tokens`, `base`, `components`, `utilities`.
- Use CSS nesting for local relationships only; keep selector specificity low and predictable.
- Prefer custom properties for tokens, logical properties for direction, Grid/Flexbox for layout, and container queries for component adaptation.
- Keep breakpoints few and tied to information architecture: wide split view, compact split view, one-column phone view.
- Use `:focus-visible`, `color-scheme`, `light-dark()`, `prefers-color-scheme`, `prefers-reduced-motion`, and `@container` where supported by the target engines; retain explicit theme overrides.
- Avoid global element styling that leaks into React Aria popovers/dialogs. Scope styles by component class or stable data attributes.
- Do not introduce runtime CSS-in-JS or build-time styling packages.

## Dependencies

W12 adds **no new dependencies**. Use only the current S5 allowlist and React Aria as the sole interactive primitive layer. Licenses below are read from the installed package manifests in this worktree.

| Package | Current version | License |
|---|---:|---|
| `react` | 19.3.1 | MIT |
| `react-dom` | 19.3.1 | MIT |
| `react-aria-components` | 1.22.1 | Apache-2.0 |
| `@tanstack/react-virtual` | 3.14.21 | MIT |
| `@tauri-apps/api` | 2.12.4 | Apache-2.0 OR MIT |
| `react-markdown` | 10.1.0 | MIT |
| `rehype-sanitize` | 6.0.0 | MIT |
| `@axe-core/playwright` | 4.14.0 | MPL-2.0 |
| `@eslint/js` | 9.39.5 | MIT |
| `@playwright/test` | 1.64.1 | Apache-2.0 |
| `@tauri-apps/cli` | 2.12.3 | Apache-2.0 OR MIT |
| `@types/react` | 19.2.7 | MIT |
| `@types/react-dom` | 19.2.3 | MIT |
| `@vitejs/plugin-react` | 5.0.4 | MIT |
| `babel-plugin-react-compiler` | 1.0.0 | MIT |
| `eslint` | 9.39.5 | MIT |
| `eslint-plugin-jsx-a11y` | 6.10.2 | MIT |
| `eslint-plugin-react-compiler` | 19.1.0-rc.2 | MIT |
| `eslint-plugin-react-hooks` | 7.1.1 | MIT |
| `globals` | 17.13.0 | MIT |
| `tsx` | 4.20.6 | MIT |
| `typescript` | 5.9.3 | Apache-2.0 |
| `typescript-eslint` | 8.71.2 | MIT |
| `vite` | 7.3.7 | MIT |
| `vitest` | 5.0.5 | MIT |
| `zod-validation-error` | 4.0.2 | MIT |
| `fake-indexeddb` (development) | 6.2.4 | Apache-2.0 |

## Research sources

Research is current as of 2026-10-26. Product structure and security decisions below defer to the repository plan and owner decisions; external examples inform visual execution, not Shardrail’s safety behavior.

- [Planning 08 — Interface and Visual System](../../planning/08-ui-ux.md), especially §2 and §6: React 19, React Aria, TanStack Virtual, sanitized Markdown, information architecture, mobile rules.
- [Planning 06 — Security and Trust](../../planning/06-security.md), especially §3, §3b, and §5: host warning, per-session checks, structured approval data, untrusted rendering.
- [Planning 12 — Decisions](../../planning/12-decisions.md), D3/D20/D22/D38/D47: approved UI stack, trust boundary, untrusted approval fields, no protected status.
- [Planning 16 — UI technology research](../../planning/16-ui-technology-research.md): rationale for the current React stack and implementation constraints.
- [WCAG 2.2 Recommendation](https://www.w3.org/TR/WCAG22/): AA text contrast, focus visibility/obscuration, reflow, target size minimum, and use of color.
- [MDN: `color-scheme`](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/color-scheme): native control and page color-scheme behavior; supports pairing this property with `light-dark()`.
- [MDN: `prefers-color-scheme`](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/At-rules/%40media/prefers-color-scheme): system theme preference behavior.
- [W3C CSS Color 5](https://www.w3.org/TR/css-color-5/): standardized `light-dark()` behavior and color-scheme selection.
- [W3C CSS Color 4, Oklab/OkLCh](https://www.w3.org/TR/css-color-4/#ok-lab): perceptual color spaces for palette construction; color choice still requires contrast testing.
- [W3C WCAG 3.0 Working Draft](https://www.w3.org/TR/wcag-3.0/): contrast method remains undecided in this draft; APCA is not a WCAG 2.2 conformance replacement.
- [React Aria Components](https://react-aria.adobe.com/components): accessible, styleable React primitives and component behavior reference.
- [React Aria Dialog](https://react-aria.adobe.com/Modal/useDialog): dialog naming, modal visibility, focus containment, and focus restoration patterns.
- [MDN: CSS cascade](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Cascade/Introduction): cascade layers organize style precedence and reduce specificity conflict.
- [MDN: CSS nesting](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Nesting): native nesting and selector behavior.
- [MDN: container queries](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Containment/Container_queries): component adaptation by available container width, with media queries for viewport layout.
- [MDN: reduced-motion accessibility](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Media_queries/Using_for_accessibility): reduce or remove motion according to the user preference.
- [OpenAI Codex](https://openai.com/codex/) and [Anthropic Claude Code](https://docs.anthropic.com/en/docs/claude-code/overview): current coding-agent products center task/session context, streamed assistant output, and tool activity in a readable work surface. Shardrail adapts the session/transcript/tool grouping while keeping approval decisions visibly separate.
- [VS Code Copilot Chat](https://code.visualstudio.com/docs/copilot/chat/chat-agent-mode): agent mode surfaces tool progress and user review in the same task context; Shardrail keeps approval facts separate from agent-authored explanations.

These product references illustrate familiar interaction patterns only. They do not supply Shardrail’s permission semantics, which are specified by the security plan above.
