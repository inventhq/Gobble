//! event-filter — Lightweight event filtering pipeline (our Vector alternative).
//!
//! Reads raw events from the Iggy `events` topic, applies built-in and per-tenant
//! filter rules, and writes clean events to the `events-clean` topic. All downstream
//! consumers read from the clean topic instead of the raw one.
//!
//! Built-in rules (always active):
//!   - Bot user-agent detection (known crawlers, headless browsers)
//!   - Empty user-agent rejection
//!   - Per-IP rate limiting (configurable threshold per minute)
//!
//! Per-tenant rules loaded from Turso `filter_rules` table, hot-reloaded every 30s.
//!
//! Architecture: Iggy consumer task → mpsc channel → filter+produce task.
//! Same at-least-once delivery pattern as other consumers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine;
use iggy::prelude::*;
use tokio::signal;
use tokio::sync::{mpsc as tokio_mpsc, RwLock};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::event::TrackingEvent;
use tracker_core::health::{HealthCounters, spawn_health_server};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of event IDs to remember for deduplication.
const DEDUP_CAPACITY: usize = 100_000;


/// Default per-IP rate limit (events per minute). 0 = disabled.
const DEFAULT_IP_RATE_LIMIT: u32 = 200;

/// How often to reload per-tenant filter rules from Turso (seconds).
const RULES_RELOAD_INTERVAL_SECS: u64 = 30;

/// IP rate window in seconds.
const IP_RATE_WINDOW_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Bot detection — known bot user-agent substrings (lowercase)
// ---------------------------------------------------------------------------

const BOT_UA_PATTERNS: &[&str] = &[
    "bot", "crawler", "spider", "scraper", "wget", "curl",
    "headlesschrome", "phantomjs", "selenium", "puppeteer",
    "python-requests", "python-urllib", "go-http-client",
    "java/", "apache-httpclient", "okhttp", "libwww-perl",
    "mechanize", "scrapy", "httpclient", "feedfetcher",
    "facebookexternalhit", "twitterbot", "linkedinbot",
    "slackbot", "discordbot", "telegrambot", "whatsapp",
    "googlebot", "bingbot", "yandexbot", "baiduspider",
    "duckduckbot", "sogou", "exabot", "ia_archiver",
    "semrushbot", "ahrefsbot", "mj12bot", "dotbot",
    "rogerbot", "seznambot", "archive.org_bot",
];

// ---------------------------------------------------------------------------
// Event wrapper with offset metadata
// ---------------------------------------------------------------------------

struct EventWithOffset {
    tenant: String,
    event: TrackingEvent,
    partition_id: u32,
    offset: u64,
}

// ---------------------------------------------------------------------------
// Filter rules (per-tenant, loaded from Turso)
// ---------------------------------------------------------------------------

/// A single filter rule loaded from the `filter_rules` table.
#[derive(Debug, Clone)]
struct FilterRule {
    tenant_id: String,
    field: String,       // "user_agent", "referer", "ip", "param:<key>"
    operator: String,    // "contains", "equals", "is_empty", "matches"
    value: String,       // pattern to match against
    action: String,      // "drop" or "flag"
}

/// Thread-safe rule store, hot-reloaded from Turso.
type RuleStore = Arc<RwLock<Vec<FilterRule>>>;

// ---------------------------------------------------------------------------
// Count-Min Sketch — fixed-memory probabilistic frequency counter
// ---------------------------------------------------------------------------

/// Number of hash functions (rows). More rows = lower false positive rate.
const CMS_DEPTH: usize = 4;
/// Width of each row. 65536 slots × 4 rows × 4 bytes = 1 MB per sketch.
const CMS_WIDTH: usize = 65_536;

/// Count-Min Sketch: O(1) increment and query, fixed memory regardless of
/// cardinality. At 1M unique IPs, expected over-count is ~15 per slot with
/// 4 rows × 64K width — well within acceptable bounds for rate limiting.
struct CountMinSketch {
    table: Vec<Vec<u32>>,
}

impl CountMinSketch {
    fn new() -> Self {
        Self {
            table: vec![vec![0u32; CMS_WIDTH]; CMS_DEPTH],
        }
    }

