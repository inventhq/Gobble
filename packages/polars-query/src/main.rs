//! polars-query — HTTP query service for historical analytics over Delta Lake tables on R2.
//!
//! Axum HTTP server. Opens a Delta table on Cloudflare R2, uses DataFusion for
//! partition-pruned reads, then converts results to JSON.
//!
//! Delta table location: s3://{bucket}/events/
//! Partition columns: tenant_id, date_path (YYYY-MM-DD)
//!
//! Stateless — no persistent local storage. Reads Parquet row groups on demand.
//! Designed to run on a dedicated Civo instance, isolated from tracker-core.
//!
//! Endpoints:
//!   POST /query   — flexible event query with filters, grouping, aggregation
//!   GET  /health  — health check

use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use arrow::array::{Array, AsArray};
use arrow::datatypes::DataType;
use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use deltalake::DeltaTable;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use url::Url;

/// Application state shared across handlers.
#[derive(Clone)]
struct AppState {
    r2_endpoint: String,
    r2_access_key_id: String,
    r2_secret_access_key: String,
    r2_bucket: String,
}

impl AppState {
    fn from_env() -> Self {
        Self {
            r2_endpoint: env::var("R2_ENDPOINT").expect("R2_ENDPOINT is required"),
            r2_access_key_id: env::var("R2_ACCESS_KEY_ID").expect("R2_ACCESS_KEY_ID is required"),
            r2_secret_access_key: env::var("R2_SECRET_ACCESS_KEY")
                .expect("R2_SECRET_ACCESS_KEY is required"),
            r2_bucket: env::var("R2_BUCKET").unwrap_or_else(|_| "tracker-events".into()),
        }
    }

    /// Delta table URI on R2.
    fn delta_table_uri(&self) -> String {
        format!("s3://{}/events", self.r2_bucket)
    }

    /// Storage options for Delta Lake R2 access.
    fn storage_options(&self) -> HashMap<String, String> {
        let mut opts = HashMap::new();
        opts.insert("AWS_ACCESS_KEY_ID".into(), self.r2_access_key_id.clone());
        opts.insert("AWS_SECRET_ACCESS_KEY".into(), self.r2_secret_access_key.clone());
        opts.insert("AWS_ENDPOINT_URL".into(), self.r2_endpoint.clone());
        opts.insert("AWS_REGION".into(), "auto".into());
        opts.insert("aws_conditional_put".into(), "etag".into());
        opts.insert("AWS_S3_ALLOW_UNSAFE_RENAME".into(), "true".into());
        opts
    }

    /// Open the Delta table (read-only).
    async fn open_table(&self) -> Result<DeltaTable, String> {
        let uri = self.delta_table_uri();
        let url = Url::parse(&uri)
            .map_err(|e| format!("Invalid Delta table URI: {}", e))?;
        deltalake::open_table_with_storage_options(url, self.storage_options())
            .await
            .map_err(|e| format!("Failed to open Delta table at {}: {:?}", uri, e))
    }
}

/// Query request body.
#[derive(Debug, Deserialize)]
struct QueryRequest {
    tenant_id: String,
    tu_id: Option<String>,
    event_type: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    param_key: Option<String>,
    param_value: Option<String>,
    group_by: Option<String>,
    limit: Option<u32>,
    mode: Option<String>,
    /// Custom SQL query (mode="custom"). The query runs against the `events` table
    /// with JSON functions available (json_get_str, json_get_int, json_get_float, etc.).
    /// A mandatory `WHERE tenant_id = '<tenant_id>'` clause is injected for safety.
    custom_sql: Option<String>,
}

/// Query response.
#[derive(Debug, Serialize)]
struct QueryResponse {
    count: usize,
    rows: Vec<serde_json::Value>,
    partitions_scanned: usize,
    query_ms: u64,
}

