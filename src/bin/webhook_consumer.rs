//! webhook-consumer — Iggy event consumer that dispatches webhooks to registered endpoints.
//!
//! Subscribes to the tracker Iggy stream, reads events, and for each event:
//! 1. Looks up registered webhooks for the tenant + event_type from Turso
//! 2. POSTs the event payload to each webhook URL with an HMAC-SHA256 signature
//! 3. Retries failed deliveries with exponential backoff (max 3 attempts)
//! 4. Logs delivery results to the `webhook_deliveries` table
//!
//! Designed to run as a long-lived process alongside tracker-core and stats-consumer.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::env;
use std::str::FromStr;
use std::time::Duration;

use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use iggy::prelude::*;
use sha2::Sha256;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::event::TrackingEvent;
use tracker_core::health::{HealthCounters, spawn_health_server};

/// Maximum delivery attempts per webhook per event.
const MAX_ATTEMPTS: u32 = 3;

/// Maximum number of event IDs to remember for deduplication.
/// Covers a wide enough window to catch cross-partition duplicates.
const DEDUP_CAPACITY: usize = 100_000;

/// Base delay for exponential backoff between retries.
const RETRY_BASE_DELAY_MS: u64 = 500;

/// Timeout for each webhook HTTP POST.
const WEBHOOK_TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Turso HTTP client (same pattern as stats-consumer)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct TursoStatement {
    q: String,
}

#[derive(serde::Serialize)]
struct TursoRequest {
    statements: Vec<TursoStatement>,
}

/// Turso query response structures for deserializing webhook rows.
#[derive(serde::Deserialize)]
struct TursoResponse {
    results: TursoResults,
}

#[derive(serde::Deserialize)]
struct TursoResults {
    #[allow(dead_code)]
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

/// A registered webhook loaded from Turso.
#[derive(Debug, Clone)]
struct Webhook {
    id: String,
    url: String,
    secret: String,
    #[allow(dead_code)]
    event_types: Vec<String>,
}

struct TursoClient {
    http: reqwest::Client,
    url: String,
    auth_token: String,
}

impl TursoClient {
    fn new(url: String, auth_token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            auth_token,
        }
    }

    /// Execute SQL and return raw response body.
    async fn query(&self, sql: &str) -> Result<Vec<TursoResponse>, String> {
        let body = TursoRequest {
            statements: vec![TursoStatement { q: sql.to_string() }],
        };

        let mut req = self.http.post(&self.url);
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

        resp.json::<Vec<TursoResponse>>()
            .await
            .map_err(|e| format!("Failed to parse Turso response: {e}"))
    }

