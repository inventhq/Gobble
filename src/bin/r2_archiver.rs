//! r2-archiver — Iggy consumer that archives tracking events to Cloudflare R2 as Delta Lake tables.
//!
//! Runs as its own Iggy consumer group ("r2-archiver"), independent of risingwave-consumer.
//! Reads events from the Iggy stream, batches them, and appends to a Delta table on R2.
//!
//! Delta table location: s3://{bucket}/events/
//! Partition columns: tenant_id, date_path (YYYY-MM-DD)
//! Delta log: s3://{bucket}/events/_delta_log/
//!
//! Features:
//!   - ACID writes via Delta Lake transaction log
//!   - Automatic file management (no manual chunk numbering)
//!   - Periodic OPTIMIZE compaction (merges small files into large ones)
//!   - R2 conditional PUT (etag) for lock-free concurrency
//!
//! Zero load on RisingWave — reads directly from Iggy.

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::sync::Arc;

use aggregate_schema::paths as agg_paths;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use base64::Engine;
use chrono::{TimeZone, Utc};
use deltalake::operations::optimize::OptimizeType;
use deltalake::operations::write::SchemaMode;
use deltalake::protocol::SaveMode;
use deltalake::DeltaTable;
use iggy::prelude::*;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjPath;
use object_store::ObjectStore;
use tokio::signal;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

use tracker_core::event::TrackingEvent;

/// Maximum number of event IDs to remember for deduplication.
const DEDUP_CAPACITY: usize = 100_000;


/// Default batch size — flush to R2 after this many events.
const DEFAULT_BATCH_SIZE: usize = 5_000;

/// Default flush interval in milliseconds.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 30_000;

/// Number of flushes between OPTIMIZE compaction runs.
const COMPACTION_INTERVAL_FLUSHES: u64 = 60;

/// Archiver configuration loaded from environment variables.
struct ArchiverConfig {
    iggy_url: String,
    iggy_http_url: String,
    iggy_stream: String,
    iggy_topic: String,
    r2_endpoint: String,
    r2_access_key_id: String,
    r2_secret_access_key: String,
    r2_bucket: String,
    batch_size: usize,
    flush_interval_ms: u64,
}

impl ArchiverConfig {
    fn from_env() -> Self {
        Self {
            iggy_url: env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into()),
            iggy_http_url: env::var("IGGY_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
            iggy_stream: env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into()),
            iggy_topic: env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into()),
            r2_endpoint: env::var("R2_ENDPOINT").expect("R2_ENDPOINT is required"),
            r2_access_key_id: env::var("R2_ACCESS_KEY_ID").expect("R2_ACCESS_KEY_ID is required"),
            r2_secret_access_key: env::var("R2_SECRET_ACCESS_KEY")
                .expect("R2_SECRET_ACCESS_KEY is required"),
            r2_bucket: env::var("R2_BUCKET").unwrap_or_else(|_| "tracker-events".into()),
            batch_size: env::var("R2_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_BATCH_SIZE),
            flush_interval_ms: env::var("R2_FLUSH_INTERVAL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS),
        }
    }

    /// Build the Delta table URI for R2.
    fn delta_table_uri(&self) -> String {
        format!("s3://{}/events", self.r2_bucket)
    }

    /// Build storage_options for Delta Lake R2 access.
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

    /// Build an object_store S3 client for writing aggregate Parquet files to R2.
    fn build_aggregate_store(&self) -> Arc<dyn ObjectStore> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&self.r2_endpoint)
            .with_bucket_name(&self.r2_bucket)
            .with_access_key_id(&self.r2_access_key_id)
            .with_secret_access_key(&self.r2_secret_access_key)
            .with_region("auto")
            .with_virtual_hosted_style_request(false)
            .build()
            .expect("Failed to build aggregate object store");
        Arc::new(store)
    }
}

/// A single event ready for Parquet serialization.
#[derive(Clone)]
struct EventRow {
    event_id: String,
    tenant_id: String,
    tu_id: String,
    event_type: String,
    timestamp_ms: i64,
    ip: String,
    user_agent: String,
    referer: Option<String>,
    request_path: String,
    request_host: String,
    params: String, // JSON string
    raw_payload: Option<String>, // JSON string of full nested payload
    date_path: String, // partition column: YYYY-MM-DD
    event_hour: String, // partition column: "00"–"23"
}