/// Error response.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Convert Arrow RecordBatches to JSON rows.
fn batches_to_json(batches: &[arrow::array::RecordBatch]) -> Result<Vec<serde_json::Value>, String> {
    let mut rows = Vec::new();

    for batch in batches {
        let schema = batch.schema();
        for row_idx in 0..batch.num_rows() {
            let mut obj = serde_json::Map::new();
            for (col_idx, field) in schema.fields().iter().enumerate() {
                let col = batch.column(col_idx);
                let value = if col.is_null(row_idx) {
                    serde_json::Value::Null
                } else {
                    match field.data_type() {
                        DataType::Utf8 => {
                            let arr = col.as_string::<i32>();
                            serde_json::Value::String(arr.value(row_idx).to_string())
                        }
                        DataType::LargeUtf8 => {
                            let arr = col.as_string::<i64>();
                            serde_json::Value::String(arr.value(row_idx).to_string())
                        }
                        DataType::Int64 => {
                            let arr = col.as_primitive::<arrow::datatypes::Int64Type>();
                            serde_json::Value::Number(arr.value(row_idx).into())
                        }
                        DataType::UInt64 => {
                            let arr = col.as_primitive::<arrow::datatypes::UInt64Type>();
                            serde_json::Value::Number(arr.value(row_idx).into())
                        }
                        DataType::Float64 => {
                            let arr = col.as_primitive::<arrow::datatypes::Float64Type>();
                            let v = arr.value(row_idx);
                            serde_json::Number::from_f64(v)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null)
                        }
                        _ => {
                            // Fallback: use debug format
                            let formatter = arrow::util::display::ArrayFormatter::try_new(col.as_ref(), &Default::default())
                                .map_err(|e| format!("Formatter error: {}", e))?;
                            serde_json::Value::String(formatter.value(row_idx).to_string())
                        }
                    }
                };
                obj.insert(field.name().clone(), value);
            }
            rows.push(serde_json::Value::Object(obj));
        }
    }

    Ok(rows)
}

/// Execute a query against the Delta table on R2 using DataFusion.
async fn execute_query(
    state: &AppState,
    req: &QueryRequest,
) -> Result<QueryResponse, String> {
    let start = std::time::Instant::now();

    // Open the Delta table
    let table = state.open_table().await?;
    let version = table.version();

    info!(
        "Opened Delta table (version {:?}), building query...",
        version
    );

    // Use DataFusion to query the Delta table with partition pruning
    let mut ctx = deltalake::datafusion::prelude::SessionContext::new();
    // Register JSON functions (json_get_str, json_get_int, json_get_float, etc.)
    // so SQL queries can extract values from raw_payload and params JSON strings.
    datafusion_functions_json::register_all(&mut ctx)
        .map_err(|e| format!("Failed to register JSON functions: {:?}", e))?;
    let table_state = table.snapshot().map_err(|e| format!("Failed to get snapshot: {:?}", e))?;
    let eager_snapshot = table_state.snapshot().clone();
    // Disable dictionary encoding for partition columns so DataFusion sees them as plain Utf8.
    // Default is wrap_partition_values=true which causes Dictionary(UInt16, Utf8) type that
    // breaks GROUP BY queries with physical/logical schema mismatch errors.
    let scan_config = deltalake::delta_datafusion::DeltaScanConfigBuilder::new()
        .wrap_partition_values(false)
        .build(&eager_snapshot)
        .map_err(|e| format!("Failed to build scan config: {:?}", e))?;

    let provider = deltalake::delta_datafusion::DeltaTableProvider::try_new(
        eager_snapshot,
        table.log_store().clone(),
        scan_config,
    )
    .map_err(|e| format!("Failed to create DeltaTableProvider: {:?}", e))?;

    ctx.register_table("events", Arc::new(provider))
        .map_err(|e| format!("Failed to register table: {:?}", e))?;

    let limit = req.limit.unwrap_or(1000);
    let mode = req.mode.as_deref().unwrap_or("stats");

    // Build SQL based on mode
    let sql = match mode {
        "events" => build_events_sql(req, limit),
        "custom" => build_custom_sql(req, limit)?,
        _ => build_stats_sql(req, limit),
    };

    info!("DataFusion SQL: {}", sql);

    let df = ctx.sql(&sql).await
        .map_err(|e| format!("DataFusion SQL error: {:?}", e))?;

    let batches = df.collect().await
        .map_err(|e| format!("DataFusion collect error: {:?}", e))?;

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    if total_rows == 0 {
        return Ok(QueryResponse {
            count: 0,
            rows: vec![],
            partitions_scanned: 0,
            query_ms: start.elapsed().as_millis() as u64,
        });
    }

    let rows = batches_to_json(&batches)?;
    let count = rows.len();

    Ok(QueryResponse {
        count,
        rows,
        partitions_scanned: total_rows,
        query_ms: start.elapsed().as_millis() as u64,
    })
}

