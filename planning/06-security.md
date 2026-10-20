# 06 — Security and Trust
> 2026-10-20 (owner): D49–D54 recorded — React retained on partial S5 evidence; journal opens atomically without following symlinks; daemon refuses network/synced data filesystems; Codex is first in P1a; tokio/clap/nix are allowed daemon dependencies; contracts remain generated with client helpers in `packages/client`.

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Security claims are qualified; none is verified by testing yet — spikes S7–S9 do that. 2026-10-19 (owner, D46): code-signing peer admission verifies Daliys's own local/self-signed identity (no Developer ID); release-signing items deferred to 09 §5. 2026-10-19 (owner): U21 answered — v1 never shows a 'protected' status; each session shows a verification checklist (D47); S7 gates any future 'protected' status, which needs a separate decision. §3b added. 2026-10-19 (owner): remaining owner questions answered in bulk (17); Hermes not in Phase 1 (D48); core spikes S1-lite, S2, S5, S9-lite authorized.
> Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): provider approval-bypass paths (§3a, D37), agent-authored approval text (§5, D38), macOS containment technique (§8, D43), extra inherited capabilities, supply chain (§12), Grok caveats.

## 1. Threat model

Sources of attack on a machine: untrusted repositories (prompt injection, hooks, Git configuration), model output, MCP servers, project-level provider configuration, other processes of the same user — **including the supervised agents themselves**. External network does not reach the daemon (no listening network port). A phone is treated as a device that can be stolen.

The core safety invariant (N9): **no supervised agent action that needs approval proceeds without an explicit decision by an authorized human, and Shardrail never misreports that decision.**

## 2. Trust boundaries

| ID | Trusted side | Untrusted side | Enforcement |
| --- | --- | --- | --- |
| TB-1 | daemon | provider output, model text, tool output | schema validation, size caps, never executed |
| TB-2 | daemon data | content inside events, as rendered in clients | untrusted-rendering policy (§5) |
| TB-3 | authenticated client with scope | any other process, including agents | client identity (§4), approval protection (§3) |
| TB-4 | Shardrail code | repository content, `.git/config`, attributes, environment | safe Git (§6), workspace trust (§7) |
| TB-5 | SSH-authenticated bridge | network | OpenSSH; daemon never listens on a network interface |
| TB-6 | — | each provider's environment and credentials | separate environments; separate Unix user optional (U11, deferred to Phase 2) |

## 3. Approval protection against the agents (D20)

**Problem.** Agents run as the same Unix user as the daemon. File modes and UID checks do not stop them. An agent able to write in the user's home can do more than steal a secret: edit Shardrail's database (trust, policy, client keys), replace binaries, pose as the daemon on its socket path, or edit the **provider's own approval configuration** so the provider never asks. *(Earlier designs protected only secrets, then labelled sessions individually; reviews showed both were insufficient.)*

**Unit.** Protection is a **host × UID** state: `protected` or `unprotected` — internal values only; in v1 the UI never displays `protected` (D47), only `not protected` with causes and the per-session checklist (§3b). It is `protected` only while every live Shardrail-launched process on that host and UID runs under a profile that satisfies the boundary below, and no boundary-breaking approval has been allowed since protection was last established.

**Boundary** (verified per provider × sandbox profile in spike S7):
- (a) no write to Shardrail's data directory;
- (b) no write to Shardrail's binaries or to any file of a provider's install tree or interpreter that Shardrail launches;
- (c) no bind/connect to Shardrail's socket directory and no read of file-based Shardrail identity material;
- (d) no write to files the provider reads to decide whether to ask for approval or what to run — user level (`~/.codex/config.toml`, `~/.claude/settings*.json`, `~/.hermes/config.yaml`, Grok config) and project level (`.codex/`, `.claude/settings*.json`, equivalents) — **or** those settings are pinned at launch through documented flags/SDK options and verified by a hash snapshot (hashes only, never content) at session start, **before every turn** and at session end (a mismatch blocks dispatch and is shown with the changed path); for Grok this includes `~/.grok/` rules/sandbox profiles and the project `.grok/sandbox.toml`, which every Grok profile can write (EV-27);
- (e) writes confined to the session's writable roots and temp, so no persistence paths (LaunchAgents, shell rc files, PATH directories, `~/.ssh/config`, `~/.gitconfig`, crontab, the app bundle, WebKit/Tauri storage).

