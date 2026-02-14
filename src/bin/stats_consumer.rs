//! stats-consumer — Iggy event consumer that aggregates tracking events into Turso.
//!
//! Subscribes to the tracker Iggy stream, reads events, and writes:
//! 1. Pre-aggregated hourly stats (clicks/postbacks/impressions per tenant per hour)
//! 2. Rolling window of recent events per tenant (last N events for debugging)
//!
//! **Batch writes optimization:** Events are accumulated in memory and flushed
//! to Turso in a single HTTP call containing all SQL statements. Stats use an
//! idempotent ledger pattern: INSERT OR IGNORE into `stats_ledger`, then
//! recompute counts from the ledger. This ensures replayed events during
//! consumer rebalance are never double-counted (financial-grade accuracy).
//!
//! Designed to run as a long-lived process alongside tracker-core.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::env;
use std::future::Future;

use base64::Engine;
use iggy::prelude::*;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::event::TrackingEvent;

/// Maximum recent events to keep per tenant (older events are pruned).
const MAX_RECENT_EVENTS_PER_TENANT: u64 = 1000;

/// Maximum number of event IDs to remember for deduplication.
const DEDUP_CAPACITY: usize = 100_000;

/// Maximum number of flush retries before giving up (offset stays uncommitted).
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff between retries (1s, 2s, 4s).
const RETRY_BASE_MS: u64 = 1000;

/// Maximum events to accumulate before flushing to Turso.
const BATCH_SIZE: usize = 1000;

/// Maximum rows per multi-row INSERT statement.
/// Each row is ~500-800 bytes of SQL, so 500 rows ≈ 250-400KB per statement.
/// Keeps individual statements well under Turso's limits.
const MAX_ROWS_PER_INSERT: usize = 500;

/// Maximum time to wait before flushing a partial batch (milliseconds).
const FLUSH_INTERVAL_MS: u64 = 500;

/// Turso HTTP client for writing stats and events.
struct TursoWriter {
    client: reqwest::Client,
    url: String,
    auth_token: String,
}

/// A single request item for the Turso v2/pipeline HTTP API.
#[derive(serde::Serialize)]
struct PipelineRequest {
    #[serde(rename = "type")]
    req_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stmt: Option<PipelineStmt>,
}

/// SQL statement wrapper for the Turso v2/pipeline API.
#[derive(serde::Serialize)]
struct PipelineStmt {
    sql: String,
}

/// Request body for the Turso v2/pipeline HTTP API.
#[derive(serde::Serialize)]
struct TursoPipelineBody {
    requests: Vec<PipelineRequest>,
}

impl TursoWriter {
    fn new(url: String, auth_token: String) -> Self {
        // Normalize libsql:// to https:// and append the HTTP pipeline endpoint
        let base = url
            .replace("libsql://", "https://")
            .trim_end_matches('/')
            .to_string();
        let api_url = if base.contains("/v2/pipeline") {
            base
        } else {
            format!("{base}/v2/pipeline")
        };
        Self {
            client: reqwest::Client::new(),
            url: api_url,
            auth_token,
        }
    }

