# 13 — Risks and Open Questions
> 2026-10-20 (owner): D49–D54 recorded — React retained on partial S5 evidence; journal opens atomically without following symlinks; daemon refuses network/synced data filesystems; Codex is first in P1a; tokio/clap/nix are allowed daemon dependencies; contracts remain generated with client helpers in `packages/client`.

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): review recommendations added to U12, U22 (since answered); U25, U26 and R32–R43 added. 2026-10-18: U13 answered by the owner (Rust) and moved to §1a. 2026-10-18 (owner): U8 answered — React 19 + TS strict + Vite SPA + React Aria + TanStack Virtual (D3); all code is written by AI coding agents (D44); U8 moved to §1a; R44–R45 added. 2026-10-19 (owner): U9/U25 answered — internal-use scope (D45); Claude via Agent SDK / `claude-agent-acp` with the user's own API key; Codex app-server with the user's own login; reopen trigger R46. U9, U25 moved to §1a; R3, R43 downgraded. 2026-10-19 (owner): single-user scope (D46) — Daliys is the only user on his own machines until the product fully works; distribution items deferred to 09 §5 (trigger R47). U3/U14 answered; U5, U7, R28, R29 deferred; R31, R41 adjusted; R47 added. 2026-10-19 (owner): U21 answered — v1 never shows a 'protected' status; each session shows a verification checklist (D47); S7 gates any future 'protected' status, which needs a separate decision. 2026-10-19 (owner): remaining owner questions answered in bulk (17); Hermes not in Phase 1 (D48); core spikes S1-lite, S2, S5, S9-lite authorized. All U questions are in §1a except U5/U7 (deferred until other users).

## 1. Owner decisions needed

Only U5 and U7 remain, both deferred until other users (D46, R47). Everything else is answered (§1a; bulk answers 2026-10-19 in [17](17-owner-decisions-2026-10-19.md)).

| ID | Question | Blocks | Recommendation / interim |
| --- | --- | --- | --- |
| U5 | Trademark check for "Shardrail" | public marketing — **deferred until other users (R47, 09 §5)** | internal name acceptable |
| U7 | Include provider binaries in a commercial installer? | release packaging — **deferred until other users (R47, 09 §5)** | no |

### 1a. Answered by the owner