/// Build WHERE clause from request filters.
fn build_where_clause(req: &QueryRequest) -> String {
    let mut conditions: Vec<String> = Vec::new();

    // Partition filter: tenant_id (pruned at file level by DataFusion)
    if req.tenant_id != "*" {
        conditions.push(format!("tenant_id = '{}'", req.tenant_id.replace('\'', "''")));
    }

    // Partition filter: date_path (pruned at file level by DataFusion)
    if let Some(ref date_from) = req.date_from {
        conditions.push(format!("date_path >= '{}'", date_from));
    }
    if let Some(ref date_to) = req.date_to {
        conditions.push(format!("date_path <= '{}'", date_to));
    }

    // Row-level filters
    if let Some(ref tu_id) = req.tu_id {
        conditions.push(format!("tu_id = '{}'", tu_id.replace('\'', "''")));
    }
    if let Some(ref event_type) = req.event_type {
        conditions.push(format!("event_type = '{}'", event_type.replace('\'', "''")));
    }
    if let Some(ref date_from) = req.date_from {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_from, "%Y-%m-%d") {
            let ts = d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
            conditions.push(format!("timestamp_ms >= {}", ts));
        }
    }
    if let Some(ref date_to) = req.date_to {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_to, "%Y-%m-%d") {
            let ts = d.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp_millis() + 999;
            conditions.push(format!("timestamp_ms <= {}", ts));
        }
    }
    if let Some(ref param_key) = req.param_key {
        if let Some(ref param_value) = req.param_value {
            let search = format!("\"{}\":\"{}\"", param_key, param_value);
            conditions.push(format!("params LIKE '%{}%'", search.replace('\'', "''")));
        }
    }

    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

/// Build SQL for raw events mode.
/// Uses ROW_NUMBER() to deduplicate by event_id — handles duplicate rows
/// that may exist in the Delta table due to consumer rebalance replay.
fn build_events_sql(req: &QueryRequest, limit: u32) -> String {
    let where_clause = build_where_clause(req);
    // Use explicit column list excluding rn. raw_payload may not exist in older Delta tables,
    // so the caller should handle the error gracefully or the table should be migrated.
    format!(
        "SELECT event_id, tenant_id, tu_id, event_type, timestamp_ms, ip, user_agent, referer, request_path, request_host, params \
         FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn FROM events{}) \
         WHERE rn = 1 ORDER BY timestamp_ms DESC LIMIT {}",
        where_clause, limit
    )
}

/// Build SQL for custom query mode.
/// Wraps the user-provided SQL fragment in a dedup CTE with mandatory tenant_id filter.
/// The custom_sql field should be a SELECT statement that queries from `deduped`.
/// JSON functions (json_get_str, json_get_int, json_get_float, etc.) are available.
fn build_custom_sql(req: &QueryRequest, limit: u32) -> Result<String, String> {
    let custom = req.custom_sql.as_deref()
        .ok_or_else(|| "custom_sql is required when mode=custom".to_string())?;

    // Safety: reject if it contains dangerous keywords
    let lower = custom.to_lowercase();
    for forbidden in &["drop ", "delete ", "insert ", "update ", "alter ", "create ", "truncate "] {
        if lower.contains(forbidden) {
            return Err(format!("Forbidden keyword in custom_sql: {}", forbidden.trim()));
        }
    }

    let tenant_filter = format!("tenant_id = '{}'", req.tenant_id.replace('\'', "''"));

    // Build date filters if provided
    let mut extra_filters = Vec::new();
    if let Some(ref date_from) = req.date_from {
        extra_filters.push(format!("date_path >= '{}'", date_from));
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_from, "%Y-%m-%d") {
            let ts = d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
            extra_filters.push(format!("timestamp_ms >= {}", ts));
        }
    }
    if let Some(ref date_to) = req.date_to {
        extra_filters.push(format!("date_path <= '{}'", date_to));
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_to, "%Y-%m-%d") {
            let ts = d.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp_millis() + 999;
            extra_filters.push(format!("timestamp_ms <= {}", ts));
        }
    }

    let mut where_parts = vec![tenant_filter];
    where_parts.extend(extra_filters);
    let where_clause = format!(" WHERE {}", where_parts.join(" AND "));

    // Dedup CTE scoped to tenant
    let dedup_cte = format!(
        "WITH deduped AS (SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn FROM events{}) WHERE rn = 1)",
        where_clause
    );

    Ok(format!("{} {} LIMIT {}", dedup_cte, custom, limit))
}

