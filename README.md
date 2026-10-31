# Shardrail_Web

Web UI (`apps/web`), WebSocket relay server (`crates/relay`), deployment configurations, and master roadmap for Shardrail.

## Repository Overview

- `apps/web`: Single Page Application built with React 19, TypeScript, Vite, Tailwind CSS, and React Aria Components. Consumes `@shardrail/contracts` and `@shardrail/client` from `Shardrail_Core`.
- `crates/relay`: Outbound WebSocket relay service built with Actix-web and Tokio for routing E2E-encrypted traffic between `shardraild` daemons and client interfaces.
- `deploy/`: Local, systemd, and reverse proxy deployment configurations.
- `PLAN.md`: The single master plan and authoritative source of truth for the entire Shardrail project across all repositories.
- `AGENTS.md`: Operational, UI, and deployment guidelines for AI coding agents.
- `docs/`: Architectural specifications and task completion records (`docs/tasks/`).
- `planning/`: Historical document archive (see `planning/ARCHIVE.md`).

## Prerequisites

- Rust 1.84.0+ (`rust-toolchain.toml`)
- Node.js 22+ & npm
- Chromium / Firefox / WebKit for browser E2E tests (`npx playwright install`)

## Building

```bash
# Build relay server (Rust)
cargo build --release -p shardrail_relay

# Build Web UI (TypeScript / Vite)
cd apps/web
npm ci
npm run build
```

## Testing

```bash
# Rust relay tests, formatting, clippy, dependency checks
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check

# UI unit tests and license check
node scripts/check-npm-licenses.mjs
cd apps/web
npm test

# Browser E2E tests (requires shardraild binary from Core)
SHARDRAILD_BIN=/path/to/shardraild npm run test:e2e
```

## Running Locally

See [`deploy/README.md`](deploy/README.md) for full instructions on running the relay server, connecting daemon instances, and serving the web client.

## Master Plan & Rules

- **Master Plan**: Consult [`PLAN.md`](PLAN.md) before starting any task.
- **Rules for Agents**: Consult [`AGENTS.md`](AGENTS.md) and §7 of [`PLAN.md`](PLAN.md).