| ID | Question | Answer | Recorded in |
| --- | --- | --- | --- |
| U13 | Daemon language: Rust, Go or C#/.NET NativeAOT | **Rust** — owner decision, 2026-10-18. Go and C#/.NET NativeAOT rejected; no dual-language prototype | D1 (12), 02 §8 |
| U8 | UI framework (SolidJS, React, Svelte, …) | **React 19 + TypeScript strict + Vite (plain SPA, no Next.js/SSR) + React Aria Components + TanStack Virtual** — owner decision, 2026-10-18. Documented fallback: Svelte 5 + Bits UI, only if S5 shows React misses B3/B4/B5 in WKWebView | D3 (12), 02 §8, 08 §6, 16 |
| U9 | May Shardrail drive Claude via the Agent SDK / ACP adapter, and with which login? | **Internal use only (D45):** Agent SDK / `claude-agent-acp` with the user's own Anthropic API key; no claude.ai login offered in-app, no token brokering; no request to Anthropic needed now — owner decision, 2026-10-19. **Reopen before release if scope changes (R46)** | D29, D45 (12), 03 §6, 09 §2 |
| U25 | May Shardrail drive a user's locally logged-in Codex through `codex app-server`? | **Internal use only (D45):** yes, with the user's own Codex login; no request to OpenAI needed now — owner decision, 2026-10-19. **Reopen before release if scope changes (R46)** | D45 (12), 03 §6, 09 §2 |
| U3 | Product license | **Proprietary, all rights reserved for now** — owner decision, 2026-10-19 (single-user scope, D46). Reopen with distribution (R47); earlier options kept in 09 §5 | D46 (12), 09 §2, §5 |
| U14 | Repository visibility | **Private** — owner decision, 2026-10-19 (D46). Reopen with distribution (R47) | D46 (12), 09 §2, §5 |
| U21 | Threshold for claiming "protected approvals" after S7 | **No "protected" status in v1;** each session shows a verification checklist of what was actually verified (06 §3b). A "protected" status only after S7 confirms the checks are reliable, and only as a separate future decision — owner decision, 2026-10-19 | D47 (12), 06 §3b, 08 §1 |
| U1 | Performance reference machine | Daliys's own current Apple Silicon Mac; model and memory recorded with every measurement — owner decision 2026-10-19 | 07 §2 |
| U2 | Fork Horizon brand book | **Answered: defer** (single user); neutral dark tokens from 08 §4 — owner decision 2026-10-19 | 08 §4 |
| U4 | How a phone reaches the daemon | **Answered: defer to Phase 3**; then an existing mesh VPN or SSH — no custom crypto (no own mTLS) — owner decision 2026-10-19 | 12 D28, 10 P3 |
| U6 | Continuous agent work with the app closed | not in MVP; later an opt-in macOS launch agent (Phase 2, E2.5) — owner decision 2026-10-19 | 02 §3, 12 D11 |
| U10 | Default approval expiry | deny after 180 s, or earlier if the provider's known limit is lower — owner decision 2026-10-19 | 05 §6 |
| U11 | Separate Unix user for agents on remote hosts | **Answered: defer to Phase 2**; decided once the spawner mechanism is specified — owner decision 2026-10-19 | 02 §4, 06 TB-6, 11 M01/M05 |
| U12 | Journal encryption at rest | no journal encryption in v1 (FileVault covers the disk); crypto-shredding **deferred** (revisit with remote hosts / other users) — owner decision 2026-10-19 | 04 §6 |
| U15 | Cross-host inbox in Phase 1 or 2 | Phase 2, with the first remote host — owner decision 2026-10-19 | 10 P2 |
| U16 | Phase 1 browser-only? | no — keep the Tauri desktop shell (peer-UID admission, keychain, notifications, no TCP listener) — owner decision 2026-10-19 | 12 D2 |
| U17 | Maximum approval hold and staleness | 2 h maximum; re-confirm when the answer is older than 15 min or the workspace changed — owner decision 2026-10-19 | 05 §6 |
| U18 | Orphan-daemon policy after a shell crash | running turns get 10 min without any client, then are cancelled (desynced processes excepted, U22) — owner decision 2026-10-19 | 02 §3, 05 §6, 12 D11 |
| U19 | Manual handoff MVP at Phase 1 close | yes — simple bundle the user reviews and edits before sending — owner decision 2026-10-19 | 04 §8, 10 E1.16, 12 D10 |
| U20 | Pre-create a protected `.codex/` in trusted workspaces | ask when the workspace is marked trusted; default no — owner decision 2026-10-19 | 03 §6 |
| U22 | Terminate automatically when the link breaks but the process lives | never automatically: keep the process contained, mark the session `desynced`, put it in the inbox; the user decides — owner decision 2026-10-19 | 04 §7, 05 §2 |
| U23 | Exclude the data directory from Time Machine | yes by default, with a visible toggle — owner decision 2026-10-19 | 04 §6 |
| U24 | Second session on a worktree without provider read-only | refuse and offer a separate worktree — owner decision 2026-10-19 | 06 §7, 12 D31 |
| U26 | Hermes in Phase 1 | **no** — Hermes is not in Phase 1; if ever added, a permanent "file edits without approval" banner — owner decision 2026-10-19 | 12 D48, 03 §6, 10 Post-P1 candidate |

### 1b. Open S5 decisions

| Question | Status / decision needed | Recorded in |
| --- | --- | --- |
| What history/memory retention cap should the Rust-side event store use? | Open; the spike currently retains all durable rows so older ranges remain queryable | S5, `spikes/reviews11/w4j-final.md` |
| Is the minimum of 500 measured rows (1%) a hard B4 gate for 50k-event runs? | Open; W4-j's sample-floor delivery is not established, and live remeasurement is pending | S5, `spikes/reviews11/w4j-final.md` |

## 2. Risks

