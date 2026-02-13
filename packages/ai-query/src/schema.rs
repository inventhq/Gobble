//! Schema context builder for SLM prompts.
//!
//! Fetches plugin table schemas from the plugin-runtime's schema discovery
//! endpoint and combines them with the Delta Lake events table schema to
//! build a complete schema context for the SLM.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::Config;
use crate::delta::EVENTS_SCHEMA;

/// A column in a plugin table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub col_type: String,
    #[serde(default)]
    pub primary_key: bool,
}

/// A table owned by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub full_name: String,
    pub columns: Vec<ColumnInfo>,
    pub row_count: Option<u64>,
}

/// A plugin's schema (tables it owns).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSchema {
    pub plugin_id: String,
    pub plugin_name: String,
    pub tables: Vec<TableInfo>,
}

/// Full schema discovery response from plugin-runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDiscoveryResponse {
    pub key_prefix: String,
    pub plugins: Vec<PluginSchema>,
}

/// Fetch plugin schemas from the plugin-runtime.
pub async fn fetch_plugin_schemas(
    config: &Config,
    key_prefix: &str,
) -> Result<Vec<PluginSchema>, String> {
    let runtime_url = match &config.plugin_runtime_url {
        Some(url) => url,
        None => {
            info!("PLUGIN_RUNTIME_URL not set, skipping plugin schema discovery");
            return Ok(vec![]);
        }
    };

    let client = Client::new();
    let url = format!("{}/schemas/{}?counts=true", runtime_url, key_prefix);

    let mut req = client.get(&url);
    if let Some(ref api_key) = config.admin_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let resp = req.send().await.map_err(|e| {
        warn!("Failed to fetch plugin schemas: {}", e);
        format!("Plugin schema discovery failed: {}", e)
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!("Plugin schema discovery returned {}: {}", status, body);
        return Ok(vec![]);
    }

    let discovery: SchemaDiscoveryResponse = resp.json().await.map_err(|e| {
        format!("Failed to parse plugin schema response: {}", e)
    })?;

    info!(
        "Discovered {} plugin(s) with {} total table(s) for key_prefix={}",
        discovery.plugins.len(),
        discovery.plugins.iter().map(|p| p.tables.len()).sum::<usize>(),
        key_prefix
    );

    Ok(discovery.plugins)
}

/// Build the full schema context string for the SLM prompt.
///
/// Includes the Delta Lake events table schema and all plugin table schemas.
pub async fn build_schema_context(
    config: &Config,
    key_prefix: &str,
) -> String {
    let mut context = String::new();

    // Delta Lake events table (always available)
    context.push_str("=== DATA SOURCES ===\n\n");
    context.push_str("--- Source 1: Delta Lake (raw event history) ---\n");
    context.push_str(EVENTS_SCHEMA);
    context.push('\n');

    // Plugin tables (from plugin-runtime)
    match fetch_plugin_schemas(config, key_prefix).await {
        Ok(plugins) if !plugins.is_empty() => {
            context.push_str("--- Source 2: Plugin Tables (Turso, structured business data) ---\n");
            context.push_str("These tables are queried via the plugin-runtime Turso proxy.\n");
            context.push_str("Use the full_name when constructing SQL for plugin tables.\n\n");

            for plugin in &plugins {
                context.push_str(&format!(
                    "Plugin: {} ({})\n",
                    plugin.plugin_name, plugin.plugin_id
                ));
                for table in &plugin.tables {
                    context.push_str(&format!("  Table: {} (full: {})", table.name, table.full_name));
                    if let Some(count) = table.row_count {
                        context.push_str(&format!(" — {} rows", count));
                    }
                    context.push('\n');
                    context.push_str("  Columns:\n");
                    for col in &table.columns {
                        let pk = if col.primary_key { " [PK]" } else { "" };
                        context.push_str(&format!(
                            "    - {}: {}{}\n",
                            col.name, col.col_type, pk
                        ));
                    }
                }
                context.push('\n');
            }
        }
        Ok(_) => {
            context.push_str("--- No plugin tables available for this tenant ---\n\n");
        }
        Err(e) => {
            context.push_str(&format!(
                "--- Plugin schema discovery failed: {} ---\n\n",
                e
            ));
        }
    }

    context
}
