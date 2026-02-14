//! risingwave-consumer — Iggy event consumer that writes tracking events to RisingWave.
//!
//! Subscribes to the tracker Iggy stream, reads events, and inserts them into
//! RisingWave via the Postgres wire protocol. RisingWave's materialized views
//! handle all aggregation (hourly stats, totals) automatically with sub-second
//! freshness.
//!
//! Architecture mirrors stats-consumer: Iggy consumer task → mpsc channel → writer task.
//! The writer batches events and uses multi-row INSERT for efficiency.
//!
//! Designed to run alongside (or eventually replace) stats-consumer.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::env;
use std::future::Future;

use base64::Engine;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use iggy::prelude::*;
use tokio::signal;
use tokio_postgres::NoTls;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::event::TrackingEvent;

/// Maximum number of event IDs to remember for deduplication.
const DEDUP_CAPACITY: usize = 100_000;

/// Maximum number of flush retries before giving up (offset stays uncommitted).
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff between retries (1s, 2s, 4s).
const RETRY_BASE_MS: u64 = 1000;

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
                    "Flush attempt {}/{} failed: {} — retrying in {}ms",
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

/// Consumer configuration loaded from environment variables.
struct ConsumerConfig {
    pool_size: usize,
    batch_size: usize,
    flush_interval_ms: u64,
}

impl ConsumerConfig {
    fn from_env() -> Self {
        Self {
            pool_size: env::var("RW_POOL_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            batch_size: env::var("RW_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            flush_interval_ms: env::var("RW_FLUSH_INTERVAL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(250),
        }
    }
}

/// Create a deadpool connection pool for RisingWave.
fn create_pool(connection_string: &str, pool_size: usize) -> Result<Pool, String> {
    let pg_config: tokio_postgres::Config = connection_string
        .parse()
        .map_err(|e| format!("Failed to parse connection string: {e}"))?;

    let use_ssl = connection_string.contains("sslmode=require");

    if use_ssl {
        let tls_connector = native_tls::TlsConnector::builder()
            .build()
            .map_err(|e| format!("Failed to build TLS connector: {e}"))?;
        let connector = postgres_native_tls::MakeTlsConnector::new(tls_connector);

        let mgr_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };
        let mgr = Manager::from_config(pg_config, connector, mgr_config);
        let pool = Pool::builder(mgr)
            .max_size(pool_size)
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| format!("Failed to build pool: {e}"))?;
        Ok(pool)
    } else {
        let mgr_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };
        let mgr = Manager::from_config(pg_config, NoTls, mgr_config);
        let pool = Pool::builder(mgr)
            .max_size(pool_size)
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| format!("Failed to build pool: {e}"))?;
        Ok(pool)
    }
}

/// Create the schema if it doesn't exist.
async fn ensure_schema(pool: &Pool) -> Result<(), String> {
    let client = pool.get().await.map_err(|e| format!("Pool error: {e}"))?;
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS events (
                event_id       VARCHAR PRIMARY KEY,
                tenant_id      VARCHAR NOT NULL,
                event_type     VARCHAR NOT NULL,
                timestamp_ms   BIGINT NOT NULL,
                ip             VARCHAR NOT NULL,
                user_agent     VARCHAR NOT NULL,
                referer        VARCHAR,
                request_path   VARCHAR NOT NULL,
                request_host   VARCHAR NOT NULL,
                params         JSONB NOT NULL DEFAULT '{}',
                raw_payload    JSONB
            );

            CREATE MATERIALIZED VIEW IF NOT EXISTS stats_hourly AS
            SELECT
                tenant_id,
                event_type,
                (timestamp_ms / 1000 / 3600 * 3600) AS hour,
                COUNT(*) AS count
            FROM events
            GROUP BY tenant_id, event_type, (timestamp_ms / 1000 / 3600 * 3600);

            CREATE MATERIALIZED VIEW IF NOT EXISTS stats_total AS
            SELECT
                tenant_id,
                event_type,
                COUNT(*) AS total
            FROM events
            GROUP BY tenant_id, event_type;

            CREATE MATERIALIZED VIEW IF NOT EXISTS stats_hourly_by_link AS
            SELECT
                tenant_id,
                params->>'tu_id' AS tu_id,
                event_type,
                (timestamp_ms / 1000 / 3600 * 3600) AS hour,
                COUNT(*) AS count
            FROM events
            WHERE params->>'tu_id' IS NOT NULL
            GROUP BY tenant_id, params->>'tu_id', event_type, (timestamp_ms / 1000 / 3600 * 3600);",
        )
        .await
        .map_err(|e| format!("Failed to create schema: {:?}", e))?;

    info!("RisingWave schema ensured (events table + materialized views)");
    Ok(())
}