| ID | Risk | Severity | Treatment |
| --- | --- | --- | --- |
| R1 | Codex app-server schema changes between releases (help marks it experimental) | high | pinned reference schema, CI drift test, compatibility classes, no experimental API |
| R2 | No Claude channel provides reliable two-way approval | high | S4; SDK fallback; exclusion without affecting others |
| R3 | Claude SDK terms restrict subscription login in third-party products | low while internal (D45); high if scope changes | D29 (API key only, no claude.ai login, no token brokering); R46 trigger |
| R4 | Agents on the same UID tamper with Shardrail state, impersonate the daemon, or self-approve | high | D20 host protection; D21; S7; U11 |
| R5 | Few provider profiles satisfy the protection boundary, weakening the product's central safety claim | high (product) | no "protected" status in v1; per-session verification checklist (D47); S7 gates any future status |
| R6 | Agents edit provider approval configuration so providers stop asking | high | D20 (d): pinning + hash snapshot; U20 |
| R7 | Inherited capabilities (SSH agent, TCC grants, deep links) let an agent act as a client | high | D20, D21 |
| R8 | Repository configuration executes programs through Git | high | D23 |
| R9 | Untrusted rendering exfiltrates data or disguises an approval command | high | D22 |
| R10 | Project hooks or instructions load in untrusted workspaces | high | F13 per provider |
| R11 | Crashed daemon leaves detached tools running; second process on the same session | high (macOS) / low (Linux) | D32; S9 |
| R12 | Delivered approval misreported | high | D19, F21 |
| R13 | Two agents write one worktree; diff shown for the wrong cwd | high | D31; unknown cwd shown as unknown |
| R14 | Provider release cadence outpaces a solo maintainer | high | D24; contract fixtures; capability negotiation |
| R15 | Phase 1 scope exceeds solo capacity | high | sliced Phase 1; fake agents first |
| R16 | WKWebView misses B3/B5 | medium | S5; Swift shell fallback; transport independent of Tauri |
| R17 | Provider auto-update mid-session | medium | suppress where officially possible; compatibility classes |
| R18 | User mistakes a permanent provider rule (`allow_always`) for a one-time allow | medium | scope labels; widening options desktop-only |
| R19 | Node wrappers/adapters inflate memory | medium | provider footprint column; never hidden in daemon budget |
| R20 | Journal exposes secrets or proprietary code at rest | medium | sensitivity classification, redaction, retention, purge, U12 |
| R21 | Purged content survives in copies/snapshots | medium | D26 procedure; U23; documented residual |
| R22 | Unknown provider requests block turns | medium | refusal invariant |
| R23 | Escalating a cancel on a shared provider process kills other sessions' turns | medium | confirmation guard |
| R24 | Remote sessions die at logout without linger | medium | linger check in host setup |
| R25 | WebView memory growth over multi-day use | medium | watchdog reload; 72 h soak |
| R26 | False LFS diffs drive wrong decisions | medium | native LFS comparison |
| R27 | TCC grants accumulate on Shardrail for all agents | medium | document; show triggering agent; granting sensitive TCC voids protection |
| R28 | Legal ambiguity in bundling Claude packages | low (deferred, D46) | no bundling before license review; parked in 09 §5 until R47 |
| R29 | "Shardrail" name unavailable | low (deferred, D46) | U5; parked in 09 §5 until R47 |
| R30 | Pressure to scrape a TUI for a missing feature | high | S6 recorded; no feature without an official method |
| R31 | `clientInfo.name` exposure to OpenAI compliance logs | low | known to the single user; user-facing disclosure deferred (09 §5) |
| R32 | ACP v2 changes again before final; v1 agents and v2 agents coexist for a long time | medium | dual-stack (D35); fake agent speaks both; normalized model independent of wire version |
| R33 | Codex routes approvals to its automatic reviewer (`approvalsReviewer`) or relies on experimental methods, so the human never sees a request | high | D37 pinning + echo check; D36 no experimental API; S1-lite |
| R34 | Grok sandbox is weaker than assumed: default `off`, every profile writes `~/.grok/`, project `.grok/sandbox.toml` adds profiles, fails open when it cannot be applied | high | runtime confirmation, fail closed, config hash before every turn, never set bypass flags (03 §6); Grok follows Codex in P1b (D52) |
| R35 | Claude settings allow rules skip `canUseTool` silently; an omitted `permissionMode` may start in `auto` | high | explicit `permissionMode: "default"` (D37); allow rules shown as "pre-approved" (06 §3a); S4 |
| R36 | Hermes file edits never request approval | high (if Hermes included) | indicator; U26 |
| R37 | Agent-written approval text disguises a dangerous action ("safe cleanup" over `rm -rf`) | high | D38; rendering fixtures T1.10 |
| R38 | Cross-agent prompt injection through handoff bundles | medium | quoted, labelled excerpts; preview highlights instructions (04 §8) |
| R39 | Agents reuse inherited capabilities beyond `SSH_AUTH_SOCK`: OpenSSH ControlMaster sockets, keychain-backed CLI tokens (`gh`, `gcloud`) | high | S7 checks; boundary (c)/(e); documented residual (06 §3) |
| R40 | TCC responsibility lets agent descendants modify Shardrail's own app bundle | high (macOS) | S7 checks writes to `/Applications/Shardrail.app`; boundary (e); helper-bundle spawn investigated (Inferred) |
| R41 | Supply-chain compromise: unpinned npm adapter, git-installed Hermes, own dependencies, updater key | medium | 06 §12: lockfiles, `cargo-deny`, npm signature checks, adapter integrity check (kept); SBOM, signed artifacts, offline updater key deferred until other users (09 §5, R47) |
| R42 | User-configured MCP servers run outside the provider sandbox, so MCP tool calls bypass the sandbox boundary | high | MCP side-effecting calls classed `boundary-breaking` (03 §2); S7 checks inheritance |
| R43 | Codex usage terms exclude commercial/hosted use of app-server auth | low while internal (D45); high if scope changes | D45 internal use with the user's own login; R46 trigger; exclusion path remains |
| R44 | AI-written UI code renders untrusted agent text unsafely (raw HTML, unsafe links, hidden characters, injected approval wording) | high | one `untrusted-render` module with sanitization (06 §5, D22, D38); lint bans on `dangerouslySetInnerHTML`/`innerHTML`; rendering fixtures T1.10; cross-family review (08 §6, 11 §3) |
| R45 | Framework/library versions drift from what models were trained on, so agents emit outdated or hallucinated APIs | medium-high | pinned versions; `AGENTS.md` with version → llms.txt/MCP table; official MCP servers (React Aria); CI type/lint gates; per-phase freshness check (11 §3); avoid pre-1.0/RC frameworks (16 §1) |
| R46 | **Scope trigger:** Shardrail becomes commercial, hosted, or distributed to third parties outside Fork Horizon while still relying on the internal-use answers to U9/U25 | high (legal) | release gate: U9 and U25 must be reopened and answered for the new scope before any such release (D45); provider terms re-read at that time (EV-04, EV-24, EV-26) |
| R47 | **Second-user trigger:** a first user other than Daliys receives Shardrail while distribution items are still parked | medium (security/legal hygiene) | before that user gets a build: review 09 §5 (signing, updater, `NOTICE`, SBOM, signed artifacts, bug template, privacy notes, packages) and reopen U3, U14, U5, U7; if the move is also commercial/hosted/third-party, R46 applies (D46) |

