# 09 — Release, Distribution and Licensing

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Licensing and repository visibility are **owner decisions** — answered 2026-10-19 (U3/U14: private, all rights reserved for now; D46). Revised 2026-10-18 after the independent review ([15](15-independent-review.md)): Codex usage terms (U25), Claude legal-page nuance (U9), supply chain. 2026-10-19 (owner): U9/U25 answered — internal-use scope (D45); Claude via Agent SDK / `claude-agent-acp` with the user's own API key; Codex app-server with the user's own login; reopen trigger R46. Distribution-related items kept pending the owner's review (see 13). 2026-10-19 (owner): single-user scope (D46) — Daliys is the only user on his own machines until the product fully works; distribution items deferred to 09 §5 (trigger R47). U3/U14 answered.

## 1. Delivery targets

**Scope (D45, D46, 2026-10-19):** internal use only, and until the product fully works the only user is Daliys on his own machines. Targets below are what that single user needs; everything aimed at distributing Shardrail to others is parked in §5 until the trigger fires (R47).

| Target | Form |
| --- | --- |
| macOS | locally built app signed with Daliys's own local/self-signed (or Apple Development) code-signing identity — needed for code-signing peer admission (06 §4); the daemon lives inside the bundle; updates by rebuilding/reinstalling (no updater). Developer ID, notarization, distribution hardened-runtime profile, Tauri updater and the Mac App Store note: deferred (§5) |
| Linux | one `shardraild` binary built for Daliys's own hosts, systemd user unit (with `KillMode=control-group`, per-provider sub-cgroups); release checksums/signatures and distro packages deferred (§5) |
| Browser | static assets from the same frontend, served by the daemon only through the opt-in loopback listener |
| Android | later phase (D28, unchanged); sideloaded build for Daliys's own device; Play internal track deferred (§5) |
| Providers | never inside the Shardrail installer (D9) |

Shardrail's updates never pull new Codex, Claude, Grok or Hermes versions (see 03 §8).

## 2. Licensing and terms

- **Product license and visibility (owner decision 2026-10-19, U3/U14; D46):** repository private; proprietary, all rights reserved for now. Reopened together with distribution (R47); the earlier open-licensing options (FSL/BUSL, Apache-2.0) are kept in §5.
- Dependencies: Apache-2.0/MIT crates (including `agent-client-protocol` and Tauri); no GPL/AGPL without a separate legal decision; system WebKit is not distributed by Shardrail.
- Codex (Apache-2.0) and Hermes (MIT) are not build dependencies; users run them side by side; their sources are not vendored. **Codex usage terms:** Codex documentation states that app-server authentication has never been permitted for commercial or hosted services and points to "Sign in with ChatGPT" (EV-03, EV-24); for internal use (D45) Shardrail drives `codex app-server` with the user's own login (U25 answered 2026-10-19); commercial/hosted/third-party distribution reopens U25 before release (R46).
- Claude: the ACP adapter is Apache-2.0; the Agent SDK is governed by Anthropic's Commercial Terms, and its documentation restricts claude.ai login in third-party products (03 §6). Owner decision (U9, 2026-10-19; D29, D45): internal use with the user's own Anthropic API key, no claude.ai login in-app, no token brokering; commercial/hosted/third-party distribution reopens U9 before release (R46). Anthropic's legal page (EV-26) also permits end users to sign in to the unmodified Claude Code binary; that path is not used. Bundling any npm package requires a license review first; until then the adapter is installed on the user's machine like Claude Code itself.
- Kept now: supply-chain controls — committed lockfiles, `cargo-deny` (licenses, advisories, bans), npm signature/audit checks, adapter integrity recording (06 §12). Deferred (§5): `NOTICE`, SBOM, signed release artifacts, offline updater key.
- N8 (licenses and usage terms permit the intended distribution) is trivially met while the only user is Daliys (D46); copyleft dependencies still need a separate decision.

## 3. Privacy and support

- Nothing leaves the machine by default. Codex reports `clientInfo.name` to OpenAI compliance logs (known to Daliys; user-facing privacy notes deferred, §5).
- Secrets, `auth.json` and session databases go into `.gitignore` from the first code commit (also `.DS_Store`, and `/spikes/` — spike code is never committed, 10 §2).

## 4. Name

Internal name only; the trademark check (U5, R29) is deferred (§5).

## 5. Deferred until other users (trigger: first user other than Daliys)

*Owner decision 2026-10-19 (D46).* Nothing here is deleted; these items are parked, removed from the critical path and lower priority. When the trigger fires (**R47: the first user other than Daliys**), this section is reviewed and U3, U14, U5 and U7 are reopened before that user receives a build; if the move is also commercial, hosted or third-party, U9/U25 reopen too (R46).

| Item | Original plan text (kept) | Reopens |
| --- | --- | --- |
| macOS distribution signing | Developer ID–signed, notarized app with hardened runtime (no `get-task-allow`) | R47 |
| Shell updater | the shell updates through its own channel (Tauri updater while D2 holds); updater signing key kept offline with a documented rotation procedure (06 §12) | R47 |
| Mac App Store | Mac App Store distribution is effectively incompatible with spawning user-installed CLIs under the App Sandbox (still true; recorded for the future) | R47 |
| License and visibility beyond "private, all rights reserved" | U3/U14 options: source-available (FSL/BUSL converting to Apache-2.0 after 2–4 years) or Apache-2.0; open the repository together with the license | R47 → U3, U14 |
| Trademark | the Shardrail trademark is checked before public marketing; the repository name alone does not make the name free (U5, R29) | R47 → U5 |
| Provider bundling / commercial installer | providers never inside the installer (D9 stays); a commercial installer with legal review is the only exception (U7); bundling any npm package requires a license review first (R28) | R47 → U7 (and R46) |
| `NOTICE` | `NOTICE` maintained from the first dependency | R47 |
| SBOM | an SBOM per release (06 §12) | R47 |
| Signed Linux artifacts | Linux artifacts signed (minisign or Sigstore) in addition to checksums (06 §12) | R47 |
| Public bug template | public issues never contain session journals; the bug template asks for Shardrail version, provider version and redacted diagnostics | R47 |
| Privacy disclosure | user-facing privacy notes, including that Codex reports `clientInfo.name` to OpenAI compliance logs (03 §6, R31) | R47 |
| Linux distro packages | distro packages later if demanded | R47 |
| Android store track | Play internal track (vs direct APK) | R47 (mobile phase itself unchanged, D28) |

