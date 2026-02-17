//! HTTP endpoint handlers for the tracker-core server.
//!
//! Provides six routes:
//! - **`GET /health`** — Health check with event counter and Iggy connection status.
//! - **`GET /health/broker`** — Iggy broker readiness check (200 if connected, 503 if NOOP).
//! - **`GET /t`** — Click tracking: validates the signed/encrypted URL, publishes
//!   a `"click"` event, and returns a 307 redirect to the destination.
//! - **`GET /p`** — Postback tracking: publishes a `"postback"` event and returns 200.
//! - **`GET /i`** — Impression tracking: publishes an `"impression"` event and returns
//!   a 1x1 transparent GIF with no-cache headers.
//! - **`POST /batch`** — Bulk event ingestion: accepts a JSON array of pre-built
//!   tracking events and enqueues them all in a single producer call.
//!
//! All tracking endpoints capture the full HTTP context (IP, User-Agent, Referer,
//! Accept-Language) and forward all query parameters as an opaque `params` map.

use axum::extract::connect_info::ConnectInfo;
use axum::extract::{Json, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use std::collections::HashMap;
use std::net::SocketAddr;
use tracing::warn;

use serde::Deserialize;

use crate::config::{Config, UrlMode};
use crate::crypto;
use crate::event::TrackingEvent;
use crate::ingest_token_cache::IngestTokenCache;
use crate::producer::SharedProducer;
use crate::rate_limiter::RateLimiter;
use crate::tenant_cache::{TenantCache, parse_prefixed_sig};
use crate::tracking_url_cache::TrackingUrlCache;

/// Shared application state injected into every Axum handler.
#[derive(Clone)]
pub struct AppState {
    /// Server configuration (URL mode, secrets, Iggy settings).
    pub config: Config,
    /// Thread-safe handle to the Iggy event producer (raw `events` topic).
    pub producer: SharedProducer,
    /// Producer for the `events-clean` topic (bypasses event filter).
    /// Used by `/ingest` where auth is already handled upstream.
    pub clean_producer: SharedProducer,
    /// Multi-tenant secret cache (prefix → HMAC secret / encryption key).
    pub tenant_cache: TenantCache,
    /// Tracking URL cache (tu_id → destination + key_prefix).
    pub tracking_url_cache: TrackingUrlCache,
    /// Ingest token cache for `/ingest` endpoint authentication.
    pub ingest_token_cache: IngestTokenCache,
    /// Per-tenant token bucket rate limiter.
    pub rate_limiter: RateLimiter,
}

/// Extract the client IP address from the request.
///
/// Priority: `X-Forwarded-For` (first entry) → `X-Real-IP` → socket peer address.
/// This ensures correct IP capture whether behind a reverse proxy or not.
fn extract_ip(headers: &HeaderMap, peer_addr: &SocketAddr) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        return real_ip.to_string();
    }
    peer_addr.ip().to_string()
}

/// Extract a single header value as an owned `String`, if present and valid UTF-8.
fn extract_header(headers: &HeaderMap, key: &str) -> Option<String> {
    headers.get(key).and_then(|v| v.to_str().ok()).map(|v| v.to_string())
}

/// Extract the `Host` header, falling back to `"unknown"`.
fn extract_host(headers: &HeaderMap) -> String {
    extract_header(headers, "host").unwrap_or_else(|| "unknown".to_string())
}

/// Return a 429 Too Many Requests response with Retry-After header.
fn rate_limited_response() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            ("content-type", "application/json"),
            ("retry-after", "1"),
        ],
        r#"{"error":"Rate limit exceeded"}"#.to_string(),
    )
        .into_response()
}

/// `GET /` — Service info landing page.
pub async fn handle_root() -> Response {
    let body = serde_json::json!({
        "service": "tracker-core",
        "description": "High-performance event tracking & ingestion server",
        "endpoints": {
            "GET /health": "Health check with event counters",
            "GET /health/broker": "Iggy broker readiness",
            "GET /t?url=<signed_url>": "Click tracking (307 redirect)",
            "GET /t/<tu_id>": "Tracked URL click (307 redirect)",
            "GET /p": "Postback tracking",
            "GET /i": "Impression tracking (1x1 GIF)",
            "POST /batch": "Bulk event ingestion (JSON array)",
            "POST /ingest": "External event ingestion (Bearer token auth)"
        },
        "docs": "https://github.com/inventhq/tracker"
    });
    (StatusCode::OK, [("content-type", "application/json")], body.to_string()).into_response()
}