    /// Execute one or more SQL statements against Turso via its HTTP API.
    async fn execute(&self, statements: Vec<String>) -> Result<(), String> {
        if statements.is_empty() {
            return Ok(());
        }

        let mut requests: Vec<PipelineRequest> = statements
            .into_iter()
            .map(|sql| PipelineRequest {
                req_type: "execute".to_string(),
                stmt: Some(PipelineStmt { sql }),
            })
            .collect();

        // Close the connection after all statements
        requests.push(PipelineRequest {
            req_type: "close".to_string(),
            stmt: None,
        });

        let body = TursoPipelineBody { requests };

        let mut req = self.client.post(&self.url);

        if !self.auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.auth_token));
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Turso request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Turso returned {status}: {text}"));
        }

        Ok(())
    }

    /// Flush a batch of events to Turso in a single HTTP call.
    ///
    /// Uses an idempotent ledger pattern for stats: events are INSERT OR IGNORE'd
    /// into `stats_ledger` (dedup key: tenant_id + event_id), then stats are
    /// recomputed from the ledger via COUNT(*). This ensures replayed events
    /// during consumer rebalance are never double-counted — critical for
    /// financial accuracy.
    ///
    /// Recent events use INSERT OR IGNORE (already idempotent via PK).
    async fn flush_batch(&self, events: &[(String, TrackingEvent)]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }

        let mut statements: Vec<String> = Vec::new();

        // --- Collect affected (tenant, event_type, hour) combos for stats recompute ---
        let mut affected_keys: HashSet<(String, String, u64)> = HashSet::new();
        let mut tenants_seen: HashSet<String> = HashSet::new();

        // --- INSERT OR IGNORE into stats_ledger (idempotent dedup) ---
        let mut ledger_rows: Vec<String> = Vec::with_capacity(events.len());

        for (tenant, event) in events {
            let hour = timestamp_to_hour(event.timestamp);
            let eid = event.event_id.replace('\'', "''");
            let tenant_escaped = tenant.replace('\'', "''");
            let etype = event.event_type.replace('\'', "''");

            ledger_rows.push(format!(
                "('{eid}', '{tenant_escaped}', '{etype}', {hour})"
            ));

            affected_keys.insert((tenant.clone(), event.event_type.clone(), hour));
            tenants_seen.insert(tenant.clone());
        }

        // Chunk ledger inserts to stay under payload limits
        for chunk in ledger_rows.chunks(MAX_ROWS_PER_INSERT) {
            let values = chunk.join(", ");
            statements.push(format!(
                "INSERT OR IGNORE INTO stats_ledger (event_id, tenant_id, event_type, hour) \
                 VALUES {values}"
            ));
        }

        // --- Recompute stats from ledger for each affected (tenant, event_type, hour) ---
        for (tenant, event_type, hour) in &affected_keys {
            let tenant_escaped = tenant.replace('\'', "''");
            let etype_escaped = event_type.replace('\'', "''");
            statements.push(format!(
                "INSERT OR REPLACE INTO stats (tenant_id, event_type, hour, count) \
                 SELECT tenant_id, event_type, hour, COUNT(*) \
                 FROM stats_ledger \
                 WHERE tenant_id = '{tenant_escaped}' AND event_type = '{etype_escaped}' AND hour = {hour} \
                 GROUP BY tenant_id, event_type, hour"
            ));
        }

        // --- Multi-row INSERT for recent events (already idempotent via PK) ---
        let mut value_rows: Vec<String> = Vec::with_capacity(events.len());

        for (tenant, event) in events {
            let params_json = serde_json::to_string(&event.params).unwrap_or_default();
            let ip = event.ip.replace('\'', "''");
            let ua = event.user_agent.replace('\'', "''");
            let referer = event
                .referer
                .as_deref()
                .map(|r| format!("'{}'", r.replace('\'', "''")))
                .unwrap_or_else(|| "NULL".to_string());
            let path = event.request_path.replace('\'', "''");
            let host = event.request_host.replace('\'', "''");
            let params_escaped = params_json.replace('\'', "''");

            value_rows.push(format!(
                "('{eid}_{tenant}', '{tenant}', '{eid}', '{etype}', {ts}, '{ip}', '{ua}', {referer}, '{path}', '{host}', '{params_escaped}')",
                eid = event.event_id,
                etype = event.event_type,
                ts = event.timestamp,
            ));
        }

        // Chunk into multi-row INSERTs to stay under payload limits
        for chunk in value_rows.chunks(MAX_ROWS_PER_INSERT) {
            let values = chunk.join(", ");
            statements.push(format!(
                "INSERT OR IGNORE INTO recent_events \
                 (id, tenant_id, event_id, event_type, timestamp, ip, user_agent, referer, request_path, request_host, params) \
                 VALUES {values}"
            ));
        }

        // --- Prune once per tenant (not per event) ---
        for tenant in &tenants_seen {
            statements.push(format!(
                "DELETE FROM recent_events WHERE tenant_id = '{tenant}' \
                 AND id NOT IN ( \
                   SELECT id FROM recent_events WHERE tenant_id = '{tenant}' \
                   ORDER BY timestamp DESC LIMIT {MAX_RECENT_EVENTS_PER_TENANT} \
                 )"
            ));
        }

        self.execute(statements).await
    }

    /// Prune stats_ledger entries older than the given number of days.
    /// Called periodically (e.g., every hour) to keep ledger size bounded.
    async fn prune_ledger(&self, days: u64) -> Result<(), String> {
        let cutoff_hour = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            // hour = timestamp_ms / 1000 / 3600 * 3600, so cutoff in epoch seconds
            let cutoff_secs = now.saturating_sub(days * 86400);
            cutoff_secs / 3600 * 3600
        };

        self.execute(vec![format!(
            "DELETE FROM stats_ledger WHERE hour < {cutoff_hour}"
        )])
        .await
    }
}

