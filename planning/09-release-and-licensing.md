# 09 — Release, Distribution and Licensing

> Part of the Shardrail architecture plan. Status: **candidate, not owner-approved**. Licensing and repository visibility are **owner decisions**; nothing here decides them.

## 1. Delivery targets

| Target | Form |
| --- | --- |
| macOS | Developer ID–signed, notarized app with hardened runtime (no `get-task-allow`); the shell updates through its own channel (Tauri updater while D2 holds); the daemon lives inside the bundle. Mac App Store distribution is effectively incompatible with spawning user-installed CLIs under the App Sandbox. |
| Linux | one `shardraild` binary, systemd user unit (with `KillMode=control-group`, per-provider sub-cgroups), checksums in the release; distro packages later if demanded |
| Browser | static assets from the same frontend, served by the daemon only through the opt-in loopback listener |
| Android | later phase; Play internal track or direct APK decided then |
| Providers | never inside the Shardrail installer (D9) |

Shardrail's updates never pull new Codex, Claude, Grok or Hermes versions (see 03 §8).

## 2. Licensing and terms

- Product license: owner decision (U3). Repository visibility (public/private): separate owner decision (U14). The original draft recommended "private, all rights reserved" until decided; this is recorded, not adopted.
- Dependencies: Apache-2.0/MIT crates (including `agent-client-protocol` and Tauri); no GPL/AGPL without a separate legal decision; system WebKit is not distributed by Shardrail.
- Codex (Apache-2.0) and Hermes (MIT) are not build dependencies; users run them side by side; their sources are not vendored.
- Claude: the ACP adapter is Apache-2.0; the Agent SDK is governed by Anthropic's Commercial Terms, and its documentation restricts claude.ai login in third-party products (03 §6, U9). Bundling any npm package requires a license review first; until then the adapter is installed on the user's machine like Claude Code itself.
- `NOTICE` maintained from the first dependency.

## 3. Privacy and support

- Nothing leaves the machine by default. Codex reports `clientInfo.name` to OpenAI compliance logs — disclosed.
- Public issues never contain session journals; the bug template asks for Shardrail version, provider version and redacted diagnostics.
- Secrets, `auth.json` and session databases go into `.gitignore` from the first code commit (also `.DS_Store`).

## 4. Name

The Shardrail trademark is checked before public marketing (U5); the repository name alone does not make the name free.
