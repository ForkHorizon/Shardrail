# N2 daemon status

Date: 2026-10-20  
Branch: `p1a/coordinator`

## Implemented

- Added the `shardraild` binary with a single startup function ordered as requested: resolve path, refuse known synced paths and network filesystem names, create/chmod `0700`, acquire the existing process lock, descriptor-relative `openat(O_NOFOLLOW)` journal probe, open SQLite with `SQLITE_OPEN_NOFOLLOW`, verify epoch/host metadata, recover global approval state once, then bind the Unix listener (`0600`).
- The socket protocol negotiates protocol v1 and journal epoch, accepts a host cursor, emits durable events after resume, and supports `session.create` with a UUID command ID. Events carry journal-persisted daemon monotonic and wall-clock receipt stamps. Wire frames use tagged Rust enums and generated TypeScript contracts.
- Real-binary tests cover session creation, event delivery and fan-out, cursor replay after SIGKILL, duplicate-daemon refusal, command retries, epoch mismatch replay, and symlink refusal without modifying the target. Filesystem-name checks have unit coverage.
- Added `packages/client` as a typed stub over the generated contract types. Browser JavaScript cannot open the local Unix socket; a Tauri/native relay is still required.

## Limits and open decisions

- The checked-in decision register stops at D48. The task's D50–D54 labels are not defined there, so this implementation records the concrete requested scope here rather than inventing decision rationales.
- N4's asymmetric proof is not implemented. The daemon currently accepts local Unix-socket peers that can access the `0700` directory and reports `limited`; it ignores the optional key/proof fields. `packages/client/README.md` records this boundary. A crypto dependency/key flow needs an explicit allowlist decision. Do not treat this gateway as protecting approvals from malicious same-UID agents.
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