impl EventRow {
    /// Convert a TrackingEvent + tenant prefix into an EventRow.
    fn from_tracking_event(tenant: &str, event: &TrackingEvent) -> Self {
        let tu_id = event
            .params
            .get("tu_id")
            .cloned()
            .unwrap_or_else(|| "_no_link".into());
        let params_json = serde_json::to_string(&event.params).unwrap_or_else(|_| "{}".into());
        let ts = event.timestamp as i64;
        let dt = Utc
            .timestamp_millis_opt(ts)
            .single()
            .unwrap_or_else(Utc::now);
        let date_path = dt.format("%Y-%m-%d").to_string();
        let event_hour = dt.format("%H").to_string();

        Self {
            event_id: event.event_id.clone(),
            tenant_id: tenant.to_string(),
            tu_id,
            event_type: event.event_type.clone(),
            timestamp_ms: ts,
            ip: event.ip.clone(),
            user_agent: event.user_agent.clone(),
            referer: event.referer.clone(),
            request_path: event.request_path.clone(),
            request_host: event.request_host.clone(),
            params: params_json,
            raw_payload: event.raw_payload.as_ref().map(|v| v.to_string()),
            date_path,
            event_hour,
        }
    }
}

/// Event row wrapper that carries Iggy offset metadata through the channel so the
/// writer task can report back which offset to commit after a successful flush.
struct EventRowWithOffset {
    row: EventRow,
    partition_id: u32,
    offset: u64,
}

/// Extract the tenant prefix from the event's params.
fn extract_tenant_prefix(event: &TrackingEvent) -> String {
    event
        .params
        .get("key_prefix")
        .cloned()
        .unwrap_or_else(|| "_global".to_string())
}

/// Arrow schema for the Delta table.
/// Partition columns: `tenant_id`, `date_path`, `event_hour` for efficient pruning.
fn delta_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("event_id", DataType::Utf8, false),
        Field::new("tenant_id", DataType::Utf8, false),
        Field::new("tu_id", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("ip", DataType::Utf8, false),
        Field::new("user_agent", DataType::Utf8, false),
        Field::new("referer", DataType::Utf8, true),
        Field::new("request_path", DataType::Utf8, false),
        Field::new("request_host", DataType::Utf8, false),
        Field::new("params", DataType::Utf8, false),
        Field::new("raw_payload", DataType::Utf8, true),
        Field::new("date_path", DataType::Utf8, false),
        Field::new("event_hour", DataType::Utf8, false),
    ]))
}

/// Build a RecordBatch from a list of events.
fn events_to_record_batch(events: &[EventRow], schema: &Arc<Schema>) -> RecordBatch {
    let event_ids: Vec<&str> = events.iter().map(|e| e.event_id.as_str()).collect();
    let tenant_ids: Vec<&str> = events.iter().map(|e| e.tenant_id.as_str()).collect();
    let tu_ids: Vec<&str> = events.iter().map(|e| e.tu_id.as_str()).collect();
    let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    let timestamps: Vec<i64> = events.iter().map(|e| e.timestamp_ms).collect();
    let ips: Vec<&str> = events.iter().map(|e| e.ip.as_str()).collect();
    let user_agents: Vec<&str> = events.iter().map(|e| e.user_agent.as_str()).collect();
    let referers: Vec<Option<&str>> = events.iter().map(|e| e.referer.as_deref()).collect();
    let request_paths: Vec<&str> = events.iter().map(|e| e.request_path.as_str()).collect();
    let request_hosts: Vec<&str> = events.iter().map(|e| e.request_host.as_str()).collect();
    let params: Vec<&str> = events.iter().map(|e| e.params.as_str()).collect();
    let raw_payloads: Vec<Option<&str>> = events.iter().map(|e| e.raw_payload.as_deref()).collect();
    let date_paths: Vec<&str> = events.iter().map(|e| e.date_path.as_str()).collect();
    let event_hours: Vec<&str> = events.iter().map(|e| e.event_hour.as_str()).collect();

    RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(event_ids)),
            Arc::new(StringArray::from(tenant_ids)),
            Arc::new(StringArray::from(tu_ids)),
            Arc::new(StringArray::from(event_types)),
            Arc::new(Int64Array::from(timestamps)),
            Arc::new(StringArray::from(ips)),
            Arc::new(StringArray::from(user_agents)),
            Arc::new(StringArray::from(referers)),
            Arc::new(StringArray::from(request_paths)),
            Arc::new(StringArray::from(request_hosts)),
            Arc::new(StringArray::from(params)),
            Arc::new(StringArray::from(raw_payloads)),
            Arc::new(StringArray::from(date_paths)),
            Arc::new(StringArray::from(event_hours)),
        ],
    )
    .expect("Failed to create RecordBatch")
}

