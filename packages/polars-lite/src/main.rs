//! polars-lite — Warm tier query service for free-plan analytics.
//!
//! Reads pre-computed hourly aggregate Parquet files from R2 using Polars.
//! 30-day rolling window over aggregate data (not raw events).
//!
//! Aggregate files written by r2-archiver at:
//!   s3://{bucket}/aggregates/tenant_id={X}/date_path={YYYY-MM-DD}/hour={HH}/agg_{flush_id}.parquet
//!
//! Schema: tenant_id, event_type, date_path, hour, count
//!
//! Endpoints:
//!   POST /query   — aggregate stats query (counts by event_type, date, hour)
//!   GET  /health  — health check

use std::env;
use std::sync::Arc;

use aggregate_schema::paths as agg_paths;
use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use chrono::Utc;
use futures_util::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::ObjectStore;
use polars::prelude::*;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Maximum warm window in days (free tier limit).
const MAX_WARM_DAYS: i64 = 30;

/// Application state shared across handlers.
#[derive(Clone)]
struct AppState {
    store: Arc<dyn ObjectStore>,
    r2_bucket: String,
}

impl AppState {
    fn from_env() -> Self {
        let r2_endpoint = env::var("R2_ENDPOINT").expect("R2_ENDPOINT is required");
        let r2_access_key_id = env::var("R2_ACCESS_KEY_ID").expect("R2_ACCESS_KEY_ID is required");
        let r2_secret_access_key =
            env::var("R2_SECRET_ACCESS_KEY").expect("R2_SECRET_ACCESS_KEY is required");
        let r2_bucket = env::var("R2_BUCKET").unwrap_or_else(|_| "tracker-events".into());

        let store = AmazonS3Builder::new()
            .with_endpoint(&r2_endpoint)
            .with_bucket_name(&r2_bucket)
            .with_access_key_id(&r2_access_key_id)
            .with_secret_access_key(&r2_secret_access_key)
            .with_region("auto")
            .with_virtual_hosted_style_request(false)
            .build()
            .expect("Failed to build object store");

        Self {
            store: Arc::new(store),
            r2_bucket,
        }
    }
}

/// Query request body.
#[derive(Debug, Deserialize)]
struct QueryRequest {
    tenant_id: String,
    event_type: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    group_by: Option<String>,
    limit: Option<u32>,
}

/// Query response.
#[derive(Debug, Serialize)]
struct QueryResponse {
    count: usize,
    rows: Vec<serde_json::Value>,
    partitions_scanned: usize,
    query_ms: u64,
    tier: &'static str,
}

/// Error response.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Build date range for the warm window (up to MAX_WARM_DAYS).
fn warm_date_range(
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let today = Utc::now().date_naive();
    let earliest = today - chrono::Duration::days(MAX_WARM_DAYS);

    let from = date_from
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or(earliest)
        .max(earliest); // Clamp to warm window

    let to = date_to
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or(today)
        .min(today); // Can't query the future

    (from, to)
}

/// List aggregate Parquet file keys from R2 for the given tenant + date range.
async fn list_aggregate_keys(
    store: &dyn ObjectStore,
    tenant_id: &str,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
) -> Vec<object_store::path::Path> {
    let mut keys = Vec::new();
    let mut current = from;

    while current <= to {
        let date_str = current.format("%Y-%m-%d").to_string();
        let prefix = agg_paths::tenant_date_prefix(tenant_id, &date_str);
        let prefix_path = object_store::path::Path::from(prefix.as_str());

        let mut stream = store.list(Some(&prefix_path));
        while let Some(item) = stream.next().await {
            match item {
                Ok(meta) => {
                    if meta.location.as_ref().ends_with(".parquet") {
                        keys.push(meta.location);
                    }
                }
                Err(e) => {
                    warn!("Error listing aggregates for {}/{}: {}", tenant_id, date_str, e);
                }
            }
        }

        current += chrono::Duration::days(1);
    }

    keys
}

/// Download a Parquet file from R2 into a byte buffer.
async fn download_parquet(
    store: &dyn ObjectStore,
    key: &object_store::path::Path,
) -> Result<bytes::Bytes, String> {
    let result = store
        .get(key)
        .await
        .map_err(|e| format!("Failed to get {}: {}", key, e))?;
    result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes for {}: {}", key, e))
}

