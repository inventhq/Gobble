//! Application configuration loaded from environment variables.
//!
//! All settings are read once at startup via [`Config::from_env()`].
//! Sensible defaults are provided for most values; only the secret
//! matching the chosen [`UrlMode`] is required.

use std::env;
use std::net::SocketAddr;

/// Determines how redirect URLs on the `/t` endpoint are secured.
#[derive(Debug, Clone)]
pub enum UrlMode {
    /// URL is visible in the query string, protected by an HMAC-SHA256 signature.
    /// Requires `HMAC_SECRET` to be set.
    Signed,
    /// URL is encrypted with AES-256-GCM and passed as an opaque base64url blob.
    /// Requires `ENCRYPTION_KEY` (exactly 32 bytes, hex-encoded) to be set.
    Encrypted,
}

/// Runtime configuration for the tracker-core server.
///
/// Loaded from environment variables (with `.env` file support via `dotenvy`).
/// Validated at construction time — if a required secret is missing or malformed,
/// [`from_env()`](Self::from_env) returns an error and the server refuses to start.
#[derive(Debug, Clone)]
pub struct Config {
    /// How redirect URLs on `/t` are validated (signed or encrypted).
    pub url_mode: UrlMode,
    /// HMAC-SHA256 secret for signed mode. `None` when using encrypted mode.
    pub hmac_secret: Option<String>,
    /// AES-256-GCM key (32 bytes) for encrypted mode. `None` when using signed mode.
    pub encryption_key: Option<Vec<u8>>,
    /// Iggy TCP server address (e.g. `"127.0.0.1:8090"`).
    pub iggy_url: String,
    /// Iggy stream name to publish events into.
    pub iggy_stream: String,
    /// Iggy topic name within the stream (raw events, pre-filter).
    pub iggy_topic: String,
    /// Iggy topic for clean/pre-authenticated events (bypasses event filter).
    pub iggy_topic_clean: String,
    /// Number of partitions for the Iggy topics (for horizontal consumer scaling).
    pub iggy_partitions: u32,
    /// Socket address the HTTP server binds to.
    pub listen_addr: SocketAddr,
    /// Maximum number of events allowed in a single `POST /batch` request.
    pub max_batch_size: usize,
    /// Platform API URL for loading tenant secrets (optional, enables multi-tenant mode).
    pub platform_api_url: Option<String>,
    /// Platform API admin key for authenticating secret fetches.
    pub platform_api_key: Option<String>,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Returns `Err` with a human-readable message if validation fails
    /// (e.g. missing secret, invalid key length, unparseable address).
    pub fn from_env() -> Result<Self, String> {
        let url_mode = match env::var("URL_MODE").unwrap_or_else(|_| "signed".into()).as_str() {
            "signed" => UrlMode::Signed,
            "encrypted" => UrlMode::Encrypted,
            other => return Err(format!("Invalid URL_MODE: {other}. Use 'signed' or 'encrypted'")),
        };

        let hmac_secret = env::var("HMAC_SECRET").ok();
        let encryption_key = env::var("ENCRYPTION_KEY").ok().map(|k| {
            hex::decode(&k).unwrap_or_else(|_| k.as_bytes().to_vec())
        });

        // Validate that the right secret is present for the chosen mode
        match &url_mode {
            UrlMode::Signed => {
                if hmac_secret.is_none() {
                    return Err("HMAC_SECRET is required when URL_MODE=signed".into());
                }
            }
            UrlMode::Encrypted => {
                match &encryption_key {
                    Some(key) if key.len() == 32 => {}
                    Some(key) => {
                        return Err(format!(
                            "ENCRYPTION_KEY must be exactly 32 bytes (got {})",
                            key.len()
                        ));
                    }
                    None => return Err("ENCRYPTION_KEY is required when URL_MODE=encrypted".into()),
                }
            }
        }

        let iggy_url = env::var("IGGY_URL").unwrap_or_else(|_| "127.0.0.1:8090".into());
        let iggy_stream = env::var("IGGY_STREAM").unwrap_or_else(|_| "tracker".into());
        let iggy_topic = env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".into());
        let iggy_topic_clean = env::var("IGGY_TOPIC_CLEAN").unwrap_or_else(|_| "events-clean".into());
        let iggy_partitions: u32 = env::var("IGGY_PARTITIONS")
            .unwrap_or_else(|_| "24".into())
            .parse()
            .map_err(|e| format!("Invalid IGGY_PARTITIONS — {e}"))?;

        let max_batch_size: usize = env::var("MAX_BATCH_SIZE")
            .unwrap_or_else(|_| "10000".into())
            .parse()
            .map_err(|e| format!("Invalid MAX_BATCH_SIZE — {e}"))?;

        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let port = env::var("PORT").unwrap_or_else(|_| "3000".into());
        let listen_addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e| format!("Invalid HOST:PORT — {e}"))?;

        let platform_api_url = env::var("PLATFORM_API_URL").ok();
        let platform_api_key = env::var("PLATFORM_API_KEY").ok();

        Ok(Config {
            url_mode,
            hmac_secret,
            encryption_key,
            iggy_url,
            iggy_stream,
            iggy_topic,
            iggy_topic_clean,
            iggy_partitions,
            listen_addr,
            max_batch_size,
            platform_api_url,
            platform_api_key,
        })
    }
}
