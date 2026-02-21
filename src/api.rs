//! Tenant-scoped analytics API (`/api/v1/*`).
//!
//! Provides a Tinybird-style pipe layer that authenticates callers via their
//! `pt_{key_prefix}_{secret}` ingest token, derives the tenant, and proxies
//! queries to the internal analytics services with `tenant_id` injected
//! server-side (callers can never query another tenant's data).
//!
//! Backend services:
//!   - **polars-query** (port 3040) — cold tier, raw events on Delta Lake / R2
//!   - **polars-lite**  (port 3041) — warm tier, pre-computed hourly aggregates
//!   - **ai-query**     (port 3060) — natural language → SQL, vector search
//!
//! Endpoints:
//!   GET  /api/v1/events              — list recent events (paginated)
//!   GET  /api/v1/events/:event_id    — single event by ID
//!   POST /api/v1/events/query        — custom SQL over raw events
//!   GET  /api/v1/analytics/summary   — total counts by event_type
//!   GET  /api/v1/analytics/timeseries — time-bucketed volumes
//!   GET  /api/v1/analytics/top       — top event types / dimensions
//!   POST /api/v1/query/nl            — natural language → SQL → results
//!   POST /api/v1/query/similar       — vector similarity search

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::routes::AppState;

// ─── Tenant Auth ────────────────────────────────────────────────────────────

/// Extracted tenant identity from a validated `pt_` ingest token.
struct TenantAuth {
    key_prefix: String,
}

/// Extract and validate the Bearer token from request headers.
/// Returns the tenant's `key_prefix` or an HTTP error response.
async fn extract_tenant(headers: &HeaderMap, state: &AppState) -> Result<TenantAuth, Response> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth_header.starts_with("Bearer ") {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Missing Authorization: Bearer <ingest_token>"})),
        )
            .into_response());
    }

    let token = &auth_header[7..];
    match state.ingest_token_cache.validate(token).await {
        Some(key_prefix) => Ok(TenantAuth { key_prefix }),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid or expired ingest token"})),
        )
            .into_response()),
    }
}

// ─── Shared Types ───────────────────────────────────────────────────────────

/// Upstream response from polars-query / polars-lite.
#[derive(Debug, Deserialize, Serialize)]
struct QueryServiceResponse {
    count: usize,
    rows: Vec<serde_json::Value>,
    #[serde(default)]
    partitions_scanned: usize,
    #[serde(default)]
    query_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
}

/// Standard error body.
#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

// ─── Events Endpoints (polars-query) ────────────────────────────────────────

/// Query parameters for `GET /api/v1/events`.
#[derive(Debug, Deserialize)]
pub struct EventsParams {
    event_type: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
}

/// `GET /api/v1/events` — list recent events for the tenant.
pub async fn list_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<EventsParams>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let limit = params.limit.unwrap_or(100).min(1000);

    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "event_type": params.event_type,
        "date_from": params.from,
        "date_to": params.to,
        "limit": limit,
        "mode": "events",
    });

    proxy_to_polars_query(&state, &body).await
}

/// `GET /api/v1/events/id/:event_id` — fetch a single event by ID.
pub async fn get_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(event_id): Path<String>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    // Use custom SQL mode to filter by event_id within tenant scope.
    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "limit": 1,
        "mode": "custom",
        "custom_sql": format!(
            "SELECT * FROM deduped WHERE event_id = '{}'",
            event_id.replace('\'', "''")
        ),
    });

    proxy_to_polars_query(&state, &body).await
}

/// Request body for `POST /api/v1/events/query`.
#[derive(Debug, Deserialize)]
pub struct CustomQueryRequest {
    /// SQL SELECT statement — runs against `deduped` CTE (auto-scoped to tenant).
    sql: String,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
}

/// `POST /api/v1/events/query` — custom SQL over raw events.
pub async fn custom_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CustomQueryRequest>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let limit = req.limit.unwrap_or(100).min(1000);

    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "date_from": req.from,
        "date_to": req.to,
        "limit": limit,
        "mode": "custom",
        "custom_sql": req.sql,
    });

    proxy_to_polars_query(&state, &body).await
}

/// Forward a query to polars-query and return the response.
async fn proxy_to_polars_query(state: &AppState, body: &serde_json::Value) -> Response {
    let url = format!("{}/query", state.config.polars_query_url);

    info!("API → polars-query: {}", body);

    match state.http_client.post(&url).json(body).send().await {
        Ok(resp) => forward_upstream_response(resp).await,
        Err(e) => {
            error!("polars-query proxy error: {}", e);
            api_err(StatusCode::BAD_GATEWAY, format!("polars-query unavailable: {e}"))
        }
    }
}

// ─── Analytics Endpoints (polars-lite) ──────────────────────────────────────

/// Query parameters for analytics endpoints.
#[derive(Debug, Deserialize)]
pub struct AnalyticsParams {
    event_type: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
}

/// `GET /api/v1/analytics/summary` — total event counts by type.
pub async fn analytics_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AnalyticsParams>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "event_type": params.event_type,
        "date_from": params.from,
        "date_to": params.to,
        "limit": params.limit.unwrap_or(100),
    });

    proxy_to_polars_lite(&state, &body).await
}