/// Event wrapper that carries Iggy offset metadata through the channel so the
/// writer task can report back which offset to commit after a successful flush.
struct EventWithOffset {
    tenant: String,
    event: TrackingEvent,
    partition_id: u32,
    offset: u64,
}

/// Retry a flush operation with exponential backoff. Returns the result of the
/// first successful attempt, or the last error after all retries are exhausted.
async fn flush_with_retry<F, Fut, T>(f: F) -> Result<T, String>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    for attempt in 0..=MAX_RETRIES {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt == MAX_RETRIES {
                    return Err(e);
                }
                let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                warn!(
                    "Flush attempt {}/{} failed: {} \u{2014} retrying in {}ms",
                    attempt + 1,
                    MAX_RETRIES,
                    e,
                    delay
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    }
    unreachable!()
}

/// Extract the tenant prefix from the event's params.
///
/// The SDK embeds `key_prefix` in the params when generating links.
/// If not present, falls back to `"_global"` for single-tenant mode.
fn extract_tenant_prefix(event: &TrackingEvent) -> String {
    event
        .params
        .get("key_prefix")
        .cloned()
        .unwrap_or_else(|| "_global".to_string())
}

/// Convert a millisecond timestamp to an hour bucket (seconds since epoch, floored to hour).
fn timestamp_to_hour(timestamp_ms: u64) -> u64 {
    let secs = timestamp_ms / 1000;
    secs - (secs % 3600)
}

/// A single message from the Iggy HTTP API.
struct HttpMessage {
    offset: u64,
    payload: Vec<u8>,
}