**Sticky loss.** Once `unprotected`, the host stays so until the user explicitly re-establishes protection: all Shardrail-launched processes stopped, daemon restarted, binaries and data integrity verified. (Later on macOS: a keychain-held MAC with a monotonic counter against rollback. On a shared-UID Linux host integrity cannot be verified — documented.)

**Inherited capabilities.** Provider environments drop `SSH_AUTH_SOCK` and GPG agent sockets by default (an opt-in per session makes the host `unprotected`); OpenSSH **ControlMaster sockets** (`ControlPath`, often under `~/.ssh/`) and keychain-backed CLI tokens (`gh`, `gcloud`, cloud CLIs) are reachable by same-UID agents unless the provider sandbox denies them — S7 tests both and boundary (c)/(e) covers them (R39); **TCC responsibility:** agents spawned by Shardrail are attributed to Shardrail, so they may inherit Shardrail's right to modify its own app bundle under App Management — boundary (e) must be verified for `/Applications/Shardrail.app` specifically (Inferred, R40, S7); deep links never carry approval, policy or pairing actions; granting Shardrail Accessibility, Automation or Screen Recording voids protection (shown); hardened runtime without `get-task-allow`; Linux `ptrace_scope` shown in diagnostics. Defense in depth: environment scrubbing of all Shardrail variables, rejection of socket peers descending from provider process trees (heuristic only — defeated by `setsid`), provider binary path and code-signing identity shown and change-flagged.

**Runtime confirmation.** A sandbox profile counts toward `protected` only if its application is confirmed at runtime, not inferred from the flag passed (Grok fails open, EV-27).

### 3a. Provider approval-bypass paths (D37)

Paths through which an action needing human approval can proceed without reaching Shardrail. Each is mitigated or shown; none is silently accepted.

| Provider | Bypass path | Mitigation |
| --- | --- | --- |
| Codex | `approvalsReviewer` routes approvals to the automatic reviewer (`auto_review`) | send `approvalsReviewer: "user"` on every thread/turn request; verify in responses; mismatch → session `degraded`, host `unprotected` |
| Codex | `tool/requestUserInput` with `autoResolutionMs`; app tool approvals via user input | treat as approval records; ignore auto-resolution |
| Codex | execpolicy rules / `acceptForSession` / `acceptWithExecpolicyAmendment` | classed `widening`, desktop-only (D19); rules files covered by boundary (d) |
| Codex | network approval grouping (one answer unblocks several requests) | classed `widening`; card states "applies to all pending requests to host:port" |
| Claude | settings-file allow rules skip `canUseTool` silently | `settingSources: []` in `unknown`/`restricted`; in `trusted`, effective allow rules shown per session as "pre-approved by Claude settings" |
| Claude | omitted `permissionMode` may start in `auto` (classifier) | always pass `permissionMode: "default"`; mode changes are policy events |
| Grok | `--always-approve`, `_meta.yoloMode`, rules in `~/.grok/` writable from every profile | never set; hash snapshot before every turn (boundary d) |
| Grok | sandbox fails open when it cannot be applied | runtime confirmation or refuse to start |
| Hermes | file edits never request approval; `allow_always` permanent allowlist; hosts may auto-answer | persistent "file edits without approval" indicator; `allow_always` desktop-only + labelled permanent; Hermes not in Phase 1 (U26, D48) — if added, the indicator is a permanent banner |
| ACP (all) | agent-initiated mode/config change (`config_option_update`, `current_mode_update`, tool kind `switch_mode`) | journaled policy event; a change that widens permissions blocks further dispatch until a `full` client acknowledges, or makes the host `unprotected` if the provider gives no way to hold |
| All | user-configured MCP servers usually run outside the provider sandbox | side-effecting MCP tool calls classed `boundary-breaking` (R42); S7 checks whether MCP processes inherit the sandbox |