    /// Execute SQL without caring about the response.
    async fn execute(&self, statements: Vec<String>) -> Result<(), String> {
        let body = TursoRequest {
            statements: statements
                .into_iter()
                .map(|q| TursoStatement { q })
                .collect(),
        };

        let mut req = self.http.post(&self.url);
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

    /// Fetch active webhooks for a tenant that match the given event_type.
    async fn get_webhooks_for_tenant(
        &self,
        tenant_id: &str,
        event_type: &str,
    ) -> Result<Vec<Webhook>, String> {
        // tenant_id in webhooks table is the tenant row ID, but the consumer
        // only has key_prefix. We need to resolve prefix → tenant ID first.
        let sql = format!(
            "SELECT w.id, w.url, w.secret, w.event_types \
             FROM webhooks w \
             JOIN tenants t ON w.tenant_id = t.id \
             WHERE t.key_prefix = '{}' AND w.active = 1",
            tenant_id.replace('\'', "''")
        );

        let responses = self.query(&sql).await?;
        let Some(response) = responses.into_iter().next() else {
            return Ok(vec![]);
        };

        let mut webhooks = Vec::new();
        for row in &response.results.rows {
            if row.len() < 4 {
                continue;
            }

            let id = row[0].as_str().unwrap_or_default().to_string();
            let url = row[1].as_str().unwrap_or_default().to_string();
            let secret = row[2].as_str().unwrap_or_default().to_string();
            let event_types_raw = row[3].as_str().unwrap_or("[]");

            let event_types: Vec<String> =
                serde_json::from_str(event_types_raw).unwrap_or_default();

            // Check if this webhook subscribes to the event_type
            if event_types.contains(&"*".to_string())
                || event_types.contains(&event_type.to_string())
            {
                webhooks.push(Webhook {
                    id,
                    url,
                    secret,
                    event_types,
                });
            }
        }

        Ok(webhooks)
    }

    /// Check if a successful delivery already exists for this webhook + event combo.
    /// Used to skip duplicate dispatches during consumer rebalance replay.
    async fn delivery_exists(&self, webhook_id: &str, event_id: &str) -> Result<bool, String> {
        let sql = format!(
            "SELECT 1 FROM webhook_deliveries \
             WHERE webhook_id = '{}' AND event_id = '{}' AND status_code >= 200 AND status_code < 300 \
             LIMIT 1",
            webhook_id.replace('\'', "''"),
            event_id.replace('\'', "''")
        );

        let responses = self.query(&sql).await?;
        if let Some(response) = responses.into_iter().next() {
            Ok(!response.results.rows.is_empty())
        } else {
            Ok(false)
        }
    }

    /// Log a webhook delivery attempt.
    async fn log_delivery(
        &self,
        delivery_id: &str,
        webhook_id: &str,
        event_id: &str,
        status_code: Option<u16>,
        attempt: u32,
        error_msg: Option<&str>,
    ) -> Result<(), String> {
        let status = status_code
            .map(|s| s.to_string())
            .unwrap_or_else(|| "NULL".to_string());
        let err = error_msg
            .map(|e| format!("'{}'", e.replace('\'', "''")))
            .unwrap_or_else(|| "NULL".to_string());

        let sql = format!(
            "INSERT INTO webhook_deliveries (id, webhook_id, event_id, status_code, attempt, error) \
             VALUES ('{delivery_id}', '{webhook_id}', '{event_id}', {status}, {attempt}, {err})"
        );

        self.execute(vec![sql]).await
    }
}

// ---------------------------------------------------------------------------
// Webhook dispatcher
// ---------------------------------------------------------------------------

/// Sign a webhook payload with HMAC-SHA256 using the webhook's secret.
fn sign_payload(secret: &str, payload: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload);
    hex::encode(mac.finalize().into_bytes())
}

/// Dispatch a single event to a single webhook URL with retries.
///
/// Returns `true` if delivery succeeded, `false` if all attempts failed.
async fn dispatch_webhook(
    http: &reqwest::Client,
    turso: &TursoClient,
    webhook: &Webhook,
    event: &TrackingEvent,
    payload_json: &str,
) -> bool {
    let signature = sign_payload(&webhook.secret, payload_json.as_bytes());

    for attempt in 1..=MAX_ATTEMPTS {
        let delivery_id = format!(
            "{}_{}_{}",
            event.event_id,
            webhook.id,
            attempt
        );

        let result = http
            .post(&webhook.url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Signature", &signature)
            .header("X-Webhook-Id", &webhook.id)
            .header("X-Event-Id", &event.event_id)
            .header("X-Event-Type", &event.event_type)
            .body(payload_json.to_string())
            .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let success = resp.status().is_success();

                // Log delivery
                let err_msg = if success {
                    None
                } else {
                    Some(format!("HTTP {status}"))
                };

                if let Err(e) = turso
                    .log_delivery(
                        &delivery_id,
                        &webhook.id,
                        &event.event_id,
                        Some(status),
                        attempt,
                        err_msg.as_deref(),
                    )
                    .await
                {
                    warn!("Failed to log delivery: {}", e);
                }

                if success {
                    return true;
                }

                warn!(
                    "Webhook {} returned {} for event {} (attempt {}/{})",
                    webhook.url, status, event.event_id, attempt, MAX_ATTEMPTS
                );
            }
            Err(e) => {
                let err_msg = format!("{e}");
                if let Err(log_err) = turso
                    .log_delivery(
                        &delivery_id,
                        &webhook.id,
                        &event.event_id,
                        None,
                        attempt,
                        Some(&err_msg),
                    )
                    .await
                {
                    warn!("Failed to log delivery: {}", log_err);
                }

                warn!(
                    "Webhook {} failed for event {}: {} (attempt {}/{})",
                    webhook.url, event.event_id, e, attempt, MAX_ATTEMPTS
                );
            }
        }

        // Exponential backoff before retry
        if attempt < MAX_ATTEMPTS {
            let delay = RETRY_BASE_DELAY_MS * 2u64.pow(attempt - 1);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }

    error!(
        "Webhook {} exhausted all {} attempts for event {}",
        webhook.url, MAX_ATTEMPTS, event.event_id
    );
    false
}