/// Open or create the Delta table on R2.
async fn open_or_create_table(
    table_uri: &str,
    storage_options: HashMap<String, String>,
) -> Result<DeltaTable, String> {
    let url = Url::parse(table_uri)
        .map_err(|e| format!("Invalid Delta table URI '{}': {}", table_uri, e))?;

    match deltalake::open_table_with_storage_options(url.clone(), storage_options.clone()).await {
        Ok(table) => {
            info!("Opened existing Delta table at {} (version {:?})", table_uri, table.version());
            Ok(table)
        }
        Err(_) => {
            info!("Delta table not found at {} — will be created on first write", table_uri);
            let table = DeltaTable::try_from_url_with_storage_options(url, storage_options)
                .await
                .map_err(|e| format!("Failed to create DeltaTable: {}", e))?;
            Ok(table)
        }
    }
}

/// Append a batch of events to the Delta table.
async fn flush_to_delta(
    table: DeltaTable,
    events: &[EventRow],
) -> Result<(DeltaTable, usize), String> {
    if events.is_empty() {
        return Ok((table, 0));
    }

    let schema = delta_schema();
    let batch = events_to_record_batch(events, &schema);
    let count = events.len();

    let table = table
        .write(vec![batch])
        .with_save_mode(SaveMode::Append)
        .with_schema_mode(SchemaMode::Merge)
        .with_partition_columns(vec!["tenant_id", "date_path"])
        .await
        .map_err(|e| format!("Delta write failed: {:?}", e))?;

    Ok((table, count))
}

/// Run OPTIMIZE compaction on the Delta table to merge small files.
async fn run_compaction(table: DeltaTable) -> Result<DeltaTable, String> {
    info!("Running OPTIMIZE compaction...");
    let (table, metrics) = table
        .optimize()
        .with_type(OptimizeType::Compact)
        .await
        .map_err(|e| format!("OPTIMIZE failed: {:?}", e))?;

    info!(
        "Compaction complete: {} files added, {} files removed",
        metrics.num_files_added, metrics.num_files_removed
    );
    Ok(table)
}