/// Build SQL for stats/aggregation mode.
/// All stats queries use a dedup CTE to ensure duplicate rows from consumer
/// rebalance replay are not double-counted (financial-grade accuracy).
fn build_stats_sql(req: &QueryRequest, limit: u32) -> String {
    let where_clause = build_where_clause(req);

    // CTE that deduplicates events by event_id before aggregation
    let dedup_cte = format!(
        "WITH deduped AS (SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn FROM events{}) WHERE rn = 1)",
        where_clause
    );

    match req.group_by.as_deref() {
        Some(gb) if gb.starts_with("param:") => {
            let param_key = &gb[6..];
            // Extract param value from JSON string using regexp
            let pattern = format!("\"{}\":\"([^\"]+)\"", param_key);
            format!(
                "{} SELECT regexp_match(params, '{}') AS param_value, event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY param_value, event_type ORDER BY count DESC LIMIT {}",
                dedup_cte, pattern, limit
            )
        }
        Some("event_type") => {
            format!(
                "{} SELECT event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY event_type ORDER BY count DESC",
                dedup_cte
            )
        }
        Some("tu_id") => {
            format!(
                "{} SELECT tu_id, event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY tu_id, event_type ORDER BY count DESC LIMIT {}",
                dedup_cte, limit
            )
        }
        Some("date") => {
            // Compute date from timestamp_ms to avoid Dictionary-encoded partition column in GROUP BY.
            // Partition pruning still happens via WHERE clause on date_path.
            format!(
                "{} SELECT CAST(to_timestamp(timestamp_ms / 1000) AS DATE) AS date, event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY date, event_type ORDER BY date ASC",
                dedup_cte
            )
        }
        Some("hour") => {
            format!(
                "{} SELECT CAST(to_timestamp(timestamp_ms / 1000) AS DATE) AS date, \
                 lpad(CAST(EXTRACT(HOUR FROM to_timestamp(timestamp_ms / 1000)) AS VARCHAR), 2, '0') AS hour, \
                 event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY date, hour, event_type ORDER BY date ASC, hour ASC",
                dedup_cte
            )
        }
        _ => {
            format!(
                "{} SELECT event_type, COUNT(*) AS count \
                 FROM deduped GROUP BY event_type",
                dedup_cte
            )
        }
    }
}

/// POST /query handler.
async fn query_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Query: tenant={} tu_id={:?} type={:?} from={:?} to={:?} group={:?} mode={:?}",
        req.tenant_id, req.tu_id, req.event_type, req.date_from, req.date_to, req.group_by, req.mode
    );

    match execute_query(&state, &req).await {
        Ok(response) => {
            info!(
                "Query complete: {} rows, {} partitions, {}ms",
                response.count, response.partitions_scanned, response.query_ms
            );
            Ok(Json(response))
        }
        Err(e) => {
            error!("Query error: {}", e);
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
        "service": "polars-query"
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
    let port = env::var("PORT").unwrap_or_else(|_| "3040".into());
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());

    info!("Starting polars-query (Delta Lake + DataFusion) on {}:{}...", host, port);
    info!("Delta table: {}", state.delta_table_uri());

    let app = Router::new()
        .route("/query", post(query_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    info!("polars-query listening on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
