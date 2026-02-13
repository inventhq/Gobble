# Tracker — Distributed Computing Bible

> Lessons learned, architectural patterns, and implementation details from building a distributed event tracking platform in Rust. This is the reference document for understanding *why* things work the way they do.

---

## 1. Message Broker: Why Iggy Over Kafka

**Decision:** Apache Iggy 0.8 over Kafka/Redpanda/NATS.

**Rationale:**
- Rust-native — no JVM, no GC pauses, no ZooKeeper/KRaft
- Single binary, ~50MB memory footprint vs Kafka's 1GB+ JVM heap
- TCP wire protocol with Rust SDK — zero serialization overhead
- Consumer groups with partition-exclusive assignment (same model as Kafka)
- Path dependency: we had local source access for debugging

**Tradeoff:** Smaller ecosystem, fewer operational tools, no managed cloud offering. Acceptable because we own the full stack and don't need Kafka Connect/Schema Registry.

**Partition strategy:** 24 partitions per topic, keyed by `key_prefix` (tenant ID). This gives tenant affinity (all events for a tenant go to the same partition → ordered within tenant) and supports up to 24 parallel consumer instances per consumer type.

---

## 2. At-Least-Once Delivery

**Status:** All 5 durable consumers use at-least-once delivery. SSE gateway is ephemeral (auto-commit).

### The offset feedback pattern

All channel-based consumers (risingwave, r2-archiver, stats) use a two-task architecture:

```
┌─────────────┐    EventWithOffset    ┌──────────────┐
│  Iggy Task  │ ──── mpsc tx ──────▶  │  Writer Task │
│             │                       │              │
│  consumer   │ ◀── mpsc rx ────────  │  flush()     │
│  .next()    │    Vec<(part, off)>   │  retry(3x)   │
└─────────────┘                       └──────────────┘
```

1. **Event channel** (Iggy → Writer): carries `EventWithOffset` with `partition_id` and `offset`
2. **Ack channel** (Writer → Iggy): carries confirmed `Vec<(partition_id, offset)>` after successful flush

**Critical detail:** The Iggy task uses `tokio::select!` to race between `consumer.next()` and `ack_rx.recv()`. Without this, offset acks would only be committed when new messages arrive — if the stream goes idle, the last batch's offsets would never be committed until the next message, creating a crash window.

```rust
tokio::select! {
    msg = consumer.next() => { /* forward to writer */ }
    ack = ack_rx.recv()   => { /* commit offsets */ }
}
```

### Per-partition offset tracking

Each batch may span multiple Iggy partitions. The writer collects `max(offset)` per partition using `HashMap<u32, u64>` and sends all partition offsets in a single ack. Committing only the global max offset would skip partitions that had lower offsets in the batch.

### Poison pill protection

If a message fails JSON deserialization, its offset is committed immediately so the consumer doesn't get stuck in an infinite retry loop on restart. The malformed message is logged and skipped. Without this, a single bad message would block the entire partition forever.

### Retry with exponential backoff

All consumers retry flush failures 3× with exponential backoff (1s → 2s → 4s). If all retries fail, the offset is NOT committed — events will be re-delivered on restart. This is the correct behavior: we'd rather re-process than lose data.

### Consumer settings

| Consumer | AutoCommit | Offset Strategy |
|----------|-----------|-----------------|
| risingwave-consumer | Disabled | Ack channel after flush |
| r2-archiver | Disabled | Ack channel after flush |
| stats-consumer | Disabled | Ack channel after flush |
| webhook-consumer | Disabled | Inline after each event |
| sse-gateway | When(PollingMessages) | Ephemeral, no durability needed |

---

## 3. Idempotency at Every Sink

At-least-once delivery means duplicates are inevitable during consumer restarts and partition rebalancing. Every sink must handle them:

### RisingWave (hot tier)
`event_id VARCHAR PRIMARY KEY` — duplicate INSERT is silently rejected by the database. Zero application logic needed.

### Turso / stats-consumer (idempotent ledger pattern)
**Problem:** The original `count + N` upsert would double-count replayed events.

**Solution:** Two-phase approach:
1. `INSERT OR IGNORE INTO stats_ledger (tenant_id, event_id, event_type, hour)` — each event is recorded exactly once
2. Stats recomputed from `SELECT COUNT(*) FROM stats_ledger WHERE tenant_id = ? AND event_type = ? AND hour = ?`

The ledger is the source of truth. `INSERT OR IGNORE` makes replays a no-op. Stats are always derived, never accumulated. This is **financial-grade idempotency** — you can replay the entire stream and get the exact same counts.