/// Query parameters for timeseries endpoint.
#[derive(Debug, Deserialize)]
pub struct TimeseriesParams {
    event_type: Option<String>,
    from: Option<String>,
    to: Option<String>,
    /// Granularity: "date" (daily) or "hour" (hourly). Default: "date".
    granularity: Option<String>,
    limit: Option<u32>,
}

/// `GET /api/v1/analytics/timeseries` — time-bucketed event volumes.
pub async fn analytics_timeseries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<TimeseriesParams>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let group_by = match params.granularity.as_deref() {
        Some("hour") => "hour",
        _ => "date",
    };

    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "event_type": params.event_type,
        "date_from": params.from,
        "date_to": params.to,
        "group_by": group_by,
        "limit": params.limit.unwrap_or(1000),
    });

    proxy_to_polars_lite(&state, &body).await
}

/// `GET /api/v1/analytics/top` — top event types or dimensions.
pub async fn analytics_top(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AnalyticsParams>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let body = serde_json::json!({
        "tenant_id": tenant.key_prefix,
        "event_type": params.event_type,
        "date_from": params.from,
        "date_to": params.to,
        "group_by": "event_type",
        "limit": params.limit.unwrap_or(50),
    });

    proxy_to_polars_lite(&state, &body).await
}

/// Forward a query to polars-lite and return the response.
async fn proxy_to_polars_lite(state: &AppState, body: &serde_json::Value) -> Response {
    let url = format!("{}/query", state.config.polars_lite_url);

    info!("API → polars-lite: {}", body);

    match state.http_client.post(&url).json(body).send().await {
        Ok(resp) => forward_upstream_response(resp).await,
        Err(e) => {
            error!("polars-lite proxy error: {}", e);
            api_err(StatusCode::BAD_GATEWAY, format!("polars-lite unavailable: {e}"))
        }
    }
}

// ─── AI Query Endpoints (ai-query) ─────────────────────────────────────────

/// Request body for `POST /api/v1/query/nl`.
#[derive(Debug, Deserialize)]
pub struct NlQueryRequest {
    /// Natural language question.
    pub prompt: String,
    pub limit: Option<u32>,
}

/// `POST /api/v1/query/nl` — natural language → SQL → results.
pub async fn query_nl(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<NlQueryRequest>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let body = serde_json::json!({
        "prompt": req.prompt,
        "key_prefix": tenant.key_prefix,
        "limit": req.limit.unwrap_or(100),
    });

    let url = format!("{}/query/nl", state.config.ai_query_url);

    info!("API → ai-query /query/nl: prompt='{}'", req.prompt);

    match state.http_client.post(&url).json(&body).send().await {
        Ok(resp) => forward_upstream_response(resp).await,
        Err(e) => {
            error!("ai-query proxy error: {}", e);
            api_err(StatusCode::BAD_GATEWAY, format!("ai-query unavailable: {e}"))
        }
    }
}

/// Request body for `POST /api/v1/query/similar`.
#[derive(Debug, Deserialize)]
pub struct SimilarQueryRequest {
    /// Event ID to find similar events for.
    pub event_id: Option<String>,
    /// Or raw text to search for.
    pub query: Option<String>,
    pub limit: Option<u32>,
}

/// `POST /api/v1/query/similar` — vector similarity search.
pub async fn query_similar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SimilarQueryRequest>,
) -> Response {
    let tenant = match extract_tenant(&headers, &state).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let body = serde_json::json!({
        "event_id": req.event_id,
        "query": req.query,
        "key_prefix": tenant.key_prefix,
        "limit": req.limit.unwrap_or(10),
    });

    let url = format!("{}/query/similar", state.config.ai_query_url);

    info!("API → ai-query /query/similar");

    match state.http_client.post(&url).json(&body).send().await {
        Ok(resp) => forward_upstream_response(resp).await,
        Err(e) => {
            error!("ai-query proxy error: {}", e);
            api_err(StatusCode::BAD_GATEWAY, format!("ai-query unavailable: {e}"))
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Forward an upstream reqwest::Response as an Axum Response, preserving
/// the status code and JSON body.
async fn forward_upstream_response(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    match resp.json::<serde_json::Value>().await {
        Ok(body) => (status, Json(body)).into_response(),
        Err(e) => {
            error!("Failed to read upstream response: {}", e);
            api_err(StatusCode::BAD_GATEWAY, "Invalid upstream response")
        }
    }
}

// ─── Router ─────────────────────────────────────────────────────────────────

/// Build the `/api/v1` router. Must be nested under `/api/v1` in main.
pub fn router() -> Router<AppState> {
    Router::new()
        // Events (cold tier — polars-query)
        .route("/events", get(list_events))
        .route("/events/id/{event_id}", get(get_event))
        .route("/events/query", post(custom_query))
        // Analytics (warm tier — polars-lite)
        .route("/analytics/summary", get(analytics_summary))
        .route("/analytics/timeseries", get(analytics_timeseries))
        .route("/analytics/top", get(analytics_top))
        // AI query
        .route("/query/nl", post(query_nl))
        .route("/query/similar", post(query_similar))
}
