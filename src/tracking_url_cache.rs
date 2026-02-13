//! Tracking URL cache for short URL resolution.
//!
//! Loads tracking URL ID → destination mappings from the Platform API and
//! stores them in an in-memory `HashMap` for O(1) lookups on every request.
//!
//! The cache is loaded once at startup and refreshed periodically via a
//! background task. When a request hits `GET /t/:tu_id`, the cache resolves
//! the `tu_id` to its destination URL and tenant `key_prefix`.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// A cached tracking URL entry: destination + owning tenant's key_prefix.
#[derive(Debug, Clone)]
pub struct TrackingUrlEntry {
    /// The destination URL to redirect to.
    pub destination: String,
    /// The tenant's key_prefix (used to tag events for tenant isolation).
    pub key_prefix: String,
}

/// Thread-safe, read-optimized cache of tracking URLs keyed by `tu_id`.
///
/// Uses `RwLock` so reads (every HTTP request) are concurrent, and writes
/// (periodic refresh) briefly block readers.
#[derive(Clone)]
pub struct TrackingUrlCache {
    inner: Arc<RwLock<HashMap<String, TrackingUrlEntry>>>,
    platform_api_url: Option<String>,
    platform_api_key: Option<String>,
}

impl TrackingUrlCache {
    /// Create a new empty cache.
    pub fn new(platform_api_url: Option<String>, platform_api_key: Option<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            platform_api_url,
            platform_api_key,
        }
    }

    /// Look up a tracking URL by its ID.
    ///
    /// Returns the destination URL and tenant key_prefix, or `None` if not found.
    pub async fn get(&self, tu_id: &str) -> Option<TrackingUrlEntry> {
        let cache = self.inner.read().await;
        cache.get(tu_id).cloned()
    }

    /// Load (or refresh) all tracking URLs from the Platform API.
    ///
    /// Calls `GET /internal/tracking-urls` and replaces the entire cache.
    pub async fn load(&self) -> Result<usize, String> {
        let url = self
            .platform_api_url
            .as_deref()
            .ok_or("PLATFORM_API_URL not configured")?;
        let key = self
            .platform_api_key
            .as_deref()
            .ok_or("PLATFORM_API_KEY not configured")?;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/internal/tracking-urls", url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch tracking URLs: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Platform API returned {}", resp.status()));
        }

        let body: TrackingUrlsResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse tracking URLs response: {e}"))?;

        let count = body.urls.len();
        let mut cache = self.inner.write().await;
        cache.clear();
        for (tu_id, entry) in body.urls {
            cache.insert(
                tu_id,
                TrackingUrlEntry {
                    destination: entry.destination,
                    key_prefix: entry.key_prefix,
                },
            );
        }

        info!("Loaded {} tracking URLs from Platform API", count);
        Ok(count)
    }

    /// Returns the number of tracking URLs currently in the cache.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Start a background task that refreshes the cache every `interval`.
    pub fn start_refresh_task(self, interval: std::time::Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // skip first immediate tick
            loop {
                tick.tick().await;
                match self.load().await {
                    Ok(n) => info!("Refreshed tracking URL cache: {} URLs", n),
                    Err(e) => warn!("Failed to refresh tracking URL cache: {}", e),
                }
            }
        });
    }
}

/// JSON response from `GET /internal/tracking-urls`.
#[derive(serde::Deserialize)]
struct TrackingUrlsResponse {
    urls: HashMap<String, TrackingUrlEntryJson>,
}

#[derive(serde::Deserialize)]
struct TrackingUrlEntryJson {
    destination: String,
    key_prefix: String,
}
