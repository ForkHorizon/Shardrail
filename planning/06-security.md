# 06 — Security and Trust

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Security claims are qualified; none is verified by testing yet — spikes S7–S9 do that.

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
| TB-6 | — | each provider's environment and credentials | separate environments; separate Unix user optional (U11) |

## 3. Approval protection against the agents (D20)

**Problem.** Agents run as the same Unix user as the daemon. File modes and UID checks do not stop them. An agent able to write in the user's home can do more than steal a secret: edit Shardrail's database (trust, policy, client keys), replace binaries, pose as the daemon on its socket path, or edit the **provider's own approval configuration** so the provider never asks. *(Earlier designs protected only secrets, then labelled sessions individually; reviews showed both were insufficient.)*

**Unit.** Protection is a **host × UID** state: `protected` or `unprotected`. It is `protected` only while every live Shardrail-launched process on that host and UID runs under a profile that satisfies the boundary below, and no boundary-breaking approval has been allowed since protection was last established.

**Boundary** (verified per provider × sandbox profile in spike S7):
- (a) no write to Shardrail's data directory;
- (b) no write to Shardrail's binaries or to any file of a provider's install tree or interpreter that Shardrail launches;
- (c) no bind/connect to Shardrail's socket directory and no read of file-based Shardrail identity material;
- (d) no write to files the provider reads to decide whether to ask for approval or what to run — user level (`~/.codex/config.toml`, `~/.claude/settings*.json`, `~/.hermes/config.yaml`, Grok config) and project level (`.codex/`, `.claude/settings*.json`, equivalents) — **or** those settings are pinned at launch through documented flags/SDK options and verified by a hash snapshot (hashes only, never content) at session start and end;
- (e) writes confined to the session's writable roots and temp, so no persistence paths (LaunchAgents, shell rc files, PATH directories, `~/.ssh/config`, `~/.gitconfig`, crontab, the app bundle, WebKit/Tauri storage).

**Sticky loss.** Once `unprotected`, the host stays so until the user explicitly re-establishes protection: all Shardrail-launched processes stopped, daemon restarted, binaries and data integrity verified. (Later on macOS: a keychain-held MAC with a monotonic counter against rollback. On a shared-UID Linux host integrity cannot be verified — documented.)

**Inherited capabilities.** Provider environments drop `SSH_AUTH_SOCK` and GPG agent sockets by default (an opt-in per session makes the host `unprotected`); deep links never carry approval, policy or pairing actions; granting Shardrail Accessibility, Automation or Screen Recording voids protection (shown); hardened runtime without `get-task-allow`; Linux `ptrace_scope` shown in diagnostics. Defense in depth: environment scrubbing of all Shardrail variables, rejection of socket peers descending from provider process trees (heuristic only — defeated by `setsid`), provider binary path and code-signing identity shown and change-flagged.

**Scope statement (UI and docs).** The claim covers Shardrail's approval decisions while the host is `protected`. It does not cover processes outside Shardrail (the user's terminals, IDEs, other agents) or code the user later runs from the workspace (`.envrc`, `package.json` scripts, editor tasks).

**Pre-registered decision rule.** For each Phase 1a provider × profile, S7 yields protected / not. If fewer than the U21 threshold (recommended: all Phase 1a providers in their default Shardrail profile) can reach `protected`, the product does not market "protected approvals"; the indicator becomes a host-level banner with one acknowledgement per host × provider. Starting a session that makes the host unprotected otherwise requires an explicit acknowledgement, like allow-all.

## 4. Client identity and scopes (D21)

- Clients hold **asymmetric** keys (desktop: keychain); the daemon stores public keys only. The daemon's identity key is in the macOS keychain; on Linux it is a `0600` file readable by the same UID (documented weakness, one more reason for U11).
- Handshake: mutual and bound to the transcript (a Noise-style pattern, chosen in detailed planning); the remote daemon's key is pinned end to end through the SSH bridge in addition to SSH host authentication (*why:* the SSH host key authenticates the machine, not the daemon behind the socket).
- Scopes: **`full`** — local desktop admitted by peer UID + code-signing identity of the peer process, or a desktop presenting its keychain key over the SSH bridge; **`limited`** — SSH bridge alone, and paired browser/mobile clients: observe, prompt, answer approvals with once/deny only.
- Pairing (one-time code shown in a `full` client) applies only to `limited` clients.
- Policy-weakening actions (trust changes, sandbox weakening, allow-all) and `widening`/`boundary-breaking` approval options require `full`.
- Developer builds use a build-time admission constant and always report the host as `unprotected`.
- Phase 1a has a single local desktop client; asymmetric pairing and the mutual handshake arrive with remote/browser clients in Phase 2.

## 5. Untrusted content rendering (D22)

All provider, tool and file content is untrusted: allowlisted Markdown subset, no raw HTML, strict CSP with no remote origins (images only from a local cache after explicit user action — prevents Markdown-image data exfiltration), ANSI through a safe parser, size caps with incremental parsing. Approval cards are built only from structured protocol fields (never from model text claiming "I already asked"), show the exact command/argv or paths, cwd, both sandbox and approval axes, and option scope labels, and make control, bidi and zero-width characters visible, with a raw-bytes view.

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
- **Write lease (D31):** one write-capable session per worktree by default; a second session starts read-only or gets a separate (provider-native) worktree; if the provider cannot enforce read-only, the second session is refused (U24). The lease is journaled and released only when the holder's process has exited and no tracked descendant is alive.

## 8. Containment (D32)

All descendants of a provider are contained, including those that change session or process group. **Linux:** one cgroup per provider, killed with `cgroup.kill` — a guarantee; the daemon refuses to start providers outside a cgroup it controls or marks containment best effort; PR_SET_PDEATHSIG only as secondary (set from one dedicated spawner thread, parent re-checked). **macOS:** best effort — a helper is the provider's parent, started first, tracks descendants and kills them when the daemon goes away; residual risk is shown. Containment is crash safety, not an adversarial control. Records include pid, start time and boot id; Linux uses pidfds; spawn intent is journaled before spawning.

## 9. Approvals (D19)

Records resolve exactly once (05 §4). Expiry (default deny after 180 s or a lower known provider limit; opt-in hold up to an owner-set maximum, proposed 2 h) maps to the provider's reject option, or cancels the turn if none is offered. Allows older than 15 minutes, or after the workspace HEAD/diff changed, need re-confirmation. Delivered decisions are never shown as withdrawn. Boundary-breaking allows end host protection.

## 10. Secrets, logs and telemetry

- Shardrail has no secret manager for providers; login uses provider commands (`codex login`, `grok login`, `hermes model`, Claude's flow). "Not logged in" is detected from protocol errors; reading `auth.json` to check is forbidden.
- The journal is sensitive (N3): `0600` file, `0700` data dir, best-effort redaction (prefix denylist + format/entropy detectors) applied to journal, logs, diagnostics export and handoff; retention and purge (04 §6).
- Daemon logs record structured spans (`provider.spawn`, `provider.event`, `journal.append`, `client.flush`, `approval.wait`, `approval.respond`, `command.accept`, `command.dispatch`, `gateway.fanout`); prompt bodies and tool output are not logged by default; a per-session debug flag exists.
- Nothing leaves the machine by default; diagnostics export is a user-saved file with the same redaction. No external APM.

## 11. Residual risks (stated, not hidden)

Unsandboxed agents on a shared UID can act as the user; macOS containment is best effort; macOS network deny for Grok profiles is a no-op; provider-internal auto-approvals are invisible to Shardrail; handoff can carry a secret the human pastes; secrets in tool output may persist until purge; APFS snapshots may retain purged data.