**Pruning:** Ledger entries older than 30 days are pruned hourly. At 1M events/hour, that's ~1.4GB/day — acceptable for Turso.

### Delta Lake / r2-archiver (cold tier)
In-memory `HashSet<event_id>` dedup within each flush batch. At query time, `polars-query` applies `ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) WHERE rn = 1` to deduplicate across Parquet files. Belt and suspenders.

### Webhooks
`webhook_deliveries` table in Turso tracks successful deliveries. Before dispatching, the consumer checks `SELECT 1 FROM webhook_deliveries WHERE webhook_id = ? AND event_id = ? AND status_code >= 200 AND status_code < 300`. If a successful delivery exists, skip. This prevents re-dispatching on replay.

### Query-time dedup (polars-query)
Both `build_events_sql` and `build_stats_sql` wrap queries in a CTE:
```sql
WITH deduped AS (
    SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn
    FROM events
) SELECT ... FROM deduped WHERE rn = 1
```
This handles any duplicates that slip through to the Delta table.

---

## 4. Producer Durability & NOOP Fallback

### Fire-and-forget pattern
HTTP endpoints return 200/307 **before** Iggy confirmation. The event is handed to a background `tokio::spawn` task that batches and flushes asynchronously. This gives sub-millisecond hot-path latency.

```rust
let producer = state.producer.clone();
tokio::spawn(async move {
    producer.send(&event, partition_key.as_deref()).await;
});
```

**Tradeoff:** If the process crashes between HTTP response and Iggy flush, those in-flight events are lost. Acceptable for tracking pixels where latency matters more than guaranteed delivery. The `/batch` endpoint could optionally await confirmation in a future iteration.

### NOOP mode & auto-reconnect
If Iggy is unavailable at startup, tracker-core starts in NOOP mode — events are counted but not persisted. A background task attempts reconnection every 30s:

```
Start → Iggy down → NOOP mode → 30s → retry → Iggy up → swap producer → events flow
```

The swap uses `RwLock<ProducerInner>` — the hot path takes a read lock (zero contention), the reconnect task takes a write lock only during the swap.

**`GET /health/broker`** returns 200 `{"broker":"connected"}` or 503 `{"broker":"noop"}` for monitoring.

---

## 5. Count-Min Sketch for IP Rate Limiting

**Problem:** Per-IP rate limiting in event-filter needs to handle 1M+ unique IPs/sec. A `HashMap<String, u32>` would grow unbounded.

**Solution:** Count-Min Sketch (CMS) — a probabilistic data structure that provides:
- O(1) lookup and increment
- Fixed memory: 2MB (4 rows × 64K slots × 8 bytes)
- Zero allocations after initialization
- Over-counts but never under-counts (safe for rate limiting)

### Tumbling window with overlap
Two alternating CMS instances handle time boundaries:
```
Window A: [0s ──── 60s]
Window B:      [30s ──── 90s]
```
An IP is rate-limited if it exceeds the threshold in **either** window. This prevents the "boundary reset" problem where an attacker sends N-1 requests at 59s and N-1 at 61s (2N-2 requests in 2 seconds, but each window only sees N-1).

### Why not HashMap?
At 1M unique IPs, a `HashMap<String, u32>` uses ~80MB and requires periodic cleanup (which IPs to evict?). The CMS uses 2MB regardless of cardinality, and the tumbling window handles cleanup automatically.

---

## 6. Consumer Group Rebalancing & Horizontal Scaling

### How it works
Iggy consumer groups assign partitions exclusively to one member. With 24 partitions:
- 1 instance → gets all 24 partitions
- 3 instances → 8 partitions each
- 24 instances → 1 partition each (maximum parallelism)

When an instance dies, its partitions are reassigned to surviving members. When it restarts, partitions rebalance again.

### The replay window
During rebalance, the new partition owner resumes from the last committed offset. Events between that offset and the crash point are re-delivered. This is why every sink must be idempotent (see section 3).

### No shared state needed
Each consumer instance is fully independent — no Redis, no distributed locks, no coordination. Iggy handles partition assignment, and idempotent sinks handle duplicates. This is the key architectural insight: **idempotency at the sink eliminates the need for distributed coordination**.

---

## 7. 3-Tier Query Architecture

### Why 3 tiers?
No single engine optimally serves all query patterns:
- **Real-time tickers** need sub-second latency → streaming MVs (RisingWave)
- **Monthly dashboards** need low cost → pre-computed aggregates (Polars-lite)
- **Deep-dive analytics** need full history → columnar scan (DataFusion + Delta Lake)

