# Agent rules

These rules follow `planning/11-modules.md` §3 and `planning/08-ui-ux.md` §6. The plan is the authority when a rule here is unclear.

- Read the relevant planning sections before changing a module. Keep changes within the requested phase and record uncovered decisions in `docs/dev/foundations-status.md`.
- Do not run providers, call model APIs, or add provider integration without explicit authorization. Never use experimental provider APIs.
- Direct dependencies are allowlisted here: Rust `serde`, `serde_json`, `schemars`, `thiserror`, `uuid`, `rusqlite`, `tokio`, `clap`, `nix`, `ed25519-dalek`, `sha2`; dev-only `tempfile`, `proptest`; TypeScript `json-schema-to-typescript`, `typescript`, `@types/node`; runtime `@noble/ed25519` pinned to `3.1.0` for the client handshake (D62). Additions require owner authorization and an update here; keep all lockfiles committed. `cargo-deny` checks the Rust dependency graph. Check official documentation for pinned dependencies before relying on APIs.
- Rust contract types are the source of truth. Export JSON Schema to `schema/`, generate TypeScript from that schema, and never hand-edit generated files.
- Approval defaults, failures, disconnects, timeouts and recovery must never produce `allow`. Journal approval intent durably before writing a provider response.
- Durable journal events are append-only. Journal errors fail closed; do not dispatch or send an approval without a durable write.
- Never use repository content as executable configuration. Provider output and agent-authored approval text are untrusted.
- Keep Rust safe (`unsafe_code = forbid`); resolve all Clippy warnings. Put protocol behavior in fixtures/fake agents rather than provider mocks.
- Do not use “protected” as a UI/session status. v1 uses a per-session verification checklist and the “approvals not protected” host banner.
- Every change must pass the repository checks below before commit. Do not commit a red tree.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo deny check
npm --prefix packages/contracts ci
npm --prefix packages/client ci
npm --prefix packages/client run typecheck
npm --prefix packages/client test
scripts/check-schema-drift.sh
```

## Dependency references

| Dependency | Version source | Documentation |
| --- | --- | --- |
| Rust toolchain | `rust-toolchain.toml` | https://doc.rust-lang.org/ |
| serde / serde_json | `Cargo.lock` | https://serde.rs/ |
| schemars | `Cargo.lock` | https://docs.rs/schemars/ |
| rusqlite | `Cargo.lock` | https://docs.rs/rusqlite/ |
| uuid | `Cargo.lock` | https://docs.rs/uuid/ |
| thiserror | `Cargo.lock` | https://docs.rs/thiserror/ |
| tokio | `Cargo.lock` | https://docs.rs/tokio/ |
| clap | `Cargo.lock` | https://docs.rs/clap/ |
| nix | `Cargo.lock` | https://docs.rs/nix/ |
| ed25519-dalek | `Cargo.lock` | https://docs.rs/ed25519-dalek/ |
| sha2 | `Cargo.lock` | https://docs.rs/sha2/ |
| tempfile / proptest | `Cargo.lock` | https://docs.rs/ |
| json-schema-to-typescript | `packages/contracts/package-lock.json` | https://github.com/bcherny/json-schema-to-typescript |
