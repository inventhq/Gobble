# Tracker — Project Constitution

> High-performance, multi-tenant event tracking platform. This document is the single source of truth for any AI or developer working on this codebase. Read it before making any changes.

---

## 1. What This Is

A managed event tracking platform (alternative to Everflow/RedTrack) where developers use our SDK to generate tracking links, and we run the infrastructure. Developers never see Iggy, Turso, or any internals — they interact through the SDK, Platform API, or Dashboard.

**Business model:** Managed SaaS. Multi-tenant. Developers get API keys, generate signed URLs with our SDK, and query their events through the Platform API or Dashboard.

---

## 2. Architecture Overview

```
SDK (TypeScript) → tracker-core (Rust/Axum) → Iggy (message broker)
                        │                          ↓
                        │ /t /p /i          events (raw topic)
                        │                          ↓
                        │                    event-filter (bot/rate-limit)
                        │                          ↓
                        │ /ingest ────→ events-clean (filtered topic)
                        │                          ↓
                        │               ┌──────────┼──────────┬──────────┐
                        │               ↓          ↓          ↓          ↓
                        │         r2-archiver  stats-consumer  sse-gw  webhook-consumer
                        │         (→ Delta/R2) (→ Turso)    (→ SSE)  (→ HTTP dispatch)
                        │               ↓          ↓          ↓
                        │         polars-query  Turso (DB)  Dashboard (live feed)
                        │         (cold tier)      ↓
                        │                Platform API (Hono/CF Workers)
                        │                          ↓
                        │                  Dashboard (SvelteKit)
```

**Topic topology:**
- **`events`** (raw) — tracker-core publishes `/t /p /i` events here. Only the event-filter reads this topic.
- **`events-clean`** (filtered) — event-filter writes cleaned events here. `/ingest` events go here directly (already authenticated via plugin runtime, no filtering needed). All downstream consumers read from this topic in production.
- In **dev/testing**, consumers default to `IGGY_TOPIC=events` (skips event-filter) so your own IP doesn't get rate-limited. For testing `/ingest` events locally, run consumers with `IGGY_TOPIC=events-clean`.

**Key principle:** tracker-core is stateless and fast. It captures HTTP context, publishes raw events to Iggy, and returns immediately. All enrichment, aggregation, and delivery happens downstream in consumers.

---

## 3. Tech Stack & Why

| Component | Technology | Why chosen |
|-----------|-----------|------------|
| **Tracking core** | Rust + Axum 0.8 | Raw performance (111K req/sec single, 1.45M events/sec batch) |
| **Message broker** | Apache Iggy 0.8 | Rust-native, no JVM, lighter than Kafka, path dependency to local source |
| **Database** | Turso (libSQL) | Edge-native, HTTP API for Workers, SQLite compatibility |
| **Platform API** | Hono + Cloudflare Workers | Lightweight, edge-first, zero cold start |
| **Dashboard** | SvelteKit 5 + TailwindCSS v4 | Svelte 5 runes, fast, minimal bundle |
| **Auth** | Stytch (Consumer project) | Magic links + Google OAuth, vanilla JS SDK |
| **RBAC** | Permit.io | Externalized authorization, Management API for role lookups |
| **SDK** | TypeScript (zero deps) | Pure functions, Node.js crypto only, no network for link generation |
| **SSE Gateway** | Rust + Axum SSE | Real-time event streaming from Iggy to browser via broadcast channel |
| **MCP Server** | @modelcontextprotocol/sdk | AI-native management interface, 17 tools |
| **URL security** | HMAC-SHA256 + AES-256-GCM | Configurable per deployment (URL_MODE env var) |
| **Event IDs** | UUIDv7 | Time-sortable, globally unique |

---

## 4. Repository Structure

```
tracker/
├── Cargo.toml                    # Rust workspace: 4 binaries
├── src/
│   ├── main.rs                   # tracker-core binary (Axum HTTP server)
│   ├── lib.rs                    # Library crate (shared modules)
│   ├── config.rs                 # Env-based configuration
│   ├── crypto.rs                 # HMAC-SHA256 + AES-256-GCM
│   ├── event.rs                  # TrackingEvent struct (UUIDv7, envelope + opaque params)
│   ├── producer.rs               # Iggy producer (background mode, key-based partitioning)
│   ├── routes.rs                 # Axum handlers: /t, /p, /i, /batch, /health
│   ├── tenant_cache.rs           # Multi-tenant secret cache (from Platform API)
│   └── bin/
│       ├── stats_consumer.rs     # Iggy → Turso (hourly stats + recent events)
│       ├── webhook_consumer.rs   # Iggy → HTTP webhook dispatch
│       └── sse_gateway.rs        # Iggy → SSE (real-time event streaming, port 3031)
├── .env                          # tracker-core env vars
├── .env.example                  # Documented env template
├── packages/
│   ├── sdk-typescript/           # @tracker/sdk — zero-dep TS SDK
│   │   └── src/
│   │       ├── crypto.ts         # HMAC + AES (Rust-compatible)
│   │       ├── links.ts          # URL builders (signed, encrypted, postback, impression)
│   │       ├── client.ts         # TrackerClient (batch, auto-flush, retry)
│   │       ├── types.ts          # TrackingEvent, TrackerConfig, BatchResponse
│   │       └── *.test.ts         # 19/19 tests passing
│   ├── platform-api/             # @tracker/platform-api — Hono + CF Workers
│   │   ├── src/
│   │   │   ├── index.ts          # Hono app, route mounting
│   │   │   ├── types.ts          # Env bindings (Turso, admin key, Permit.io)
│   │   │   ├── routes/
│   │   │   │   ├── tenants.ts    # Tenant CRUD + Permit.io auto-provisioning
│   │   │   │   ├── keys.ts       # API key management
│   │   │   │   ├── webhooks.ts   # Webhook CRUD + test delivery
│   │   │   │   ├── events.ts     # Event queries + stats aggregation
│   │   │   │   └── internal.ts   # /internal/secrets (for tracker-core cache)
│   │   │   ├── middleware/
│   │   │   │   └── auth.ts       # Bearer token auth (admin key or API key hash)
│   │   │   ├── db/
│   │   │   │   ├── client.ts     # @libsql/client/web (HTTP-only)
│   │   │   │   └── schema.sql    # Full schema (6 tables, 6 indexes)
│   │   │   └── lib/
│   │   │       └── permit.ts     # Permit.io user sync helper
│   │   ├── .dev.vars             # Local dev secrets (Turso, admin key, Permit.io)
│   │   └── wrangler.toml         # CF Workers config
│   ├── app/                      # @tracker/app — SvelteKit Dashboard
│   │   └── src/
│   │       ├── lib/
│   │       │   ├── api/          # One file per resource (client, tenants, keys, webhooks, events, health)
│   │       │   ├── auth/
│   │       │   │   ├── stytch.ts # Stytch vanilla JS SDK
│   │       │   │   └── permit.ts # Permit.io RBAC (proxied through server-side route)
│   │       │   ├── stores/
│   │       │   │   └── auth.svelte.ts  # Svelte 5 runes auth state
│   │       │   ├── components/layout/  # Sidebar, TopBar, StatCard
│   │       │   └── utils/        # cn.ts, constants.ts, format.ts
│   │       └── routes/
│   │           ├── dashboard/    # Stats, events, webhooks, keys, tenants, settings
│   │           ├── login/        # Stytch login UI
│   │           ├── authenticate/ # Stytch callback handler
│   │           └── api/permissions/ # Server-side Permit.io proxy
│   └── mcp-server/              # @tracker/mcp-server — 16 AI-callable tools
│       └── src/index.ts          # MCP stdio transport, zod schemas
```