/// Execute a query against aggregate Parquet files on R2.
async fn execute_query(
    state: &AppState,
    req: &QueryRequest,
) -> Result<QueryResponse, String> {
    let start = std::time::Instant::now();

    let (from, to) = warm_date_range(req.date_from.as_deref(), req.date_to.as_deref());

    info!(
        "Warm query: tenant={} from={} to={} type={:?} group={:?}",
        req.tenant_id, from, to, req.event_type, req.group_by
    );

    // List aggregate files with partition pruning (tenant_id + date prefix)
    let keys = list_aggregate_keys(state.store.as_ref(), &req.tenant_id, from, to).await;
    let partitions_scanned = keys.len();

    if keys.is_empty() {
        return Ok(QueryResponse {
            count: 0,
            rows: vec![],
            partitions_scanned: 0,
            query_ms: start.elapsed().as_millis() as u64,
            tier: "warm",
        });
    }

    info!("Scanning {} aggregate files", partitions_scanned);

    // Download and read into Polars DataFrames
    let mut frames: Vec<LazyFrame> = Vec::new();
    for key in &keys {
        match download_parquet(state.store.as_ref(), key).await {
            Ok(data) => {
                let cursor = std::io::Cursor::new(data);
                match ParquetReader::new(cursor).finish() {
                    Ok(df) => frames.push(df.lazy()),
                    Err(e) => warn!("Failed to read Parquet {}: {}", key, e),
                }
            }
            Err(e) => warn!("Failed to download {}: {}", key, e),
        }
    }

    if frames.is_empty() {
        return Ok(QueryResponse {
            count: 0,
            rows: vec![],
            partitions_scanned,
            query_ms: start.elapsed().as_millis() as u64,
            tier: "warm",
        });
    }

    // Union all frames
    let mut lf = if frames.len() == 1 {
        frames.into_iter().next().unwrap()
    } else {
        concat(frames, UnionArgs::default())
            .map_err(|e| format!("Failed to concat frames: {}", e))?
    };

    // Filter by event_type if specified
    if let Some(ref event_type) = req.event_type {
        lf = lf.filter(col("event_type").eq(lit(event_type.clone())));
    }

    let limit = req.limit.unwrap_or(1000) as u32;

    // Aggregate based on group_by
    let df = match req.group_by.as_deref() {
        Some("date") => {
            // Daily totals: sum counts per (date_path, event_type)
            lf.group_by([col("date_path"), col("event_type")])
                .agg([col("count").sum().alias("count")])
                .sort(["date_path"], SortMultipleOptions::default())
                .limit(limit)
                .collect()
                .map_err(|e| format!("Query failed: {}", e))?
        }
        Some("hour") => {
            // Hourly totals: sum counts per (date_path, hour, event_type)
            lf.group_by([col("date_path"), col("hour"), col("event_type")])
                .agg([col("count").sum().alias("count")])
                .sort(
                    ["date_path", "hour"],
                    SortMultipleOptions::default(),
                )
                .limit(limit)
                .collect()
                .map_err(|e| format!("Query failed: {}", e))?
        }
        _ => {
            // Default: total counts per event_type
            lf.group_by([col("event_type")])
                .agg([col("count").sum().alias("count")])
                .sort(
                    ["count"],
                    SortMultipleOptions::default().with_order_descending(true),
                )
                .collect()
                .map_err(|e| format!("Query failed: {}", e))?
        }
    };

    let rows = df_to_json(&df)?;
    let count = rows.len();

    Ok(QueryResponse {
        count,
        rows,
        partitions_scanned,
        query_ms: start.elapsed().as_millis() as u64,
        tier: "warm",
    })
}

/// Convert a Polars DataFrame to a Vec of JSON objects.
fn df_to_json(df: &DataFrame) -> Result<Vec<serde_json::Value>, String> {
    let mut buf = Vec::new();
    let mut writer = polars::io::json::JsonWriter::new(&mut buf)
        .with_json_format(polars::io::json::JsonFormat::Json);
    writer
        .finish(&mut df.clone())
        .map_err(|e| format!("JSON serialization failed: {}", e))?;

    let json_str = String::from_utf8(buf).map_err(|e| format!("UTF-8 error: {}", e))?;
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    Ok(rows)
}

/// POST /query handler.
async fn query_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Warm query: tenant={} type={:?} from={:?} to={:?} group={:?}",
        req.tenant_id, req.event_type, req.date_from, req.date_to, req.group_by
    );

    match execute_query(&state, &req).await {
        Ok(response) => {
            info!(
                "Warm query complete: {} rows, {} files, {}ms",
                response.count, response.partitions_scanned, response.query_ms
            );
            Ok(Json(response))
        }
        Err(e) => {
            error!("Warm query error: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            ))
        }
    }
}

/// GET /health handler.
async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "polars-lite",
        "tier": "warm",
        "max_days": MAX_WARM_DAYS
    }))
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let state = Arc::new(AppState::from_env());
    let port = env::var("PORT").unwrap_or_else(|_| "3041".into());
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());

    info!(
        "Starting polars-lite (Warm tier, Polars + R2 aggregates) on {}:{}...",
        host, port
    );
    info!("R2 bucket: {}, warm window: {} days", state.r2_bucket, MAX_WARM_DAYS);

    let app = Router::new()
        .route("/query", post(query_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    info!("polars-lite listening on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