### Hot tier: RisingWave Cloud
- Materialized views: `stats_total`, `stats_hourly`, `stats_hourly_by_link`
- Events table with `event_id` PK (natural dedup)
- Postgres wire protocol — queried from Cloudflare Workers via `pg` (node-postgres)
- <2s event-to-query latency

### Warm tier: Polars-lite
- Reads pre-computed hourly aggregate Parquet from `s3://{bucket}/aggregates/`
- Written by r2-archiver alongside raw Delta Lake writes
- 30-day rolling window, ~500ms query latency
- Zero idle cost — only runs when queried

### Cold tier: DataFusion + Delta Lake
- Delta table at `s3://{bucket}/events/` with partition columns `tenant_id` + `date_path`
- ACID transactions via `_delta_log/`, R2 conditional PUT (etag) for lock-free concurrency
- `OPTIMIZE` compaction every 60 flushes + final compaction on shutdown
- Full SQL via DataFusion — supports arbitrary WHERE, GROUP BY, window functions

### Query routing
Platform API auto-routes by plan tier:
- Free plan → warm tier (30-day aggregates)
- Pro/enterprise → cold tier (full Delta Lake)
- `?tier=warm` or `?tier=cold` overrides automatic routing

### Schema consistency
Storage schemas intentionally differ (RisingWave JSONB vs Delta Utf8 vs Polars aggregates). Normalization happens at the API boundary:
- `timestamp_ms` → `timestamp` (cold tier)
- `date_path` → `date` (warm tier)
- `params` JSON string → parsed object (cold tier)

The `aggregate-schema` shared Rust crate prevents warm-tier schema drift between r2-archiver (writer) and polars-lite (reader).

---

## 8. Delta Lake on Cloudflare R2

### Why Delta Lake over raw Parquet?
- **ACID transactions** — concurrent writers don't corrupt data
- **Schema evolution** — `SchemaMode::Merge` adds columns without rewriting existing files
- **Time travel** — `_delta_log/` tracks every write, enabling rollback
- **Partition pruning** — `WHERE tenant_id = ? AND date_path = ?` skips irrelevant files

### Lock-free concurrency on R2
R2 supports conditional PUT via etag, which Delta-RS uses for optimistic concurrency (`DefaultLogStore`). No DynamoDB lock table needed (unlike S3). Two writers can safely append to the same table — the first to commit wins, the second retries.

### Partition strategy
`tenant_id` + `date_path` (YYYY-MM-DD). `event_hour` is a regular column (not partition) added via `SchemaMode::Merge` for backward compatibility. Promoting it to a 3rd partition column requires a fresh table (partition columns are immutable in Delta Lake).

---

## 9. Frontend Reactivity (Svelte 5)

### The problem
SSE events for any link triggered re-renders of ALL SparkCards. The stats poller (every 10s) caused the events table to jump (rows appearing/disappearing).

### Root causes and fixes

**Bug 1: Object spread on every SSE event**
```javascript
// BAD: creates new object ref → every SparkCard re-renders
liveByLink = { ...liveByLink, [tuId]: [...] };

// GOOD: Svelte 5 $state proxy tracks property access granularly
liveByLink[tuId] = [event, ...existing.slice(0, 19)];
```
Svelte 5's `$state` proxy uses JavaScript Proxy to track which properties each component reads. Direct property mutation only notifies components that read that specific key.

**Bug 2: Length-based snapshot tracking**
```javascript
// BAD: liveEvents.length changes when array is capped at 20
const newCount = liveEvents.length - liveSnapshotLen;

// GOOD: ID-based tracking is immune to array capping and poller resets
const snapshotIds = new Set(tableEvents.map(e => e.event_id));
const newEvents = liveEvents.filter(e => !snapshotIds.has(e.event_id));
```

**Bug 3: Function creates new array ref every render**
```javascript
// BAD: new array ref on every call → LinkDrawer re-renders
function liveEventsFor(tuId) { return liveByLink[tuId] ?? []; }

// GOOD: $derived creates stable ref, only updates when selectedTuId or liveByLink[selectedTuId] changes
const selectedLiveEvents = $derived(selectedTuId ? (liveByLink[selectedTuId] ?? []) : []);
```

### Live counter stability
Live counters (Clicks/Postbacks/Impressions badges) use `countedIds: Set<string>` instead of length comparison. When the poller refreshes, both `liveCounts` and `countedIds` reset together — no double-counting, no jump.

---

## 10. Event Ordering Guarantees