---

## 5. Rust Binaries (3)

### tracker-core (`src/main.rs`)
- **Port:** 3030
- **Endpoints:** `GET /t` (click → 307), `GET /p` (postback → 200), `GET /i` (impression → 1x1 GIF), `POST /batch` (bulk), `GET /health`
- **Iggy producer:** Background mode, sharded workers, 1000 batch_length, 1ms linger
- **Partitioning:** Key-based by `key_prefix` param (tenant ID) — same tenant → same partition
- **Multi-tenant:** Loads secrets from Platform API via `/internal/secrets`, refreshes every 60s
- **URL validation:** HMAC signature with prefixed format (`prefix_hmac`), or AES-GCM decryption
- **NOOP fallback:** If Iggy is unreachable, events are counted but not persisted

### stats-consumer (`src/bin/stats_consumer.rs`)
- **Reads from:** Iggy stream `tracker`, topic `events`
- **Writes to:** Turso via HTTP API (`TURSO_URL` + `TURSO_AUTH_TOKEN`)
- **Tables:** `stats` (hourly aggregates, `ON CONFLICT count + 1`) and `recent_events` (rolling window, 1000 per tenant)
- **Dedup:** In-memory HashSet + VecDeque (100K capacity, bounded LRU)
- **Consumer group:** `stats-consumer`, auto-commit on poll, 100ms poll interval, batch of 100

### webhook-consumer (`src/bin/webhook_consumer.rs`)
- **Reads from:** Same Iggy stream/topic
- **Dispatches:** HTTP POST to registered webhook URLs with HMAC signature header
- **Retry:** Configurable attempts with backoff
- **Logging:** Writes delivery results to `webhook_deliveries` table
- **Dedup:** Same pattern as stats-consumer

---

## 6. Event Schema

```json
{
  "event_id": "019462a7-...",
  "event_type": "click",
  "timestamp": 1707350000000,
  "ip": "203.0.113.42",
  "user_agent": "Mozilla/5.0...",
  "referer": "https://...",
  "accept_language": "en-US",
  "request_path": "/t",
  "request_host": "track.example.com",
  "params": {
    "key_prefix": "6vct",
    "offer_id": "123",
    "aff_id": "456"
  }
}
```

The core never interprets `params`. Downstream consumers extract `key_prefix` for tenant routing. Everything else is domain-specific.

---

## 7. Database Schema (Turso)

6 tables: `tenants`, `api_keys`, `webhooks`, `webhook_deliveries`, `stats`, `recent_events`

Key design decisions:
- **`stats`** uses composite PK `(tenant_id, event_type, hour)` with `ON CONFLICT DO UPDATE SET count = count + 1` — pre-aggregated, no need to scan raw events
- **`recent_events`** is a rolling window (consumer prunes to 1000 per tenant) — for debugging, not analytics
- **`api_keys`** stores `key_hash` (SHA-256), never the raw key — key is shown once at creation
- **`tenants`** has `key_prefix` (4-char unique), `hmac_secret`, `encryption_key` — generated at creation
- **No ORM** — raw SQL everywhere via `@libsql/client/web`

---

## 8. Auth & RBAC

### Stytch (Authentication)
- **Project type:** Consumer (NOT B2B) — this matters, B2B SDK is incompatible
- **Methods:** Magic links + Google OAuth
- **SDK:** `@stytch/vanilla-js` (not React/Next bindings)
- **Flow:** `/login` → Stytch UI → redirect to `/authenticate` → validate token → `/dashboard`
- **Server-side:** `hooks.server.ts` validates session cookie, redirects to `/login` if invalid
- **Mock mode:** When `VITE_STYTCH_PUBLIC_TOKEN` is not set, auto-authenticates as admin

### Permit.io (Authorization)
- **User key:** Email address (from Stytch session)
- **Roles:** `admin` (full access), `tenant` (scoped to own resources)
- **API:** Management API (`api.permit.io/v2/facts/...`) for role lookups — NOT Cloud PDP (has CORS issues and propagation delays)
- **Proxy:** Dashboard calls `/api/permissions` server-side route which queries Permit.io — browser never talks to Permit.io directly
- **Auto-provisioning:** When a tenant is created with an email, Platform API syncs the user to Permit.io with `tenant` role
- **Mock mode:** When `VITE_PERMIT_API_KEY` is not set, everyone gets admin access

### Platform API Auth
- **Admin:** `ADMIN_BOOTSTRAP_KEY` env var — full access to all endpoints
- **Tenant:** API keys (Bearer token) — scoped to own resources, resolved via `key_hash` lookup
- **Middleware:** `auth.ts` checks Bearer token, sets `tenantId` and `isAdmin` in Hono context

---

## 9. Environment Variables

