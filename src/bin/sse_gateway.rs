//! sse-gateway — Real-time event streaming via Server-Sent Events.
//!
//! Subscribes to the Iggy tracker stream and fans out events to connected
//! SSE clients. Each client receives a filtered stream of JSON-serialized
//! `TrackingEvent` payloads matching their tenant's `key_prefix`.
//!
//! Endpoints:
//!   - `GET /sse/events?key_prefix=<prefix>` — SSE stream of events for a tenant
//!   - `GET /health` — Health check
//!
//! Designed to run alongside tracker-core and the other consumers.

use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::Method;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use futures_util::stream::Stream;
use iggy::prelude::*;
use serde_json::json;
use tokio::signal;
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::event::TrackingEvent;

/// Maximum number of events buffered in the broadcast channel.
/// If a slow client falls behind, it will miss events (acceptable for SSE).
const BROADCAST_CAPACITY: usize = 4096;

/// Shared application state for SSE handlers.
#[derive(Clone)]
struct AppState {
    /// Broadcast sender — Iggy consumer pushes events here, SSE handlers subscribe.
    tx: broadcast::Sender<Arc<TrackingEvent>>,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Starting sse-gateway...");

    let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let iggy_http_url = env::var("IGGY_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
    let iggy_topic = env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into());
    let iggy_topic_clean = env::var("IGGY_TOPIC_CLEAN").unwrap_or_else(|_| "events-clean".into());
    let port: u16 = env::var("SSE_PORT")
        .unwrap_or_else(|_| "3031".into())
        .parse()
        .expect("SSE_PORT must be a valid port number");

    info!("Iggy: {} HTTP: {}  Stream: {}  Topics: {}, {}", iggy_url, iggy_http_url, iggy_stream, iggy_topic, iggy_topic_clean);

    // --- Broadcast channel for fan-out ---
    let (tx, _) = broadcast::channel::<Arc<TrackingEvent>>(BROADCAST_CAPACITY);

    let state = AppState { tx: tx.clone() };

    // --- Spawn Iggy consumer task for raw events topic ---
    let iggy_url_clone = iggy_url.clone();
    let iggy_http_url_clone = iggy_http_url.clone();
    let iggy_stream_clone = iggy_stream.clone();
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        iggy_consumer_loop(iggy_url_clone, iggy_http_url_clone, iggy_stream_clone, iggy_topic, tx_clone, "sse-gateway").await;
    });

    // --- Spawn Iggy consumer task for clean/ingest events topic ---
    let iggy_url_clone2 = iggy_url.clone();
    let iggy_http_url_clone2 = iggy_http_url.clone();
    tokio::spawn(async move {
        iggy_consumer_loop(iggy_url_clone2, iggy_http_url_clone2, iggy_stream, iggy_topic_clean, tx, "sse-gateway-clean").await;
    });

    // --- Axum HTTP server with CORS ---
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET])
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/sse/events", get(sse_handler))
        .route("/health", get(health_handler))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind SSE gateway");

    info!("sse-gateway listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("SSE gateway error");

    info!("sse-gateway shut down gracefully");
}

