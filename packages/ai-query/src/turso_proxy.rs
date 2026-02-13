//! Proxy for executing SQL against plugin-runtime's Turso database.
//!
//! Uses the plugin-runtime's `POST /schemas/:key_prefix/query` endpoint
//! to execute cross-plugin SQL queries against structured business data.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::Config;

/// Request body for the plugin-runtime query proxy.
#[derive(Debug, Serialize)]
struct TursoQueryRequest {
    sql: String,
    params: Vec<serde_json::Value>,
}

/// Response from the plugin-runtime query proxy.
#[derive(Debug, Deserialize)]
struct TursoQueryResponse {
    columns: Option<Vec<String>>,
    rows: Option<Vec<Vec<serde_json::Value>>>,
    error: Option<String>,
}

/// Execute a SQL query against the plugin-runtime's Turso proxy.
///
/// Returns JSON rows (array of objects).
pub async fn execute_turso_sql(
    config: &Config,
    key_prefix: &str,
    sql: &str,
) -> Result<(Vec<serde_json::Value>, u64), String> {
    let start = std::time::Instant::now();

    let runtime_url = config
        .plugin_runtime_url
        .as_ref()
        .ok_or_else(|| "PLUGIN_RUNTIME_URL not configured — cannot query plugin tables".to_string())?;

    let client = Client::new();
    let url = format!("{}/schemas/{}/query", runtime_url, key_prefix);

    let mut req = client.post(&url).json(&TursoQueryRequest {
        sql: sql.to_string(),
        params: vec![],
    });

    if let Some(ref api_key) = config.admin_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    info!("Turso proxy query for {}: {}", key_prefix, sql);

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Turso proxy request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Turso proxy returned {}: {}", status, body));
    }

    let turso_resp: TursoQueryResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Turso proxy response: {}", e))?;

    if let Some(error) = turso_resp.error {
        return Err(format!("Turso SQL error: {}", error));
    }

    let columns = turso_resp.columns.unwrap_or_default();
    let raw_rows = turso_resp.rows.unwrap_or_default();

    // Convert column+row format to JSON objects
    let rows: Vec<serde_json::Value> = raw_rows
        .into_iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, val) in row.into_iter().enumerate() {
                let col_name = columns
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("col_{}", i));
                obj.insert(col_name, val);
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    let query_ms = start.elapsed().as_millis() as u64;

    info!(
        "Turso proxy returned {} rows in {}ms",
        rows.len(),
        query_ms
    );

    Ok((rows, query_ms))
}
