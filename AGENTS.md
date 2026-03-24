# AGENTS.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

## Build & Development Commands

### Rust (tracker-core + consumers)

```bash
# Build all binaries
cargo build

# Build release (required for benchmarks)
cargo build --release

# Run a specific binary
cargo run --bin tracker-core
cargo run --bin event-filter
cargo run --bin sse-gateway
cargo run --bin risingwave-consumer
cargo run --bin r2-archiver
cargo run --bin stats-consumer
cargo run --bin webhook-consumer

# Tests and linting
cargo test
cargo clippy --all-targets
cargo check --all-targets
```

### Separate Rust crates (packages/)

polars-query, polars-lite, and ai-query are independent Cargo projects (not workspace members of the root). Build them from their own directories:

```bash
cargo build --manifest-path packages/polars-query/Cargo.toml
cargo build --manifest-path packages/polars-lite/Cargo.toml
cargo build --manifest-path packages/ai-query/Cargo.toml
cargo test --manifest-path packages/aggregate-schema/Cargo.toml
```

### Platform API (Hono + Cloudflare Workers)

```bash
cd packages/platform-api && npx wrangler dev        # local dev server
cd packages/platform-api && npx wrangler deploy      # deploy to CF
cd packages/platform-api && node scripts/migrate.js  # run DB migrations
```

### Dashboard (SvelteKit 5)

```bash
cd packages/app && npm run dev       # dev server
cd packages/app && npm run build     # production build
cd packages/app && npm run check     # svelte-check + TypeScript
```

### Local Iggy (required for tracker-core with persistence)

```bash
docker compose -f docker-compose.local.yml up  # Iggy on port 8090
```

Without Iggy, tracker-core starts in NOOP mode (events counted but not persisted) and auto-reconnects every 30s.

## Architecture

### Core Data Flow

All HTTP events flow through two Iggy topics:

1. **`events` topic** — raw events from `/t`, `/p`, `/i`, `/batch`, `/t/auto`
2. **`events-clean` topic** — filtered events (bot/rate-limit cleaned) + `/ingest` events (pre-authenticated, bypass filter)

The `event-filter` binary reads from `events`, applies bot detection (UA pattern matching), Count-Min Sketch IP rate limiting, and per-tenant custom rules (loaded from Turso), then writes to `events-clean`. All downstream consumers (`risingwave-consumer`, `r2-archiver`, `sse-gateway`, `webhook-consumer`, `stats-consumer`) read from `events-clean`.

### 3-Tier Query Architecture

- **Hot** (RisingWave): `risingwave-consumer` → materialized views, <2s latency, last 7 days
- **Warm** (polars-lite): pre-computed hourly aggregate Parquet files on R2, read by `packages/polars-lite`
- **Cold** (polars-query): DataFusion over Delta Lake on R2, full event history via `packages/polars-query`

The `r2-archiver` writes both raw events (Delta Lake) and hourly aggregates (Hive-partitioned Parquet) to R2. The `aggregate-schema` crate (`packages/aggregate-schema`) is a zero-dep shared crate that defines column names and R2 path conventions for warm tier files — used by both `r2-archiver` and `polars-lite` to prevent schema drift.

### Tenant Isolation

All data is scoped by `key_prefix` (tenant identifier). It appears in `params.key_prefix` on every event and is used for:
- Iggy partition routing (consistent hashing by key_prefix)
- Per-tenant HMAC secrets / encryption keys (loaded from Platform API, cached in `TenantCache`)
- Per-tenant rate limits (token bucket in `RateLimiter`, CMS in `event-filter`)
- Query scoping in all API endpoints

### Producer Architecture

`EventProducer` uses Iggy SDK's background send mode: `send()` enqueues into sharded in-memory buffers and returns immediately. Background threads batch (1000 msgs or 1ms linger) and flush over TCP. Backpressure timeout is 2s — events are shed (counted in `events_dropped`) if consumers are dead. This is acceptable because users can resync historical data.

### tracker-core API Layer (`src/api.rs`)

