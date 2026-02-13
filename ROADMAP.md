# Roadmap

## Trojan Horse: Migration from RedTrack / Voluum

### Why this works

RedTrack and Voluum are the two dominant affiliate tracking platforms. Both use a rigid, column-based data model with fixed sub-ID slots (RedTrack: `sub1`–`sub20`, Voluum: `var1`–`var10`). Our `params` map is schema-free — any key-value pair passes through untouched. This means:

- Their "columns" are our "params" — migration is just a name mapping
- Their postback format is identical to ours — same query string structure, different domain
- Their sub-ID limits don't exist for us — unlimited arbitrary keys
- Our AI layer (MCP) gives them something neither competitor offers

### Param mapping: RedTrack → Us → Voluum

| Concept | RedTrack | Us (params) | Voluum |
|---|---|---|---|
| Traffic source | `source` | `sub1` or `utm_source` | `trafficSourceId` |
| Campaign | `campaign_id` | `campaign_id` | `campaignId` |
| Offer | `offer_id` | `offer_id` | `offerId` |
| Landing page | `lander_id` | `lander_id` | `landerId` |
| Click ID | `clickid` | `click_id` | `clickId` |
| Sub-IDs | `sub1`–`sub20` | `sub1`–`subN` (unlimited) | `var1`–`var10` |
| Payout | `payout` | `payout` | `revenue` |
| Geo | `country` | `geo` | `country` |
| Cost | `cost` | `cost` | `cost` |
| Conversion status | `status` | `conversion_type` | `status` |

### Migration path for a client

**Step 1: URL rewrite (zero code change)**

RedTrack click URL:
```
https://redtrack.io/click?campaign_id=123&source=google&sub1=keyword&sub2=adgroup&cost=0.50
```

Becomes:
```
https://track.ours.com/t/tu_abc?sig=...&campaign_id=123&source=google&sub1=keyword&sub2=adgroup&cost=0.50
```

Same params, different domain. Everything flows through unchanged.

**Step 2: Postback receiver swap**

RedTrack postback:
```
https://redtrack.io/postback?clickid={clickid}&payout={payout}&status=approved
```

Ours:
```
https://track.ours.com/p?click_id={clickid}&payout={payout}&conversion_type=approved&key_prefix=XXXX
```

Ad networks (MaxBounty, ClickBank, etc.) fire postbacks with macros like `{clickid}` and `{payout}`. These are just query params — they work identically with our `/p` endpoint.

**Step 3: Historical data import**

Export CSV/API dump from RedTrack/Voluum → map columns to our params → write Parquet to R2 → queryable via Polars cold layer with the same `param_key`/`param_value` API.

### What we need to build

#### 1. Import Wizard
- "Paste your RedTrack/Voluum API key, we'll pull your campaigns and create tracking URLs"
- Auto-creates `tu_id` entries for each of their campaigns
- Maps their campaign settings to our tracking URL destinations
- Could also accept a CSV/JSON export if API access isn't available

#### 2. Param Mapping Templates
- Presets for "RedTrack style" (`sub1`–`sub20`, `clickid`, `source`) and "Voluum style" (`var1`–`var10`, `clickId`, `trafficSourceId`)
- Dashboard labels params using the template's display names (e.g. `sub1` → "Traffic Source")
- Clients can customize or create their own templates
- Templates are metadata only — they don't change how params are stored or queried

#### 3. Postback URL Generator
- "Select your ad network" dropdown (MaxBounty, ClickBank, CJ, ShareASale, etc.)
- Generates the correct postback URL with that network's macro format
- Pre-fills `key_prefix` and `tu_id` from the client's account
- Copy-to-clipboard ready — paste directly into the ad network's postback settings

#### 4. CSV Import
- Drag-and-drop historical data from RedTrack/Voluum CSV exports
- Auto-detects column mapping (or lets user manually map)
- Writes events to the pipeline (or directly to R2 Parquet for cold storage)
- Preserves original timestamps so historical analytics are accurate

### Why clients switch

| Pain point | RedTrack/Voluum | Us |
|---|---|---|
| Complexity | 47-tab UI, GTM-like setup | Ask AI in natural language |
| Sub-ID limits | 10-20 fixed slots | Unlimited arbitrary params |
| Real-time data | Minutes to hours delay | Sub-second (SSE + RisingWave) |
| AI analytics | None | MCP — "Which source converts best?" |
| Privacy | Cookie-heavy, consent required | No cookies, no client-side JS |
| Page performance | Client-side JS overhead | Zero client-side impact |
| Pricing | $149-499/mo for volume tiers | TBD (competitive) |
| Self-hosted option | No | Yes (Dockerized) |

---

## Future: Attribution & Revenue (low priority)

These are **not** part of the current backlog. They require domain-specific interpretation of params, which we want to keep at the edges (AI layer, client dashboards, or optional add-on services).

### Attribution Models
- First-touch / last-touch: generic — earliest/latest event for a user by `click_id`
- Multi-touch with weighted credit: opinionated — would need configurable models
- Implementation: RisingWave MV or Polars query that groups events by a correlation key and applies a model
- Keep as optional layer, not core