/// `GET /health` — Returns JSON with server status, Iggy connection state,
/// and the total number of events processed since startup.
pub async fn handle_health(State(state): State<AppState>) -> Response {
    let body = serde_json::json!({
        "status": "ok",
        "iggy_connected": state.producer.is_connected().await,
        "events_sent": state.producer.events_sent(),
        "events_dropped": state.producer.events_dropped(),
    });
    (StatusCode::OK, [("content-type", "application/json")], body.to_string()).into_response()
}

/// `GET /health/broker` — Iggy broker readiness check.
///
/// Returns 200 if the producer is connected to Iggy (events are being persisted),
/// or 503 if running in NOOP mode (events are being silently dropped).
/// Use this for monitoring/alerting — separate from `/health` which is a
/// liveness check for load balancers.
pub async fn handle_broker_health(State(state): State<AppState>) -> Response {
    let connected = state.producer.is_connected().await;
    let status = if connected { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let body = serde_json::json!({
        "broker": if connected { "connected" } else { "noop" },
        "events_sent": state.producer.events_sent(),
        "events_dropped": state.producer.events_dropped(),
    });
    (status, [("content-type", "application/json")], body.to_string()).into_response()
}

/// `GET /t` — Click tracking endpoint.
///
/// Validates the destination URL (HMAC signature or AES-GCM decryption),
/// captures the full HTTP context into a [`TrackingEvent`], enqueues it
/// to Iggy, and returns a 307 redirect to the destination.
pub async fn handle_click(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let mut params = params;

    // Rate limit check (by tenant key_prefix if present)
    if let Some(kp) = params.get("key_prefix") {
        if !state.rate_limiter.check(kp).await {
            return rate_limited_response();
        }
    }

    // Extract and validate the redirect URL based on URL_MODE
    let destination = match &state.config.url_mode {
        UrlMode::Signed => {
            let Some(url) = params.remove("url") else {
                return (StatusCode::BAD_REQUEST, "Missing 'url' parameter").into_response();
            };
            let Some(sig) = params.remove("sig") else {
                return (StatusCode::BAD_REQUEST, "Missing 'sig' parameter").into_response();
            };

            // Multi-tenant: check for prefixed signature (e.g. "tk8a_c740665...")
            let valid = if let Some((prefix, raw_sig)) = parse_prefixed_sig(&sig) {
                if let Some(tenant_secret) = state.tenant_cache.get_hmac_secret(prefix).await {
                    crypto::verify_hmac(&tenant_secret, &url, raw_sig)
                } else {
                    warn!("Unknown tenant prefix: {}", prefix);
                    false
                }
            } else {
                // No prefix — fall back to global HMAC secret
                let secret = state.config.hmac_secret.as_deref().unwrap();
                crypto::verify_hmac(secret, &url, &sig)
            };

            if !valid {
                warn!("Invalid HMAC signature for URL: {}", url);
                return (StatusCode::BAD_REQUEST, "Invalid signature").into_response();
            }
            url
        }
        UrlMode::Encrypted => {
            let Some(d) = params.remove("d") else {
                return (StatusCode::BAD_REQUEST, "Missing 'd' parameter").into_response();
            };
            let key = state.config.encryption_key.as_deref().unwrap();
            match crypto::decrypt_url(key, &d) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Failed to decrypt URL: {}", e);
                    return (StatusCode::BAD_REQUEST, "Invalid encrypted URL").into_response();
                }
            }
        }
    };

    let event = TrackingEvent::new(
        "click",
        extract_ip(&headers, &addr),
        extract_header(&headers, "user-agent").unwrap_or_default(),
        extract_header(&headers, "referer"),
        extract_header(&headers, "accept-language"),
        "/t",
        extract_host(&headers),
        params,
    );

    // Fire-and-forget: the background producer batches and flushes
    // asynchronously, so we don't need to await the send.
    let partition_key = event.params.get("key_prefix").cloned();
    let producer = state.producer.clone();
    tokio::spawn(async move {
        producer.send(&event, partition_key.as_deref()).await;
    });

    Redirect::temporary(&destination).into_response()
}