The `/api/v1/*` endpoints authenticate via `pt_{key_prefix}_{secret}` ingest tokens, extract the tenant, and proxy requests to the internal services (polars-query, polars-lite, ai-query) with `tenant_id` injected server-side. Callers cannot choose their own tenant — prevents data leakage.

### Ingestion Surfaces

The platform is a general-purpose, domain-agnostic events platform with multiple ingestion paths:

- **Tracking endpoints** (`/t`, `/p`, `/i`, `/t/auto`) — click/postback/impression with URL signing, encryption, redirects
- **Bulk ingestion** (`POST /batch`) — up to 10K pre-built events per call. The TypeScript SDK (`packages/sdk-typescript`) wraps this with a buffered `TrackerClient`.
- **Authenticated external ingestion** (`POST /ingest`) — Bearer token auth (`pt_{key_prefix}_{secret}`), accepts any `event_type`, flat `params`, and nested `raw_payload`. Designed for plugin adapters (Stripe, Shopify), data imports, and any external system. Goes directly to `events-clean`, bypassing bot filter.
- **Browser beacon** (`packages/beacon/t.js`) — cookieless, zero-persistence browser tracking via `sendBeacon()`

All paths produce the same `TrackingEvent` shape and flow through the same Iggy pipeline.

### Agentic Interface (MCP + Vivgrid)

The platform exposes 32 tools via two AI agent interfaces:

- **MCP Server** (`packages/mcp-server`) — stdio transport, auto-registered from `packages/tool-definitions`
- **Vivgrid chat** (`packages/vivgrid-tools`) — OpenAI function-calling format with multi-turn chat loop via `POST /api/chat`

Tool definitions live in `packages/tool-definitions/src/tools.ts` — single source of truth. Add a tool definition there and it appears in both MCP and Vivgrid automatically.

Tool categories: tenant CRUD (5), API keys (3), webhooks with param filtering (5), events & analytics (5), historical queries (3), tracking URLs (5), filter rules (4), ingest tokens (3), onboarding helpers (3: `generate_tracking_link`, `ingest_event`, `get_beacon_snippet`), AI query (3), health (1).

Onboarding helper routes live in `packages/platform-api/src/routes/onboarding.ts`.

### Webhook Subscription Filtering

Webhooks support param-level filtering via `filter_param_key` and `filter_param_value` columns on the `webhooks` table. The webhook-consumer (`src/bin/webhook_consumer.rs`) evaluates these filters in the dispatch loop: if `filter_param_key` is set, the event must have that key in `params`; if `filter_param_value` is also set, the value must match exactly. NULL filters mean "match everything" (backward compatible).

## Key Design Principles

- **Business-agnostic core**: the platform never interprets event semantics. All domain logic belongs in `params` keys, not in platform code. Never add logic that depends on specific param keys.
- **Opaque params**: `params` is a black-box `HashMap<String, String>`. Downstream consumers and the query layer treat it as generic key-value data.
- **Fire-and-forget ingestion**: `/t`, `/p`, `/i` handlers must remain sub-millisecond. Never block on downstream processing — use `tokio::spawn` for producer sends.
- **Consumer independence**: each Iggy consumer runs as its own binary with its own consumer group. They must not depend on each other.

## Commit Conventions

Format: `type(scope): description`

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`

## Environment Setup

Copy `.env.example` to `.env`. Required: `HMAC_SECRET` (for signed URL mode) or `ENCRYPTION_KEY` (32-byte hex for encrypted mode). The `PLATFORM_API_URL` and `PLATFORM_API_KEY` are optional — without them, multi-tenant mode is disabled and only the global secret is used.

## CI/CD

GitHub Actions (`.github/workflows/deploy.yaml`) uses `dorny/paths-filter` to detect changes and only rebuilds affected Docker images. All Rust binaries share a common Dockerfile (`docker/Dockerfile.core`) parameterized by `BINARY` build arg. Deploys to Civo K3s via `kubectl apply`.

Changes to shared Rust code (`src/lib.rs`, `src/config.rs`, `src/event.rs`, `src/producer.rs`, `Cargo.toml`, `Cargo.lock`, `packages/aggregate-schema/`) trigger rebuilds of ALL Rust binaries.
