//! tracker-core — high-performance event tracking server.
//!
//! Entry point that wires together configuration, the Iggy producer,
//! and the Axum HTTP router. Supports graceful shutdown via SIGTERM / Ctrl+C.

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use tracker_core::config::Config;
use tracker_core::ingest_token_cache::IngestTokenCache;
use tracker_core::producer::EventProducer;
use tracker_core::rate_limiter::RateLimiter;
use tracker_core::routes::{self, AppState};
use tracker_core::tenant_cache::TenantCache;
use tracker_core::tracking_url_cache::TrackingUrlCache;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env().expect("Invalid configuration");
    info!("Starting tracker-core on {}", config.listen_addr);

    let producer = EventProducer::new(
        &config.iggy_url,
        &config.iggy_stream,
        &config.iggy_topic,
        config.iggy_partitions,
    )
    .await
    .expect("Failed to initialize Iggy producer");

    // Initialize multi-tenant secret cache
    let tenant_cache = TenantCache::new(
        config.platform_api_url.clone(),
        config.platform_api_key.clone(),
    );

    // Initialize tracking URL cache
    let tracking_url_cache = TrackingUrlCache::new(
        config.platform_api_url.clone(),
        config.platform_api_key.clone(),
    );

    // Initialize ingest token validation cache
    let ingest_token_cache = IngestTokenCache::new(
        config.platform_api_url.clone(),
        config.platform_api_key.clone(),
    );

    // Initialize per-tenant rate limiter
    let rate_limiter = RateLimiter::new();

    // Load caches from Platform API (if configured)
    if config.platform_api_url.is_some() {
        match tenant_cache.load().await {
            Ok(n) => info!("Loaded {} tenant secrets from Platform API", n),
            Err(e) => warn!("Could not load tenant secrets: {} — multi-tenant disabled", e),
        }
        match tracking_url_cache.load().await {
            Ok(n) => info!("Loaded {} tracking URLs from Platform API", n),
            Err(e) => warn!("Could not load tracking URLs: {}", e),
        }
        // Seed rate limiter with initial tenant rates
        rate_limiter.update_rates(tenant_cache.rate_limits().await).await;
        info!("Rate limiter seeded with tenant limits");

        // Refresh every 60 seconds in the background
        tenant_cache.clone().start_refresh_task(std::time::Duration::from_secs(60));
        tracking_url_cache.clone().start_refresh_task(std::time::Duration::from_secs(60));
        // Prune expired token cache entries every 5 minutes
        ingest_token_cache.clone().start_cleanup_task(std::time::Duration::from_secs(300));
        // Prune inactive rate limiter buckets every 5 minutes
        rate_limiter.clone().start_cleanup_task(std::time::Duration::from_secs(300));
        // Sync rate limits from tenant cache every 60 seconds
        {
            let rl = rate_limiter.clone();
            let tc = tenant_cache.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                tick.tick().await; // skip first immediate tick
                loop {
                    tick.tick().await;
                    rl.update_rates(tc.rate_limits().await).await;
                }
            });
        }
    }

    let producer = Arc::new(producer);

    // Second producer for events-clean topic — /ingest events bypass the event filter
    let clean_producer = EventProducer::new(
        &config.iggy_url,
        &config.iggy_stream,
        &config.iggy_topic_clean,
        config.iggy_partitions,
    )
    .await
    .expect("Failed to initialize Iggy clean producer");
    let clean_producer = Arc::new(clean_producer);

    // Start NOOP → Iggy reconnection tasks (runs every 30s if in NOOP mode)
    producer.start_reconnect_task();
    clean_producer.start_reconnect_task();

    let state = AppState {
        config: config.clone(),
        producer,
        clean_producer,
        tenant_cache,
        tracking_url_cache,
        ingest_token_cache,
        rate_limiter,
    };

    let app = Router::new()
        .route("/", get(routes::handle_root))
        .route("/health", get(routes::handle_health))
        .route("/health/broker", get(routes::handle_broker_health))
        .route("/t", get(routes::handle_click))
        .route("/t/{tu_id}", get(routes::handle_tracked_click))
        .route("/p", get(routes::handle_postback))
        .route("/i", get(routes::handle_impression))
        .route("/batch", post(routes::handle_batch))
        .route("/t/auto", post(routes::handle_auto_beacon)
            .layer(DefaultBodyLimit::max(16_384)))
        .route("/ingest", post(routes::handle_ingest)
            .layer(DefaultBodyLimit::max(1_048_576)))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .expect("Failed to bind to address");

    info!("tracker-core listening on {}", config.listen_addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("Server error");

    info!("tracker-core shut down gracefully");
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
/// Used by Axum's graceful shutdown to drain in-flight connections.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, draining connections...");
}