/// Build the INSERT SQL for a batch of events.
fn build_insert_sql(events: &[(String, TrackingEvent)]) -> String {
    let mut values: Vec<String> = Vec::with_capacity(events.len());

    for (tenant, event) in events {
        let params_json = serde_json::to_string(&event.params).unwrap_or_else(|_| "{}".into());
        let eid = escape_sql(&event.event_id);
        let tid = escape_sql(tenant);
        let etype = escape_sql(&event.event_type);
        let ip = escape_sql(&event.ip);
        let ua = escape_sql(&event.user_agent);
        let referer = event
            .referer
            .as_deref()
            .map(|r| format!("'{}'", escape_sql(r)))
            .unwrap_or_else(|| "NULL".to_string());
        let path = escape_sql(&event.request_path);
        let host = escape_sql(&event.request_host);
        let params_escaped = escape_sql(&params_json);
        let raw_payload_sql = event
            .raw_payload
            .as_ref()
            .map(|v| format!("'{}'", escape_sql(&v.to_string())))
            .unwrap_or_else(|| "NULL".to_string());

        values.push(format!(
            "('{eid}', '{tid}', '{etype}', {ts}, '{ip}', '{ua}', {referer}, '{path}', '{host}', '{params_escaped}', {raw_payload_sql})",
            ts = event.timestamp,
        ));
    }

    format!(
        "INSERT INTO events (event_id, tenant_id, event_type, timestamp_ms, ip, user_agent, referer, request_path, request_host, params, raw_payload) \
         VALUES {}",
        values.join(", ")
    )
}

/// Flush a batch of events to RisingWave using a pooled connection.
async fn flush_batch(pool: &Pool, events: &[(String, TrackingEvent)]) -> Result<usize, String> {
    if events.is_empty() {
        return Ok(0);
    }

    let count = events.len();
    let sql = build_insert_sql(events);

    let client = pool.get().await.map_err(|e| format!("Pool error: {e}"))?;

    match client.execute(&sql, &[]).await {
        Ok(_) => Ok(count),
        Err(e) => {
            let err_str = format!("{:?}", e);
            if err_str.contains("duplicate key") {
                warn!("Duplicate key in batch (non-fatal, already deduped in-memory)");
                Ok(count)
            } else {
                let sql_preview = &sql[..sql.len().min(500)];
                Err(format!("Failed to insert events: {:?} | SQL: {}", e, sql_preview))
            }
        }
    }
}

/// Escape single quotes for SQL string literals.
fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// Extract the tenant prefix from the event's params.
fn extract_tenant_prefix(event: &TrackingEvent) -> String {
    event
        .params
        .get("key_prefix")
        .cloned()
        .unwrap_or_else(|| "_global".to_string())
}

/// A single message from the Iggy HTTP API.
struct HttpMessage {
    offset: u64,
    payload: Vec<u8>,
}

