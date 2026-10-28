# 18 — Browser-First Roadmap

**Owner direction D66 (2026-10-23):** Phase 1 targets the React UI in a normal browser connected to the local daemon. Tauri, mobile and other platforms are later ports of the same UI. **D67 (2026-10-24)** adds multi-machine and phone-browser access as later product scope; it does not change the current local Phase 1.

## Sequence

1. **W5-a — opt-in listener and pairing.** Add a WebSocket endpoint bound only to `127.0.0.1`, reusing the daemon wire protocol. Enrollment is distinct from authentication: the user starts it with a local daemon command that issues a cryptographically random, single-use code with at least 128 bits of entropy and expires it after five minutes; entering the code in the intended browser registers its public key at `limited` scope, after which the existing transcript-bound Ed25519 handshake authenticates it. The daemon atomically consumes codes, throttles invalid attempts and rejects wrong, expired, replayed or competing redemptions. A local daemon command is required to grant `full`. Keep the listener off until explicitly enabled.
2. **S1-lite — Codex app-server.** Run the authorized test-repository spike after W5-a; verify the real Codex thread/turn/approval/interrupt path and pinned user approval routing.
3. **S2, S5 and S9-lite.** Continue the already authorized core spikes. Reuse S5 measurements; record B4 as deferred debt, not a gate to starting the product slice.
4. **P1a browser slice.** Deliver the daemon, journal, Codex adapter and the primary inbox/session UI over the browser connection. Keep the existing honest approval banner/checklist and provider-reported diffs/files. Tauri shell work remains a later desktop port.
5. **Later ports.** Add the Tauri shell around the same React UI when desktop-specific features are needed; mobile and other platforms remain at their existing later phase gates.
6. **Later multi-machine step (D67):** run the same daemon on the M1 MacBook home server and reach it first through the existing SSH stdio bridge from the laptop/browser client. This is later than the current local Phase 1 and belongs with P2/E2.2.
7. **Later hub candidate (D67, D69, D70):** evaluate a hub/relay on Daliys's purchased server, with outbound daemon connections and phone/laptop browsers connecting to it. D69 provides end-to-end AEAD and encrypted history blobs; D70 resolves descriptive metadata confidentiality and records unavoidable connection leakage. Paired clients decrypt metadata and render the full overview on the site. The hub still needs a design/security spike, including key exchange, revocation, DoS and retention follow-ups, before commitment. Phone browser via hub is one candidate under D28 alongside PWA via mesh VPN/SSH.

## Reuse and new work

| Reuse from `spikes/wt-coordinator` | New for browser-first |
| --- | --- |
| `packages/contracts`: generated wire DTOs and frame types. | Opt-in daemon WebSocket listener that maps WebSocket messages to the existing authenticated client protocol and Unix-socket gateway. |
| `crates/shardraild`: client registry, scope enforcement, Ed25519 transcript-bound handshake (for already enrolled clients), cursor/replay and backpressure behavior. | Browser-safe WebSocket transport in `packages/client`; its current Node client imports `node:net` and `node:crypto` and is Unix-socket-only. |
| `packages/client`: v1 transcript framing, validation shape, reconnect/resume semantics and existing shared handshake vector; preserve D55/D56 scope enforcement. | Browser-safe enrollment and storage: local command-issued one-time code, limited-scope public-key registration, then the existing handshake; non-extractable WebCrypto Ed25519 in IndexedDB when supported, otherwise the disclosed `@noble/ed25519` seed-in-IndexedDB fallback. Never put credentials in URLs/query strings. |
| `apps/s5-prototype/src/pairing.ts`: pairing state transitions, expiry, mismatch and retry behavior; `main.tsx` pairing screen and error wording. | Replace Tauri `invoke` pairing adapter with browser WebSocket operations; provide a local daemon pairing/authorization flow, binding limited by default and requiring explicit local confirmation for full scope. |
| `packages/contracts` and `test-vectors/client-handshake-v1.json`: protocol and crypto interoperability baseline. | Exact Host and Origin validation before WebSocket upgrade/authentication, opt-in listener controls, browser CSP/configuration and browser-oriented tests. |

The prototype pairing screen calls Tauri commands (`pairing_info`, `pair_desktop`, `unpair_desktop`); its UI/state logic is reusable, but those commands are not a browser transport. The existing Node client is not browser-compatible. Do not copy the Tauri-only shell-minted token, WebView isolation pattern or keychain storage into the browser path.

## Acceptance criteria

- The daemon does not bind a TCP port unless the user explicitly enables the browser listener; when enabled it binds only `127.0.0.1` and reports/verifies that exact socket address. IPv6 `::1` and wildcard/dual-stack binds are unsupported.
- Accept only `Host: 127.0.0.1:<listener-port>` and `Origin: http://127.0.0.1:<listener-port>`. Reject missing or duplicate headers, malformed authorities, lookalike/suffix hosts, `Origin: null`, and mismatched ports before upgrade; rejected requests receive neither a WebSocket upgrade nor a handshake challenge. Cover DNS-rebinding Host mismatch and cross-site origins with browser-driven tests as well as raw malformed-header cases. Credentials are never accepted in URLs, query strings, referrers or logs.
- Enrollment is initiated by an explicit local command and issues a cryptographically random one-time code with at least 128 bits of entropy, expiring after five minutes. Successful redemption registers only the presented browser key at `limited` scope and then requires the existing transcript-bound handshake. Wrong, expired, replayed and simultaneous redemptions fail; invalid attempts are throttled; an unknown key remains unable to authenticate without enrollment. The ceremony pins the daemon public key, checks the displayed fingerprint, detects identity changes, and supports unpair/revoke. Only an explicit local daemon authorization can assign `full`; the daemon remains authoritative for scopes and approvals.
- Browser identity survives reload through IndexedDB, reconnect resumes from the last delivered cursor, and unpair/revoke prevents further authenticated commands. Unsupported WebCrypto Ed25519 is detected and the weaker documented fallback is clearly disclosed before pairing.
- Existing Unix-socket clients and the Tauri prototype protocol remain unaffected; transport changes preserve message DTOs, command IDs, replay and bounded-client behavior.
- The React UI remains the same SPA and works in a normal browser without a Tauri runtime. P1a acceptance covers inbox, session view, approvals and reconnect/resync.

## Tests and test window

- Run headless Playwright only during the day. Cover first-run enrollment, code entry, fingerprint confirmation, expiry, mismatch, unpair, reload/key persistence, unsupported-crypto fallback, reconnect/resume, and normal UI flows.
- Exercise the daemon listener with browser-driven WebSocket tests and raw handshake cases for successful enrollment; wrong, expired, replayed and concurrent code redemption; throttling; unknown-key rejection; missing/duplicate/malformed Host and Origin; lookalike/suffix Host; `Origin: null`; mismatched ports; DNS-rebinding Host mismatch; exact loopback bind; rejected requests receiving no upgrade/challenge; unauthenticated commands; no credential-in-URL handling; replay; revocation; and `limited`/`full` authorization boundaries.
- Reuse the shared handshake vector and existing protocol/scope tests; add no live-provider or physical-device requirement to W5-a. Use the test daemon and test keys only.

## Known limits

The default `limited` scope cannot create sessions or change trust. Phase 1 must use the explicit local full-scope authorization path for those actions. The fallback private seed in IndexedDB is extractable by same-origin script; browser XSS/CSP and untrusted-rendering controls remain critical, and the UI must disclose this fallback. No remote listener or generic cross-origin browser access is included.