// ---------------------------------------------------------------------------
// Tenant prefix extraction (same as stats-consumer)
// ---------------------------------------------------------------------------

fn extract_tenant_prefix(event: &TrackingEvent) -> String {
    event
        .params
        .get("key_prefix")
        .cloned()
        .unwrap_or_else(|| "_global".to_string())
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

    info!("Starting webhook-consumer...");

    let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
    let iggy_topic = env::var("IGGY_TOPIC_CLEAN").unwrap_or_else(|_| "events-clean".into());
    let turso_url = match env::var("TURSO_URL") {
        Ok(u) if !u.is_empty() && u != "CHANGE_ME" => u,
        _ => {
            warn!("TURSO_URL not configured — webhook-consumer cannot run. Sleeping forever.");
            loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
        }
    };
    let turso_token = env::var("TURSO_AUTH_TOKEN").unwrap_or_default();

    info!("Iggy: {}  Stream: {}  Topic: {}", iggy_url, iggy_stream, iggy_topic);
    info!("Turso: {}", turso_url);

    // --- Connect to Iggy ---
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
    info!("Connected to Iggy at {}", iggy_url);

    // --- Build Iggy consumer (separate consumer group from stats-consumer) ---
    let mut consumer = client
        .consumer_group("webhook-consumer", &iggy_stream, &iggy_topic)
        .expect("Failed to create consumer group builder")
        .auto_commit(AutoCommit::Disabled)
        .polling_strategy(PollingStrategy::next())
        .poll_interval(IggyDuration::from_str("100ms").unwrap())
        .batch_length(50)
        .init_retries(10, IggyDuration::from_str("2s").unwrap())
        .build();

    consumer.init().await.expect("Failed to init Iggy consumer");
    info!(
        "Iggy consumer initialized (stream: {}, topic: {}) \u{2014} manual offset commit (at-least-once)",
        iggy_stream, iggy_topic
    );

    // --- Turso + HTTP clients ---
    let turso = TursoClient::new(turso_url, turso_token);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
        .build()
        .expect("Failed to build HTTP client");

    info!("Webhook dispatcher ready");

    // --- Health server ---
    let health_port: u16 = env::var("HEALTH_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3040);
    let health = HealthCounters::new("webhook-consumer", &[
        "events_processed", "webhooks_dispatched", "deduped", "errors", "offsets_committed",
    ]);
    spawn_health_server(health.clone(), health_port);

    // --- Dedup set: tracks recently seen event_ids to prevent duplicate deliveries ---
    let mut seen_ids: HashSet<String> = HashSet::with_capacity(DEDUP_CAPACITY);
    let mut seen_order: VecDeque<String> = VecDeque::with_capacity(DEDUP_CAPACITY);

    // --- Consumer loop ---
    let mut events_processed: u64 = 0;
    let mut webhooks_dispatched: u64 = 0;
    let mut deduped: u64 = 0;
    let mut errors: u64 = 0;
    let mut offsets_committed: u64 = 0;

    info!("Consuming events (at-least-once)...");

    while let Some(result) = consumer.next().await {
        match result {
            Ok(message) => {
                let offset = message.message.header.offset;
                let partition_id = message.partition_id;
                let payload = &message.message.payload;

                // Deserialize the TrackingEvent from JSON
                let event: TrackingEvent = match serde_json::from_slice(payload) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Failed to deserialize event at offset {}: {} — skipping (committing offset to avoid poison pill)", offset, e);
                        if let Err(ce) = consumer.store_offset(offset, Some(partition_id)).await {
                            error!("Failed to commit offset {} for poison pill: {}", offset, ce);
                        }
                        errors += 1;
                        continue;
                    }
                };

                let tenant = extract_tenant_prefix(&event);
                events_processed += 1;
                health.set("events_processed", events_processed);

                // Dedup: skip if we've already processed this event_id
                if seen_ids.contains(&event.event_id) {
                    deduped += 1;
                    health.set("deduped", deduped);
                    continue;
                }

                // Track this event_id for dedup
                if seen_ids.len() >= DEDUP_CAPACITY {
                    if let Some(old) = seen_order.pop_front() {
                        seen_ids.remove(&old);
                    }
                }
                seen_ids.insert(event.event_id.clone());
                seen_order.push_back(event.event_id.clone());

                // Skip _global events (no tenant, no webhooks)
                if tenant == "_global" {
                    if events_processed % 5000 == 0 {
                        info!(
                            "Processed {} events, dispatched {} webhooks ({} errors)",
                            events_processed, webhooks_dispatched, errors
                        );
                    }
                    continue;
                }

                // Look up webhooks for this tenant + event_type
                let webhooks = match turso
                    .get_webhooks_for_tenant(&tenant, &event.event_type)
                    .await
                {
                    Ok(w) => w,
                    Err(e) => {
                        warn!("Failed to fetch webhooks for {}: {}", tenant, e);
                        errors += 1;
                        continue;
                    }
                };

                if webhooks.is_empty() {
                    if events_processed % 5000 == 0 {
                        info!(
                            "Processed {} events, dispatched {} webhooks ({} errors)",
                            events_processed, webhooks_dispatched, errors
                        );
                    }
                    continue;
                }

                // Serialize event payload once for all webhooks
                let payload_json = serde_json::to_string(&event).unwrap_or_default();

                // Dispatch to all matching webhooks (with delivery dedup check)
                for webhook in &webhooks {
                    // Skip if this webhook+event was already successfully delivered
                    // (handles duplicate dispatch during consumer rebalance replay)
                    match turso.delivery_exists(&webhook.id, &event.event_id).await {
                        Ok(true) => {
                            deduped += 1;
                            continue;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            warn!("Failed to check delivery existence: {} — dispatching anyway", e);
                        }
                    }

                    let success =
                        dispatch_webhook(&http, &turso, webhook, &event, &payload_json).await;
                    if success {
                        webhooks_dispatched += 1;
                        health.set("webhooks_dispatched", webhooks_dispatched);
                    } else {
                        errors += 1;
                        health.set("errors", errors);
                    }
                }

                // Commit offset after processing this event (all dispatches done)
                if let Err(e) = consumer.store_offset(offset, Some(partition_id)).await {
                    error!("Failed to commit offset {} for partition {}: {}", offset, partition_id, e);
                } else {
                    offsets_committed += 1;
                    health.set("offsets_committed", offsets_committed);
                }

                if events_processed % 1000 == 0 {
                    info!(
                        "Processed {} events, dispatched {} webhooks, deduped {}, offsets committed {} ({} errors)",
                        events_processed, webhooks_dispatched, deduped, offsets_committed, errors
                    );
                }
            }
            Err(e) => {
                error!("Error consuming message: {}", e);
                errors += 1;
            }
        }
    }

    info!(
        "Consumer stream ended. Total: {} events, {} webhooks dispatched, {} deduped, {} offsets committed, {} errors",
        events_processed, webhooks_dispatched, deduped, offsets_committed, errors
    );
}
