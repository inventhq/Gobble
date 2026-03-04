# Tracker

High-performance, multi-tenant event tracking platform built in Rust. Captures clicks, postbacks, and impressions at sub-millisecond latency, streams events through Apache Iggy, and serves analytics from a 3-tier query architecture (Hot/Warm/Cold).

Designed as a **business-agnostic core** — the platform never interprets what events mean. Users bring their own vocabulary through opaque `params`. All domain logic lives in the user's param keys, not in the platform.

## Architecture

```
                         ┌──────────────────────────────────────┐
                         │         tracker-core (Rust/Axum)     │
                         │  /t → click (307)   /p → postback   │
                         │  /i → impression    /batch → bulk    │
                         │  /t/:tu_id → short URL click         │
                         └──────────────┬───────────────────────┘
                                        │ fire-and-forget
                                        ▼
                              ┌──────────────────┐
                              │   Apache Iggy    │
                              │  24 partitions   │
                              │  (by tenant ID)  │
                              └────────┬─────────┘
                                       │
              ┌────────────┬───────────┼───────────┬─────────────┐
              ▼            ▼           ▼           ▼             ▼
        event-filter  risingwave   sse-gateway  r2-archiver  webhook
        (bot/rate)    -consumer    (live SSE)   (Delta Lake)  -consumer
              │            │           │           │             │
              ▼            ▼           ▼           ▼             ▼
        events-clean  RisingWave   Browser     R2 (Parquet)  HTTP POST
        (Iggy topic)  Cloud (Hot)  Dashboard   Delta table   to webhooks
                           │                       │
                           │           ┌───────────┤
                           ▼           ▼           ▼
                      ┌─────────────────────────────────┐
                      │  Platform API (Hono/CF Workers)  │
                      │  Query router: Hot/Warm/Cold     │
                      └──────────────┬──────────────────┘
                                     │
                          ┌──────────┼──────────┐
                          ▼          ▼          ▼
                     Dashboard   MCP Tools   SDK
                     (SvelteKit) (26 tools)  (TypeScript)
```

## Binaries

8 Rust binaries across 3 Cargo projects:

| Binary | Crate | Port | Purpose |
|--------|-------|------|---------|
| `tracker-core` | main | 3030 | HTTP ingestion, Iggy producer, NOOP fallback |
| `event-filter` | main | — | Bot detection, CMS rate limiting, custom rules. `events` → `events-clean` |
| `sse-gateway` | main | 3031 | Iggy → SSE broadcast to browser clients |
| `risingwave-consumer` | main | — | Iggy → RisingWave Cloud (hot tier, <2s latency) |
| `stats-consumer` | main | — | Iggy → Turso stats (idempotent ledger pattern) |
| `webhook-consumer` | main | — | Iggy → HTTP dispatch with delivery dedup |
| `r2-archiver` | main | — | Iggy → Delta Lake on Cloudflare R2 (cold tier) |
| `polars-query` | packages/polars-query | 3040 | DataFusion over Delta Lake (cold tier queries) |

Plus `polars-lite` (packages/polars-lite, port 3041) for warm tier pre-computed aggregates.

## 3-Tier Query Architecture

| Tier | Engine | Data | Latency | Use Case |
|------|--------|------|---------|----------|
| **Hot** | RisingWave MVs | Last 7 days | <2s | Real-time dashboards, live tickers |
| **Warm** | Polars-lite | 30-day aggregates | ~500ms | Free-tier monthly dashboards |
| **Cold** | DataFusion + Delta Lake | Full history | ~2s | Deep-dive analytics, pro tier |

The Platform API routes queries automatically by plan tier (free → warm, pro → cold) with manual `?tier=` override.

## Event Schema

Every event published to Iggy:

```json
{
  "event_id": "019c4633-5078-7411-8858-4aeafb6298bb",
  "event_type": "click",
  "timestamp": 1770704294008,
  "ip": "203.0.113.1",
  "user_agent": "Mozilla/5.0 ...",
  "referer": "https://example.com",
  "accept_language": "en-US",
  "request_path": "/t",
  "request_host": "tracker.example.com",
  "params": {
    "key_prefix": "6vct",
    "tu_id": "tu_019c3f8d-aa19-7261-a5d6-9aa75cea309d",
    "sub1": "google_search",
    "campaign_id": "camp_8821"
  }
}
```

