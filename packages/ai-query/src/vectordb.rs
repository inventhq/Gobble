//! LanceDB Cloud integration for vector similarity search.
//!
//! Uses the LanceDB Cloud REST API directly via `reqwest` — no heavy
//! `lancedb` crate needed (that pulls in 200+ crates for the local engine).
//!
//! The REST API uses `db://` URIs which map to:
//!   `https://{db_name}.{region}.api.lancedb.com`
//!
//! Auth: `x-api-key` header.

use std::sync::Arc;

use arrow::array::{
    FixedSizeListArray, Int64Array, StringArray,
};
use arrow::datatypes::{DataType, Field, Float32Type, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use reqwest::Client;
use serde::Serialize;
use tracing::{error, info, warn};

use crate::config::Config;

/// Dimension of the embedding vectors.
/// 384 = all-MiniLM-L6-v2 (sentence-transformers default, small + fast).
const EMBEDDING_DIM: usize = 384;
const EMBEDDING_DIM_I32: i32 = EMBEDDING_DIM as i32;

/// Table name in LanceDB Cloud.
const TABLE_NAME: &str = "events";

/// Default region for LanceDB Cloud.
const DEFAULT_REGION: &str = "us-east-1";

/// Parse a `db://name` URI into the REST API base URL.
fn base_url(config: &Config) -> Result<String, String> {
    let uri = config
        .lancedb_uri
        .as_ref()
        .ok_or_else(|| "LANCEDB_URI not configured".to_string())?;

    let db_name = uri
        .strip_prefix("db://")
        .ok_or_else(|| format!("LANCEDB_URI must start with db:// (got: {})", uri))?;

    let region = std::env::var("LANCEDB_REGION").unwrap_or_else(|_| DEFAULT_REGION.into());
    Ok(format!("https://{}.{}.api.lancedb.com", db_name, region))
}

/// Build a reqwest client with the LanceDB API key header.
fn api_client(config: &Config) -> Result<(Client, String), String> {
    let api_key = config
        .lancedb_api_key
        .as_ref()
        .ok_or_else(|| "LANCEDB_API_KEY not configured".to_string())?
        .clone();
    Ok((Client::new(), api_key))
}

/// Check if LanceDB Cloud is reachable. Returns true if configured and responding.
pub async fn check_connection(config: &Config) -> bool {
    if config.lancedb_uri.is_none() || config.lancedb_api_key.is_none() {
        warn!("LANCEDB_URI or LANCEDB_API_KEY not set — vector search disabled");
        return false;
    }

    let url = match base_url(config) {
        Ok(u) => u,
        Err(e) => {
            error!("Invalid LANCEDB_URI: {}", e);
            return false;
        }
    };

    let (client, api_key) = match api_client(config) {
        Ok(c) => c,
        Err(e) => {
            error!("{}", e);
            return false;
        }
    };

    // List tables to verify connectivity
    match client
        .get(format!("{}/v1/table/", url))
        .header("x-api-key", &api_key)
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                info!("LanceDB Cloud connected: {}", url);
                true
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                error!("LanceDB Cloud returned {}: {}", status, body);
                false
            }
        }
        Err(e) => {
            error!("LanceDB Cloud unreachable: {}", e);
            false
        }
    }
}

/// Arrow schema for the events vector table.
fn events_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("event_id", DataType::Utf8, false),
        Field::new("tenant_id", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIM_I32,
            ),
            true,
        ),
    ]))
}

/// Serialize a RecordBatch to Arrow IPC stream format (bytes).
fn batch_to_ipc(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buf, &batch.schema())
            .map_err(|e| format!("IPC writer init failed: {}", e))?;
        writer
            .write(batch)
            .map_err(|e| format!("IPC write failed: {}", e))?;
        writer
            .finish()
            .map_err(|e| format!("IPC finish failed: {}", e))?;
    }
    Ok(buf)
}

/// Create the events table in LanceDB Cloud.
/// Sends an empty Arrow IPC stream with the schema to define the table.
pub async fn create_table(config: &Config) -> Result<String, String> {
    let url = base_url(config)?;
    let (client, api_key) = api_client(config)?;

    let schema = events_schema();

    // Build an empty RecordBatch with the schema
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(Vec::<&str>::new())),
            Arc::new(StringArray::from(Vec::<&str>::new())),
            Arc::new(StringArray::from(Vec::<&str>::new())),
            Arc::new(Int64Array::from(Vec::<i64>::new())),
            Arc::new(StringArray::from(Vec::<&str>::new())),
            Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                Vec::<Option<Vec<Option<f32>>>>::new(),
                EMBEDDING_DIM_I32,
            )),
        ],
    )
    .map_err(|e| format!("Failed to create empty batch: {}", e))?;

    let ipc_bytes = batch_to_ipc(&batch)?;

    let resp = client
        .post(format!("{}/v1/table/{}/create/", url, TABLE_NAME))
        .header("x-api-key", &api_key)
        .header("Content-Type", "application/vnd.apache.arrow.stream")
        .body(ipc_bytes)
        .send()
        .await
        .map_err(|e| format!("Create table request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Create table returned {}: {}", status, body));
    }

    let body = resp.text().await.unwrap_or_default();
    info!("Created LanceDB table '{}': {}", TABLE_NAME, body);
    Ok(body)
}

