//! Lightweight health server for consumer binaries.
//!
//! Provides a tiny Axum HTTP server that exposes `/health` with JSON counters.
//! Each consumer spawns this alongside its main loop to enable the god-mode
//! observability dashboard to aggregate health across all services.
//!
//! Usage:
//! ```ignore
//! let counters = HealthCounters::new("webhook-consumer");
//! counters.add("events_processed", 0);
//! counters.add("webhooks_dispatched", 0);
//! counters.add("errors", 0);
//! spawn_health_server(counters.clone(), 3040);
//! // ... in consumer loop:
//! counters.inc("events_processed");
//! ```

use axum::{extract::State, routing::get, Json, Router};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{error, info};

/// Thread-safe health counters shared between the consumer loop and HTTP server.
#[derive(Clone)]
pub struct HealthCounters {
    service_name: String,
    counters: Arc<HashMap<String, AtomicU64>>,
    start_time: std::time::Instant,
}

impl HealthCounters {
    /// Create a new set of health counters for a named service.
    /// Call `add()` to register counter names before spawning the health server.
    pub fn new(service_name: &str, counter_names: &[&str]) -> Self {
        let mut map = HashMap::new();
        for name in counter_names {
            map.insert(name.to_string(), AtomicU64::new(0));
        }
        Self {
            service_name: service_name.to_string(),
            counters: Arc::new(map),
            start_time: std::time::Instant::now(),
        }
    }

    /// Increment a counter by 1.
    pub fn inc(&self, name: &str) {
        if let Some(c) = self.counters.get(name) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Set a counter to a specific value.
    pub fn set(&self, name: &str, val: u64) {
        if let Some(c) = self.counters.get(name) {
            c.store(val, Ordering::Relaxed);
        }
    }

    /// Get a counter value.
    pub fn get(&self, name: &str) -> u64 {
        self.counters
            .get(name)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Build the JSON response for /health.
    fn to_json(&self) -> serde_json::Value {
        let mut counters = serde_json::Map::new();
        for (name, val) in self.counters.as_ref() {
            counters.insert(name.clone(), json!(val.load(Ordering::Relaxed)));
        }
        json!({
            "status": "ok",
            "service": self.service_name,
            "uptime_secs": self.start_time.elapsed().as_secs(),
            "counters": counters,
        })
    }
}

/// Health endpoint handler.
async fn health_handler(State(counters): State<HealthCounters>) -> Json<serde_json::Value> {
    Json(counters.to_json())
}

/// Spawn a lightweight health HTTP server on the given port.
/// Runs in background — does not block.
pub fn spawn_health_server(counters: HealthCounters, port: u16) {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/health", get(health_handler))
            .with_state(counters.clone());

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        info!("[{}] Health server listening on {}", counters.service_name, addr);

        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind health server on {}: {}", addr, e);
                return;
            }
        };

        if let Err(e) = axum::serve(listener, app).await {
            error!("Health server error: {}", e);
        }
    });
}