**Scope statement (UI and docs).** Every checklist item (§3b), and any future protection status, covers only Shardrail's approval decisions on that host and UID. It does not cover processes outside Shardrail (the user's terminals, IDEs, other agents) or code the user later runs from the workspace (`.envrc`, `package.json` scripts, editor tasks).

**Display rule (owner decision 2026-10-19, U21; D47).** v1 never shows a "protected" host or approval status and makes no "protected approvals" claim. The host banner says "approvals not protected" with causes; each session shows the verification checklist below. Starting a session whose checklist has a failed item requires an explicit acknowledgement, once per host × provider, like allow-all. S7 yields a verified / not verified / failed result per provider × profile for each S7-dependent item. A "protected" status may be introduced only after S7 shows these checks are reliable, and then only by a **separate future owner decision**; it is not part of this plan.

### 3b. Per-session verification checklist (D47)

Each item shows `verified`, `not verified`, `failed` or `n/a`, its source (runtime check or S7 result for this provider × profile, with S7 version) and when it was last checked. Items are never added up into a single "protected" verdict. Failed and unverified items sort first.

| Item | Derived from | How verified |
| --- | --- | --- |
| Sandbox profile applied | §3 runtime confirmation; Grok fail-open (§3a) | confirmed at runtime, not inferred from the flag; failure → refuse to start or `failed` |
| Bypass / yolo modes off | §3a: Codex `danger-full-access` + `approval never`, Grok `--always-approve` / `_meta.yoloMode`, Claude `permissionMode` | launch parameters plus provider responses |
| Approval routing pinned to Shardrail | §3a: Codex `approvalsReviewer: "user"`, Claude `settingSources: []` (or listed "pre-approved by Claude settings" rules), auto-resolution ignored | checked on every thread/turn response; mismatch → `failed` + session `degraded` |
| Provider approval config unchanged | §3 boundary (d) | hash snapshot at session start, before every turn, at session end |
| No unacknowledged permission-widening mode/config change | §3a ACP mode changes | journaled policy events |
| Process containment active | 04/05 supervisor: process group / cgroup, descendant snapshot; macOS helper when present (D43) | runtime |
| Agent credential sockets dropped | §3 inherited capabilities (`SSH_AUTH_SOCK`, GPG agent) | environment at spawn; per-session opt-in → `failed` |
| Writes confined to writable roots (no write to Shardrail data, binaries, provider install trees, persistence paths) | §3 boundaries (a), (b), (e) | S7 result for provider × profile; `not verified` until S7 |
| Shardrail socket and identity unreachable; ControlMaster / CLI tokens unreachable | §3 boundary (c), R39 | S7 result; `not verified` until S7 |
| MCP servers inside the sandbox | §3a (R42) | S7 result; otherwise `not verified` and side-effecting MCP calls stay `boundary-breaking` |
| No boundary-breaking approval allowed on this host since last re-establishment | §3 sticky loss, D20 | approval journal |
| Known provider gaps | §3a (e.g. Hermes file edits without approval) | static per provider; always shown |


## 4. Client identity and scopes (D21)

- Clients hold **asymmetric** keys (desktop: keychain); the daemon stores public keys only. The daemon's identity key is in the macOS keychain; on Linux it is a `0600` file readable by the same UID (documented weakness, one more reason for U11).
- Handshake: mutual and bound to the transcript (a Noise-style pattern, chosen in detailed planning); the remote daemon's key is pinned end to end through the SSH bridge in addition to SSH host authentication (*why:* the SSH host key authenticates the machine, not the daemon behind the socket).
- Scopes: **`full`** — local desktop admitted by peer UID + code-signing identity of the peer process, or a desktop presenting its keychain key over the SSH bridge; **`limited`** — SSH bridge alone, and paired browser/mobile clients: observe, prompt, answer approvals with once/deny only.
- Pairing (one-time code shown in a `full` client) applies only to `limited` clients.
- Policy-weakening actions (trust changes, sandbox weakening, allow-all) and `widening`/`boundary-breaking` approval options require `full`.
- Developer builds use a build-time admission constant and always report the host as `unprotected`.
- Phase 1a has a single local desktop client admitted by peer UID plus a shell-minted token (dev builds are `unprotected` anyway); **code-signing peer admission moves to Phase 1e hardening** (D40). Under the single-user scope (D46) it verifies Daliys's own local/self-signed (or Apple Development) signing identity — the daemon pins that certificate/designated requirement at install — instead of requiring Developer ID; unsigned dev builds stay `unprotected`. Asymmetric pairing and the mutual handshake arrive with remote/browser clients in Phase 2.