/// Insert events with pre-computed embeddings into LanceDB Cloud.
pub async fn insert_events(
    config: &Config,
    event_ids: Vec<String>,
    tenant_ids: Vec<String>,
    event_types: Vec<String>,
    timestamps: Vec<i64>,
    texts: Vec<String>,
    vectors: Vec<Vec<f32>>,
) -> Result<usize, String> {
    let count = event_ids.len();
    if count == 0 {
        return Ok(0);
    }

    let url = base_url(config)?;
    let (client, api_key) = api_client(config)?;

    let schema = events_schema();

    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vectors
            .into_iter()
            .map(|v| Some(v.into_iter().map(Some).collect::<Vec<_>>())),
        EMBEDDING_DIM_I32,
    );

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(event_ids)),
            Arc::new(StringArray::from(tenant_ids)),
            Arc::new(StringArray::from(event_types)),
            Arc::new(Int64Array::from(timestamps)),
            Arc::new(StringArray::from(texts)),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| format!("Failed to create batch: {}", e))?;

    let ipc_bytes = batch_to_ipc(&batch)?;

    let resp = client
        .post(format!("{}/v1/table/{}/insert/", url, TABLE_NAME))
        .header("x-api-key", &api_key)
        .header("Content-Type", "application/vnd.apache.arrow.stream")
        .body(ipc_bytes)
        .send()
        .await
        .map_err(|e| format!("Insert request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Insert returned {}: {}", status, body));
    }

    info!("Inserted {} events into LanceDB", count);
    Ok(count)
}

/// Create a vector index on the events table for fast ANN search.
pub async fn create_index(config: &Config) -> Result<String, String> {
    let url = base_url(config)?;
    let (client, api_key) = api_client(config)?;

    let body = serde_json::json!({
        "columns": ["vector"],
        "index_type": "IVF_PQ",
        "metric_type": "L2",
    });

    let resp = client
        .post(format!("{}/v1/table/{}/create_index/", url, TABLE_NAME))
        .header("x-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Create index request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Create index returned {}: {}", status, body));
    }

    let result = resp.text().await.unwrap_or_default();
    info!("Created vector index on '{}': {}", TABLE_NAME, result);
    Ok(result)
}

/// Build a text representation of an event for embedding.
pub fn event_to_text(
    event_type: &str,
    ip: &str,
    user_agent: &str,
    params: &str,
) -> String {
    format!(
        "event_type:{} ip:{} user_agent:{} params:{}",
        event_type, ip, user_agent, params
    )
}

/// Search request body for the LanceDB Cloud REST API.
#[derive(Debug, Serialize)]
struct VectorSearchRequest {
    /// The query vector.
    vector: Vec<f32>,
    /// Column name containing vectors.
    #[serde(skip_serializing_if = "Option::is_none")]
    vector_column: Option<String>,
    /// Max results.
    k: u32,
    /// SQL filter expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
    /// Columns to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
}

/// Search for similar events by vector using the LanceDB Cloud REST API.
/// Response is Arrow IPC format — we deserialize and convert to JSON.
pub async fn search_similar(
    config: &Config,
    query_vector: Vec<f32>,
    tenant_id: &str,
    limit: u32,
) -> Result<(Vec<serde_json::Value>, u64), String> {
    use arrow::ipc::reader::FileReader;
    use std::io::Cursor;

    let start = std::time::Instant::now();

    let url = base_url(config)?;
    let (client, api_key) = api_client(config)?;

    let filter = format!("tenant_id = '{}'", tenant_id.replace('\'', "''"));

    let search_req = VectorSearchRequest {
        vector: query_vector,
        vector_column: Some("vector".into()),
        k: limit,
        filter: Some(filter),
        columns: Some(vec![
            "event_id".into(),
            "tenant_id".into(),
            "event_type".into(),
            "timestamp_ms".into(),
            "text".into(),
        ]),
    };

    let resp = client
        .post(format!("{}/v1/table/{}/query/", url, TABLE_NAME))
        .header("x-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&search_req)
        .send()
        .await
        .map_err(|e| format!("LanceDB search request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("LanceDB search returned {}: {}", status, body));
    }

    // Response is Arrow IPC file format
    let ipc_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read LanceDB response bytes: {}", e))?;

    let cursor = Cursor::new(ipc_bytes.as_ref());
    let reader = FileReader::try_new(cursor, None)
        .map_err(|e| format!("Failed to parse Arrow IPC response: {}", e))?;

    let mut batches = Vec::new();
    for batch_result in reader {
        let batch = batch_result
            .map_err(|e| format!("Failed to read Arrow batch: {}", e))?;
        batches.push(batch);
    }

    let rows = crate::delta::batches_to_json(&batches)?;
    let query_ms = start.elapsed().as_millis() as u64;

    info!(
        "LanceDB search returned {} results in {}ms",
        rows.len(),
        query_ms
    );

    Ok((rows, query_ms))
}

/// Generate a deterministic placeholder embedding from text.
/// In production, replace with a call to an embedding API (e.g. OpenAI, Sentence-Transformers).
pub fn placeholder_embedding(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; EMBEDDING_DIM];
    let bytes = text.as_bytes();
    for (i, v) in vec.iter_mut().enumerate() {
        let b = bytes.get(i % bytes.len()).copied().unwrap_or(0);
        *v = ((b as f32 * 0.00784) + (i as f32 * 0.001)).sin();
    }
    // Normalize to unit vector
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
    vec
}