/// `GET /t/:tu_id` — Tracked click via short URL.
///
/// Resolves the `tu_id` from the in-memory tracking URL cache to get the
/// destination URL and tenant key_prefix. Validates the HMAC signature
/// (which signs the `tu_id`, not the destination). Captures a click event
/// with `tu_id` in params, then 307 redirects to the destination.
///
/// This enables short, stable URLs where the destination can be rotated
/// server-side without regenerating distributed links.
pub async fn handle_tracked_click(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::extract::Path(tu_id): axum::extract::Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let mut params = params;

    // Resolve tu_id → destination from cache
    let entry = match state.tracking_url_cache.get(&tu_id).await {
        Some(e) => e,
        None => {
            warn!("Unknown tracking URL: {}", tu_id);
            return (StatusCode::NOT_FOUND, "Unknown tracking URL").into_response();
        }
    };

    // Rate limit check (key_prefix from tracking URL cache)
    if !state.rate_limiter.check(&entry.key_prefix).await {
        return rate_limited_response();
    }

    // Validate HMAC signature if provided (optional for tracked URLs —
    // the tu_id is a server-controlled lookup key, destination is in cache)
    if let Some(sig) = params.remove("sig") {
        let valid = if let Some((prefix, raw_sig)) = parse_prefixed_sig(&sig) {
            if let Some(tenant_secret) = state.tenant_cache.get_hmac_secret(prefix).await {
                crypto::verify_hmac(&tenant_secret, &tu_id, raw_sig)
            } else {
                warn!("Unknown tenant prefix: {}", prefix);
                false
            }
        } else {
            // No prefix — fall back to global HMAC secret
            if let Some(secret) = state.config.hmac_secret.as_deref() {
                crypto::verify_hmac(secret, &tu_id, &sig)
            } else {
                false
            }
        };

        if !valid {
            warn!("Invalid signature for tracking URL: {}", tu_id);
            return (StatusCode::BAD_REQUEST, "Invalid signature").into_response();
        }
    }

    // Inject tu_id and key_prefix into event params
    params.insert("tu_id".to_string(), tu_id);
    params.insert("key_prefix".to_string(), entry.key_prefix);

    let event = TrackingEvent::new(
        "click",
        extract_ip(&headers, &addr),
        extract_header(&headers, "user-agent").unwrap_or_default(),
        extract_header(&headers, "referer"),
        extract_header(&headers, "accept-language"),
        "/t",
        extract_host(&headers),
        params,
    );

    let partition_key = event.params.get("key_prefix").cloned();
    let producer = state.producer.clone();
    tokio::spawn(async move {
        producer.send(&event, partition_key.as_deref()).await;
    });

    Redirect::temporary(&entry.destination).into_response()
}

/// `GET /p` — Postback / conversion tracking endpoint.
///
/// Captures the full HTTP context into a [`TrackingEvent`], enqueues it
/// to Iggy, and returns 200 OK. Used by affiliate networks and ad platforms
/// to fire server-to-server conversion notifications.
pub async fn handle_postback(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // Rate limit check (by tenant key_prefix if present)
    if let Some(kp) = params.get("key_prefix") {
        if !state.rate_limiter.check(kp).await {
            return rate_limited_response();
        }
    }

    let event = TrackingEvent::new(
        "postback",
        extract_ip(&headers, &addr),
        extract_header(&headers, "user-agent").unwrap_or_default(),
        extract_header(&headers, "referer"),
        extract_header(&headers, "accept-language"),
        "/p",
        extract_host(&headers),
        params,
    );

    let partition_key = event.params.get("key_prefix").cloned();
    let producer = state.producer.clone();
    tokio::spawn(async move {
        producer.send(&event, partition_key.as_deref()).await;
    });

    StatusCode::OK.into_response()
}

/// Minimal 1x1 transparent GIF (43 bytes) served for impression tracking.
/// Returned with no-cache headers to ensure every request is counted.
const PIXEL_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00,
    0x80, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x21,
    0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44,
    0x01, 0x00, 0x3b,
];

/// `GET /i` — Impression tracking endpoint.
///
/// Captures the full HTTP context into a [`TrackingEvent`], enqueues it
/// to Iggy, and returns a 1x1 transparent GIF with no-cache headers.
/// Designed to be embedded as an `<img>` tag in HTML pages or emails.
pub async fn handle_impression(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // Rate limit check (by tenant key_prefix if present)
    if let Some(kp) = params.get("key_prefix") {
        if !state.rate_limiter.check(kp).await {
            return rate_limited_response();
        }
    }

    let event = TrackingEvent::new(
        "impression",
        extract_ip(&headers, &addr),
        extract_header(&headers, "user-agent").unwrap_or_default(),
        extract_header(&headers, "referer"),
        extract_header(&headers, "accept-language"),
        "/i",
        extract_host(&headers),
        params,
    );

    let partition_key = event.params.get("key_prefix").cloned();
    let producer = state.producer.clone();
    tokio::spawn(async move {
        producer.send(&event, partition_key.as_deref()).await;
    });

    (
        StatusCode::OK,
        [
            ("content-type", "image/gif"),
            ("cache-control", "no-store, no-cache, must-revalidate"),
            ("pragma", "no-cache"),
            ("expires", "0"),
        ],
        PIXEL_GIF,
    )
        .into_response()
}

