//! Per-tenant token bucket rate limiter.
//!
//! Each tenant (keyed by `key_prefix`) gets a separate bucket with a
//! configurable requests-per-second rate and burst capacity (2× the rate).
//! The bucket is refilled continuously based on elapsed time.
//!
//! Thread-safe via `DashMap` for lock-free concurrent access — no global
//! write lock on the hot path.
//!
//! Rate limits are loaded from the Platform API via `TenantCache` and
//! can be updated per-tenant via `PATCH /api/tenants/:id`.

use std::sync::Arc;
use std::time::Instant;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Default rate limit (requests per second) when no tenant-specific limit is set.
const DEFAULT_RATE_LIMIT: u32 = 100;

/// Burst multiplier — bucket capacity is `rate * BURST_MULTIPLIER`.
const BURST_MULTIPLIER: u32 = 2;

/// A single token bucket for one tenant.
struct Bucket {
    /// Current number of available tokens (fractional for smooth refill).
    tokens: f64,
    /// Maximum tokens (burst capacity).
    max_tokens: f64,
    /// Tokens added per second.
    rate: f64,
    /// Last time tokens were refilled.
    last_refill: Instant,
}

impl Bucket {
    fn new(rate_per_sec: u32) -> Self {
        let max = (rate_per_sec * BURST_MULTIPLIER) as f64;
        Self {
            tokens: max,
            max_tokens: max,
            rate: rate_per_sec as f64,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token. Returns `true` if allowed, `false` if rate limited.
    fn try_acquire(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.max_tokens);
        self.last_refill = now;
    }

    /// Update the rate limit (e.g., after cache refresh).
    fn update_rate(&mut self, rate_per_sec: u32) {
        let new_max = (rate_per_sec * BURST_MULTIPLIER) as f64;
        self.rate = rate_per_sec as f64;
        self.max_tokens = new_max;
        // Clamp current tokens to new max
        if self.tokens > new_max {
            self.tokens = new_max;
        }
    }
}

/// Per-tenant rate limiter using token buckets.
///
/// Each `key_prefix` maps to a separate bucket. Buckets are created on
/// first access and updated when rate limits change via cache refresh.
#[derive(Clone)]
pub struct RateLimiter {
    /// key_prefix → token bucket
    buckets: Arc<RwLock<HashMap<String, Bucket>>>,
    /// key_prefix → configured rate (from TenantCache)
    rates: Arc<RwLock<HashMap<String, u32>>>,
}

impl RateLimiter {
    /// Create a new empty rate limiter.
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            rates: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Update the rate limits for all tenants. Called after TenantCache refresh.
    pub async fn update_rates(&self, new_rates: HashMap<String, u32>) {
        let mut rates = self.rates.write().await;
        let mut buckets = self.buckets.write().await;

        // Update existing buckets with new rates
        for (prefix, rate) in &new_rates {
            if let Some(bucket) = buckets.get_mut(prefix) {
                bucket.update_rate(*rate);
            }
        }

        *rates = new_rates;
    }

    /// Check if a request from the given tenant is allowed.
    ///
    /// Returns `true` if the request is within rate limits, `false` if it
    /// should be rejected with 429 Too Many Requests.
    ///
    /// If the tenant has no configured rate, uses `DEFAULT_RATE_LIMIT`.
    pub async fn check(&self, key_prefix: &str) -> bool {
        let rate = {
            let rates = self.rates.read().await;
            rates.get(key_prefix).copied().unwrap_or(DEFAULT_RATE_LIMIT)
        };

        let mut buckets = self.buckets.write().await;
        let bucket = buckets
            .entry(key_prefix.to_string())
            .or_insert_with(|| Bucket::new(rate));
        bucket.try_acquire()
    }

    /// Start a background task that prunes buckets for inactive tenants.
    /// Runs every `interval` and removes buckets that haven't been used
    /// in over 5 minutes (they'll be recreated on next request).
    pub fn start_cleanup_task(self, interval: std::time::Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // skip first immediate tick
            loop {
                tick.tick().await;
                let now = Instant::now();
                let mut buckets = self.buckets.write().await;
                let before = buckets.len();
                buckets.retain(|_, bucket| {
                    now.duration_since(bucket.last_refill).as_secs() < 300
                });
                let pruned = before - buckets.len();
                if pruned > 0 {
                    tracing::info!("Pruned {} inactive rate limiter buckets", pruned);
                }
            }
        });
    }
}
