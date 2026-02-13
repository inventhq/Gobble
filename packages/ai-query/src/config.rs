//! Configuration loaded from environment variables.

use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    /// Listen host (default: 0.0.0.0)
    pub host: String,
    /// Listen port (default: 3060)
    pub port: String,
    /// Baseten model endpoint URL for SLM inference.
    /// e.g. https://model-{id}.api.baseten.co/production/predict
    pub slm_url: Option<String>,
    /// Baseten API key for authentication.
    pub slm_api_key: Option<String>,
    /// Plugin-runtime URL for schema discovery and cross-plugin queries.
    /// e.g. http://localhost:3050
    pub plugin_runtime_url: Option<String>,
    /// Admin API key for authenticating requests to plugin-runtime.
    pub admin_api_key: Option<String>,
    /// LanceDB Cloud URI (e.g. db://ai-query-8kc6p2).
    pub lancedb_uri: Option<String>,
    /// LanceDB Cloud API key.
    pub lancedb_api_key: Option<String>,
    /// R2 endpoint for Delta Lake access.
    pub r2_endpoint: String,
    /// R2 access key ID.
    pub r2_access_key_id: String,
    /// R2 secret access key.
    pub r2_secret_access_key: String,
    /// R2 bucket name (default: tracker-events).
    pub r2_bucket: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("AI_QUERY_PORT").unwrap_or_else(|_| "3060".into()),
            slm_url: env::var("SLM_URL").ok(),
            slm_api_key: env::var("SLM_API_KEY").ok(),
            plugin_runtime_url: env::var("PLUGIN_RUNTIME_URL").ok(),
            admin_api_key: env::var("ADMIN_API_KEY").ok(),
            lancedb_uri: env::var("LANCEDB_URI").ok(),
            lancedb_api_key: env::var("LANCEDB_API_KEY").ok(),
            r2_endpoint: env::var("R2_ENDPOINT").expect("R2_ENDPOINT is required"),
            r2_access_key_id: env::var("R2_ACCESS_KEY_ID").expect("R2_ACCESS_KEY_ID is required"),
            r2_secret_access_key: env::var("R2_SECRET_ACCESS_KEY")
                .expect("R2_SECRET_ACCESS_KEY is required"),
            r2_bucket: env::var("R2_BUCKET").unwrap_or_else(|_| "tracker-events".into()),
        }
    }
}