/// `POST /batch` — Bulk event ingestion endpoint.
///
/// Accepts a JSON array of pre-built [`TrackingEvent`]s and enqueues them
/// all in a single producer call. This amortizes HTTP overhead across many
/// events, enabling millions of events/sec through fewer HTTP requests.
///
/// The maximum batch size is configurable via the `MAX_BATCH_SIZE` env var
/// (default: 10,000).
///
/// Returns JSON with the number of events accepted.
///
/// # Request Body
/// ```json
/// [
///   { "event_id": "...", "event_type": "click", "timestamp": 1707350000000, ... },
///   { "event_id": "...", "event_type": "postback", "timestamp": 1707350000001, ... }
/// ]
/// ```
pub async fn handle_batch(
    State(state): State<AppState>,
    Json(events): Json<Vec<TrackingEvent>>,
) -> Response {
    if events.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            r#"{"error":"empty batch"}"#.to_string(),
        )
            .into_response();
    }

    let max = state.config.max_batch_size;
    if events.len() > max {
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            format!(r#"{{"error":"batch too large, max {}"}}"#, max),
        )
            .into_response();
    }

    let count = events.len();
    let producer = state.producer.clone();
    tokio::spawn(async move {
        producer.send_batch(&events).await;
    });

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        format!(r#"{{"accepted":{}}}"#, count),
    )
        .into_response()
}

/// Request body for `POST /ingest` — external event ingestion.
#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    /// Event type (e.g. "charge.succeeded", "order.created", or any custom string).
    pub event_type: String,
    /// Flat key-value params promoted from the payload for fast querying.
    #[serde(default)]
    pub params: HashMap<String, String>,
    /// Full nested JSON payload from the external source.
    #[serde(default)]
    pub raw_payload: Option<serde_json::Value>,
}

/// `POST /ingest` — External event ingestion endpoint.
///
/// Accepts a JSON body with `event_type`, flat `params`, and an optional
/// nested `raw_payload`. Generates a UUIDv7 event ID and timestamp,
/// then publishes to Iggy like any other tracking event.
///
/// **Authentication required:** `Authorization: Bearer pt_{key_prefix}_{secret}`.
/// The token is validated via the Platform API (cached for 5 minutes).
/// The `key_prefix` is injected from the token — callers cannot choose
/// their own key_prefix (prevents tenant spoofing).
///
/// Designed for:
/// - Plugin Runtime adapters (Stripe, Shopify, GitHub webhooks)
/// - Competitor data imports (Everflow, RedTrack CSV/API)
/// - Any external system that needs to push structured events
///
/// Body size is limited to 1 MB via the route layer in `main.rs`.
pub async fn handle_ingest(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<IngestRequest>,
) -> Response {
    // --- Auth: validate Bearer token and extract key_prefix ---
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth_header.starts_with("Bearer ") {
        return (
            StatusCode::UNAUTHORIZED,
            [("content-type", "application/json")],
            r#"{"error":"Missing Authorization: Bearer <ingest_token>"}"#.to_string(),
        )
            .into_response();
    }

    let token = &auth_header[7..];
    let key_prefix = match state.ingest_token_cache.validate(token).await {
        Some(kp) => kp,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                [("content-type", "application/json")],
                r#"{"error":"Invalid or expired ingest token"}"#.to_string(),
            )
                .into_response();
        }
    };

    // Rate limit check (key_prefix from validated token)
    if !state.rate_limiter.check(&key_prefix).await {
        return rate_limited_response();
    }

    if req.event_type.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            r#"{"error":"event_type is required"}"#.to_string(),
        )
            .into_response();
    }

    let mut params = req.params;
    // Inject key_prefix from the validated token — overrides any caller-provided
    // value to prevent tenant spoofing.
    params.insert("key_prefix".to_string(), key_prefix.clone());

    let mut event = TrackingEvent::new(
        &req.event_type,
        extract_ip(&headers, &addr),
        extract_header(&headers, "user-agent").unwrap_or_default(),
        extract_header(&headers, "referer"),
        extract_header(&headers, "accept-language"),
        "/ingest",
        extract_host(&headers),
        params,
    );
    event.raw_payload = req.raw_payload;

    let event_id = event.event_id.clone();
    // Send directly to events-clean topic — /ingest events are authenticated
    // via ingest token, so they bypass the event filter pipeline.
    let producer = state.clean_producer.clone();
    tokio::spawn(async move {
        producer.send(&event, Some(&key_prefix)).await;
    });

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        format!(r#"{{"accepted":1,"event_id":"{}"}}"#, event_id),
    )
        .into_response()
}
