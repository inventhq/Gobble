//! Delta Lake integration via DataFusion.
//!
//! Opens the Delta table on R2, registers it as a DataFusion table provider,
//! and executes tenant-scoped SQL queries with mandatory isolation via CTE wrapper.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::DataType;
use deltalake::DeltaTable;
use tracing::info;
use url::Url;

use crate::config::Config;

/// Storage options for Delta Lake R2 access.
pub fn storage_options(config: &Config) -> HashMap<String, String> {
    let mut opts = HashMap::new();
    opts.insert("AWS_ACCESS_KEY_ID".into(), config.r2_access_key_id.clone());
    opts.insert(
        "AWS_SECRET_ACCESS_KEY".into(),
        config.r2_secret_access_key.clone(),
    );
    opts.insert("AWS_ENDPOINT_URL".into(), config.r2_endpoint.clone());
    opts.insert("AWS_REGION".into(), "auto".into());
    opts.insert("aws_conditional_put".into(), "etag".into());
    opts.insert("AWS_S3_ALLOW_UNSAFE_RENAME".into(), "true".into());
    opts
}

/// Delta table URI on R2.
pub fn delta_table_uri(config: &Config) -> String {
    format!("s3://{}/events", config.r2_bucket)
}

/// Open the Delta table (read-only).
pub async fn open_table(config: &Config) -> Result<DeltaTable, String> {
    let uri = delta_table_uri(config);
    let url =
        Url::parse(&uri).map_err(|e| format!("Invalid Delta table URI: {}", e))?;
    deltalake::open_table_with_storage_options(url, storage_options(config))
        .await
        .map_err(|e| format!("Failed to open Delta table at {}: {:?}", uri, e))
}

/// Execute a tenant-scoped SQL query against the Delta table.
///
/// The SQL is wrapped in a dedup CTE with mandatory `tenant_id` filter.
/// Returns JSON rows.
pub async fn execute_sql(
    config: &Config,
    tenant_id: &str,
    sql: &str,
    limit: u32,
) -> Result<(Vec<serde_json::Value>, u64), String> {
    let start = std::time::Instant::now();

    let table = open_table(config).await?;
    let version = table.version();
    info!("Opened Delta table (version {:?})", version);

    let mut ctx = deltalake::datafusion::prelude::SessionContext::new();
    datafusion_functions_json::register_all(&mut ctx)
        .map_err(|e| format!("Failed to register JSON functions: {:?}", e))?;

    let table_state = table
        .snapshot()
        .map_err(|e| format!("Failed to get snapshot: {:?}", e))?;
    let eager_snapshot = table_state.snapshot().clone();

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

    // Wrap user SQL in a dedup CTE scoped to tenant
    let safe_tenant = tenant_id.replace('\'', "''");
    let has_limit = sql.to_lowercase().contains(" limit ");
    let wrapped = if has_limit {
        format!(
            "WITH scoped AS (\
                SELECT * FROM (\
                    SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn \
                    FROM events WHERE tenant_id = '{}'\
                ) WHERE rn = 1\
            ) {}",
            safe_tenant, sql
        )
    } else {
        format!(
            "WITH scoped AS (\
                SELECT * FROM (\
                    SELECT *, ROW_NUMBER() OVER (PARTITION BY event_id ORDER BY timestamp_ms DESC) AS rn \
                    FROM events WHERE tenant_id = '{}'\
                ) WHERE rn = 1\
            ) {} LIMIT {}",
            safe_tenant, sql, limit
        )
    };

    info!("DataFusion SQL: {}", wrapped);

    let df = ctx
        .sql(&wrapped)
        .await
        .map_err(|e| format!("DataFusion SQL error: {:?}", e))?;

    let batches = df
        .collect()
        .await
        .map_err(|e| format!("DataFusion collect error: {:?}", e))?;

    let rows = batches_to_json(&batches)?;
    let query_ms = start.elapsed().as_millis() as u64;

    Ok((rows, query_ms))
}

/// Convert Arrow RecordBatches to JSON rows.
pub fn batches_to_json(batches: &[RecordBatch]) -> Result<Vec<serde_json::Value>, String> {
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
                            let formatter = arrow::util::display::ArrayFormatter::try_new(
                                col.as_ref(),
                                &Default::default(),
                            )
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

/// Schema description for the events table (used in SLM prompts).
pub const EVENTS_SCHEMA: &str = "\
Table: events (Delta Lake on R2 — raw tracking events, full history)
Columns:
  - event_id: TEXT (UUIDv7, primary key)
  - tenant_id: TEXT (tenant identifier, partition column)
  - tu_id: TEXT (tracking URL ID, nullable)
  - event_type: TEXT ('click', 'postback', 'impression', or custom types from plugins)
  - timestamp_ms: BIGINT (Unix milliseconds)
  - ip: TEXT (client IP address)
  - user_agent: TEXT
  - referer: TEXT (nullable)
  - request_path: TEXT
  - request_host: TEXT
  - params: TEXT (JSON object with custom key-value params, e.g. {\"sub1\":\"google\",\"click_id\":\"abc\"})
  - raw_payload: TEXT (JSON, nullable — full webhook/API payload from plugins)
  - date_path: TEXT (YYYY-MM-DD, partition column)
  - event_hour: TEXT ('00'-'23')

Notes:
  - Query from the `scoped` CTE (already filtered to tenant and deduplicated).
  - Use json_get_str(params, '$.key') to extract param values.
  - Use json_get_str(raw_payload, '$.path.to.field') for raw_payload fields.
  - Timestamps are in Unix milliseconds. Use to_timestamp(timestamp_ms / 1000) for date functions.
  - Common aggregations: COUNT(*), GROUP BY event_type, GROUP BY date_path, GROUP BY event_hour.
";