/// Compute hourly aggregates from a batch of events and write them as Parquet to R2.
///
/// Groups events by (tenant_id, event_type, date_path, hour) and writes one
/// aggregate Parquet file per unique (tenant_id, date_path, hour) combination.
async fn write_inline_aggregates(
    store: &dyn ObjectStore,
    events: &[EventRow],
    flush_id: u64,
) {
    if events.is_empty() {
        return;
    }

    // Group: (tenant_id, event_type, date_path, hour) → count
    let mut counts: HashMap<(String, String, String, String), i64> = HashMap::new();
    for event in events {
        let hour = Utc
            .timestamp_millis_opt(event.timestamp_ms)
            .single()
            .unwrap_or_else(Utc::now)
            .format("%H")
            .to_string();
        let key = (
            event.tenant_id.clone(),
            event.event_type.clone(),
            event.date_path.clone(),
            hour,
        );
        *counts.entry(key).or_insert(0) += 1;
    }

    // Group by (tenant_id, date_path, hour) for file-level grouping
    let mut file_groups: HashMap<(String, String, String), Vec<(String, i64)>> = HashMap::new();
    for ((tenant_id, event_type, date_path, hour), count) in &counts {
        file_groups
            .entry((tenant_id.clone(), date_path.clone(), hour.clone()))
            .or_default()
            .push((event_type.clone(), *count));
    }

    // Write one Parquet file per (tenant_id, date_path, hour)
    for ((tenant_id, date_path, hour), type_counts) in &file_groups {
        let n = type_counts.len();
        let tenant_ids: Vec<&str> = vec![tenant_id.as_str(); n];
        let date_paths: Vec<&str> = vec![date_path.as_str(); n];
        let hours: Vec<&str> = vec![hour.as_str(); n];
        let event_types: Vec<&str> = type_counts.iter().map(|(t, _)| t.as_str()).collect();
        let count_vals: Vec<i64> = type_counts.iter().map(|(_, c)| *c).collect();

        let schema = Arc::new(Schema::new(vec![
            Field::new(aggregate_schema::columns::TENANT_ID, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::EVENT_TYPE, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::DATE_PATH, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::HOUR, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::COUNT, DataType::Int64, false),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(tenant_ids)),
                Arc::new(StringArray::from(event_types)),
                Arc::new(StringArray::from(date_paths)),
                Arc::new(StringArray::from(hours)),
                Arc::new(Int64Array::from(count_vals)),
            ],
        );

        let batch = match batch {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to create aggregate RecordBatch: {} — skipping", e);
                continue;
            }
        };

        // Serialize to Parquet in memory
        let mut buf = Vec::new();
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        let mut writer = match parquet::arrow::ArrowWriter::try_new(&mut buf, schema, Some(props)) {
            Ok(w) => w,
            Err(e) => {
                warn!("Failed to create Parquet writer for aggregates: {} — skipping", e);
                continue;
            }
        };
        if let Err(e) = writer.write(&batch) {
            warn!("Failed to write aggregate batch: {} — skipping", e);
            continue;
        }
        if let Err(e) = writer.close() {
            warn!("Failed to close aggregate Parquet writer: {} — skipping", e);
            continue;
        }

        // Upload to R2
        let key = agg_paths::aggregate_key(tenant_id, date_path, hour, flush_id);
        let obj_path = ObjPath::from(key.as_str());
        match store.put(&obj_path, bytes::Bytes::from(buf).into()).await {
            Ok(_) => {}
            Err(e) => {
                warn!("Failed to upload aggregate {}: {} — skipping", key, e);
            }
        }
    }

    let total_agg_rows: i64 = counts.values().sum();
    info!(
        "Wrote {} aggregate rows across {} files (flush_id={})",
        total_agg_rows,
        file_groups.len(),
        flush_id
    );
}