    /// Hash an IP string into a slot index for a given row.
    /// Uses FNV-1a with row index as seed differentiation.
    fn hash_slot(&self, ip: &str, row: usize) -> usize {
        let mut h: u64 = 14695981039346656037u64.wrapping_add(row as u64 * 2654435761);
        for byte in ip.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(1099511628211);
        }
        (h as usize) % CMS_WIDTH
    }

    /// Increment the count for an IP. Returns the estimated count (minimum
    /// across all rows — the Count-Min guarantee).
    fn increment(&mut self, ip: &str) -> u32 {
        let mut min_count = u32::MAX;
        for row in 0..CMS_DEPTH {
            let slot = self.hash_slot(ip, row);
            self.table[row][slot] = self.table[row][slot].saturating_add(1);
            min_count = min_count.min(self.table[row][slot]);
        }
        min_count
    }

    /// Query the estimated count without incrementing.
    fn query(&self, ip: &str) -> u32 {
        let mut min_count = u32::MAX;
        for row in 0..CMS_DEPTH {
            let slot = self.hash_slot(ip, row);
            min_count = min_count.min(self.table[row][slot]);
        }
        min_count
    }

    /// Reset all counters to zero. Called at the start of each time window.
    fn reset(&mut self) {
        for row in &mut self.table {
            for slot in row.iter_mut() {
                *slot = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IP rate limiter — Count-Min Sketch with tumbling window
// ---------------------------------------------------------------------------

/// Fixed-memory IP rate limiter using a Count-Min Sketch with tumbling windows.
///
/// Instead of tracking per-IP sliding windows (O(n) memory), we use two
/// alternating CMS instances — "current" and "previous" window. The estimated
/// count is `current + previous` (conservative, handles window boundaries).
/// Every `window_secs`, current becomes previous and a fresh sketch starts.
///
/// Memory: 2 × (4 rows × 64K slots × 4 bytes) = 2 MB fixed, regardless of
/// how many unique IPs we see. Handles 1M+ unique IPs/sec with zero allocations
/// in the hot path.
struct IpRateLimiter {
    current: CountMinSketch,
    previous: CountMinSketch,
    limit: u32,
    window_secs: u64,
    window_start: u64,
}

impl IpRateLimiter {
    fn new(limit: u32, window_secs: u64) -> Self {
        Self {
            current: CountMinSketch::new(),
            previous: CountMinSketch::new(),
            limit,
            window_secs,
            window_start: 0,
        }
    }

    /// Returns true if the IP exceeds the rate limit.
    /// O(1) time, zero allocations.
    fn is_rate_limited(&mut self, ip: &str, now_secs: u64) -> bool {
        if self.limit == 0 {
            return false;
        }

        // Rotate windows if needed
        if self.window_start == 0 {
            self.window_start = now_secs;
        }
        if now_secs >= self.window_start + self.window_secs {
            // Swap: current → previous, reset current
            std::mem::swap(&mut self.current, &mut self.previous);
            self.current.reset();
            self.window_start = now_secs;
        }

        // Increment in current window and get estimated count
        let current_count = self.current.increment(ip);
        // Add previous window count for cross-boundary accuracy
        let prev_count = self.previous.query(ip);
        let total = current_count.saturating_add(prev_count);

        total > self.limit
    }

    /// No-op — CMS doesn't need cleanup. Fixed memory.
    fn cleanup(&mut self, _now_secs: u64) {}
}

// ---------------------------------------------------------------------------
// Filter logic
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum FilterResult {
    Pass,
    Drop(String), // reason
}

/// Apply built-in filters (bot UA, empty UA, IP rate).
fn apply_builtin_filters(
    event: &TrackingEvent,
    ip_limiter: &mut IpRateLimiter,
) -> FilterResult {
    // 1. Empty user-agent
    if event.user_agent.is_empty() {
        return FilterResult::Drop("empty_user_agent".into());
    }

    // 2. Bot user-agent
    let ua_lower = event.user_agent.to_lowercase();
    for pattern in BOT_UA_PATTERNS {
        if ua_lower.contains(pattern) {
            return FilterResult::Drop(format!("bot_ua:{}", pattern));
        }
    }

    // 3. IP rate limit
    let now_secs = event.timestamp / 1000;
    if ip_limiter.is_rate_limited(&event.ip, now_secs) {
        return FilterResult::Drop(format!("ip_rate_limit:{}", event.ip));
    }

    FilterResult::Pass
}

/// Apply per-tenant custom rules.
fn apply_tenant_rules(event: &TrackingEvent, tenant: &str, rules: &[FilterRule]) -> FilterResult {
    for rule in rules {
        if rule.tenant_id != "*" && rule.tenant_id != tenant {
            continue;
        }

        let field_value = match rule.field.as_str() {
            "user_agent" => Some(event.user_agent.as_str()),
            "referer" => event.referer.as_deref(),
            "ip" => Some(event.ip.as_str()),
            "request_path" => Some(event.request_path.as_str()),
            "request_host" => Some(event.request_host.as_str()),
            "event_type" => Some(event.event_type.as_str()),
            f if f.starts_with("param:") => {
                let key = &f[6..];
                event.params.get(key).map(|v| v.as_str())
            }
            _ => None,
        };

        let matched = match rule.operator.as_str() {
            "contains" => field_value
                .map_or(false, |v| v.to_lowercase().contains(&rule.value.to_lowercase())),
            "equals" => field_value.map_or(false, |v| v == rule.value),
            "is_empty" => field_value.map_or(true, |v| v.is_empty()),
            "not_empty" => field_value.map_or(false, |v| !v.is_empty()),
            "starts_with" => field_value
                .map_or(false, |v| v.to_lowercase().starts_with(&rule.value.to_lowercase())),
            _ => false,
        };

        if matched && rule.action == "drop" {
            return FilterResult::Drop(format!(
                "rule:{}:{}:{}",
                rule.field, rule.operator, rule.value
            ));
        }
    }

    FilterResult::Pass
}

// ---------------------------------------------------------------------------
// Turso rule loader
// ---------------------------------------------------------------------------

async fn load_rules_from_turso(turso_url: &str, turso_token: &str) -> Vec<FilterRule> {
    let client = reqwest::Client::new();
    let url = turso_url.replace("libsql://", "https://");

    let body = serde_json::json!({
        "requests": [
            {"type": "execute", "stmt": {"sql": "SELECT tenant_id, field, operator, value, action FROM filter_rules WHERE active = 1"}},
            {"type": "close"}
        ]
    });

    match client
        .post(format!("{}/v2/pipeline", url))
        .header("Authorization", format!("Bearer {}", turso_token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                warn!("Turso filter_rules query failed: {}", resp.status());
                return vec![];
            }
            match resp.json::<serde_json::Value>().await {
                Ok(data) => parse_turso_rules(&data),
                Err(e) => {
                    warn!("Failed to parse Turso response: {}", e);
                    vec![]
                }
            }
        }
        Err(e) => {
            warn!("Failed to fetch filter_rules from Turso: {}", e);
            vec![]
        }
    }
}

fn parse_turso_rules(data: &serde_json::Value) -> Vec<FilterRule> {
    let mut rules = Vec::new();

    // Turso v2 pipeline response: { "results": [ { "response": { "result": { "rows": [...] } } } ] }
    if let Some(results) = data.get("results").and_then(|r| r.as_array()) {
        for result in results {
            if let Some(rows) = result
                .pointer("/response/result/rows")
                .and_then(|r| r.as_array())
            {
                for row in rows {
                    if let Some(cols) = row.as_array() {
                        if cols.len() >= 5 {
                            let get_str = |idx: usize| -> String {
                                cols[idx]
                                    .get("value")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            rules.push(FilterRule {
                                tenant_id: get_str(0),
                                field: get_str(1),
                                operator: get_str(2),
                                value: get_str(3),
                                action: get_str(4),
                            });
                        }
                    }
                }
            }
        }
    }

    rules
}


// ---------------------------------------------------------------------------
// Extract tenant prefix
// ---------------------------------------------------------------------------

fn extract_tenant_prefix(event: &TrackingEvent) -> String {
    event
        .params
        .get("key_prefix")
        .cloned()
        .unwrap_or_else(|| "_global".to_string())
}

// ---------------------------------------------------------------------------
// HTTP polling helpers (avoids Iggy server-side delivered-offset tracking bug)
// ---------------------------------------------------------------------------

/// A single message from the Iggy HTTP API.
struct HttpMessage {
    offset: u64,
    payload: Vec<u8>,
}

/// Incrementing counter for unique consumer_id per poll request.
static POLL_SEQ: AtomicU64 = AtomicU64::new(1);

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
    let cid = POLL_SEQ.fetch_add(1, Ordering::Relaxed);
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

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Starting event-filter...");

    // --- Config ---
    let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let iggy_http_url = env::var("IGGY_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
    let iggy_topic_raw = env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into());
    let iggy_topic_clean = env::var("IGGY_TOPIC_CLEAN").unwrap_or_else(|_| "events-clean".into());
    let batch_size: usize = env::var("FILTER_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let flush_interval_ms: u64 = env::var("FILTER_FLUSH_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    let ip_rate_limit: u32 = env::var("FILTER_IP_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_IP_RATE_LIMIT);

    let turso_url = env::var("TURSO_URL").ok();
    let turso_token = env::var("TURSO_AUTH_TOKEN").ok();

    info!("Iggy: {}  Stream: {}  Raw topic: {}  Clean topic: {}", iggy_url, iggy_stream, iggy_topic_raw, iggy_topic_clean);
    info!("Batch size: {}  Flush interval: {}ms  IP rate limit: {}/min", batch_size, flush_interval_ms, ip_rate_limit);

    // --- Load initial rules from Turso ---
    let rules: RuleStore = Arc::new(RwLock::new(Vec::new()));
    if let (Some(ref url), Some(ref token)) = (&turso_url, &turso_token) {
        let initial_rules = load_rules_from_turso(url, token).await;
        info!("Loaded {} filter rules from Turso", initial_rules.len());
        *rules.write().await = initial_rules;
    } else {
        info!("No TURSO_URL configured — running with built-in rules only");
    }

    // --- Background rule reloader ---
    let rules_clone = rules.clone();
    let turso_url_clone = turso_url.clone();
    let turso_token_clone = turso_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(RULES_RELOAD_INTERVAL_SECS));
        loop {
            interval.tick().await;
            if let (Some(ref url), Some(ref token)) = (&turso_url_clone, &turso_token_clone) {
                let new_rules = load_rules_from_turso(url, token).await;
                let count = new_rules.len();
                *rules_clone.write().await = new_rules;
                info!("Reloaded {} filter rules from Turso", count);
            }
        }
    });

    // --- Connect Iggy producer for clean topic ---
    let resolved_iggy = tracker_core::producer::resolve_server_addr(&iggy_url).await;
    let producer_client = IggyClientBuilder::new()
        .with_tcp()
        .with_server_address(resolved_iggy.clone())
        .with_auto_sign_in(AutoLogin::Enabled(iggy_common::Credentials::UsernamePassword(
            DEFAULT_ROOT_USERNAME.to_string(),
            DEFAULT_ROOT_PASSWORD.to_string(),
        )))
        .build()
        .expect("Failed to build Iggy producer client");

    producer_client.connect().await.expect("Failed to connect producer to Iggy");

    // Ensure stream exists (idempotent)
    match producer_client.create_stream(&iggy_stream).await {
        Ok(_) => info!("Created stream: {}", iggy_stream),
        Err(_) => {} // already exists
    }

    let producer = producer_client
        .producer(&iggy_stream, &iggy_topic_clean)
        .expect("Failed to create producer builder")
        .background(
            BackgroundConfig::builder()
                .batch_length(batch_size)
                .linger_time(IggyDuration::from(1))
                .build(),
        )
        .partitioning(Partitioning::balanced())
        .create_topic_if_not_exists(
            24, // 24 partitions for horizontal scaling (matches events topic)
            None,
            IggyExpiry::NeverExpire,
            MaxTopicSize::ServerDefault,
        )
        .build();

    producer.init().await.expect("Failed to init Iggy producer");
    info!("Iggy producer initialized for clean topic '{}'", iggy_topic_clean);

    // --- Graceful shutdown signal ---
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    // --- Channel to decouple Iggy consumption from filtering ---
    let (tx, mut rx) = tokio_mpsc::channel::<EventWithOffset>(10_000);

    // --- Feedback channel: filter task → Iggy consumer (confirmed offsets) ---
    let (ack_tx, mut ack_rx) = tokio_mpsc::channel::<Vec<(u32, u64)>>(100);

    // --- Task 1: Iggy consumer (raw topic) → channel ---
    let iggy_handle = tokio::spawn(async move {
        let shutdown_rx = shutdown_rx; // move into task

        // TCP client for offset management only
        let client = IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(resolved_iggy.clone())
            .with_auto_sign_in(AutoLogin::Enabled(iggy_common::Credentials::UsernamePassword(
                DEFAULT_ROOT_USERNAME.to_string(),
                DEFAULT_ROOT_PASSWORD.to_string(),
            )))
            .build()
            .expect("Failed to build Iggy consumer client");

        client.connect().await.expect("Failed to connect to Iggy");

        // HTTP client for polling (avoids Iggy server-side delivered-offset tracking bug)
        let http = reqwest::Client::new();
        let mut iggy_token = iggy_http_login(&http, &iggy_http_url)
            .await
            .expect("Failed to login to Iggy HTTP API");
        info!("Connected to Iggy TCP={} HTTP={}", iggy_url, iggy_http_url);

        let consumer_id = Consumer::new(Identifier::named("event-filter").unwrap());
        let stream_id = Identifier::named(&iggy_stream).unwrap();
        let topic_id = Identifier::named(&iggy_topic_raw).unwrap();

        // Discover partition count
        let topic_info = client
            .get_topic(&stream_id, &topic_id)
            .await
            .expect("Failed to get topic info")
            .unwrap();
        let partition_count = topic_info.partitions_count;
        info!("Topic {} has {} partitions", iggy_topic_raw, partition_count);

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
            "Consuming events (event-filter, poll_count={}, HTTP polling, at-least-once)...",
            poll_count
        );

        loop {
            if *shutdown_rx.borrow() {
                info!("Shutdown signal received in Iggy consumer task, stopping...");
                break;
            }

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
                    &iggy_stream, &iggy_topic_raw, pid, offset, poll_count,
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
                        error!("Filter channel closed — stopping consumer");
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

    // --- Health server ---
    let health_port: u16 = env::var("HEALTH_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3043);
    let health = HealthCounters::new("event-filter", &[
        "events_passed", "events_dropped", "poll_errors",
    ]);
    spawn_health_server(health.clone(), health_port);

    // --- Task 2: Filter + produce to clean topic ---
    let events_passed = Arc::new(AtomicU64::new(0));
    let events_dropped = Arc::new(AtomicU64::new(0));
    let mut ip_limiter = IpRateLimiter::new(ip_rate_limit, IP_RATE_WINDOW_SECS);
    let mut last_cleanup = std::time::Instant::now();
    let mut last_stats = std::time::Instant::now();

    // Passed events to produce to clean topic
    let mut pass_batch: Vec<EventWithOffset> = Vec::with_capacity(batch_size);
    // All offsets (passed + dropped) for commit tracking
    let mut offset_tracker: HashMap<u32, u64> = HashMap::new();
    let mut batch_count: usize = 0;

    let flush_interval = std::time::Duration::from_millis(flush_interval_ms);
    let mut last_flush = std::time::Instant::now();

    // Drop reason counters for periodic logging
    let mut drop_reasons: HashMap<String, u64> = HashMap::new();

    info!(
        "Filtering events (batch_size={}, flush_interval={}ms, ip_rate_limit={}/min, at-least-once)...",
        batch_size, flush_interval_ms, ip_rate_limit
    );

    loop {
        let remaining = flush_interval.saturating_sub(last_flush.elapsed());
        let timeout_dur = if remaining.is_zero() {
            std::time::Duration::from_millis(1)
        } else {
            remaining
        };

        match tokio::time::timeout(timeout_dur, rx.recv()).await {
            Ok(Some(ewo)) => {
                // Track offset regardless of pass/drop (for commit)
                let entry = offset_tracker.entry(ewo.partition_id).or_insert(0);
                if ewo.offset > *entry {
                    *entry = ewo.offset;
                }
                batch_count += 1;

                // Apply filters
                let rules_snapshot = rules.read().await;

                let result = apply_builtin_filters(&ewo.event, &mut ip_limiter);
                let result = match result {
                    FilterResult::Pass => apply_tenant_rules(&ewo.event, &ewo.tenant, &rules_snapshot),
                    drop_result => drop_result,
                };

                drop(rules_snapshot);

                match result {
                    FilterResult::Pass => {
                        events_passed.fetch_add(1, Ordering::Relaxed);
                        health.set("events_passed", events_passed.load(Ordering::Relaxed));
                        pass_batch.push(ewo);
                    }
                    FilterResult::Drop(reason) => {
                        events_dropped.fetch_add(1, Ordering::Relaxed);
                        health.set("events_dropped", events_dropped.load(Ordering::Relaxed));
                        *drop_reasons.entry(reason).or_insert(0) += 1;
                        // Event is dropped — offset tracked above, event discarded
                    }
                }
            }
            Ok(None) => {
                info!("Channel closed, flushing remaining events");
                break;
            }
            Err(_) => {
                // Timeout — flush timer expired
            }
        }

        // Periodic IP limiter cleanup (every 60s)
        if last_cleanup.elapsed().as_secs() >= 60 {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            ip_limiter.cleanup(now_secs);
            last_cleanup = std::time::Instant::now();
        }

        // Periodic stats logging (every 30s)
        if last_stats.elapsed().as_secs() >= 30 {
            let passed = events_passed.load(Ordering::Relaxed);
            let dropped = events_dropped.load(Ordering::Relaxed);
            let total = passed + dropped;
            let drop_rate = if total > 0 { dropped as f64 / total as f64 * 100.0 } else { 0.0 };
            info!(
                "Filter stats: {} passed, {} dropped ({:.1}% drop rate), CMS {}x{} ({}KB fixed)",
                passed, dropped, drop_rate, CMS_DEPTH, CMS_WIDTH, CMS_DEPTH * CMS_WIDTH * 4 * 2 / 1024
            );
            if !drop_reasons.is_empty() {
                let mut sorted: Vec<_> = drop_reasons.iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                let top5: Vec<String> = sorted.iter().take(5).map(|(k, v)| format!("{}:{}", k, v)).collect();
                info!("Top drop reasons: {}", top5.join(", "));
            }
            last_stats = std::time::Instant::now();
        }

        // Flush when batch is full or timer expired
        if batch_count >= batch_size
            || (batch_count > 0 && last_flush.elapsed() >= flush_interval)
        {
            // Produce passed events to clean topic
            if !pass_batch.is_empty() {
                let mut produce_errors = 0u64;
                for ewo in pass_batch.drain(..) {
                    let payload = ewo.event.to_bytes();
                    match IggyMessage::builder()
                        .payload(bytes::Bytes::from(payload))
                        .build()
                    {
                        Ok(msg) => {
                            if let Err(e) = producer.send(vec![msg]).await {
                                error!("Failed to produce to clean topic: {}", e);
                                produce_errors += 1;
                            }
                        }
                        Err(e) => {
                            error!("Failed to build IggyMessage: {}", e);
                            produce_errors += 1;
                        }
                    }
                }
                if produce_errors > 0 {
                    error!("{} events failed to produce to clean topic", produce_errors);
                }
            }

            // Commit all offsets (passed + dropped)
            let offsets: Vec<(u32, u64)> = offset_tracker.drain().collect();
            if !offsets.is_empty() {
                let _ = ack_tx.send(offsets).await;
            }

            pass_batch.clear();
            batch_count = 0;
            last_flush = std::time::Instant::now();
        }
    }

    // Flush remaining
    if !pass_batch.is_empty() {
        for ewo in pass_batch.drain(..) {
            let payload = ewo.event.to_bytes();
            if let Ok(msg) = IggyMessage::builder()
                .payload(bytes::Bytes::from(payload))
                .build()
            {
                let _ = producer.send(vec![msg]).await;
            }
        }
    }
    if !offset_tracker.is_empty() {
        let offsets: Vec<(u32, u64)> = offset_tracker.drain().collect();
        let _ = ack_tx.send(offsets).await;
    }

    // Drop ack_tx so the Iggy task's ack_rx drains and the task can finish
    drop(ack_tx);

    let _ = iggy_handle.await;

    info!(
        "Event-filter done. Total: {} passed, {} dropped",
        events_passed.load(Ordering::Relaxed),
        events_dropped.load(Ordering::Relaxed)
    );
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
