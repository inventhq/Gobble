# Contributing to Tracker

Thank you for your interest in contributing! This guide will help you get started.

## Code of Conduct

Be respectful, constructive, and inclusive. We're here to build great software together.

## Getting Started

### Prerequisites

- **Rust** (stable, latest) — [rustup.rs](https://rustup.rs)
- **Node.js ≥ 18** — for platform-api, dashboard, SDK, and beacon packages
- **Apache Iggy** — message broker ([iggy.apache.org](https://iggy.apache.org))
- **Docker** (optional) — for containerized builds and local Iggy

### Local Development Setup

```bash
# Clone the repo
git clone https://github.com/inventhq/tracker.git
cd tracker

# Copy environment template
cp .env.example .env
# Edit .env with your local settings

# Start Iggy (option A: Docker)
docker run -p 8090:8090 iggyrs/iggy

# Start Iggy (option B: from source)
cargo install iggy-server && iggy-server

# Build and run tracker-core
cargo run --bin tracker-core

# Run tests
cargo test

# Run clippy
cargo clippy --all-targets
```

### Project Structure

The project is a Cargo workspace with multiple binaries:

| Binary | Description |
|--------|-------------|
| `tracker-core` | HTTP ingestion server (Axum) |
| `event-filter` | Bot detection + rate limiting pipeline |
| `sse-gateway` | Real-time SSE event broadcast |
| `r2-archiver` | Delta Lake archiver (cold tier) |
| `risingwave-consumer` | Hot tier materializer |
| `stats-consumer` | Turso stats ledger |
| `webhook-consumer` | HTTP webhook dispatch |

Additional packages live in `packages/`:

| Package | Language | Description |
|---------|----------|-------------|
| `polars-query` | Rust | DataFusion cold tier query service |
| `polars-lite` | Rust | Polars warm tier aggregates |
| `platform-api` | TypeScript | Hono + Cloudflare Workers API |
| `app` | TypeScript | SvelteKit dashboard |
| `beacon` | JavaScript | Browser tracking beacon (t.js) |
| `sdk-typescript` | TypeScript | Server-side SDK |
| `mcp-server` | TypeScript | MCP tool server (26 tools) |

## How to Contribute

### Reporting Bugs

1. Check [existing issues](https://github.com/inventhq/tracker/issues) first
2. Include reproduction steps, expected vs actual behavior, and environment details
3. If possible, include relevant logs or error messages

### Suggesting Features

Open a [GitHub Discussion](https://github.com/inventhq/tracker/discussions) or issue with:
- The problem you're trying to solve
- Your proposed solution
- Any alternatives you considered

### Submitting Pull Requests

1. **Fork** the repo and create a branch from `main`
2. **Keep PRs focused** — one feature or fix per PR
3. **Follow existing code style** — match the patterns you see in the codebase
4. **Add tests** for new functionality where applicable
5. **Update docs** if your change affects the public API or configuration
6. **Run checks** before submitting:

```bash
cargo check --all-targets
cargo clippy --all-targets
cargo test
```

### Commit Messages

Use clear, descriptive commit messages:

```
feat(sse): add comma-separated event_type filter
fix(producer): handle Iggy reconnection on DNS change
docs: update configuration reference for r2-archiver
```

Format: `type(scope): description`

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`

## Architecture Guidelines

- **Business-agnostic core** — the platform never interprets event semantics. All domain logic belongs in `params`, not in platform code.
- **Opaque params** — treat the `params` map as a black box. Never add platform logic that depends on specific param keys.
- **Fire-and-forget ingestion** — `/t`, `/p`, `/i` endpoints must remain sub-millisecond. Never block on downstream processing.
- **Consumer independence** — each Iggy consumer runs as its own binary with its own consumer group. They must not depend on each other.
- **Tenant isolation** — all queries, events, and SSE streams must be scoped to `key_prefix`. Never leak data across tenants.

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE).