/// Iggy consumer loop — reads events and broadcasts them to all SSE clients.
///
/// Uses HTTP polling with a unique consumer_id per request to avoid the Iggy
/// server-side delivered-offset tracking bug. TCP client retained only for
/// get/store_consumer_offset.
async fn iggy_consumer_loop(
    iggy_url: String,
    iggy_http_url: String,
    stream_name: String,
    topic_name: String,
    tx: broadcast::Sender<Arc<TrackingEvent>>,
    consumer_name: &str,
) {
    // TCP client for offset management only
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

    // HTTP client for polling (avoids Iggy server-side delivered-offset tracking bug)
    let http = reqwest::Client::new();
    let mut iggy_token = iggy_http_login(&http, &iggy_http_url)
        .await
        .expect("Failed to login to Iggy HTTP API");
    info!("[{}] Connected to Iggy TCP={} HTTP={}", consumer_name, iggy_url, iggy_http_url);

    let consumer_id = Consumer::new(Identifier::named(consumer_name).unwrap());
    let stream_id = Identifier::named(&stream_name).unwrap();
    let topic_id = Identifier::named(&topic_name).unwrap();

    // Discover partition count
    let topic_info = client
        .get_topic(&stream_id, &topic_id)
        .await
        .expect("Failed to get topic info")
        .unwrap();
    let partition_count = topic_info.partitions_count;
    info!("Topic {} has {} partitions", topic_name, partition_count);

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

    let poll_count: u32 = 100;
    let mut count: u64 = 0;
    let mut batch_max_offset: HashMap<u32, u64> = HashMap::new();

    info!(
        "Consuming events (sse-gateway, poll_count={}, HTTP polling)...",
        poll_count
    );

    loop {
        let mut got_any = false;

        for pid in 0..partition_count {
            let offset = *next_offset.get(&pid).unwrap_or(&0);

            let messages = match http_poll_messages(
                &http, &iggy_http_url, &mut iggy_token,
                &stream_name, &topic_name, pid, offset, poll_count,
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

            for msg in &messages {
                let msg_offset = msg.offset;
                next_offset.insert(pid, msg_offset + 1);

                let entry = batch_max_offset.entry(pid).or_insert(0);
                if msg_offset > *entry { *entry = msg_offset; }

                let event: TrackingEvent = match serde_json::from_slice(&msg.payload) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Failed to deserialize at offset {}: {} — skipping", msg_offset, e);
                        continue;
                    }
                };

                count += 1;

                // Broadcast to all connected SSE clients (ignore send errors — means no receivers)
                let _ = tx.send(Arc::new(event));

                if count % 1000 == 0 {
                    info!("Broadcast {} events to SSE clients", count);
                }
            }
        }

        // When idle: commit offsets to Iggy
        if !got_any {
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

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
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
/// The Iggy server tracks "delivered up to" per consumer_id, causing offset
/// gaps on sequential polls with the same ID. Using a unique ID per request
/// ensures zero server-side state accumulation.
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

/// Query parameters for the SSE endpoint.
#[derive(serde::Deserialize)]
struct SseQuery {
    /// Filter events by tenant key_prefix. If omitted, receives all events (admin mode).
    key_prefix: Option<String>,
    /// Filter by event type (click, postback, impression). If omitted, receives all types.
    event_type: Option<String>,
    /// Filter by tracking URL ID. If omitted, receives events for all links.
    tu_id: Option<String>,
}

/// SSE handler — streams real-time events to the client.
///
/// Each event is sent as a JSON-serialized `TrackingEvent` with event type "event".
/// A keep-alive comment is sent every 15 seconds to prevent connection timeout.
async fn sse_handler(
    State(state): State<AppState>,
    Query(query): Query<SseQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.tx.subscribe();
    let key_prefix = query.key_prefix;
    let event_type = query.event_type;
    let tu_id = query.tu_id;

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // Filter by key_prefix if specified
                    if let Some(ref prefix) = key_prefix {
                        let event_prefix = event.params.get("key_prefix").map(|s| s.as_str()).unwrap_or("");
                        if event_prefix != prefix {
                            continue;
                        }
                    }

                    // Filter by event_type if specified
                    if let Some(ref etype) = event_type {
                        if event.event_type != *etype {
                            continue;
                        }
                    }

                    // Filter by tracking URL ID if specified
                    if let Some(ref tid) = tu_id {
                        let event_tu_id = event.params.get("tu_id").map(|s| s.as_str()).unwrap_or("");
                        if event_tu_id != tid {
                            continue;
                        }
                    }

                    let json = serde_json::to_string(&*event).unwrap_or_default();
                    yield Ok(Event::default().event("event").data(json));
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("SSE client lagged, missed {} events", n);
                    // Send a lag notification to the client
                    yield Ok(Event::default().event("lag").data(format!("{{\"missed\":{n}}}")));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// Root landing page.
async fn root_handler() -> Json<serde_json::Value> {
    Json(json!({
        "service": "sse-gateway",
        "description": "Real-time event streaming via Server-Sent Events",
        "endpoints": {
            "GET /sse/events?key_prefix=<prefix>": "SSE stream of events for a tenant",
            "GET /health": "Health check"
        },
        "docs": "https://github.com/inventhq/tracker"
    }))
}

/// Health check endpoint.
async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "sse-gateway" }))
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

    info!("Shutdown signal received...");
}