## 3. Known tensions (accepted, revisit with evidence)

- **Verification vs provider choice:** a provider without an OS sandbox (Hermes likely) leaves checklist items unverified or failed while it runs; Grok writes its own config and fails open (R34), so Codex is first in P1a and Grok follows in P1b (D52). The host banner remains "approvals not protected" and the UI does not show an aggregate status (D47).
- **Re-confirmation vs mobile convenience:** re-confirmation only when stale or the workspace changed.
- **Extra processes vs budgets:** Git worker and macOS containment helper are measured in their own columns.
- **Same-model reviews:** the plan was refined by several Claude reviews; an independent review from a different model family was done on 2026-10-18 (15) and applied; spikes S7–S9 now matter more than further desk review.
- **Drain vs prompt upgrades:** drain-then-upgrade (D39) can delay an upgrade by up to `T_drain`; the user may override per blocker.
- **Fail closed vs availability:** a full disk cancels running turns (D41); availability is sacrificed for approval integrity.
- **Doc freshness:** provider protocols change weekly; each provider statement carries an access date, and providers are re-checked before each phase gate.

## 4. Unverified assumptions (spikes resolve them)

Codex project rules affecting approvals; Codex approval wait limits and concurrent turns; Codex `approvalsReviewer` echo and behaviour; whether MCP server processes inherit provider sandboxes; Grok runtime sandbox confirmation; ControlMaster/CLI-token reach; TCC bundle-write reach; ACP v2 stabilization; Claude settings protection and switch names; ACP acknowledgement behavior after allow/deny; Grok `session/resume`; Hermes sandboxing; macOS TCC attribution and helper tracking accuracy; gix behaviour with all permissions off; systemd `Restart=` × `KillMode`; WKWebView footprint; Android Web Push delivery for a self-hosted sender; all performance numbers.