### tracker-core (`.env`)
```
URL_MODE=signed
HMAC_SECRET=...
IGGY_URL=127.0.0.1:8090
IGGY_STREAM=tracker
IGGY_TOPIC=events
HOST=0.0.0.0
PORT=3030
MAX_BATCH_SIZE=10000
PLATFORM_API_URL=http://localhost:8787
PLATFORM_API_KEY=tk_admin_...
```

### Iggy Server
```
IGGY_ROOT_USERNAME=iggy
IGGY_ROOT_PASSWORD=iggy
```
**CRITICAL:** v0.8 generates a random root password on fresh start unless you set these env vars.

### stats-consumer
```
IGGY_URL=127.0.0.1:8090
IGGY_STREAM=tracker
IGGY_TOPIC=events              # dev default; set to events-clean for production
TURSO_URL=https://your-db.turso.io
TURSO_AUTH_TOKEN=eyJ...
```

### Platform API (`.dev.vars`)
```
TURSO_URL=libsql://your-db.turso.io
TURSO_AUTH_TOKEN=eyJ...
ADMIN_BOOTSTRAP_KEY=tk_admin_...
PERMIT_API_KEY=permit_key_...
```

### Dashboard (`.env` or env vars)
```
VITE_API_URL=http://localhost:8787
VITE_API_KEY=tk_admin_...
VITE_STYTCH_PUBLIC_TOKEN=...
VITE_PERMIT_API_KEY=permit_key_...
```

---

## 10. Running Locally

Start services in this order:

```bash
# 1. Iggy (from iggy source)
cd /Users/vv/Desktop/iggy/iggy
IGGY_ROOT_USERNAME=iggy IGGY_ROOT_PASSWORD=iggy cargo run --release --bin iggy-server

# 2. Platform API
cd packages/platform-api
npx wrangler dev --port 8787

# 3. tracker-core
cargo run --release --bin tracker-core

# 4. stats-consumer
TURSO_URL=https://... TURSO_AUTH_TOKEN=... cargo run --release --bin stats-consumer

# 5. Dashboard
cd packages/app
npm run dev
```

**Ports:** Iggy TCP 8090, Iggy HTTP 3000, Platform API 8787, tracker-core 3030, Dashboard 5173

---

## 11. Key Design Decisions

1. **Iggy over Kafka** — Rust-native, no JVM, single binary, path dependency to local source for tight SDK integration
2. **Turso over Postgres** — HTTP API works from Cloudflare Workers, edge-native, SQLite-compatible
3. **Hono on Workers over Express** — Zero cold start, edge-first, lightweight
4. **Pre-aggregated stats** — `ON CONFLICT count + 1` in SQLite, no need for time-series DB for basic analytics
5. **Opaque params** — Core never interprets query params beyond `url`, `sig`, `key_prefix`. Downstream consumers own the business logic
6. **Prefixed signatures** — `sig=prefix_hmac` format lets the core resolve the correct tenant secret without a DB lookup (in-memory cache)
7. **Background send mode** — Iggy producer uses sharded workers with 1ms linger. `send()` returns immediately, HTTP response is decoupled from Iggy I/O
8. **Key-based partitioning** — Events with same `key_prefix` go to same Iggy partition (consistent hashing). Enables future per-tenant consumer scaling
9. **Server-side Permit.io proxy** — Browser never talks to Permit.io directly (CORS + propagation delay issues). SvelteKit server route queries Management API
10. **Stytch Consumer project** — NOT B2B. Consumer project supports magic links + OAuth for individual users. B2B is for organization-based auth
11. **MCP server** — AI agents can manage the entire platform (create tenants, rotate keys, query events) without the Dashboard
12. **Zero-dep TypeScript SDK** — Only uses Node.js built-in `crypto`. No `node-fetch`, no `axios`. Link generation is pure functions with no network calls

---

## 12. Signature Format

### Signed mode (HMAC-SHA256)
- Click URL: `GET /t?url=https://...&offer_id=123&sig=prefix_hmac_hex`
- Signature covers only the `url` parameter: `HMAC-SHA256(secret, url)`
- Prefix format: `key_prefix` + `_` + hex HMAC (e.g., `6vct_a1b2c3...`)
- No prefix = use global `HMAC_SECRET` fallback

### Encrypted mode (AES-256-GCM)
- Click URL: `GET /t?d=base64url_encrypted_blob`
- The `d` parameter contains the AES-GCM encrypted destination URL

### Postback/Impression
- Use `key_prefix` + `sig` params with all-params HMAC (sorted key=value pairs)

---

## 13. Performance Benchmarks

Measured on macOS Apple Silicon, release build, `hey` with 200 concurrent connections:

| Endpoint | Throughput | Latency (p50) |
|----------|-----------|---------------|
| `GET /p` (single) | 111K req/sec | 1.2ms |
| `POST /batch` (100 events) | 1.1M events/sec | — |
| `POST /batch` (1000 events) | 1.45M events/sec | — |

---

## 14. Backlog

