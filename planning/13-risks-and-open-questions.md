# 13 — Risks and Open Questions

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**.

## 1. Owner decisions needed

| ID | Question | Blocks | Recommendation / interim |
| --- | --- | --- | --- |
| U1 | Which exact machine is the performance reference? | Phase 1 close | Apple Silicon, 16 GB, as a class |
| U2 | Fork Horizon brand book (colour, type, mark) | visual polish | neutral tokens (08 §4) |
| U3 | Product license | public release | undecided — owner |
| U4 | How does a phone reach the daemon (SSH, user mesh VPN, own mTLS)? | Phase 3 | Phase 2 uses SSH only |
| U5 | Trademark check for "Shardrail" | public marketing | internal name acceptable |
| U6 | Continuous agent work on the Mac with the app closed? | Phase 2 scope | not in MVP; opt-in launch agent later |
| U7 | Include provider binaries in a commercial installer? | release packaging | no |
| U8 | SolidJS or React | Phase 1a UI | decided in S5 (latency + accessibility cost) |
| **U9** | May Shardrail drive Claude via the Agent SDK/ACP adapter with users' claude.ai login? (policy/legal) | Claude in any build | API-key only until answered; Claude can be excluded |
| U10 | Default approval expiry | approval broker | deny after 180 s (or lower known provider limit) |
| U11 | Separate Unix user for agents on remote hosts | Phase 2 | recommended; mechanism (privileged spawner, containment, Git trust) must be specified first |
| U12 | Journal encryption at rest; key management on Linux | Phase 2 | off; retention and purge available |
| U13 | Owner proficiency in Rust vs Go | Phase 1 start | Rust unless the owner is markedly faster in Go |
| U14 | Repository visibility (public/private) | first public commit | undecided — owner |
| U15 | Cross-host inbox in Phase 1 or 2? | F14 scope | Phase 2, with the first remote host |
| U16 | Ship Phase 1 as browser-only (no desktop shell)? | Phase 1a | no — loses peer-identity admission, keychain, notifications; adds a TCP listener |
| U17 | Maximum approval hold and staleness threshold | approval broker | 2 h maximum; re-confirm after 15 min or workspace change |
| U18 | Orphan-daemon policy after a shell crash | daemon lifecycle | keep turns ≤ 10 min without any client, then cancel |
| U19 | Manual handoff MVP at Phase 1 close? | roadmap | yes |
| U20 | May Shardrail pre-create a protected `.codex/` in trusted workspaces? | Codex protection | ask per workspace at trust time; default no |
| U21 | Threshold for claiming "protected approvals" after S7 | product claims | all Phase 1a providers in default profile; otherwise host banner only |
| U22 | Terminate automatically when the protocol link breaks but the process lives? | recovery | user-confirmed with a client attached; automatic after grace when none |
| U23 | Exclude the data directory from Time Machine by default? | purge guarantees | yes, visible setting |
| U24 | Second session on a worktree when the provider cannot enforce read-only | write lease | refuse; offer a separate worktree |

## 2. Risks

| ID | Risk | Severity | Treatment |
| --- | --- | --- | --- |
| R1 | Codex app-server schema changes between releases (help marks it experimental) | high | pinned reference schema, CI drift test, compatibility classes, no experimental API |
| R2 | No Claude channel provides reliable two-way approval | high | S4; SDK fallback; exclusion without affecting others |
| R3 | Claude SDK terms restrict subscription login in third-party products | high (product) | D29, U9 |
| R4 | Agents on the same UID tamper with Shardrail state, impersonate the daemon, or self-approve | high | D20 host protection; D21; S7; U11 |
| R5 | Few provider profiles satisfy the protection boundary, weakening the product's central safety claim | high (product) | pre-registered claim rule (U21); honest labelling |
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
| R28 | Legal ambiguity in bundling Claude packages | medium | no bundling before license review |
| R29 | "Shardrail" name unavailable | medium | U5 |
| R30 | Pressure to scrape a TUI for a missing feature | high | S6 recorded; no feature without an official method |
| R31 | `clientInfo.name` exposure to OpenAI compliance logs | low | disclose |

## 3. Known tensions (accepted, revisit with evidence)

- **Protection vs provider choice:** a provider without an OS sandbox (Hermes likely) makes the host unprotected while it runs; prefer a sandboxed provider for Phase 1a.
- **Re-confirmation vs mobile convenience:** re-confirmation only when stale or the workspace changed.
- **Extra processes vs budgets:** Git worker and macOS containment helper are measured in their own columns.
- **Same-model reviews:** the plan was refined by several Claude reviews; the next review should come from a different model family, and spikes S7–S9 matter more than further desk review.

## 4. Unverified assumptions (spikes resolve them)

Codex project rules affecting approvals; Codex approval wait limits and concurrent turns; Claude settings protection and switch names; ACP acknowledgement behavior after allow/deny; Grok `session/resume`; Hermes sandboxing; macOS TCC attribution and helper tracking accuracy; gix behaviour with all permissions off; systemd `Restart=` × `KillMode`; WKWebView footprint; Android Web Push delivery for a self-hosted sender; all performance numbers.