| Scope | Guarantee | Mechanism |
|-------|-----------|-----------|
| Within a partition | Strict FIFO | Iggy partition ordering |
| Within a tenant | Ordered | All events for a tenant go to same partition (keyed by `key_prefix`) |
| Cross-tenant | No global order | By design — tenants are independent |
| Query results | Newest-first | `ORDER BY timestamp DESC` at query layer |
| SSE to browser | Partition order | SSE gateway broadcasts in consumption order |
| Dashboard display | Newest-first | SSE events prepend to top, API results sorted DESC |

UUIDv7 event IDs are time-sortable, so even if events arrive slightly out of order from different partitions, the query layer sorts them correctly.

---

## 11. Event Filter: Bot Detection & Custom Rules

### Built-in rules (always active)
- **Bot UA detection:** 65+ patterns (Googlebot, HeadlessChrome, curl, python-requests, etc.)
- **Empty UA rejection:** No User-Agent header → filtered
- **IP rate limiting:** Count-Min Sketch (see section 5)

### Per-tenant custom rules
Stored in Turso `filter_rules` table, hot-reloaded every 30s:
```json
{
  "field": "user_agent",
  "operator": "contains",
  "value": "bot",
  "action": "block"
}
```
Supported fields: `user_agent`, `referer`, `ip`, `event_type`, `request_path`, `request_host`, `param:<key>`
Supported operators: `contains`, `equals`, `is_empty`, `not_empty`, `starts_with`

### Topic routing
Event-filter reads from `events` topic and writes clean events to `events-clean` topic. Consumers can read from either topic depending on whether they want filtered or raw events.

---

## 12. Multi-Tenant Architecture

### Tenant isolation
- **Iggy partitioning:** Events keyed by `key_prefix` (tenant ID) → tenant affinity per partition
- **API authentication:** Bearer token with tenant prefix (`tk_admin_...`, `tk_6vct_...`)
- **RBAC:** Permit.io for role-based access (admin, member, viewer)
- **Query isolation:** All API queries scoped by `tenant_id` extracted from auth token

### Secret management
Per-tenant HMAC secrets stored in Turso, cached in tracker-core memory, hot-reloaded every 60s. Signatures are prefixed: `6vct_abc123...` → look up tenant `6vct`'s secret → verify HMAC.

### Tracking URL registry
Per-tenant tracking URLs with `key_prefix` association. Cache in tracker-core memory, refreshed every 60s from Platform API.

---

## 13. Conscious Tradeoffs

| Decision | Tradeoff | Why acceptable |
|----------|----------|----------------|
| Fire-and-forget producer | In-flight events lost on process crash | Sub-ms latency for tracking pixels matters more than guaranteed delivery |
| NOOP mode | Events lost when Iggy is down | Auto-reconnect recovers in 30s; Iggy HA is an infra concern |
| CMS over-counts | Rate limiter may block slightly below threshold | False positives (blocking legitimate traffic) are better than false negatives (letting bots through) |
| No global ordering | Cross-tenant events unordered | Tenants are independent; within-tenant ordering is guaranteed |
| Delta partition columns immutable | Can't add `event_hour` as partition without new table | `SchemaMode::Merge` adds it as regular column; promote on production migration |
| SSE gateway auto-commit | May re-broadcast events after crash | SSE is ephemeral; dashboard filters events older than 60s anyway |
| Ledger pruning (30 days) | Can't verify idempotency for events older than 30 days | Stats are already materialized; ledger is only needed for replay dedup |

---

## 14. Failure Modes & Recovery

| Failure | Impact | Recovery |
|---------|--------|----------|
| tracker-core crash | In-flight events lost | Restart; NOOP → auto-reconnect to Iggy |
| Iggy down | NOOP mode, events lost | tracker-core auto-reconnects every 30s |
| Consumer crash | Uncommitted events re-delivered | Restart; idempotent sinks handle dupes |
| RisingWave down | Hot tier queries fail | Platform API falls back to Turso |
| R2 outage | Cold tier writes fail | r2-archiver retries 3×; offsets uncommitted → replay on recovery |
| Turso outage | Stats/webhook writes fail | Consumers retry 3×; offsets uncommitted → replay on recovery |
| Partition rebalance | Brief re-delivery window | Idempotent sinks absorb duplicates |
| Bad message (poison pill) | Single event skipped | Offset committed, logged, consumer continues |

---

## 15. Economics

| Component | Cost at 5M events/month | Cost at 100M events/month |
|-----------|------------------------|--------------------------|
| R2 storage | ~$0.015 (1GB Parquet) | ~$0.30 (20GB) |
| RisingWave Cloud | Free tier | ~$50/month |
| Turso | Free tier | ~$30/month |
| Iggy | Self-hosted (0) | Self-hosted (0) |
| Cloudflare Workers | Free tier | ~$5/month |
| **Total infra** | **~$0** | **~$85/month** |

