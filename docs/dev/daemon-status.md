# N2 daemon status

Date: 2026-10-20
Branch: `p1a/coordinator`

## Implemented

- Added the `shardraild` binary with a single startup function ordered as requested: resolve path, refuse known synced paths and network filesystem names, descriptor-relative data-dir create/open and permission check, acquire the process lock, descriptor-relative `openat(O_NOFOLLOW)` journal probe, open SQLite with `SQLITE_OPEN_NOFOLLOW`, verify epoch/host metadata, recover global approval state once, then bind the Unix listener (`0600`).
- The socket protocol negotiates protocol v1 and journal epoch, accepts a host cursor, emits durable events after resume, and supports `session.create` with a UUID command ID. Events carry journal-persisted daemon monotonic and wall-clock receipt stamps. Wire frames use tagged Rust enums and generated TypeScript contracts.
- Real-binary tests cover session creation, event delivery and fan-out, cursor replay after SIGKILL, duplicate-daemon refusal, command retries, epoch mismatch replay, and symlink refusal without modifying the target. Filesystem-name checks have unit coverage.
- Added `packages/client` as a typed stub over the generated contract types. Browser JavaScript cannot open the local Unix socket; a Tauri/native relay is still required.

## Limits and open decisions

- The checked-in decision register stops at D48. Requested D50/D51/D53/D55/D57/D60 entries and `spikes/reviews4/n2-final.md` are absent; this implementation follows the concrete task scope without inventing decision rationales.
- Authentication now uses Ed25519 keys. `authorized-clients.json` stores each registered public key, label and scope; the daemon verifies a transcript digest covering both nonces, version range/selection and both public keys, signs it with the host key and verifies the client's signature before sending events or handling commands. `shardraild client add|list|remove` manages key registration. Host and client private keys are `0600` files. Pairing UI and native client transport remain deferred.
- The fake ACP remains connected directly to the coordinator foundation, not through the daemon. Allow/deny/timeout commands and kill/restart approval tests through the socket remain unfinished.
- `packages/client` declares the generated contracts dependency; TypeScript build/typecheck scripts remain absent because a TypeScript compiler is not allowlisted.
- SQLite's public rusqlite API opens paths, not adopted descriptors. Startup performs the no-follow `openat` probe and the SQLite open itself uses `SQLITE_OPEN_NOFOLLOW`; the private directory and retained daemon lock prevent other users and cooperating daemons from swapping the file. This does not make a file-backed database tamper-proof against a malicious process running as the same UID; D20 already classifies such a host as unprotected.
- The first receipt's monotonic timestamp is relative to the first journal append after process start. Wall-clock receipts persist across restarts. Historical rows created before this change have no stored receipt stamp.

## Verification

All repository checks passed for this work: formatting, Clippy, all workspace tests, cargo-deny, npm CI install, and schema drift. Provider executables and model APIs are not invoked.

## Review fixes (2026-10-20)

- Data-dir symlinks are rejected before chmod; relative paths resolve against the process working directory. Linux uses `statfs` magic values while macOS uses filesystem type names.
- Durable event reads use bounded 16-event pages joined with receipt stamps. The journal mutex is released before socket writes; idle clients poll for committed events. Hello resumes replay history immediately, and an epoch mismatch keeps the connection open and streams a full replay from cursor zero.
- Session-create retries are deduplicated by `(client_id, command_id)` using the committed `session_created` event; create requests with unsupported preconditions are rejected. Historical coordinators are no longer opened at startup.
- Client and daemon frames are tagged enums; JSON Schema and TypeScript were regenerated. The client package now declares and imports the local contracts package.

## W4-a hardening (2026-10-20)

- Data-dir creation/open uses `mkdirat` and `openat(O_NOFOLLOW)`, then checks ownership and mode with `fstat` on the retained descriptor and applies `0700` with `fchmod`. The process uses descriptor-relative access for the lock and journal probe. On macOS it changes the daemon process working directory with `fchdir` on the checked descriptor, so SQLite and socket APIs that accept only paths resolve relative to that pinned directory; SQLite still independently uses `SQLITE_OPEN_NOFOLLOW` because rusqlite cannot adopt the already-open descriptor.
- Each connection has a 32-frame outbound queue. Overflow disconnects that client with a reconnect/resume reason in the daemon log; the slow-client integration test confirms a second authenticated client can catch up and continue.
- Journal redaction additionally detects base64-like runs of at least 32 characters with Shannon entropy ≥4 bits/character and hex runs of at least 32 characters with entropy ≥3.0. `/`, `-`, and whitespace split candidate runs, which keeps common paths, URLs, branch names, and diff headers readable; recognized Git hash contexts are also preserved. Long underscore-delimited identifiers may still be redacted, while short or low-entropy secrets may evade this best-effort heuristic. Event kind, provider reference, causation, and payload strings are redacted before persistence.
- Client registry updates use a separate advisory lock across each read-modify-write. Handshake reads time out after five seconds; pre-authentication errors are flushed before close; limited clients cannot create sessions; and a connected client can replay from an earlier cursor. Session creation still opens a new `Journal` connection and reruns idempotent schema setup; sharing the coordinator-owned connection is deferred until the gateway owns broader session commands.