/// Incrementing counter for unique consumer_id per poll request.
static POLL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Poll messages from Iggy via HTTP API with a unique consumer_id per request.
/// Auto-refreshes the bearer token on 401 Unauthorized.
async fn http_poll_messages(
    http: &reqwest::Client,
    iggy_http_url: &str,
    token: &mut String,
    stream: &str,
    topic: &str,
    partition_id: u32,
    offset: u64,
    count: u32,
) -> Result<Vec<HttpMessage>, String> {
    let cid = POLL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let url = format!(
        "{}/streams/{}/topics/{}/messages?consumer_id={}&partition_id={}&polling_strategy=offset&value={}&count={}",
        iggy_http_url, stream, topic, cid, partition_id, offset, count
    );

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", token.as_str()))
        .send()
        .await
        .map_err(|e| format!("HTTP poll error: {}", e))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        info!("HTTP token expired, refreshing...");
        *token = iggy_http_login(http, iggy_http_url).await?;
        let resp = http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token.as_str()))
            .send()
            .await
            .map_err(|e| format!("HTTP poll error after refresh: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP poll failed after refresh: {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| format!("JSON error: {}", e))?;
        return Ok(parse_http_messages(&body));
    }

    if !resp.status().is_success() {
        return Err(format!("HTTP poll failed: {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("HTTP poll JSON error: {}", e))?;

    Ok(parse_http_messages(&body))
}

/// Parse messages from Iggy HTTP API JSON response.
fn parse_http_messages(body: &serde_json::Value) -> Vec<HttpMessage> {
    body.get("messages")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let offset = m.pointer("/header/offset")?.as_u64()?;
                    let payload_b64 = m.get("payload")?.as_str()?;
                    let payload = base64::engine::general_purpose::STANDARD
                        .decode(payload_b64)
                        .ok()?;
                    Some(HttpMessage { offset, payload })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Login to Iggy HTTP API and return bearer token.
async fn iggy_http_login(http: &reqwest::Client, iggy_http_url: &str) -> Result<String, String> {
    let resp = http
        .post(format!("{}/users/login", iggy_http_url))
        .json(&serde_json::json!({"username": "iggy", "password": "iggy"}))
        .send()
        .await
        .map_err(|e| format!("HTTP login error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP login failed: {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("HTTP login JSON error: {}", e))?;

    body.pointer("/access_token/token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No access_token in login response".to_string())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Starting stats-consumer...");

    let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let iggy_http_url = env::var("IGGY_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
    let iggy_topic = env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into());
    let turso_url = match env::var("TURSO_URL") {
        Ok(u) if !u.is_empty() && u != "CHANGE_ME" => u,
        _ => {
            warn!("TURSO_URL not configured — stats-consumer cannot run. Sleeping forever.");
            loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
        }
    };
    let turso_token = env::var("TURSO_AUTH_TOKEN").unwrap_or_default();

    info!("Iggy: {}  Stream: {}  Topic: {}", iggy_url, iggy_stream, iggy_topic);
    info!("Turso: {}", turso_url);

    // --- Channel to decouple Iggy consumption from Turso writes ---
    // The Iggy consumer's internal polling gets starved if we await Turso HTTP
    // calls on the same task, so we use a channel to separate them.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<EventWithOffset>(10_000);

    // --- Feedback channel: writer → Iggy task (confirmed offsets for manual commit) ---
    // Each ack carries a Vec of (partition_id, max_offset) pairs — one per partition in the batch.
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::channel::<Vec<(u32, u64)>>(100);

    // --- Task 1: Iggy consumer → channel (dedup + deserialize) + commit offsets on ack ---
    let iggy_handle = tokio::spawn(async move {
        // TCP client for offset management only
        let resolved_iggy = tracker_core::producer::resolve_server_addr(&iggy_url).await;
        let client = IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(resolved_iggy.clone())
            .with_auto_sign_in(AutoLogin::Enabled(iggy::prelude::Credentials::UsernamePassword(
                DEFAULT_ROOT_USERNAME.to_string(),
                DEFAULT_ROOT_PASSWORD.to_string(),
            )))
            .build()
            .expect("Failed to build Iggy client");

        client.connect().await.expect("Failed to connect to Iggy");

        // HTTP client for polling (avoids Iggy server-side delivered-offset tracking bug)
        let http = reqwest::Client::new();
        let mut iggy_token = iggy_http_login(&http, &iggy_http_url)
            .await
            .expect("Failed to login to Iggy HTTP API");
        info!("Connected to Iggy TCP={} HTTP={}", iggy_url, iggy_http_url);

        let consumer_id = Consumer::new(Identifier::named("stats-consumer").unwrap());
        let stream_id = Identifier::named(&iggy_stream).unwrap();
        let topic_id = Identifier::named(&iggy_topic).unwrap();

        // Discover partition count
        let topic_info = client
            .get_topic(&stream_id, &topic_id)
            .await
            .expect("Failed to get topic info")
            .unwrap();
        let partition_count = topic_info.partitions_count;
        info!("Topic {} has {} partitions", iggy_topic, partition_count);

        // Load stored offsets for each partition
        let mut next_offset: HashMap<u32, u64> = HashMap::new();
        for pid in 0..partition_count {
            match client
                .get_consumer_offset(&consumer_id, &stream_id, &topic_id, Some(pid))
                .await
            {
                Ok(Some(info)) => {
                    next_offset.insert(pid, info.stored_offset + 1);
                    info!("Partition {}: resuming from offset {}", pid, info.stored_offset + 1);
                }
                Ok(None) => {
                    next_offset.insert(pid, 0);
                    info!("Partition {}: starting from 0", pid);
                }
                Err(e) => {
                    next_offset.insert(pid, 0);
                    warn!("Partition {}: offset error ({}), starting from 0", pid, e);
                }
            }
        }

        let mut seen_ids: HashSet<String> = HashSet::with_capacity(DEDUP_CAPACITY);
        let mut seen_order: VecDeque<String> = VecDeque::with_capacity(DEDUP_CAPACITY);
        let mut events_read: u64 = 0;
        let mut deduped: u64 = 0;
        let mut offsets_committed: u64 = 0;
        let poll_count: u32 = 1000;

        info!(
            "Consuming events (stats-consumer, poll_count={}, HTTP polling, at-least-once)...",
            poll_count
        );

        loop {
            // Drain any pending acks and commit offsets
            while let Ok(partition_offsets) = ack_rx.try_recv() {
                for (partition_id, offset) in partition_offsets {
                    if let Err(e) = client
                        .store_consumer_offset(&consumer_id, &stream_id, &topic_id, Some(partition_id), offset)
                        .await
                    {
                        error!("Failed to commit offset {} for partition {}: {}", offset, partition_id, e);
                    } else {
                        offsets_committed += 1;
                    }
                }
            }

            let mut got_any = false;

            for pid in 0..partition_count {
                let offset = *next_offset.get(&pid).unwrap_or(&0);

                let messages = match http_poll_messages(
                    &http, &iggy_http_url, &mut iggy_token,
                    &iggy_stream, &iggy_topic, pid, offset, poll_count,
                ).await {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Failed to poll partition {}: {}", pid, e);
                        continue;
                    }
                };

                if messages.is_empty() {
                    continue;
                }

                got_any = true;
                let first_off = messages.first().map(|m| m.offset).unwrap_or(0);
                let last_off = messages.last().map(|m| m.offset).unwrap_or(0);
                info!("Polled {} msgs from partition {} offset={}..{}", messages.len(), pid, first_off, last_off);

                for msg in &messages {
                    let msg_offset = msg.offset;

                    let event: TrackingEvent = match serde_json::from_slice(&msg.payload) {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Failed to deserialize at offset {}: {} — skipping", msg_offset, e);
                            next_offset.insert(pid, msg_offset + 1);
                            continue;
                        }
                    };

                    events_read += 1;
                    next_offset.insert(pid, msg_offset + 1);

                    if seen_ids.contains(&event.event_id) {
                        deduped += 1;
                        continue;
                    }

                    if seen_ids.len() >= DEDUP_CAPACITY {
                        if let Some(old) = seen_order.pop_front() {
                            seen_ids.remove(&old);
                        }
                    }
                    seen_ids.insert(event.event_id.clone());
                    seen_order.push_back(event.event_id.clone());

                    let tenant = extract_tenant_prefix(&event);
                    if tx.send(EventWithOffset { tenant, event, partition_id: pid, offset: msg_offset }).await.is_err() {
                        error!("Writer channel closed — stopping consumer");
                        return;
                    }

                    if events_read % 5000 == 0 {
                        info!(
                            "Iggy: read {} events, deduped {}, offsets committed {}",
                            events_read, deduped, offsets_committed
                        );
                    }
                }
            }

            if !got_any {
                // Drain acks when idle
                while let Ok(partition_offsets) = ack_rx.try_recv() {
                    for (partition_id, offset) in partition_offsets {
                        if let Err(e) = client
                            .store_consumer_offset(&consumer_id, &stream_id, &topic_id, Some(partition_id), offset)
                            .await
                        {
                            error!("Failed to commit offset {} for partition {}: {}", offset, partition_id, e);
                        } else {
                            offsets_committed += 1;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    });

    // --- Task 2: Channel → batch → Turso (with retry + offset ack) ---
    let writer = TursoWriter::new(turso_url, turso_token);
    info!("Turso writer ready");

    let mut batch: Vec<EventWithOffset> = Vec::with_capacity(BATCH_SIZE);
    let flush_interval = std::time::Duration::from_millis(FLUSH_INTERVAL_MS);
    let mut last_flush = std::time::Instant::now();
    let mut last_ledger_prune = std::time::Instant::now();
    let mut events_written: u64 = 0;
    let mut flushes: u64 = 0;
    let mut errors: u64 = 0;

    info!("Consuming events (batch_size={}, flush_interval={}ms, at-least-once)...", BATCH_SIZE, FLUSH_INTERVAL_MS);

    loop {
        // Wait for events with a timeout to ensure partial batches get flushed
        let remaining = flush_interval.saturating_sub(last_flush.elapsed());
        let timeout_dur = if remaining.is_zero() {
            std::time::Duration::from_millis(1)
        } else {
            remaining
        };

        match tokio::time::timeout(timeout_dur, rx.recv()).await {
            Ok(Some(event_with_offset)) => {
                batch.push(event_with_offset);
            }
            Ok(None) => {
                // Channel closed — Iggy task ended
                info!("Channel closed, flushing remaining events");
                break;
            }
            Err(_) => {
                // Timeout — flush timer expired
            }
        }

        // Flush when batch is full or timer expired
        if batch.len() >= BATCH_SIZE || (!batch.is_empty() && last_flush.elapsed() >= flush_interval) {
            // Extract the max offset per partition for this batch
            let mut partition_max_offsets: HashMap<u32, u64> = HashMap::new();
            for item in &batch {
                let entry = partition_max_offsets.entry(item.partition_id).or_insert(0);
                if item.offset > *entry {
                    *entry = item.offset;
                }
            }

            // Convert to the format flush_batch expects
            let flush_events: Vec<(String, TrackingEvent)> = batch
                .drain(..)
                .map(|e| (e.tenant, e.event))
                .collect();
            let batch_len = flush_events.len();

            events_written += batch_len as u64;
            match flush_with_retry(|| writer.flush_batch(&flush_events)).await {
                Ok(()) => {
                    flushes += 1;
                    info!(
                        "Flushed {} events (total: {} written, {} flushes, {} errors)",
                        batch_len, events_written, flushes, errors
                    );
                    // Ack all partition offsets — Iggy task will commit each
                    let offsets: Vec<(u32, u64)> = partition_max_offsets.into_iter().collect();
                    let _ = ack_tx.send(offsets).await;
                }
                Err(e) => {
                    error!(
                        "Failed to flush batch of {} events after {} retries: {} — offset NOT committed",
                        batch_len, MAX_RETRIES, e
                    );
                    errors += batch_len as u64;
                }
            }
            last_flush = std::time::Instant::now();
        }

        // Periodic ledger prune (every hour, keep 30 days)
        if last_ledger_prune.elapsed().as_secs() >= 3600 {
            match writer.prune_ledger(30).await {
                Ok(()) => info!("Pruned stats_ledger entries older than 30 days"),
                Err(e) => warn!("Failed to prune stats_ledger: {}", e),
            }
            last_ledger_prune = std::time::Instant::now();
        }
    }

    // Flush remaining
    if !batch.is_empty() {
        let mut partition_max_offsets: HashMap<u32, u64> = HashMap::new();
        for item in &batch {
            let entry = partition_max_offsets.entry(item.partition_id).or_insert(0);
            if item.offset > *entry {
                *entry = item.offset;
            }
        }

        let flush_events: Vec<(String, TrackingEvent)> = batch
            .into_iter()
            .map(|e| (e.tenant, e.event))
            .collect();
        let batch_len = flush_events.len();

        events_written += batch_len as u64;
        match flush_with_retry(|| writer.flush_batch(&flush_events)).await {
            Ok(()) => {
                flushes += 1;
                let offsets: Vec<(u32, u64)> = partition_max_offsets.into_iter().collect();
                let _ = ack_tx.send(offsets).await;
            }
            Err(e) => {
                error!(
                    "Failed to flush final batch of {} events after {} retries: {} — offset NOT committed",
                    batch_len, MAX_RETRIES, e
                );
                errors += batch_len as u64;
            }
        }
    }

    // Drop ack_tx so the Iggy task's ack_rx drains and the task can finish
    drop(ack_tx);

    // Wait for Iggy task to finish
    let _ = iggy_handle.await;

    info!(
        "Consumer done. Total: {} written, {} flushes, {} errors",
        events_written, flushes, errors
    );
}