### Revenue Reporting
- Generic numeric param aggregation: `SUM(params->>'payout')` grouped by any dimension
- ROI calculation requires external cost data (ad spend) — not in our event stream
- Implementation: add a generic `aggregate` operator to the Platform API (e.g. `aggregate=sum:payout`)
- The AI layer can already do this by fetching events and computing client-side
- Formal API support is a convenience, not a blocker

### Funnel Query Endpoint
- `/api/events/funnel` — ordered multi-step matching with drop-off analysis
- Generic interface: user defines steps (e.g. `steps=["step:1","step:2","step:3"]`) and join key (`on=session_id`)
- Sequential LEFT JOINs in RisingWave or Polars window functions over Parquet for cold data
- Returns per-step counts and drop-off rates
- First feature that imposes "ordering" as a concept — opt-in vertical convenience, not core
- Use cases: SaaS onboarding funnels, lead capture flows, multi-page checkout

### DataFusion / ROAPI SQL Layer
- Expose R2 Parquet files via standards-based query APIs (REST/GraphQL/SQL over HTTP)
- ROAPI (built on DataFusion) auto-infers schema from Parquet, serves SQL + REST + GraphQL with zero code per dataset
- Each tenant gets a self-service analytics API — run arbitrary SQL against their archived events
- Read-only access to the same Parquet files the R2 archiver writes
- Replaces or complements the custom Polars query service with a more flexible query interface
- Enables power users to skip the Platform API entirely for ad-hoc analysis

---

## Long-term Vision: Universal Event Platform

The core insight: our engine is **doubly agnostic**. Layer 1 — the core (Iggy, RisingWave, Polars, R2) doesn't know what events mean. Layer 2 — the plugin/app layer doesn't know what *industry* it's serving. The affiliate tracker is just one app on a generic event platform.

### Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Apps / Plugins                     │
│  ┌───────────┐ ┌──────────┐ ┌──────────┐           │
│  │ Affiliate │ │  Stripe  │ │ Shopify  │  ...       │
│  │  Tracker  │ │  Plugin  │ │  Plugin  │           │
│  └─────┬─────┘ └────┬─────┘ └────┬─────┘           │
│        │             │            │                  │
│        ▼             ▼            ▼                  │
│    { event_type, timestamp, params: {}, raw_payload }│
└────────────────────────┬────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────┐
│               Core Engine (agnostic)                 │
│   Iggy → RisingWave MVs → R2 Parquet → Polars      │
│   Webhooks → MCP → Dashboard                        │
└─────────────────────────────────────────────────────┘
```

### How it works

Each plugin is a thin adapter: receive external webhook/API payload → promote key fields to flat `params` (for fast querying) → preserve full nested structure in `raw_payload` (for detail views and AI analysis) → push to Iggy. The entire downstream stack works unchanged.

- **Stripe plugin** → `event_type: "charge.succeeded"`, `params: { amount, currency, customer_id }`, `raw_payload: { full Stripe charge object }`
- **Shopify plugin** → `event_type: "order.created"`, `params: { order_id, total, items }`, `raw_payload: { full Shopify order object }`
- **GitHub plugin** → `event_type: "push"`, `params: { repo, branch, commits }`, `raw_payload: { full push event }`
- **Custom webhook** → any JSON payload, auto-flattened to params

### Why this works

- **No ETL.** Params are schema-free — a Stripe charge and a Shopify order are both `{event_type, params}`. No schema migrations, no column additions.
- **Cross-source correlation for free.** Match Stripe charges to Shopify orders by `customer_email`. Match ad clicks to conversions by `click_id`. The `match_events` endpoint already does this generically.
- **MCP becomes the killer feature.** "Which Stripe customers came from Google ads?" is a cross-source match query that traditional tools need a data warehouse for. We do it with one API call.
- **Plugin maintenance scales with AI.** Each adapter is a pure function (`incoming_json → {event_type, params}`). When a source changes its schema, an AI agent can diff and update the mapping. Plugins are isolated — one breaking doesn't affect others.

### Schema evolution: `raw_payload`

Current schema stores `params: HashMap<String, String>` (flat key-value). For nested payloads from external sources, add an optional `raw_payload: Option<serde_json::Value>` field:

- **Flat params** — promoted fields for fast querying (RisingWave MVs, Polars filters, Parquet column pruning)
- **raw_payload** — full nested JSON for detail views, AI analysis, and future re-processing
- Existing tracker functionality: zero changes (params stay flat, raw_payload is None)
- Parquet schema: one additional `raw_payload` column (JSON string)

### Positioning

Not "affiliate tracker that also does Stripe." Instead: **Business OS** — a real-time event platform where each industry vertical is an app/plugin. The affiliate tracker is the first app, proving the core works. Each additional plugin (Stripe, Shopify, CRM, etc.) expands the platform without touching the engine.

---

## Completed

### R2 + Polars Cold Analytics Layer (Feb 2026)
- **Phase 1:** R2 Archiver — Iggy consumer → Parquet → R2 (`src/bin/r2_archiver.rs`)
- **Phase 2:** Polars Query Service — Axum + Polars 0.53 (`packages/polars-query/`)
- **Phase 3:** Query Router — `/api/events/history` + `/api/events/stats/merged` with hot/cold routing
- **Phase 4:** MCP tools (`query_history`, `get_merged_stats`) + Dashboard `/history` page — 26 tools total