Competitors (Everflow, RedTrack, Voluum) charge $120-500/month for 1M events. We can offer 5M free and still be profitable at scale.

---

## 16. Plugin Runtime Architecture

### Core Principle

Tracker core is an **event analytics platform**, not a state store. It answers "what happened?" (time-series, aggregation, audit). Entity state ("what's the current state?") belongs in a relational database.

### Two-Channel Design

```
Plugin (developer-written, knows the business domain)
  ├── ctx.ingest() → tracker core /ingest → event stream (analytics, audit, time-series)
  └── ctx.db()     → tenant's Turso DB    → entity state (customers, orders, charges)
```

### Plugin Runtime Provides (agnostic, owned by us)

- **`ctx.ingest()`** — sends events to tracker core's `/ingest` endpoint
- **`ctx.db()`** — Turso client scoped to the tenant's isolated database (raw SQL, no ORM, no migrations)
- **`ctx.auth()`** — API keys, OAuth tokens for external services (Stripe, Shopify, etc.)
- **Rate limiting / backpressure / retries**

The runtime is a **dumb pipe with guardrails**. It does NOT own schema, migrations, or business logic. The plugin author decides what tables to create and what SQL to run.

### Multi-tenant Turso Integration

Each tenant gets an isolated Turso database (SQLite). The plugin author defines their own schema — the tracker platform is business-agnostic. Turso embedded replicas give the plugin runtime fast local reads.

### Why NOT Entity Projections in Tracker Core

Stitching fields across event types (e.g. "customer email from customer.created + order total from order.created") requires:
1. Last-write-wins resolution per entity
2. Joins across entity types
3. Schema definitions per entity type

That's a relational database. Building it on top of an event stream is reinventing what SQLite/Postgres already does, but worse. Let Turso handle it.

### Event Stream vs Entity State

| Question | Tracker Core (events) | Turso (entities) |
|----------|----------------------|------------------|
| How many charges yesterday? | ✅ RisingWave MV | — |
| Revenue trend this week? | ✅ time-series query | — |
| Full Stripe webhook body? | ✅ raw_payload | — |
| Customer's current email? | — | ✅ SQL query |
| Order with line items? | — | ✅ JOIN |

### Shared Events Table Safety

All tenants share one `events` table. Safe because:
- Every query is scoped by `tenant_id` (auth middleware injects it from API key)
- RisingWave MVs group by `tenant_id`
- Delta Lake partitioned by `tenant_id` (file-level pruning)
- Same model as Segment, Mixpanel, PostHog

### Developer Plugin Example

```typescript
export async function onWebhook(event: StripeEvent, ctx: PluginContext) {
  // 1. Analytics — send to tracker event stream
  await ctx.ingest({
    event_type: event.type,
    params: {
      customer_id: event.data.object.customer,
      amount_cents: String(event.data.object.amount),
    },
    raw_payload: event.data.object,
  });

  // 2. Entity state — write to tenant's Turso DB
  if (event.type === 'customer.created') {
    await ctx.db.execute(
      'INSERT INTO customers (id, email, name) VALUES (?, ?, ?)',
      [event.data.object.id, event.data.object.email, event.data.object.name]
    );
  }
}
```

The developer owns the business logic. The platform owns the infrastructure.

---

## 17. SLM Schema Compiler (Future)

### Vision

A fine-tuned Small Language Model that inspects raw API payloads (Orders, Customers, Charges, etc.) and proposes a relational schema, so users get "smart" plugins that already know what to do with their data — while the plugin runtime stays agnostic.

### Flow

```
1. User connects Stripe → plugin runtime receives sample webhooks
2. SLM inspects raw_payload shapes (charge, customer, order, refund...)
3. SLM proposes: "I'll create customers, charges, orders tables with these columns"
4. User approves → SLM generates CREATE TABLE + INSERT/UPSERT logic
5. Plugin runs autonomously: ctx.ingest() for analytics, ctx.db() for entity state
```

### Why This Works

- Plugin runtime stays a dumb pipe — the SLM generates the plugin code, not the runtime
- The `raw_payload` field (added to tracker core) is what makes this possible — the SLM needs the full nested JSON to infer the relational schema
- The SLM is essentially a **schema compiler**: reads JSON shapes, emits SQL DDL + DML
- Users get zero-config experience; power users can override the generated schema