- `event_id` — UUIDv7 (time-sortable, used for dedup across all tiers)
- `params` — **opaque** key-value bag. The platform never reads it. Users bring their own vocabulary.
- `key_prefix` — tenant identifier, used for Iggy partitioning (tenant affinity)

## Endpoints

| Endpoint | Method | Purpose | Response |
|----------|--------|---------|----------|
| `/t` | GET | Click tracking (signed/encrypted URL) | 307 redirect |
| `/t/:tu_id` | GET | Short URL click (tracking URL registry) | 307 redirect |
| `/t/auto` | POST | Browser beacon (pageviews, outbound clicks) | 204 No Content |
| `/p` | GET | Postback / server-to-server conversion | 200 OK |
| `/i` | GET | Impression tracking | 1x1 transparent GIF |
| `/batch` | POST | Bulk event ingestion (up to 10K events) | `{"accepted": N}` |
| `/ingest` | POST | Authenticated external event ingestion | `{"event_id": "..."}` |
| `/health` | GET | Health check | JSON status |
| `/health/broker` | GET | Iggy connection status | 200 or 503 |

Click redirects (`/t`, `/t/:tu_id`) automatically append `?ad_click_id=<event_id>` to the destination URL, enabling session stitching with the browser beacon.

## URL Security

Two modes for securing redirect URLs on `/t`:

- **Signed (HMAC-SHA256):** `?url=...&sig=...` — destination visible, tamper-proof
- **Encrypted (AES-256-GCM):** `?d=...` — destination hidden in encrypted blob

Multi-tenant: signatures can be prefixed with tenant key (`6vct_abc123...`) for per-tenant secrets.

## Tracking URLs (Short URL Mode)

Naked link registry — `tu_id` → destination mapping stored in Turso, cached in tracker-core memory.

```
https://track.example.com/t/tu_019c3f8d-aa19?sig=6vct_abc123&sub1=google
```

Destination can be rotated server-side without regenerating distributed links. CRUD via Platform API `/api/tracking-urls`.

## Param-Level Querying

The Platform API provides generic key-value query operators:

```bash
# Filter events by param
GET /api/events?param_key=sub1&param_value=google_search

# Stats filtered by param
GET /api/events/stats?param_key=sub1&param_value=google_search&hours=48

# Breakdown by param values
GET /api/events/stats?group_by=param:sub1&hours=168
```

Same pattern as Stripe metadata, Datadog tags, PostHog properties — generic schema, user-defined semantics.

## Event Matching

Generic event-pair matching — join any two event types by a shared param key:

```bash
GET /api/events/match?trigger=click&result=postback&on=click_id
```

| Vertical | trigger | result | on | They call it |
|---|---|---|---|---|
| Affiliate | click | postback | click_id | "conversion" |
| E-commerce | click | postback | order_id | "purchase" |
| SaaS | click | postback | user_id | "signup" |

The API returns `trigger_event`, `result_event`, `time_delta_ms`, `match_rate` — no domain vocabulary.

## Performance

Benchmarked on Apple Silicon, release build, 200 concurrent connections:

| Mode | Endpoint | Req/sec | p50 | p99 |
|------|----------|---------|-----|-----|
| With Iggy | `/p` | **21,325** | 9.2ms | 12.0ms |
| With Iggy | `/t` (HMAC) | **21,048** | 9.3ms | 12.1ms |
| NOOP | `/p` | **143,450** | 1.0ms | 6.0ms |
| Batch | `/batch` (1K events) | **1,450 batches/sec** | — | — |

Single instance: **~1.8 billion events/day** with Iggy persistence.

## Quick Start

```bash
# 1. Start Iggy
cargo run --release --bin iggy-server  # or Docker: docker run -p 8090:8090 iggyrs/iggy

# 2. Start tracker-core
cp .env.example .env
cargo run --bin tracker-core

# 3. Start SSE gateway (for live dashboard)
cargo run --bin sse-gateway

# 4. Start RisingWave consumer (for events table + stats)
RISINGWAVE_URL="postgresql://..." cargo run --bin risingwave-consumer

# 5. Start Platform API
cd packages/platform-api && npx wrangler dev --port 8787

# 6. Start Dashboard
cd packages/app && npm run dev
```

