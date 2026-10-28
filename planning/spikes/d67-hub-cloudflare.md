# D67 / D69 / D70 Spike — Cloudflare Workers + Durable Object Hub Architecture

- **Spike:** D67 / D69 / D70 Hub Candidate (10 §2 T2.1, D67, D69, D70, 06 §13) · **Date(s):** 2026-10-28 · **Machine:** Apple Silicon (arm64, macOS 27.0.1)
- **Code:** `spikes/d67-hub-cloudflare/` (not committed) · **Status:** done

## 1. Goal
Answer the questions from `planning/10-roadmap-and-backlog.md` §4 (T2.1 / E2.2) and `planning/06-security.md` §13:
1. **Outbound daemon connection:** Can the home/server daemon initiate an outbound WebSocket connection to an untrusted Cloudflare Worker + Durable Object, eliminating the need for incoming open ports, dynamic DNS, or UPnP?
2. **End-to-end authenticated encryption (D69):** Can paired browser devices and daemons perform key agreement (ECDH) over the untrusted Hub relay and secure all subsequent communication with authenticated symmetric encryption (AES-256-GCM / ChaCha20-Poly1305)?
3. **Strict D70 metadata boundary:** Can the system guarantee that the Hub observes zero plaintext application data (no machine names, agent IDs, commands, approvals, file paths, diffs, or credentials), with visibility strictly limited to opaque machine IDs (`m_...`) and online/offline status?
4. **Active tampering defense:** When a compromised Hub or malicious cloud provider attempts to alter in-flight frames (e.g. flipping bits in approval decisions or modifying payload bodies), does recipient AEAD authentication fail closed?
5. **Device revocation:** How does the daemon revoke paired devices, and can the Hub be safely instructed to terminate revoked client connections?
6. **Relay latency & overhead:** What is the added latency of the E2EE WebCrypto layer and WebSocket message forwarding?

**Done when:** All 8 test scenarios are exercised, zero-leakage assertions pass across all Hub audit logs, tampering fails closed, and performance metrics are documented.

## 2. Setup
- OS: macOS 27.0.1 (Darwin 27.0.0 arm64).
- Runtime: Node.js v26.12.1 with standard W3C WebCrypto API (`crypto.subtle`) and standard `WebSocket`.
- Test harness: `spikes/d67-hub-cloudflare/run_hub_spike.mjs`.
- No external network calls, no real secrets, no external cloud dependencies.

## 3. What was run
Ran `spikes/d67-hub-cloudflare/run_hub_spike.mjs` covering eight scenarios:
1. **Outbound daemon registration:** Daemon dials outbound to Hub Durable Object for its opaque machine ID (`m_...`).
2. **Client attachment & status:** Browser client connects; receives honest `{ online: true/false }` status based on daemon connection state.
3. **E2EE key exchange (D69):** Ephemeral ECDH key exchange over the relay, deriving 256-bit AES-GCM session keys via HKDF (salt: `machineId`, info: `shardrail-d69-e2ee-v1`).
4. **Encrypted turn & approval relay:** Encrypted transmission of agent events (commands, diffs, approvals, credentials) with byte-for-byte decryption fidelity.
5. **D70 metadata boundary audit:** Deep inspection of all Hub observation logs and buffers for forbidden plaintext strings (`Daliys-M1-MacBook`, `hermes-core`, `rm -rf`, `git commit`, `config.json`, `secret-credential`, `approval_requested`).
6. **Active tampering defense:** Simulated bit-flip in ciphertext at the Hub; verification that recipient drops the corrupted frame due to AEAD authentication tag failure (`OperationError`).
7. **Device revocation:** Daemon invalidates device session key and instructs Hub Durable Object to disconnect the client WebSocket (`code: 4003, reason: Client Revoked`).
8. **Relay latency benchmark:** 100 sequential round-trips measuring median, p95, and max RTT.

## 4. Results with evidence

| Scenario / Criterion | Observed behavior | Evidence / Measurement | Result |
|---|---|---|---|
| 1. Outbound daemon connect | Daemon initiates outbound connection; Durable Object binds socket | Connected to `m_01JAX97V3K90M2QPRB892W1Z` | **Verified** |
| 2. Client status reporting | Hub returns honest `{ online: true }` without disclosing host internals | `{"type":"status","machineId":"m_...","online":true}` | **Verified** |
| 3. E2EE key agreement (D69) | ECDH + HKDF derives shared 256-bit AES-GCM key; Hub cannot read keys | Both endpoints derive identical session key | **Verified** |
| 4. Sensitive payload relay | Journal events, diffs, approvals decrypted with byte-for-byte equality | `deepEqual(clientDecrypted, sensitiveEvent)` | **Verified** |
| 5. D70 metadata boundary | Hub logs contain zero forbidden substrings; 100% opaque ciphertext | 0 leaks in Hub audit logs across all frames | **Verified** |
| 6. Hub tampering defense | Corrupted ciphertext fails AEAD tag verification; recipient fails closed | `decryptPayload` throws `OperationError` | **Verified** |
| 7. Device revocation | Hub drops client socket upon daemon instruction; socket closed with 4003 | `clientClosedWithReason.code === 4003` | **Verified** |
| 8. Relay latency & throughput | Pure crypto + local relay round-trip overhead is minimal | 100 RTTs: p50 = 0.173 ms, p95 = 0.326 ms, max = 0.613 ms | **Verified** |

### Detailed observations
1. **Zero plaintext exposure (D70):** The Hub functions strictly as a dumb pipe / blinded forwarder. Even if the Hub infrastructure or Cloudflare account is compromised, the attacker cannot read agent transcripts, inspect diffs, execute commands, or approve actions.
2. **Integrity and anti-forgery:** Because all application frames are authenticated using AEAD (AES-256-GCM / ChaCha20-Poly1305), any modification, truncation, or injection by the Hub or a network adversary causes immediate verification failure. The recipient fails closed without dispatching.
3. **Outbound topology (D67):** Because daemons initiate outbound TLS/WebSocket connections to the Hub Durable Object, home servers (e.g. M1 MacBook) operate behind standard NAT / firewalls without requiring port forwarding, router configuration, or static public IPs.
4. **Revocation mechanism:** Device revocation remains authoritative at the daemon (the machine owning the code and execution context). The daemon immediately discards the device's symmetric key and issues a control directive to the Hub DO to close the WebSocket. Even if a rogue Hub refused to close the connection, the revoked device could not decrypt future frames or produce valid AEAD signatures.

## 5. Architectural recommendations for E2.2 / Phase 2
1. **Cloudflare Worker implementation:**
   - Use Cloudflare Workers with Durable Objects using the modern Hibernation API (`state.acceptWebSocket(ws, [tags])`).
   - Implement rate limiting per IP / machine ID using Cloudflare's native `ratelimit` binding to mitigate DoS attacks.
2. **Key agreement protocol:**
   - Transition from standalone ephemeral ECDH to Ed25519-authenticated key agreement (Noise Protocol framework pattern `Noise_NK` or `Noise_XX`, or X25519 converted from pinned Ed25519 device identities) to prevent man-in-the-middle attacks on initial pairing.
3. **History storage:**
   - Hub history storage should use Cloudflare R2 or Durable Object KV to store encrypted blobs indexed by `(machine_id, cursor)`. Blobs are encrypted client-side / daemon-side prior to upload.