- [x] **Real-time polling** — `createPoller` composable in Dashboard (10s stats/events, 30s webhooks/keys), `since` param on `GET /api/events`, `poll_new_events` MCP tool (17 tools total), auto-pause on tab hidden
- [x] **SSE gateway** — `sse-gateway` Rust binary (Axum SSE + Iggy consumer, port 3031). Broadcast channel fan-out, per-tenant `key_prefix` filtering, `event_type` filtering, CORS enabled. Dashboard live feed panel on Events page with `createSSE` composable. `VITE_SSE_URL` env var (default `http://localhost:3031`)
- [x] **Batch writes** in stats-consumer — channel-decoupled architecture (Iggy task → mpsc → writer task), 1000-event batches via Turso v2/pipeline API, multi-row INSERT (500 rows/stmt), pre-aggregated stats (`count + N`), single prune per tenant per flush. 10K events in ~10s
- [x] **Tracking URLs** (Option B: short URL mode) — naked link registry (ID → destination). Platform API CRUD (`/api/tracking-urls`), tracker-core `GET /t/:tu_id` route with in-memory cache, SDK `buildTrackedClickUrl()`, 5 MCP tools (22 total), Dashboard page. UUIDv7 IDs (`tu_` prefix). Enables destination rotation without regenerating distributed links. Zero business logic — third parties add meaning on their side
- [x] **RisingWave** — sub-second streaming analytics replacing stats-consumer for event queries. 5th Rust binary (`risingwave-consumer`): Iggy → RisingWave Cloud via Postgres wire protocol. Schema: `events` table + `stats_hourly` / `stats_total` materialized views. Platform API `GET /api/events` and `GET /api/events/stats` now query RisingWave (Turso fallback if `RISINGWAVE_URL` not set). Turso remains for CRUD (tenants, keys, webhooks, tracking URLs). `pg` (node-postgres) in Cloudflare Workers with `nodejs_compat` flag. E2E verified: <2s event-to-query latency
- [x] **At-least-once delivery** — all 4 consumers switched from at-most-once to at-least-once. `AutoCommit::Disabled`, offset feedback channel (`Vec<(partition_id, offset)>`), `tokio::select!` for concurrent ack draining, per-partition offset tracking via `HashMap<u32, u64>`, poison pill protection (commit offset for undeserializable messages), `flush_with_retry` (3 retries, 1s/2s/4s backoff). Live crash-tested: 5K events survived SIGKILL with zero data loss, zero replay on clean restart
- [x] **R2 + Polars cold analytics** — R2 archiver (Iggy → Parquet → R2, 6th Rust binary), Polars query service (Axum + Polars 0.53, port 3040), query router (`/api/events/history` cold-only, `/api/events/stats/merged` hot+cold merge with 7-day hot window), MCP tools (`query_history`, `get_merged_stats`), Dashboard `/history` page. 26 MCP tools total
- [x] **Frontend overhaul** — SparkCard grid with live SSE rolling buffer (60s sparkline), LinkDrawer detail panel (uPlot chart + events table + param filter), list/chart view toggle, create/edit/delete tracking URLs, batch stats fetch, per-link live counters
- [x] **Rate limiting** — per-tenant token bucket rate limiter in tracker-core. Each tenant (`key_prefix`) gets a separate bucket with configurable `rate_limit_rps` (default 100, burst capacity 2×). Stored in `tenants.rate_limit_rps` column, loaded via `TenantCache` → `RateLimiter`, synced every 60s. Applied to all tracking endpoints (`/t`, `/t/:tu_id`, `/p`, `/i`, `/ingest`). Returns `429 Too Many Requests` with `Retry-After: 1` header when exceeded. Per-tenant tuning via `PATCH /api/tenants/:id { "rate_limit_rps": 500 }`. Inactive buckets pruned every 5 minutes. Tested: 5 rps limit → 10 burst tokens consumed → 429 on 11th request, refills correctly over time, different tenants isolated
- [ ] **Usage quotas** — per-tenant event caps tied to plan tier (free/pro/enterprise), enforce in tracker-core, expose usage in Platform API + Dashboard
- [x] **Producer durability** — fire-and-forget via `tokio::spawn` for zero hot-path latency. `BackpressureMode::Block` prevents silent drops when buffer full. Iggy SDK auto-reconnects on brief outages (tested: zero data loss). **NOOP→Iggy reconnection loop** (every 30s via background task, `RwLock<ProducerInner>` swap) recovers from Iggy-down-at-startup without restart. **`GET /health/broker`** returns 200 `{"broker":"connected"}` or 503 `{"broker":"noop"}` for monitoring. Tested: start without Iggy → NOOP → start Iggy → auto-reconnect → events flow. **Backpressure safety net:** two-layer defense — Iggy-level `Block` mode + 256 MiB `max_buffer_size` caps memory, app-level `tokio::time::timeout(2s)` wrapping every `send_with_partitioning()` sheds events when consumers are dead (HTTP stays responsive). `events_dropped` counter exposed on `/health` and `/health/broker`. 8K burst test: 8000 sent, 0 dropped. Remaining narrow gap: process crash (in-flight buffer lost) — infra-level (process supervisor / WAL)
- [x] **Hot/warm/cold schema consistency** — Platform API now normalizes all tier responses at the boundary. `/api/events/history` (cold): `timestamp_ms` → `timestamp`, `params` JSON string → parsed object (matches hot tier `/api/events` shape). `/api/events/stats/warm`: `date_path` → `date` (matches cold tier `group_by=date` output). Both warm and cold `group_by=hour` return `{date, hour, event_type, count}` consistently. Hot tier already aliased `timestamp_ms AS timestamp` in SQL. Storage schemas intentionally differ (RisingWave JSONB vs Delta Utf8 vs Polars aggregates) — normalization happens at the API layer, not the storage layer. `aggregate-schema` shared crate prevents warm-tier drift between r2-archiver writer and polars-lite reader. Verified: all 3 tiers return identical field names for events (`timestamp`, `params` as object) and stats (`date`, `hour`, `event_type`, `count`)
- [x] **Events table reactivity** — fixed 3 bugs: (1) **Per-link reactive isolation:** replaced `liveByLink = { ...liveByLink }` object spreads with direct property mutation (`liveByLink[tuId] = ...`) — Svelte 5's `$state` proxy tracks granular property access, so only the affected link's SparkCard re-renders. Same for `liveClicksByLink` and `liveBuffers`. (2) **Stable snapshot logic:** replaced length-based `liveSnapshotLen` with ID-based `snapshotIds: Set<string>` in LinkChart's `displayEvents` — immune to array capping (20 items) and poller resets. New events identified by `!snapshotIds.has(e.event_id)`. (3) **Stable derived ref:** replaced `liveEventsFor()` function with `$derived` (`selectedLiveEvents`). Live counters use `countedIds: Set<string>` — no double-counting on array churn, no jump on 10s poller refresh
- [x] **Multiple consumer instances** — horizontal scaling without Redis or shared state. Leverages Iggy's native consumer group rebalancing (each partition assigned to exactly one member). **Financial-grade idempotency:** stats-consumer uses `stats_ledger` table (`INSERT OR IGNORE` on `(tenant_id, event_id)`) + recompute stats from ledger `COUNT(*)` — replayed events during rebalance are never double-counted. polars-query uses `ROW_NUMBER() OVER (PARTITION BY event_id)` dedup CTE for both events and stats queries. webhook-consumer checks `webhook_deliveries` for existing successful delivery before dispatch. risingwave-consumer already idempotent (`event_id` PK). event-filter needs no change (downstream sinks handle dupes). **Partitions bumped to 24** (from 3) — supports up to 24 parallel instances per consumer type (~2M events/sec headroom), zero overhead when running fewer instances. Ledger pruned hourly (keep 30 days, ~1.4GB/day at 1M events/hour). **IP rate limiter upgraded to Count-Min Sketch** — O(1) lookup, 2MB fixed memory, zero allocations, handles 1M+ unique IPs/sec. Two alternating CMS instances (4 rows × 64K slots) with tumbling window
- [x] **Hour-level querying** — `event_hour` (Utf8, `"00"`–`"23"`) added as a regular column (not partition) to the Delta table via `SchemaMode::Merge` for backward compatibility with existing 2-partition table (`tenant_id` + `date_path`). In r2-archiver: derived from `timestamp_ms` alongside `date_path`, added to `delta_schema()`, `EventRow`, and `events_to_record_batch()`. In polars-query (cold tier): new `group_by=hour` returns `(date, hour, event_type, count)` and `group_by=date` returns `(date, event_type, count)` — both computed from `timestamp_ms` to avoid DeltaTableProvider's Dictionary-encoded partition column issue in GROUP BY. Partition pruning still works via WHERE on `date_path`. Polars-lite (warm tier) already had hour-level aggregates — no change needed. Promoting `event_hour` to a 3rd partition column requires a fresh Delta table (partition columns are immutable) — do this when migrating to production
- [x] **uPlot chart time range too wide** — fixed: auto-zoom x-axis to actual data range with 10% padding and minimum 1-hour span. `getOpts()` in `LinkChart.svelte` computes `xMin`/`xMax` from data timestamps. Re-applies zoom on data updates via `setScale()`. Hours dropdown on dashboard (1h/6h/24h/3d/7d) for manual control
- [x] **raw_payload + /ingest endpoint** — `raw_payload: Option<serde_json::Value>` on `TrackingEvent` with `#[serde(default, skip_serializing_if)]` for backward compat. `POST /ingest` endpoint (1MB body limit) accepts `{event_type, params, raw_payload}` for external/plugin event ingestion. Propagated to RisingWave (`JSONB` column), Delta Lake (`Utf8` nullable), frontend (TS interface + modal JSON viewer). `/ingest` events route directly to `events-clean` topic (bypass event filter — auth handled upstream by plugin runtime). Second `EventProducer` in tracker-core for `events-clean` topic (`IGGY_TOPIC_CLEAN` env var)
- [x] **Ad-hoc SQL queries (hot + cold)** — tenant-scoped arbitrary SQL against both tiers. **Hot tier:** `POST /api/events/query` accepts `{sql, hours, limit}`, wraps user SQL in a `WITH scoped AS (SELECT * FROM events WHERE tenant_id = '...' AND timestamp_ms >= ...)` CTE for mandatory tenant isolation + time window. JSONB operators work on `params` and `raw_payload` columns (e.g. `params->>'amount_cents'`, `(params->>'amount')::int`). Keyword blocklist (`DROP/DELETE/INSERT/UPDATE/ALTER/CREATE/TRUNCATE`), 10s statement timeout, max 1000 rows. Sub-second latency against RisingWave Cloud. **Cold tier:** `mode=custom` on `POST /polars-query/query` with `custom_sql` field, same CTE pattern with `deduped` alias, `datafusion-functions-json` for `json_get_str/json_get_int/json_get_float` on `raw_payload` (stored as JSON string in Delta Lake). Full history, ~5s query latency. Both tiers are fully business-agnostic — platform provides the query engine, plugin authors bring the SQL
- [x] **Plugin Runtime (Phase 1)** — 9th binary (`packages/plugin-runtime/`, Rust + Deno Core). Tenant-authored JS/TS plugins run in sandboxed V8 isolates, react to real-time events via SSE gateway, emit derived events via `POST /ingest`. **Deno Core sandbox:** `#[op2]` async Rust ops injected as `runtime.emit()`, `runtime.query()`, `runtime.getEvents()`, `runtime.getStats()`, `runtime.getState()`, `runtime.setState()`, `runtime.log.*`. No raw `fetch()`, filesystem, or network access — plugins can only talk to the system through injected ops. **SSE multiplexer:** one SSE connection per active tenant to SSE gateway (port 3031), auto-reconnect, tears down when last plugin disabled. **Event dispatcher:** routes SSE events to matching plugins by `event_type` subscription. **Plugin registry:** SQLite — plugin_id, key_prefix, name, enabled, code, event subscriptions. **CRUD API:** `GET/POST/PATCH/DELETE /plugins` on port 3050. **E2E verified:** register plugin → click tracking URL → plugin receives event via SSE → executes JS in V8 → `emit()` calls `/ingest` → derived `echo_click` event queryable in Platform API. **Phase 2 complete:** auth on `/ingest` (Bearer tokens), execution limits, structured logs. **Phase 3 complete:** per-tenant Turso databases, schema discovery (`GET /schemas/:key_prefix`), cross-plugin SQL queries (`POST /schemas/:key_prefix/query`), schema context for AI Query Service
- [x] **Auth on /ingest** — Bearer token authentication for `POST /ingest`. Token format: `pt_{key_prefix}_{32 random chars}`. **Platform API:** `ingest_tokens` table (SHA-256 hash stored, not plaintext), CRUD at `/api/ingest-tokens` (POST create, GET list, DELETE revoke), `POST /internal/validate-ingest-token` for tracker-core validation. Supports optional `plugin_id` scope and `expires_in_days` TTL. **tracker-core:** `IngestTokenCache` module — validates tokens via Platform API with 5-minute positive cache + 1-minute negative cache, background cleanup task. `/ingest` handler extracts `Authorization: Bearer pt_...`, validates via cache, **injects `key_prefix` from token** (overrides any caller-provided value — prevents tenant spoofing). Returns 401 for missing/invalid/expired/revoked tokens. Tracking endpoints (`/t /p /i`) remain unauthenticated by design. E2E verified: no token→401, valid→200, invalid→401, spoofed key_prefix→overridden from token
- [ ] **Auth on /t /p /i** — optional request authentication for tracking endpoints. Currently open (by design — tracking pixels/redirects need to work without auth). Consider: signed request tokens, referer allowlists, or rate-limit-only approach
- [x] **Event detail modal** — click any event row in LinkChart to open a modal with all fields: event_id, type, timestamp (formatted + raw), IP, user agent, referer, request path, request host. Params displayed as key-value grid with copy button. `raw_payload` rendered as formatted JSON with copy. Escape or backdrop click to close. Implemented in `LinkChart.svelte`
- [x] **Date range picker** — From/To date inputs on `/dashboard/history` page for cold tier queries. Supports stats (aggregated) and events (raw rows) modes, cold-only or hot+cold merged sources, group-by options (event_type, tu_id, date). Hours dropdown (1h/6h/24h/3d/7d) on main dashboard for hot tier
- [ ] **Deploy to production (Civo + CF Workers + Vercel)** — 3-node Civo K3s cluster runs all Rust binaries (10 total across 5 Cargo projects) + Iggy. Dockerfile per binary with multi-stage builds. K3s Deployments + Services for HA (auto-restart, health checks, rolling updates, horizontal scaling — subsumes the old process supervisor item). Tier labels: `hot` (tracker-core + consumers + sse-gateway + plugin-runtime), `warm` (polars-lite), `cold` (polars-query + r2-archiver), `ai` (ai-query). Iggy needs PersistentVolume for message durability. Platform API on Cloudflare Workers (already Wrangler-ready). Frontend (`packages/app`) on Vercel. External managed services: RisingWave Cloud (hot tier), Turso Cloud (CRUD + plugin DBs), Cloudflare R2 (Delta Lake + aggregates), LanceDB Cloud (vector search)
- [x] **Delta-RS + 3-Tier Query Architecture (Hot/Warm/Cold)** — replaced raw Parquet + R2 PUT in r2-archiver with `deltalake 0.30` crate. Delta table at `s3://{bucket}/events/` with partition columns `tenant_id` + `date_path` (YYYY-MM-DD). ACID transactions via `_delta_log/`, R2 conditional PUT (`etag`) for lock-free concurrency (`DefaultLogStore`, no DynamoDB). `DeltaTable::write()` with `SaveMode::Append`, automatic file management (no manual chunk numbering). Background `OPTIMIZE` compaction every 60 flushes + final compaction on shutdown. Arrow bumped to v57 across the project. **3-tier query architecture:** (1) **Hot** — RisingWave MVs, last 7 days, real-time tickers; (2) **Warm** — `polars-lite` binary (`packages/polars-lite/`, Polars 0.53 + object_store, port 3041), reads pre-computed hourly aggregate Parquet from `s3://{bucket}/aggregates/`, 30-day rolling window, free-tier monthly dashboards; (3) **Cold** — `polars-query` rewritten as pure DataFusion service (`packages/polars-query/`, `DeltaTableProvider` → DataFusion SQL with partition pruning on `tenant_id`/`date_path`, predicate pushdown into Parquet row groups, streaming reads from R2), full history + Text-to-SQL, pro tier. **Warm tier pipeline:** r2-archiver writes inline hourly aggregates (`GROUP BY tenant_id, event_type, date_path, hour → COUNT`) to `s3://{bucket}/aggregates/` after each Delta flush + periodic reconciliation (DataFusion recomputes today's aggregates from raw Delta table every 60 flushes, atomic overwrite via tmp + rename). **Shared schema:** `packages/aggregate-schema/` (zero-dep crate, column names + R2 path conventions) prevents schema drift between r2-archiver and polars-lite. Polars dropped from cold tier due to `chrono` version conflict (`polars 0.53` needs `<=0.4.41`, `deltalake 0.30` needs `>=0.4.42`) — retained in warm tier as separate binary with isolated dependency tree. Subsumes the old "R2 archiver parallel uploads" item
- [x] **Event-filter** — lightweight Rust binary (our Vector alternative, 7th binary in main crate). Reads raw Iggy `events` topic → applies filters → writes clean events to `events-clean` topic. **Built-in rules (always active):** bot UA detection (65+ known patterns: Googlebot, HeadlessChrome, curl, python-requests, etc.), empty UA rejection, per-IP rate limiting via **Count-Min Sketch** (O(1) lookup, 2MB fixed memory, zero allocations — handles 1M+ unique IPs/sec). Two alternating CMS instances (4 rows × 64K slots each) with tumbling window for cross-boundary accuracy. No per-IP state, no HashMap, no VecDeque — constant memory regardless of cardinality. **Per-tenant custom rules:** stored in Turso `filter_rules` table, hot-reloaded every 30s. Rule schema: `{field, operator, value, action}` — field supports `user_agent`, `referer`, `ip`, `event_type`, `request_path`, `request_host`, `param:<key>`; operators: `contains`, `equals`, `is_empty`, `not_empty`, `starts_with`; actions: `drop` or `flag`. **Platform API CRUD:** `GET/POST/PATCH/DELETE /api/filter-rules` with admin global rules (`tenant_id='*'`) and tenant-scoped rules. **MCP/Vivgrid tools:** `list_filter_rules`, `create_filter_rule`, `update_filter_rule`, `delete_filter_rule` (31 tools total). **Architecture:** same consumer skeleton as other binaries — Iggy consumer task → mpsc → filter+produce task, `tokio::select!` for offset acks, at-least-once delivery, poison pill protection. Separate `offset_tracker` for all events (passed+dropped) ensures dropped event offsets are committed. **Observability:** periodic stats logging (passed/dropped/drop rate/IPs tracked), top 5 drop reasons. **Env vars:** `IGGY_TOPIC_CLEAN` (default `events-clean`), `FILTER_BATCH_SIZE` (1000), `FILTER_FLUSH_INTERVAL_MS` (250), `FILTER_IP_RATE_LIMIT` (200). **Tested:** processed 165K+ events, 98.6% drop rate on test data (curl UA + single-IP rate limit), clean topic populated. Downstream consumers switch to clean topic by setting `IGGY_TOPIC=events-clean`
- [x] **AI Query Service** — unified intelligence layer combining Text-to-SQL + LanceDB vector search in a single Rust binary (`packages/ai-query/`, Axum, port 3060). 10th binary, 5th Cargo project. **Architecture:** 5 modules — `config.rs` (env vars), `delta.rs` (DataFusion + Delta Lake with dedup CTE + tenant isolation), `slm.rs` (configurable SLM client for Text-to-SQL, supports OpenAI-compatible/Baseten/simple text response formats, mock mode when `SLM_URL` not set), `schema.rs` (schema context builder — fetches plugin table schemas from plugin-runtime `GET /schemas/:key_prefix?counts=true` + Delta Lake events schema, builds SLM prompt context), `turso_proxy.rs` (proxies SQL to plugin-runtime `POST /schemas/:key_prefix/query` for cross-plugin Turso queries). **SLM routing:** model output prefixed with `-- turso` routes to plugin-runtime Turso proxy, otherwise routes to Delta Lake DataFusion. Safety: rejects write keywords (DROP/DELETE/INSERT/UPDATE/ALTER/CREATE/TRUNCATE). **LanceDB Cloud:** `vectordb.rs` — pure REST client via `reqwest` (no `lancedb` crate — that pulls in 200+ crates for the local Lance storage engine we don't need). Connects to LanceDB Cloud at `https://{db}.{region}.api.lancedb.com`, auth via `x-api-key` header. Vector search via `POST /v1/table/events/query/` with tenant-scoped filter. Placeholder embeddings (384-dim, deterministic from text hash) until embedding model is deployed. **Endpoints:** `POST /query/nl` (prompt → schema context → SLM → SQL → DataFusion or Turso → rows), `POST /query/similar` (vector similarity search via LanceDB Cloud REST API), `POST /chat` (multi-turn, extracts last user message as prompt), `GET /health` (reports SLM + plugin-runtime + LanceDB config status). **Vivgrid tools:** `nl_query`, `similar_events`, `ai_chat` — registered in `@tracker/tool-definitions` (29 tools total). **Env vars:** `AI_QUERY_PORT` (3060), `SLM_URL`, `SLM_API_KEY`, `PLUGIN_RUNTIME_URL`, `ADMIN_API_KEY`, `R2_*`, `LANCEDB_URI` (db://ai-query-8kc6p2), `LANCEDB_API_KEY`, `LANCEDB_REGION` (default us-east-1). **Tested:** mock SLM — NL query returned 151K clicks from Delta Lake in 3s, chat endpoint works. LanceDB Cloud connection verified (list tables OK). Pro-tier feature
- [ ] **Embedding model** — replace `placeholder_embedding()` in ai-query's `vectordb.rs` with a real embedding model. Options: OpenAI `text-embedding-3-small` (1536-dim, API call), or self-hosted `all-MiniLM-L6-v2` on Baseten (384-dim, lower latency). Update `EMBEDDING_DIM` and LanceDB table schema accordingly. Needed for meaningful vector similarity results
- [ ] **LanceDB ingest pipeline** — auto-embed events as they flow through the system. Either: (a) new consumer binary that reads `events-clean` topic, embeds via model, inserts into LanceDB Cloud; or (b) inline in r2-archiver alongside Delta writes. Batch embeddings for throughput. Backfill existing Delta Lake events
- [ ] **ai-query dependency diet** — proxy Delta Lake queries to polars-query instead of embedding deltalake/DataFusion/AWS SDK directly. Drops ~300 crates from ai-query's build. SLM generates SQL, ai-query forwards to `POLARS_QUERY_URL`, returns results. Keep `vectordb.rs` (pure reqwest) and `turso_proxy.rs` as-is
- [x] **Platform API warm tier routing** — wired polars-lite into the Platform API query router. `POLARS_LITE_URL` env var added to `types.ts` and `.dev.vars`. `resolveTenantInfo()` returns `key_prefix` + `plan` from Turso. `queryPolarsLite()` helper POSTs to polars-lite service. `/api/events/stats/merged` now auto-routes: free plan → warm tier (polars-lite, 30-day aggregates), pro/enterprise/admin → cold tier (polars-query, full Delta Lake). `tier` query param overrides automatic routing (`?tier=warm` or `?tier=cold`). Warm window clamped to `WARM_WINDOW_DAYS` (30 days). Response includes `sources.tier` ("warm"/"cold"), `sources.plan`, `sources.warm`/`sources.cold` booleans. Tested: admin→cold ✅, admin+tier=warm→warm ✅, free tenant→warm ✅, free+tier=cold→cold ✅

---

## 15. Gotchas & Lessons Learned

1. **Iggy v0.8 generates random root password** on fresh start unless you set `IGGY_ROOT_USERNAME` and `IGGY_ROOT_PASSWORD` env vars. The SDK defaults (`iggy`/`iggy`) won't work without these.
2. **`@libsql/client` v0.6 breaks** on Cloudflare Workers — use v0.5.6.
3. **Stytch Consumer vs B2B** — completely different SDKs and APIs. Consumer uses `@stytch/vanilla-js`, B2B uses `@stytch/vanilla-js/b2b`. Wrong one = cryptic errors.
4. **Permit.io Cloud PDP** has CORS restrictions (no browser access) and propagation delays (role changes take minutes). Use the Management API directly for role lookups.
5. **Turso HTTP API** for stats-consumer uses `https://` URL format. Platform API uses `libsql://` format. They're different clients (`reqwest` vs `@libsql/client/web`).
6. **`cargo clean` needed** when switching Iggy SDK versions — path dependency means stale artifacts can cause credential mismatches.
7. **Svelte 5 runes** (`$state`, `$derived`) — not Svelte 4 stores. Don't use `writable()` or `$:` reactive statements.
8. **RisingWave SQL quirks** — no `ON CONFLICT DO NOTHING` (handle duplicate keys in app code), no `$N` parameterized queries in LIMIT/OFFSET positions (use inline values with SQL escaping).
9. **Wrangler v4 `node_compat`** is removed — use `compatibility_flags = ["nodejs_compat"]` + `compatibility_date = "2024-09-23"` or later for `pg` (node-postgres) to resolve Node.js built-ins (`crypto`, `net`, `tls`, `events`, etc.).

10. **At-least-once delivery with Iggy (channel-based consumers)** — This is the definitive guide for implementing at-least-once delivery with Apache Iggy's high-level `IggyConsumer` API in a 2-task channel architecture. Five bugs were discovered and fixed during the audit. Any AI or developer implementing this pattern should follow every step below.

    **The pattern:** Iggy task (reads messages) → mpsc channel → Writer task (flushes to storage). The challenge: offsets must only be committed *after* confirmed storage writes, but the two tasks are decoupled by a channel.

    **Step 1: Disable auto-commit.**
    ```rust
    .auto_commit(AutoCommit::Disabled)
    ```
    Never use `After(ConsumingEachMessage)` or `When(PollingMessages)` — these commit before the writer confirms the flush.

    **Step 2: Add an offset feedback channel (writer → Iggy task).**
    ```rust
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::channel::<Vec<(u32, u64)>>(100);
    ```
    The writer sends back `Vec<(partition_id, max_offset)>` after each successful flush. Use `Vec` not a single tuple — a batch may span multiple partitions.

    **Step 3: Carry offset metadata through the event channel.**
    ```rust
    struct EventWithOffset {
        tenant: String,
        event: TrackingEvent,
        partition_id: u32,
        offset: u64,
    }
    ```
    The Iggy task attaches `partition_id` and `offset` from each `ReceivedMessage` before sending to the writer.

    **Step 4: Track max offset PER PARTITION in the writer (Bug #1 fix).**
    ```rust
    let mut partition_offsets: HashMap<u32, u64> = HashMap::new();
    for event in &batch {
        partition_offsets.entry(event.partition_id)
            .and_modify(|o| { if event.offset > *o { *o = event.offset; } })
            .or_insert(event.offset);
    }
    // After successful flush:
    let _ = ack_tx.send(partition_offsets.into_iter().collect::<Vec<_>>()).await;
    ```
    **Bug #1:** If you track only a single `(partition_id, offset)` per batch, you commit the offset for one partition and silently skip all others. Events from skipped partitions replay on restart.

    **Step 5: Use `tokio::select!` in the Iggy task to drain acks concurrently (Bug #5 fix — CRITICAL).**
    ```rust
    let mut consumer_done = false;
    loop {
        if consumer_done { break; }
        tokio::select! {
            result = consumer.next() => {
                match result {
                    Some(Ok(message)) => { /* send to channel */ }
                    Some(Err(e)) => { error!("consume error: {}", e); }
                    None => { consumer_done = true; }
                }
            }
            Some(partition_offsets) = ack_rx.recv() => {
                for (partition_id, offset) in partition_offsets {
                    consumer.store_offset(offset, Some(partition_id)).await.ok();
                }
            }
        }
        // Also drain any additional acks non-blockingly
        while let Ok(partition_offsets) = ack_rx.try_recv() {
            for (partition_id, offset) in partition_offsets {
                consumer.store_offset(offset, Some(partition_id)).await.ok();
            }
        }
    }
    ```
    **Bug #5 (CRITICAL):** If you use `while let Some(result) = consumer.next().await` and only drain acks with `try_recv()` at the top of each iteration, acks are NEVER processed when the consumer is idle (no new messages). Offsets sit in the channel forever, never committed. On restart, everything replays. The `tokio::select!` races message polling against ack receiving, so offsets are committed even during idle periods.

    **Step 6: Commit offset for undeserializable messages (Bug #3 fix — poison pill).**
    ```rust
    let event: TrackingEvent = match serde_json::from_slice(payload) {
        Ok(e) => e,
        Err(e) => {
            warn!("Bad message at offset {}: {} — committing to skip", offset, e);
            consumer.store_offset(offset, Some(partition_id)).await.ok();
            continue;
        }
    };
    ```
    **Bug #3:** If you `continue` without committing the offset, the consumer replays the same malformed message forever on every restart. Always commit the offset for messages you intentionally skip.

    **Step 7: Wrap flushes with retry + exponential backoff.**
    ```rust
    async fn flush_with_retry<F, Fut, T>(f: F) -> Result<T, String>
    where F: Fn() -> Fut, Fut: Future<Output = Result<T, String>> {
        for attempt in 0..=MAX_RETRIES {
            match f().await {
                Ok(val) => return Ok(val),
                Err(e) if attempt < MAX_RETRIES => {
                    let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
    ```
    If all retries fail, do NOT send an ack — the offset stays uncommitted and events re-deliver on restart.

    **Step 8: Ensure idempotent sinks.** After a crash, events between the last committed offset and the crash point replay. Your storage must handle duplicates:
    - RisingWave: `PRIMARY KEY` rejects duplicate INSERTs
    - Turso/SQLite: `INSERT OR IGNORE` / `ON CONFLICT`
    - R2 Parquet: in-memory `HashSet` dedup + Polars `.unique()` at query time
    - Webhooks: include `event_id` in payload for receiver-side dedup (at-least-once is the best you can do across a network boundary)

    **Step 9: Concurrent flush safety.** If using `tokio::spawn` for concurrent flushes (like risingwave-consumer), each spawned task sends its own ack independently. Iggy's `store_offset` with `allow_replay=false` (the default) silently skips lower offsets, so out-of-order acks from concurrent flushes are safe — the highest offset always wins.

    **Step 10: Shutdown sequence.** On graceful shutdown: flush remaining batches → wait for in-flight flushes → drop `ack_tx` (signals Iggy task to exit after draining) → await Iggy task (which drains remaining acks via `try_recv` after the loop).

    **All 5 bugs discovered during the audit:**
    | # | Bug | Severity | Root cause |
    |---|-----|----------|------------|
    | 1 | Single offset per batch | High | Only tracked one `(partition_id, offset)` — skipped other partitions |
    | 2 | Concurrent flush ordering | Safe | Iggy SDK's `allow_replay=false` skips lower offsets automatically |
    | 3 | Poison pill | High | `continue` without committing offset for bad messages → infinite replay |
    | 4 | Silent ack drop | Safe | `let _ = ack.send()` — data already in storage, just delays offset commit |
    | 5 | Acks never drained when idle | **Critical** | `while let` loop only drains acks when new messages arrive → offsets never committed if consumer goes idle |