## 5. Untrusted content rendering (D22)

All provider, tool and file content is untrusted: allowlisted Markdown subset, no raw HTML, strict CSP with no remote origins (images only from a local cache after explicit user action — prevents Markdown-image data exfiltration), ANSI through a safe parser, size caps with incremental parsing. Approval cards are built only from structured protocol fields (never from model text claiming "I already asked"), show the exact command/argv or paths, cwd, both sandbox and approval axes, and option scope labels, and make control, bidi and zero-width characters visible, with a raw-bytes view.

**Agent-authored text is untrusted even inside protocol fields (D38).** ACP `title`/`description`, Codex `reason`, Hermes `description` and similar fields are written by the model and can be steered by prompt injection ("Run tests (safe)" over `curl … | sh`). The card's primary area shows only machine-checkable data: argv / `rawInput` / `command`, `cwd`, paths from `locations`/`changes`, network host:port, option class. Explanatory fields are rendered secondary, in a box labelled "agent's description". If the provider does not disclose the action (ACP v2 request without `subject`, Codex network approval without a meaningful command), the card says "action not disclosed by provider" and allow requires a second confirmation.

**WebView hardening (Tauri).** Tauri isolation pattern enabled; Tauri 2 capabilities grant only the IPC commands the UI needs (no shell, fs, http or opener plugins); untrusted Markdown renders in a sandboxed frame without IPC access. Script injection into the WebView equals token theft (D16), so these are prerequisites for Phase 1a, not polish.

## 6. Safe Git (D23)

**Rule:** Shardrail executes no program named by any Git configuration scope, attribute or environment variable, and performs only local read operations.

*Why:* `core.fsmonitor` in `.git/config` names a command run by `git status`; filter drivers, diff drivers, pagers, editors, credential helpers and SSH commands are all configurable programs; an agent in a trusted workspace can usually write `.git/config` — so a naive Git view becomes a way for a sandboxed agent to run code outside its sandbox (EV-07, EV-14…EV-17).

- Library-based (gix) with permissions `system=false, git=false, user=false, env=false, includes=false, git_binary=false` and an empty filter-driver list; the daemon strips `GIT_*` from its own environment; no Git CLI fallback.
- Discovery bounded to the workspace root (no upward discovery); `gitdir:` files, `core.worktree` or worktree links pointing outside the workspace are flagged and not followed; submodules not recursed in Phase 1.
- Symlinks out of the workspace are not followed (target shown as text); files with link count > 1 are flagged.
- LFS compared natively (SHA-256 + size → pointer); other filters → stat-only, marked "unverified"; never "modified" from an unfiltered comparison.
- Git work runs in a worker process with exec denied by the OS (macOS Seatbelt, Linux seccomp/Landlock), read-only workspace, no network; CI fails if the worker ever calls `execve` on the malicious-repo fixture set.

## 7. Workspace trust and write lease

- Trust states `unknown`, `restricted`, `trusted` (F13). Project-level executable configuration loads only in `trusted`. A per-provider table of project-config execution surfaces is completed by spikes S1–S4.
- New workspaces never start in bypass/full-access modes (D14). Allow-all modes exist in all four providers, are never initial, require an explicit action and show a persistent indicator.
- **Write lease (D31):** one write-capable session per worktree by default; a second session starts read-only or gets a separate (provider-native) worktree; if the provider cannot enforce read-only, the second session is refused and a separate worktree offered (U24, owner 2026-10-19). The lease is journaled and released only when the holder's process has exited and no tracked descendant is alive.

## 8. Containment (D32)

