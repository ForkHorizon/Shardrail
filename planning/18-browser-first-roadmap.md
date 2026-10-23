# 18 — Browser-First Roadmap

**Owner direction D66 (2026-10-23):** Phase 1 targets the React UI in a normal browser connected to the local daemon. Tauri, mobile and other platforms are later ports of the same UI. This plan covers only the local loopback browser path; it does not add remote browser access.

## Sequence

1. **W5-a — opt-in listener and pairing.** Add a loopback-only WebSocket endpoint to `shardraild`, reusing the daemon wire protocol. Pair the browser through the existing transcript-bound Ed25519 handshake. Pairing defaults to `limited`; a separate explicit local daemon authorization is required to grant `full`. Keep the listener off until explicitly enabled.
2. **S1-lite — Codex app-server.** Run the authorized test-repository spike after W5-a; verify the real Codex thread/turn/approval/interrupt path and pinned user approval routing.
3. **S2, S5 and S9-lite.** Continue the already authorized core spikes. Reuse S5 measurements; record B4 as deferred debt, not a gate to starting the product slice.
4. **P1a browser slice.** Deliver the daemon, journal, Codex adapter and the primary inbox/session UI over the browser connection. Keep the existing honest approval banner/checklist and provider-reported diffs/files. Tauri shell work remains a later desktop port.
5. **Later ports.** Add the Tauri shell around the same React UI when desktop-specific features are needed; mobile and other platforms remain at their existing later phase gates.

## Reuse and new work

| Reuse from `spikes/wt-coordinator` | New for browser-first |
| --- | --- |
| `packages/contracts`: generated wire DTOs and frame types. | Opt-in daemon WebSocket listener that maps WebSocket messages to the existing authenticated client protocol and Unix-socket gateway. |
| `crates/shardraild`: client registry, scope enforcement, Ed25519 transcript-bound handshake, cursor/replay and backpressure behavior. | Browser-safe WebSocket transport in `packages/client`; its current Node client imports `node:net` and `node:crypto` and is Unix-socket-only. |
| `packages/client`: v1 transcript framing, validation shape, reconnect/resume semantics and existing shared handshake vector; preserve D55/D56 scope enforcement. | Browser pairing bootstrap and storage: non-extractable WebCrypto Ed25519 in IndexedDB when supported, otherwise the disclosed `@noble/ed25519` seed-in-IndexedDB fallback. Never put credentials in URLs/query strings. |
| `apps/s5-prototype/src/pairing.ts`: pairing state transitions, expiry, mismatch and retry behavior; `main.tsx` pairing screen and error wording. | Replace Tauri `invoke` pairing adapter with browser WebSocket operations; provide a local daemon pairing/authorization flow, binding limited by default and requiring explicit local confirmation for full scope. |
| `packages/contracts` and `test-vectors/client-handshake-v1.json`: protocol and crypto interoperability baseline. | Exact Host and Origin validation before WebSocket upgrade/authentication, opt-in listener controls, browser CSP/configuration and browser-oriented tests. |

The prototype pairing screen calls Tauri commands (`pairing_info`, `pair_desktop`, `unpair_desktop`); its UI/state logic is reusable, but those commands are not a browser transport. The existing Node client is not browser-compatible. Do not copy the Tauri-only shell-minted token, WebView isolation pattern or keychain storage into the browser path.

## Acceptance criteria

- The daemon does not bind a TCP port unless the user explicitly enables the browser listener; when enabled it binds loopback only.
- The WebSocket handshake rejects non-loopback destinations, unexpected or missing Host/Origin values, DNS-rebinding Host mismatches, and cross-site WebSocket origins before authentication. Credentials are never accepted in URLs, query strings, referrers or logs.
- Pairing pins the daemon public key, checks the displayed fingerprint, expires confirmation, detects identity changes, supports unpair/revoke, and gives browser clients `limited` scope by default. Only an explicit local daemon authorization can assign `full`; the daemon remains authoritative for scopes and approvals.
- Browser identity survives reload through IndexedDB, reconnect resumes from the last delivered cursor, and unpair/revoke prevents further authenticated commands. Unsupported WebCrypto Ed25519 is detected and the weaker documented fallback is clearly disclosed before pairing.
- Existing Unix-socket clients and the Tauri prototype protocol remain unaffected; transport changes preserve message DTOs, command IDs, replay and bounded-client behavior.
- The React UI remains the same SPA and works in a normal browser without a Tauri runtime. P1a acceptance covers inbox, session view, approvals and reconnect/resync.

## Tests and test window

- Run headless Playwright only during the day. Cover first-run pairing, fingerprint confirmation, expiry, mismatch, unpair, reload/key persistence, unsupported-crypto fallback, reconnect/resume, and normal UI flows.
- Exercise the daemon listener with browser-driven WebSocket tests and direct handshake cases for invalid Host/Origin, DNS-rebinding Host mismatch, non-loopback bind, unauthenticated commands, no credential-in-URL handling, replay, revocation, and `limited`/`full` authorization boundaries.
- Reuse the shared handshake vector and existing protocol/scope tests; add no live-provider or physical-device requirement to W5-a. Use the test daemon and test keys only.

## Known limits

The default `limited` scope cannot create sessions or change trust. Phase 1 must use the explicit local full-scope authorization path for those actions. The fallback private seed in IndexedDB is extractable by same-origin script; browser XSS/CSP and untrusted-rendering controls remain critical, and the UI must disclose this fallback. No remote listener or generic cross-origin browser access is included.
