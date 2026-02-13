//! Ingest token validation cache for `/ingest` endpoint authentication.
//!
//! Validates `Bearer pt_{key_prefix}_{secret}` tokens by SHA-256 hashing
//! the token and checking against the Platform API. Valid tokens are cached
//! in-memory with a 5-minute TTL to avoid hitting the Platform API on every
//! `/ingest` request.
//!
//! The cache returns the `key_prefix` for valid tokens — tracker-core injects
//! this into the event's params, preventing tenant spoofing (callers cannot
//! choose their own key_prefix).
//!
//! Invalid tokens are also cached (negative cache) to prevent repeated
//! lookups for the same bad token.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// How long a validated token stays in the cache before re-validation.
const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

/// How long a rejected token stays in the negative cache.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60); // 1 minute

/// Cached result of a token validation.
#[derive(Clone, Debug)]
enum CacheEntry {
    /// Token is valid — contains the tenant's key_prefix.
    Valid { key_prefix: String, expires_at: Instant },
    /// Token is invalid — cached to avoid repeated lookups.
    Invalid { expires_at: Instant },
}

/// Thread-safe ingest token validation cache.
///
/// On cache miss, calls `POST /internal/validate-ingest-token` on the
/// Platform API to validate the token hash. Results are cached with TTL.
#[derive(Clone)]
pub struct IngestTokenCache {
    /// SHA-256 hash → cache entry
    inner: Arc<RwLock<HashMap<String, CacheEntry>>>,
    platform_api_url: Option<String>,
    platform_api_key: Option<String>,
    http: reqwest::Client,
}

impl IngestTokenCache {
    /// Create a new empty cache.
    pub fn new(platform_api_url: Option<String>, platform_api_key: Option<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            platform_api_url,
            platform_api_key,
            http: reqwest::Client::new(),
        }
    }

    /// Validate an ingest token. Returns `Some(key_prefix)` if valid, `None` if invalid.
    ///
    /// First checks the in-memory cache. On cache miss, calls the Platform API
    /// to validate the token hash and caches the result.
    pub async fn validate(&self, token: &str) -> Option<String> {
        // Quick format check: must start with "pt_"
        if !token.starts_with("pt_") {
            return None;
        }

        // SHA-256 hash the token
        let token_hash = sha256_hex(token);

        // Check cache first
        {
            let cache = self.inner.read().await;
            if let Some(entry) = cache.get(&token_hash) {
                match entry {
                    CacheEntry::Valid { key_prefix, expires_at } => {
                        if Instant::now() < *expires_at {
                            return Some(key_prefix.clone());
                        }
                        // Expired — fall through to re-validate
                    }
                    CacheEntry::Invalid { expires_at } => {
                        if Instant::now() < *expires_at {
                            return None;
                        }
                        // Expired — fall through to re-validate
                    }
                }
            }
        }

        // Cache miss or expired — validate via Platform API
        match self.validate_remote(&token_hash).await {
            Ok(Some(key_prefix)) => {
                let mut cache = self.inner.write().await;
                cache.insert(token_hash, CacheEntry::Valid {
                    key_prefix: key_prefix.clone(),
                    expires_at: Instant::now() + CACHE_TTL,
                });
                Some(key_prefix)
            }
            Ok(None) => {
                let mut cache = self.inner.write().await;
                cache.insert(token_hash, CacheEntry::Invalid {
                    expires_at: Instant::now() + NEGATIVE_CACHE_TTL,
                });
                None
            }
            Err(e) => {
                warn!("Failed to validate ingest token via Platform API: {}", e);
                // On Platform API failure, reject the token (fail-closed)
                None
            }
        }
    }

    /// Call Platform API to validate a token hash.
    /// Returns `Ok(Some(key_prefix))` if valid, `Ok(None)` if invalid.
    async fn validate_remote(&self, token_hash: &str) -> Result<Option<String>, String> {
        let url = self.platform_api_url.as_deref()
            .ok_or("PLATFORM_API_URL not configured")?;
        let key = self.platform_api_key.as_deref()
            .ok_or("PLATFORM_API_KEY not configured")?;

        let resp = self.http
            .post(format!("{}/internal/validate-ingest-token", url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", key))
            .json(&serde_json::json!({ "token_hash": token_hash }))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Platform API returned {}", resp.status()));
        }

        let body: ValidationResponse = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {e}"))?;

        if body.valid {
            Ok(body.key_prefix)
        } else {
            Ok(None)
        }
    }

    /// Start a background task that periodically prunes expired cache entries.
    pub fn start_cleanup_task(self, interval: Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // skip first immediate tick
            loop {
                tick.tick().await;
                let now = Instant::now();
                let mut cache = self.inner.write().await;
                let before = cache.len();
                cache.retain(|_, entry| match entry {
                    CacheEntry::Valid { expires_at, .. } => now < *expires_at,
                    CacheEntry::Invalid { expires_at } => now < *expires_at,
                });
                let pruned = before - cache.len();
                if pruned > 0 {
                    info!("Pruned {} expired ingest token cache entries", pruned);
                }
            }
        });
    }
}

/// JSON response from `POST /internal/validate-ingest-token`.
#[derive(serde::Deserialize)]
struct ValidationResponse {
    valid: bool,
    key_prefix: Option<String>,
}

/// Compute SHA-256 hex digest of a string.
fn sha256_hex(input: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}