If Iggy is unavailable, tracker-core starts in **NOOP mode** and auto-reconnects every 30s.

## Configuration

### tracker-core

| Variable | Default | Description |
|----------|---------|-------------|
| `URL_MODE` | `signed` | `signed` or `encrypted` |
| `HMAC_SECRET` | — | Required when `URL_MODE=signed` |
| `ENCRYPTION_KEY` | — | 32-byte hex key for `URL_MODE=encrypted` |
| `IGGY_URL` | `127.0.0.1:8090` | Iggy TCP address |
| `IGGY_STREAM` | `tracker` | Iggy stream name |
| `IGGY_TOPIC` | `events` | Iggy topic name |
| `IGGY_PARTITIONS` | `24` | Topic partition count |
| `MAX_BATCH_SIZE` | `10000` | Max events per `/batch` request |
| `PORT` | `3030` | HTTP bind port |
| `PLATFORM_API_URL` | `http://localhost:8787` | For tenant/tracking URL cache |

### Consumers

| Variable | Used By | Description |
|----------|---------|-------------|
| `RISINGWAVE_URL` | risingwave-consumer | PostgreSQL connection string |
| `TURSO_URL` | stats-consumer, webhook-consumer | Turso HTTP URL |
| `TURSO_AUTH_TOKEN` | stats-consumer, webhook-consumer | Turso auth token |
| `R2_BUCKET` | r2-archiver | Cloudflare R2 bucket name |
| `R2_ACCOUNT_ID` | r2-archiver | Cloudflare account ID |
| `R2_ACCESS_KEY_ID` | r2-archiver | R2 S3-compatible access key |
| `R2_SECRET_ACCESS_KEY` | r2-archiver | R2 S3-compatible secret |
| `SSE_PORT` | sse-gateway | SSE server port (default: 3031) |

## Project Structure

```
tracker/
├── src/
│   ├── main.rs                        # tracker-core entry point (Axum server)
│   ├── config.rs                      # Environment-based configuration
│   ├── crypto.rs                      # HMAC-SHA256 + AES-256-GCM
│   ├── event.rs                       # TrackingEvent struct (UUIDv7 + opaque params)
│   ├── producer.rs                    # Iggy producer (background + NOOP + auto-reconnect)
│   ├── routes.rs                      # /t, /t/:tu_id, /p, /i, /batch, /health handlers
│   ├── tenant_cache.rs               # Multi-tenant secret cache (hot-reloaded)
│   ├── tracking_url_cache.rs         # Tracking URL registry cache
│   └── bin/
│       ├── event_filter.rs            # Bot detection + CMS rate limiter + custom rules
│       ├── risingwave_consumer.rs     # Iggy → RisingWave Cloud (hot tier)
│       ├── stats_consumer.rs          # Iggy → Turso (idempotent ledger)
│       ├── r2_archiver.rs             # Iggy → Delta Lake on R2 (cold tier)
│       ├── webhook_consumer.rs        # Iggy → HTTP webhook dispatch
│       └── sse_gateway.rs             # Iggy → SSE broadcast
├── packages/
│   ├── platform-api/                  # Hono + Cloudflare Workers (API layer)
│   ├── app/                           # SvelteKit 5 dashboard
│   ├── beacon/                        # Browser beacon script (t.js) + CF Worker CDN
│   ├── sdk-typescript/                # TypeScript SDK (zero deps)
│   ├── mcp-server/                    # MCP server (26 tools)
│   ├── polars-query/                  # DataFusion cold tier service
│   ├── polars-lite/                   # Polars warm tier service
│   ├── aggregate-schema/              # Shared Rust crate for warm tier schema
│   ├── tool-definitions/              # MCP tool JSON definitions
│   └── vivgrid-tools/                 # Vivgrid AI chat integration
├── CONTRIBUTING.md                    # How to contribute
├── LICENSE                            # Apache License 2.0
└── README.md                          # This file
```

