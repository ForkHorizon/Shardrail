# Shardrail_Web — Agent Rules

These rules govern all work in `Shardrail_Web`. Shared rules across the Shardrail workspace are defined in §7 of `PLAN.md`.

## Web UI & Relay Invariants
1. **Contract Consumption**:
   - Web UI consumes `@shardrail/contracts` and `@shardrail/client` as versioned dependencies from `Shardrail_Core`.
   - Never vendor, duplicate, or hand-edit protocol contracts in this repository.
2. **Relay Blind Hub Architecture**:
   - `crates/relay` is a stateless WebSocket routing proxy for end-to-end encrypted session payloads between daemons and clients.
   - The relay must never inspect, parse, decrypt, or log E2E payload content.
   - Rust code in `crates/relay` must enforce `unsafe_code = forbid` and pass all Clippy lints with zero warnings.
3. **UI Security & Rendering**:
   - All agent output, markdown transcripts, and external inputs are untrusted.
   - Markdown must be strictly sanitized using `rehype-sanitize` before rendering. Direct HTML injection or arbitrary script execution is strictly forbidden.
   - No secrets, authorization tokens, or private keys in URLs or local storage without encryption.
4. **Fail-Closed Approval UI**:
   - Approval interactions in the UI must require explicit confirmation.
   - Reconnections, page reloads, expired nonces, or stale sessions must immediately invalidate pending approval state.
   - Never use "protected" as a session status.
5. **Accessibility & Design**:
   - All interactive UI form controls use React Aria Components.
   - Zero Axe accessibility violations across light and dark themes.
6. **Dependency Control**:
   - Dependencies must be pinned and approved.
   - Rust dependencies are checked with `cargo deny check`.
   - npm dependencies in `apps/web` are checked with `node scripts/check-npm-licenses.mjs`.

## Verification Commands

Before opening a PR or claiming task completion, all checks below must pass cleanly:

```bash
# Rust relay verification
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo deny check

# Web UI checks and unit tests
node scripts/check-npm-licenses.mjs
npm --prefix apps/web run build
npm --prefix apps/web test

# Browser E2E suite (with Core shardraild binary)
SHARDRAILD_BIN=/path/to/shardraild npm --prefix apps/web run test:e2e
```

## Shared Repository Rules

Refer to `PLAN.md` §7 for workspace rules regarding branch hygiene, commit conventions, PR checklists, and live agent testing permissions.