All descendants of a provider are contained, including those that change session or process group. **Linux:** one cgroup per provider, killed with `cgroup.kill` — a guarantee; the daemon refuses to start providers outside a cgroup it controls or marks containment best effort; PR_SET_PDEATHSIG only as secondary (set from one dedicated spawner thread, parent re-checked). **macOS:** best effort — a helper is the provider's parent, started first, tracks descendants and kills them when the daemon goes away; residual risk is shown. **Mechanism (D43, EV-28):** kqueue `NOTE_TRACK`/`NOTE_CHILD` return `ENOTSUP` since macOS 10.5 and coalitions/responsibility APIs need entitlements, so the helper combines (1) a dedicated process group (`killpg`), (2) `EVFILT_PROC` `NOTE_FORK|NOTE_EXIT` on every known descendant with a `p_puniqueid` scan to link reparented children, and (3) an inherited **sentinel file descriptor** found with `proc_listpidspath` to catch `setsid`/double-fork escapees, with `p_uniqueid` checks before signalling to avoid recycled pids. A descendant that closes all inherited fds (`closefrom`) *and* escapes before its parent is recorded remains a residual. S9 tests `fork → exit → setsid → closefrom → sleep`. Invariant I2 (05 §8) is best effort on macOS and the residual is always shown. A daemon upgrade is a planned restart (D39), not an occasion for the helper to kill running turns. Containment is crash safety, not an adversarial control. Records include pid, start time and boot id; Linux uses pidfds; spawn intent is journaled before spawning.

## 9. Approvals (D19)

Records resolve exactly once (05 §4). Expiry (default deny after 180 s or a lower known provider limit; opt-in hold up to an owner-set maximum, proposed 2 h) maps to the provider's reject option, or cancels the turn if none is offered. Allows older than 15 minutes, or after the workspace HEAD/diff changed, need re-confirmation. Delivered decisions are never shown as withdrawn. Boundary-breaking allows end host protection.

## 10. Secrets, logs and telemetry

- Shardrail has no secret manager for providers; login uses provider commands (`codex login`, `grok login`, `hermes model`, Claude's flow). "Not logged in" is detected from protocol errors; reading `auth.json` to check is forbidden.
- The journal is sensitive (N3): `0600` file, `0700` data dir, best-effort redaction (prefix denylist + format/entropy detectors) applied to journal, logs, diagnostics export and handoff; retention and purge (04 §6).
- **Journal open (D50):** the daemon opens or creates the journal atomically relative to the already-open data-directory descriptor, without following symlinks. This ships with the daemon skeleton before real providers are connected; agents are untrusted with respect to the journal even though they share the daemon's UID.
- Daemon logs record structured spans (`provider.spawn`, `provider.event`, `journal.append`, `client.flush`, `approval.wait`, `approval.respond`, `command.accept`, `command.dispatch`, `gateway.fanout`); prompt bodies and tool output are not logged by default; a per-session debug flag exists.
- Nothing leaves the machine by default; diagnostics export is a user-saved file with the same redaction. No external APM.

## 11. Residual risks (stated, not hidden)

Unsandboxed agents on a shared UID can act as the user; macOS containment is best effort (§8 residual); macOS network deny for Grok profiles is a no-op; Grok writes its own config from every profile; provider-internal auto-approvals that §3a cannot pin remain invisible to Shardrail; Hermes file edits are unapproved; handoff can carry a secret the human pastes; secrets in tool output may persist until purge; APFS snapshots may retain purged data.

## 12. Supply chain

- Shardrail dependencies (kept): committed lockfiles; `cargo-deny` (licenses, advisories, bans) and npm signature/audit checks in CI.
- Deferred until other users (09 §5, R47): SBOM per release, `NOTICE`, macOS notarization, signed Linux artifacts, offline Tauri updater key.
- User-installed provider pieces that Shardrail launches (Claude ACP adapter from npm, Hermes git install, Codex npm wrapper): Shardrail records path, version and, where available, package integrity hash at spawn, shows changes, and feeds them to the compatibility fingerprint (03 §3). It never installs or updates them (D9). Bundling any of them still requires a license review (R28, deferred with distribution; R41).