## Tech Stack

| Component | Technology | Why |
|-----------|-----------|-----|
| Tracking core | Rust + Axum 0.8 | 111K req/sec single, 1.45M events/sec batch |
| Message broker | Apache Iggy 0.8 | Rust-native, no JVM, lighter than Kafka |
| Hot tier | RisingWave Cloud | Streaming MVs, <2s event-to-query latency |
| Warm tier | Polars 0.53 | Pre-computed aggregates, zero idle cost |
| Cold tier | DataFusion + Delta Lake | ACID on R2, full history, SQL-first |
| Database | Turso (libSQL) | Edge-native, HTTP API for Workers |
| Platform API | Hono + Cloudflare Workers | Edge-first, zero cold start |
| Dashboard | SvelteKit 5 + TailwindCSS v4 | Svelte 5 runes, per-link reactive SSE |
| Auth | Stytch | Magic links + Google OAuth |
| RBAC | Permit.io | Externalized authorization |
| SDK | TypeScript (zero deps) | Pure functions, Node.js crypto only |
| AI | MCP (26 tools) + Vivgrid | AI-native management interface |

## Client-Side Tracking (Browser Beacon)

Automatic, cookieless pageview and outbound click tracking. Drop a single script tag on any page:

```html
<script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>
```

This automatically tracks:
- **Pageviews** on load and SPA navigations (pushState/replaceState/popstate)
- **Outbound link clicks** with href and link text
- **Ad attribution** — reads `ad_click_id` from the URL (appended by `/t` redirects) and includes it in every event

All events are sent via `navigator.sendBeacon()` to `POST /t/auto`. Zero cookies, zero localStorage, zero fingerprinting.

| Attribute | Required | Default | Description |
|-----------|----------|---------|-------------|
| `data-key` | **Yes** | — | Your tenant `key_prefix` |
| `data-host` | No | `https://track.juicyapi.com` | Tracker-core server URL |
| `data-track` | No | `"outbound"` | `"outbound"`, `"all"`, or `"none"` |
| `data-no-spa` | No | — | Disables SPA history tracking |

**npm:** `npm install @inventhq/tracker-beacon` — see [`packages/beacon/README.md`](packages/beacon/README.md) for full docs.

## Server-Side SDK (TypeScript)

Generate signed/encrypted tracking URLs, postback URLs, impression pixels, and batch-send events. Zero dependencies.

```bash
npm install @inventhq/tracker-sdk
```

```typescript
import { buildSignedClickUrl, TrackerClient } from "@inventhq/tracker-sdk";

// Generate a signed click URL (pure function, no network)
const url = buildSignedClickUrl("https://track.juicyapi.com", "secret", "https://offer.com/landing", {
  key_prefix: "6vct",
  offer_id: "123",
});

// Batch client for high-throughput server-side ingestion
const client = new TrackerClient({
  apiUrl: "https://track.juicyapi.com",
  mode: "signed",
  hmacSecret: "secret",
  batchSize: 100,
  flushInterval: 1000,
});
```

See [`packages/sdk-typescript/README.md`](packages/sdk-typescript/README.md) for full API reference.

## Onboarding

New to the platform? See the **[Onboarding Guide](docs/onboarding.md)** for step-by-step integration instructions covering:
- Getting your tenant credentials
- Adding the browser beacon to any site (Next.js, React, Vue, WordPress, plain HTML)
- Generating server-side tracking URLs
- End-to-end ad attribution (session stitching)
- Verification and troubleshooting

## npm Packages

| Package | Description | Install |
|---------|-------------|----------|
| [`@inventhq/tracker-beacon`](https://www.npmjs.com/package/@inventhq/tracker-beacon) | Browser beacon — cookieless pageview & click tracking | `npm i @inventhq/tracker-beacon` |
| [`@inventhq/tracker-sdk`](https://www.npmjs.com/package/@inventhq/tracker-sdk) | Server-side SDK — URL generation & batch events | `npm i @inventhq/tracker-sdk` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, architecture guidelines, and how to submit pull requests.

## License

Licensed under the [Apache License 2.0](LICENSE).

Copyright 2026 InventHQ
