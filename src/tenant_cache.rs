//! Multi-tenant secret cache for signature verification.
//!
//! Loads tenant key_prefix → secret mappings from the Platform API and
//! stores them in an in-memory `HashMap` for O(1) lookups on every request.
//!
//! The cache is loaded once at startup and can be refreshed periodically
//! via a background task. When a signature contains a prefix (e.g.
//! `sig=tk8a_c740665...`), the cache resolves the correct HMAC secret
//! for that tenant.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Per-tenant secrets loaded from the Platform API.
#[derive(Debug, Clone)]
pub struct TenantSecrets {
    /// HMAC-SHA256 secret for signed URL mode.
    pub hmac_secret: String,
    /// AES-256-GCM encryption key (hex-encoded) for encrypted URL mode.
    pub encryption_key: String,
    /// Requests per second rate limit for this tenant.
    pub rate_limit_rps: u32,
}

/// Thread-safe, read-optimized cache of tenant secrets keyed by prefix.
///
/// Uses `RwLock` so reads (every HTTP request) are concurrent, and writes
/// (periodic refresh) briefly block readers.
#[derive(Clone)]
pub struct TenantCache {
    inner: Arc<RwLock<HashMap<String, TenantSecrets>>>,
    platform_api_url: Option<String>,
    platform_api_key: Option<String>,
}

impl TenantCache {
    /// Create a new empty cache.
    ///
    /// If `platform_api_url` and `platform_api_key` are provided, the cache
    /// can be populated via [`load()`](Self::load).
    pub fn new(platform_api_url: Option<String>, platform_api_key: Option<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            platform_api_url,
            platform_api_key,
        }
    }

    /// Look up a tenant's HMAC secret by key prefix.
    ///
    /// Returns `None` if the prefix is not in the cache.
    pub async fn get_hmac_secret(&self, prefix: &str) -> Option<String> {
        let cache = self.inner.read().await;
        cache.get(prefix).map(|s| s.hmac_secret.clone())
    }

    /// Look up a tenant's encryption key by key prefix.
    ///
    /// Returns `None` if the prefix is not in the cache.
    pub async fn get_encryption_key(&self, prefix: &str) -> Option<Vec<u8>> {
        let cache = self.inner.read().await;
        cache.get(prefix).and_then(|s| hex::decode(&s.encryption_key).ok())
    }

    /// Load (or refresh) all tenant secrets from the Platform API.
    ///
    /// Calls `GET /internal/secrets` on the Platform API and replaces the
    /// entire cache contents. Returns the number of tenants loaded.
    pub async fn load(&self) -> Result<usize, String> {
        let url = self.platform_api_url.as_deref().ok_or("PLATFORM_API_URL not configured")?;
        let key = self.platform_api_key.as_deref().ok_or("PLATFORM_API_KEY not configured")?;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/internal/secrets", url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch secrets: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Platform API returned {}", resp.status()));
        }

        let body: SecretsResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse secrets response: {e}"))?;

        let count = body.secrets.len();
        let mut cache = self.inner.write().await;
        cache.clear();
        for (prefix, secret) in body.secrets {
            cache.insert(prefix, TenantSecrets {
                hmac_secret: secret.hmac_secret,
                encryption_key: secret.encryption_key,
                rate_limit_rps: secret.rate_limit_rps.unwrap_or(100),
            });
        }

        info!("Loaded {} tenant secrets from Platform API", count);
        Ok(count)
    }

    /// Returns the number of tenants currently in the cache.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Returns a snapshot of all tenant rate limits (key_prefix → rps).
    pub async fn rate_limits(&self) -> HashMap<String, u32> {
        let cache = self.inner.read().await;
        cache.iter().map(|(k, v)| (k.clone(), v.rate_limit_rps)).collect()
    }

    /// Start a background task that refreshes the cache every `interval`.
    pub fn start_refresh_task(self, interval: std::time::Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // skip first immediate tick
            loop {
                tick.tick().await;
                match self.load().await {
                    Ok(n) => info!("Refreshed tenant cache: {} tenants", n),
                    Err(e) => warn!("Failed to refresh tenant cache: {}", e),
                }
            }
        });
    }
}

/// Parse a prefixed signature into (prefix, raw_signature).
///
/// If the signature contains an underscore, the part before the first
/// underscore is the tenant key prefix and the rest is the HMAC hex.
/// If there's no underscore, returns `None` (use global secret).
///
/// # Examples
/// ```
/// assert_eq!(parse_prefixed_sig("tk8a_c740665..."), Some(("tk8a", "c740665...")));
/// assert_eq!(parse_prefixed_sig("c740665..."), None);
/// ```
pub fn parse_prefixed_sig(sig: &str) -> Option<(&str, &str)> {
    // Prefix is always short (4 chars), so check for underscore in first 10 chars
    if let Some(pos) = sig[..sig.len().min(10)].find('_') {
        let prefix = &sig[..pos];
        let hmac = &sig[pos + 1..];
        if !prefix.is_empty() && !hmac.is_empty() {
            return Some((prefix, hmac));
        }
    }
    None
}

/// JSON response from `GET /internal/secrets`.
#[derive(serde::Deserialize)]
struct SecretsResponse {
    secrets: HashMap<String, SecretEntry>,
}

#[derive(serde::Deserialize)]
struct SecretEntry {
    hmac_secret: String,
    encryption_key: String,
    rate_limit_rps: Option<u32>,
}