/// Incrementing counter for unique consumer_id per poll request.
/// The Iggy server tracks "delivered up to" per consumer_id, causing offset
/// gaps on sequential polls with the same ID. Using a unique ID per request
/// ensures zero server-side state accumulation.
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
        // Retry with new token
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
        let messages = parse_http_messages(&body);
        return Ok(messages);
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

    info!("Starting risingwave-consumer...");

    let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let iggy_http_url = env::var("IGGY_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
    let iggy_topic = env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into());
    let rw_url = match env::var("RISINGWAVE_URL") {
        Ok(u) if !u.is_empty() && u != "CHANGE_ME" => u,
        _ => {
            warn!("RISINGWAVE_URL not configured — risingwave-consumer cannot run. Sleeping forever.");
            loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
        }
    };

    let config = ConsumerConfig::from_env();

    info!("Iggy: {}  Stream: {}  Topic: {}", iggy_url, iggy_stream, iggy_topic);
    info!(
        "RisingWave: pool_size={}, batch_size={}, flush_interval={}ms",
        config.pool_size, config.batch_size, config.flush_interval_ms
    );

    // --- Create connection pool and ensure schema ---
    let pool = create_pool(&rw_url, config.pool_size)
        .expect("Failed to create RisingWave connection pool");
    info!("RisingWave pool created (max_size={})", config.pool_size);

    ensure_schema(&pool)
        .await
        .expect("Failed to ensure RisingWave schema");

    // --- Graceful shutdown signal ---
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    // --- Connect to Iggy (TCP for offset management, HTTP for polling) ---
    // The Iggy TCP binary protocol has internal delivered-offset tracking that
    // causes poll_messages() to skip offsets. The HTTP API does not have this bug.
    // We use TCP only for get/store_consumer_offset, and HTTP for poll_messages.
    let resolved_iggy = tracker_core::producer::resolve_server_addr(&iggy_url).await;
    let client = IggyClientBuilder::new()
        .with_tcp()
        .with_server_address(resolved_iggy.clone())
        .with_auto_sign_in(AutoLogin::Enabled(iggy_common::Credentials::UsernamePassword(
            DEFAULT_ROOT_USERNAME.to_string(),
            DEFAULT_ROOT_PASSWORD.to_string(),
        )))
        .build()
        .expect("Failed to build Iggy client");
    client.connect().await.expect("Failed to connect to Iggy");

    let http = reqwest::Client::new();
    let mut iggy_token = iggy_http_login(&http, &iggy_http_url)
        .await
        .expect("Failed to login to Iggy HTTP API");
    info!("Connected to Iggy TCP={} HTTP={}", iggy_url, iggy_http_url);

    let consumer_id = Consumer::new(Identifier::named("risingwave-consumer").unwrap());
    let stream_id = Identifier::named(&iggy_stream).unwrap();
    let topic_id = Identifier::named(&iggy_topic).unwrap();

    // Discover partition count from the topic
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
                info!(
                    "Partition {}: resuming from offset {} (stored={}, current={})",
                    pid, info.stored_offset + 1, info.stored_offset, info.current_offset
                );
            }
            Ok(None) => {
                next_offset.insert(pid, 0);
                info!("Partition {}: no stored offset, starting from 0", pid);
            }
            Err(e) => {
                next_offset.insert(pid, 0);
                warn!("Partition {}: failed to get offset ({}), starting from 0", pid, e);
            }
        }
    }

    // --- Sequential poll loop using low-level API ---
    // Direct poll_messages() with PollingStrategy::offset() — no SDK internal
    // state, no Stream cancellation issues, full control over offset tracking.

    let batch_size = config.batch_size;
    let poll_count: u32 = 1000; // messages per server poll

    let mut batch: Vec<(String, TrackingEvent)> = Vec::with_capacity(batch_size);
    let mut batch_max_offset: HashMap<u32, u64> = HashMap::new();

    let mut seen_ids: HashSet<String> = HashSet::with_capacity(DEDUP_CAPACITY);
    let mut seen_order: VecDeque<String> = VecDeque::with_capacity(DEDUP_CAPACITY);
    let mut events_read: u64 = 0;
    let mut events_written: u64 = 0;
    let mut deduped: u64 = 0;
    let mut flushes: u64 = 0;

    let flush_interval = std::time::Duration::from_millis(config.flush_interval_ms);
    let mut last_flush = std::time::Instant::now();

    info!(
        "Consuming events (batch_size={}, poll_count={}, pool_size={}, flush_interval={}ms, at-least-once, low-level API)...",
        batch_size, poll_count, config.pool_size, config.flush_interval_ms
    );

    loop {
        if *shutdown_rx.borrow() {
            info!("Shutdown signal received, breaking poll loop...");
            break;
        }

        // Poll each partition round-robin
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
                        // Advance past poison pill
                        next_offset.insert(pid, msg_offset + 1);
                        continue;
                    }
                };

                events_read += 1;
                next_offset.insert(pid, msg_offset + 1);

                // Track max offset for commit
                let entry = batch_max_offset.entry(pid).or_insert(0);
                if msg_offset > *entry { *entry = msg_offset; }

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
                batch.push((tenant, event));
            }

            // Flush to RisingWave when batch is full (NO offset commit here)
            if batch.len() >= batch_size {
                flush_to_rw(
                    &pool, &mut batch, batch_size,
                    &mut events_written, &mut flushes, events_read, deduped,
                ).await;
                last_flush = std::time::Instant::now();
            }
        }

        // Time-based flush: avoid partial batches sitting stale
        if !batch.is_empty() && last_flush.elapsed() >= flush_interval {
            flush_to_rw(
                &pool, &mut batch, batch_size,
                &mut events_written, &mut flushes, events_read, deduped,
            ).await;
            last_flush = std::time::Instant::now();
        }

        // When idle (no new messages from any partition):
        // Commit offsets to Iggy (ONLY here — never between polls)
        if !got_any {

            // Commit offsets only when idle — avoids corrupting poll_messages()
            if !batch_max_offset.is_empty() {
                for (partition_id, offset) in std::mem::take(&mut batch_max_offset) {
                    if let Err(e) = client
                        .store_consumer_offset(&consumer_id, &stream_id, &topic_id, Some(partition_id), offset)
                        .await
                    {
                        error!("Failed to commit offset {} for partition {}: {}", offset, partition_id, e);
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    // --- Graceful shutdown: flush remaining batch + commit offsets ---
    if !batch.is_empty() {
        info!("Flushing remaining {} events before shutdown...", batch.len());
        flush_to_rw(
            &pool, &mut batch, batch_size,
            &mut events_written, &mut flushes, events_read, deduped,
        ).await;
    }

    if !batch_max_offset.is_empty() {
        info!("Committing final offsets before shutdown...");
        for (partition_id, offset) in std::mem::take(&mut batch_max_offset) {
            if let Err(e) = client
                .store_consumer_offset(&consumer_id, &stream_id, &topic_id, Some(partition_id), offset)
                .await
            {
                error!("Failed to commit offset {} for partition {}: {}", offset, partition_id, e);
            }
        }
    }

    info!(
        "risingwave-consumer shut down gracefully. Total: {} written, {} read, {} deduped, {} flushes",
        events_written, events_read, deduped, flushes
    );
}

/// Flush batch to RisingWave only (no offset commit — that happens when idle).
async fn flush_to_rw(
    pool: &Pool,
    batch: &mut Vec<(String, TrackingEvent)>,
    batch_size: usize,
    events_written: &mut u64,
    flushes: &mut u64,
    events_read: u64,
    deduped: u64,
) {
    let flush_events = std::mem::replace(batch, Vec::with_capacity(batch_size));
    let batch_len = flush_events.len();

    match flush_with_retry(|| flush_batch(pool, &flush_events)).await {
        Ok(n) => {
            *events_written += n as u64;
            *flushes += 1;
            info!(
                "Flushed {} events (total: {} written, {} flushes, {} read, {} deduped)",
                n, events_written, flushes, events_read, deduped
            );
        }
        Err(e) => {
            error!(
                "Failed to flush {} events after {} retries: {}",
                batch_len, MAX_RETRIES, e
            );
        }
    }
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