/// Periodic reconciliation: recompute today's aggregates from the raw Delta table
/// using DataFusion, then overwrite the aggregate files on R2 (atomic via tmp + rename).
async fn run_reconciliation(
    table: &DeltaTable,
    store: &dyn ObjectStore,
    today: &str,
) {
    info!("Running aggregate reconciliation for {}...", today);

    let ctx = deltalake::datafusion::prelude::SessionContext::new();
    let table_state = match table.snapshot() {
        Ok(s) => s,
        Err(e) => {
            warn!("Reconciliation: failed to get snapshot: {:?}", e);
            return;
        }
    };
    let eager_snapshot = table_state.snapshot().clone();
    let provider = match deltalake::delta_datafusion::DeltaTableProvider::try_new(
        eager_snapshot,
        table.log_store().clone(),
        Default::default(),
    ) {
        Ok(p) => p,
        Err(e) => {
            warn!("Reconciliation: failed to create provider: {:?}", e);
            return;
        }
    };

    if let Err(e) = ctx.register_table("events", Arc::new(provider)) {
        warn!("Reconciliation: failed to register table: {:?}", e);
        return;
    }

    let sql = format!(
        "SELECT tenant_id, event_type, date_path, \
         lpad(CAST(EXTRACT(HOUR FROM to_timestamp(timestamp_ms / 1000)) AS VARCHAR), 2, '0') AS hour, \
         COUNT(*) AS count \
         FROM events WHERE date_path = '{}' \
         GROUP BY tenant_id, event_type, date_path, hour",
        today
    );

    let df = match ctx.sql(&sql).await {
        Ok(df) => df,
        Err(e) => {
            warn!("Reconciliation SQL error: {:?}", e);
            return;
        }
    };

    let batches = match df.collect().await {
        Ok(b) => b,
        Err(e) => {
            warn!("Reconciliation collect error: {:?}", e);
            return;
        }
    };

    // Group results by (tenant_id, date_path, hour) and write aggregate files
    // Use a deterministic flush_id (0) for reconciliation files so they overwrite cleanly
    let reconcile_id = 0u64;

    let mut file_groups: HashMap<(String, String, String), Vec<(String, i64)>> = HashMap::new();

    for batch in &batches {
        let tenant_col = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
        let type_col = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        let date_col = batch.column(2).as_any().downcast_ref::<StringArray>().unwrap();
        let hour_col = batch.column(3).as_any().downcast_ref::<StringArray>().unwrap();
        let count_col = batch.column(4).as_any().downcast_ref::<Int64Array>().unwrap();

        for i in 0..batch.num_rows() {
            let tenant = tenant_col.value(i).to_string();
            let event_type = type_col.value(i).to_string();
            let date_path = date_col.value(i).to_string();
            let hour = hour_col.value(i).to_string();
            let count = count_col.value(i);

            file_groups
                .entry((tenant, date_path, hour))
                .or_default()
                .push((event_type, count));
        }
    }

    let mut files_written = 0u64;
    for ((tenant_id, date_path, hour), type_counts) in &file_groups {
        let n = type_counts.len();
        let schema = Arc::new(Schema::new(vec![
            Field::new(aggregate_schema::columns::TENANT_ID, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::EVENT_TYPE, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::DATE_PATH, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::HOUR, DataType::Utf8, false),
            Field::new(aggregate_schema::columns::COUNT, DataType::Int64, false),
        ]));

        let batch = match RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![tenant_id.as_str(); n])),
                Arc::new(StringArray::from(type_counts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(vec![date_path.as_str(); n])),
                Arc::new(StringArray::from(vec![hour.as_str(); n])),
                Arc::new(Int64Array::from(type_counts.iter().map(|(_, c)| *c).collect::<Vec<_>>())),
            ],
        ) {
            Ok(b) => b,
            Err(e) => {
                warn!("Reconciliation: failed to create batch: {}", e);
                continue;
            }
        };

        let mut buf = Vec::new();
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        let mut writer = match parquet::arrow::ArrowWriter::try_new(&mut buf, schema, Some(props)) {
            Ok(w) => w,
            Err(e) => {
                warn!("Reconciliation: Parquet writer error: {}", e);
                continue;
            }
        };
        if let Err(e) = writer.write(&batch) {
            warn!("Reconciliation: write error: {}", e);
            continue;
        }
        if let Err(e) = writer.close() {
            warn!("Reconciliation: close error: {}", e);
            continue;
        }

        // Atomic overwrite: write to .tmp then rename
        let tmp_key = agg_paths::aggregate_tmp_key(tenant_id, &date_path, hour, reconcile_id);
        let final_key = agg_paths::aggregate_key(tenant_id, &date_path, hour, reconcile_id);
        let tmp_path = ObjPath::from(tmp_key.as_str());
        let final_path = ObjPath::from(final_key.as_str());

        match store.put(&tmp_path, bytes::Bytes::from(buf).into()).await {
            Ok(_) => {
                // R2 supports rename via copy + delete
                match store.rename(&tmp_path, &final_path).await {
                    Ok(_) => files_written += 1,
                    Err(e) => {
                        warn!("Reconciliation: rename failed for {}: {} — falling back to direct put", final_key, e);
                        // Fallback: the tmp file is already there, just copy it
                        let _ = store.copy(&tmp_path, &final_path).await;
                        let _ = store.delete(&tmp_path).await;
                        files_written += 1;
                    }
                }
            }
            Err(e) => {
                warn!("Reconciliation: upload failed for {}: {}", tmp_key, e);
            }
        }
    }

    info!(
        "Reconciliation complete for {}: {} aggregate files written",
        today, files_written
    );
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

    info!("Starting r2-archiver (Delta Lake mode)...");

    // Check if R2 credentials are configured before loading config
    match env::var("R2_ENDPOINT") {
        Ok(u) if !u.is_empty() && u != "CHANGE_ME" => {}
        _ => {
            warn!("R2_ENDPOINT not configured — r2-archiver cannot run. Sleeping forever.");
            loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
        }
    }

    let config = ArchiverConfig::from_env();

    info!(
        "Iggy: {}  Stream: {}  Topic: {}",
        config.iggy_url, config.iggy_stream, config.iggy_topic
    );
    info!(
        "Delta table: {}, batch_size: {}, flush_interval: {}ms",
        config.delta_table_uri(), config.batch_size, config.flush_interval_ms
    );

    // --- Graceful shutdown signal ---
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    // --- Channel to decouple Iggy consumption from R2 writes ---
    let (tx, mut rx) = tokio_mpsc::channel::<EventRowWithOffset>(10_000);

    // --- Feedback channel: writer → Iggy task (confirmed offsets for manual commit) ---
    // Each ack carries a Vec of (partition_id, max_offset) pairs — one per partition in the batch.
    let (ack_tx, mut ack_rx) = tokio_mpsc::channel::<Vec<(u32, u64)>>(100);

    // --- Task 1: Iggy consumer → channel (dedup + deserialize) + commit offsets on ack ---
    let iggy_url = config.iggy_url.clone();
    let iggy_http_url = config.iggy_http_url.clone();
    let iggy_stream = config.iggy_stream.clone();
    let iggy_topic = config.iggy_topic.clone();

    let iggy_handle = tokio::spawn(async move {
        let shutdown_rx = shutdown_rx; // move into task

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

        let consumer_id = Consumer::new(Identifier::named("r2-archiver").unwrap());
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
            "Consuming events (r2-archiver, poll_count={}, HTTP polling, at-least-once)...",
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
                    let row = EventRow::from_tracking_event(&tenant, &event);

                    if tx.send(EventRowWithOffset { row, partition_id: pid, offset: msg_offset }).await.is_err() {
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

    // --- Task 2: Channel → batch → Delta Lake append (with retry + offset ack + compaction) ---
    let batch_size = config.batch_size;
    let flush_interval_ms = config.flush_interval_ms;
    let table_uri = config.delta_table_uri();
    let storage_options = config.storage_options();

    // Build the aggregate object store for writing warm-tier Parquet files
    let agg_store = config.build_aggregate_store();

    // Open or prepare the Delta table
    let mut table = open_or_create_table(&table_uri, storage_options)
        .await
        .expect("Failed to open/create Delta table");

    let mut total_events = 0u64;
    let mut total_flushes = 0u64;

    let mut batch: Vec<EventRowWithOffset> = Vec::with_capacity(batch_size);
    let flush_interval = std::time::Duration::from_millis(flush_interval_ms);
    let mut last_flush = std::time::Instant::now();

    info!(
        "Archiving events to Delta table at {} (batch_size={}, flush_interval={}ms, compaction_every={} flushes, at-least-once)...",
        table_uri, batch_size, flush_interval_ms, COMPACTION_INTERVAL_FLUSHES
    );

    loop {
        let remaining = flush_interval.saturating_sub(last_flush.elapsed());
        let timeout_dur = if remaining.is_zero() {
            std::time::Duration::from_millis(1)
        } else {
            remaining
        };

        match tokio::time::timeout(timeout_dur, rx.recv()).await {
            Ok(Some(row_with_offset)) => {
                batch.push(row_with_offset);
            }
            Ok(None) => {
                info!("Channel closed, flushing remaining events");
                break;
            }
            Err(_) => {
                // Timeout — flush timer expired
            }
        }

        if batch.len() >= batch_size
            || (!batch.is_empty() && last_flush.elapsed() >= flush_interval)
        {
            let flush_data = std::mem::replace(&mut batch, Vec::with_capacity(batch_size));

            // Extract the max offset per partition for this batch
            let mut partition_max_offsets: HashMap<u32, u64> = HashMap::new();
            for item in &flush_data {
                let entry = partition_max_offsets.entry(item.partition_id).or_insert(0);
                if item.offset > *entry {
                    *entry = item.offset;
                }
            }

            // Separate rows from offset metadata
            let flush_rows: Vec<EventRow> = flush_data.into_iter().map(|e| e.row).collect();
            let batch_len = flush_rows.len();

            match flush_to_delta(table, &flush_rows).await {
                Ok((new_table, count)) => {
                    table = new_table;
                    total_events += count as u64;
                    total_flushes += 1;
                    info!(
                        "Delta append: {} events (version {:?}) | Total: {} events, {} flushes",
                        count, table.version(), total_events, total_flushes,
                    );

                    // Write inline hourly aggregates (warm tier)
                    write_inline_aggregates(agg_store.as_ref(), &flush_rows, total_flushes).await;

                    // Ack all partition offsets — Iggy task will commit each
                    let offsets: Vec<(u32, u64)> = partition_max_offsets.into_iter().collect();
                    let _ = ack_tx.send(offsets).await;

                    // Periodic compaction + reconciliation
                    if total_flushes % COMPACTION_INTERVAL_FLUSHES == 0 {
                        match run_compaction(table).await {
                            Ok(new_table) => {
                                table = new_table;
                                // Run reconciliation after compaction to fix any aggregate gaps
                                let today = Utc::now().format("%Y-%m-%d").to_string();
                                run_reconciliation(&table, agg_store.as_ref(), &today).await;
                            }
                            Err(e) => {
                                error!("Compaction failed (non-fatal): {}", e);
                                // Re-open the table to recover state
                                let uri = config.delta_table_uri();
                                let opts = config.storage_options();
                                table = open_or_create_table(&uri, opts)
                                    .await
                                    .expect("Failed to re-open Delta table after compaction failure");
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to flush batch of {} events: {} — offset NOT committed",
                        batch_len, e
                    );
                    // Re-open the table to recover state after write failure
                    let uri = config.delta_table_uri();
                    let opts = config.storage_options();
                    table = open_or_create_table(&uri, opts)
                        .await
                        .expect("Failed to re-open Delta table after write failure");
                }
            }

            last_flush = std::time::Instant::now();
        }
    }

    // Flush remaining + final compaction
    {
        let mut current_table = table;

        if !batch.is_empty() {
            let mut partition_max_offsets: HashMap<u32, u64> = HashMap::new();
            for item in &batch {
                let entry = partition_max_offsets.entry(item.partition_id).or_insert(0);
                if item.offset > *entry {
                    *entry = item.offset;
                }
            }

            let flush_rows: Vec<EventRow> = batch.into_iter().map(|e| e.row).collect();
            let batch_len = flush_rows.len();

            match flush_to_delta(current_table, &flush_rows).await {
                Ok((new_table, count)) => {
                    current_table = new_table;
                    total_events += count as u64;
                    total_flushes += 1;
                    info!("Flushed final {} events (version {:?})", batch_len, current_table.version());
                    // Write final inline aggregates
                    write_inline_aggregates(agg_store.as_ref(), &flush_rows, total_flushes).await;
                    let offsets: Vec<(u32, u64)> = partition_max_offsets.into_iter().collect();
                    let _ = ack_tx.send(offsets).await;
                }
                Err(e) => {
                    error!(
                        "Failed to flush final batch of {} events: {} — offset NOT committed",
                        batch_len, e
                    );
                    // Re-open table for final compaction
                    let uri = config.delta_table_uri();
                    let opts = config.storage_options();
                    current_table = open_or_create_table(&uri, opts)
                        .await
                        .expect("Failed to re-open Delta table");
                }
            }
        }

        // Final compaction before shutdown
        info!("Running final compaction before shutdown...");
        match run_compaction(current_table).await {
            Ok(_) => info!("Final compaction complete"),
            Err(e) => warn!("Final compaction failed (non-fatal): {}", e),
        }
    }

    // Drop ack_tx so the Iggy task's ack_rx drains and the task can finish
    drop(ack_tx);

    let _ = iggy_handle.await;

    info!(
        "Archiver done. Total: {} events, {} flushes",
        total_events, total_flushes,
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
