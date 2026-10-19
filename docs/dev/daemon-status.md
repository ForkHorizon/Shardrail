# N2 daemon status

Date: 2026-10-20  
Branch: `p1a/coordinator`

## Implemented

- Added the `shardraild` binary with a single startup function ordered as requested: resolve path, refuse known synced paths and network filesystem names, create/chmod `0700`, acquire the existing process lock, descriptor-relative `openat(O_NOFOLLOW)` journal probe, open SQLite with `SQLITE_OPEN_NOFOLLOW`, verify epoch/host metadata, recover global approval state once, open coordinators for replayed session IDs, then bind the Unix listener (`0600`).
- The socket protocol has Rust/schema-generated hello, command, event and receipt DTOs. It negotiates protocol v1 and journal epoch, accepts a host cursor, emits durable events after resume, and supports `session.create` with a UUID command ID. Events carry journal-persisted daemon monotonic and wall-clock receipt stamps.
- Added real-binary integration coverage for session creation and event delivery, restart and cursor replay after SIGKILL, duplicate daemon refusal, and journal symlink refusal without modifying the target. Filesystem-name checks have a deterministic mockable unit test.
- Added `packages/client` as a typed stub over the generated contract types. Browser JavaScript cannot open the local Unix socket; a Tauri/native relay is still required.

## Limits and open decisions

- The checked-in decision register stops at D48. The task's D50–D54 labels are not defined there, so this implementation records the concrete requested scope here rather than inventing decision rationales.
- N4's asymmetric proof is not implemented. The daemon currently accepts local Unix-socket peers that can access the `0700` directory and reports `limited`; it ignores the optional key/proof fields. `packages/client/README.md` records this boundary. A crypto dependency/key flow needs an explicit allowlist decision. Do not treat this gateway as protecting approvals from malicious same-UID agents.
- The fake ACP remains connected directly to the coordinator foundation, not through the daemon. Daemon commands currently support session creation and cursor replay; allow/deny/timeout routing, approval commands, background event fan-out, duplicate-command result retention and full reconnect streaming remain unfinished.
- SQLite's public rusqlite API opens paths, not adopted descriptors. Startup performs the no-follow `openat` probe and the SQLite open itself uses `SQLITE_OPEN_NOFOLLOW`; the private directory and retained daemon lock prevent other users and cooperating daemons from swapping the file. This does not make a file-backed database tamper-proof against a malicious process running as the same UID; D20 already classifies such a host as unprotected.
- The first receipt's monotonic timestamp is relative to the first journal append after process start. Wall-clock receipts persist across restarts. Historical rows created before this change have no stored receipt stamp.

## Verification

All repository checks passed for this work: formatting, Clippy, all workspace tests, cargo-deny, npm CI install, and schema drift. Provider executables and model APIs are not invoked.
