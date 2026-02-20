//! tracker-core library crate.
//!
//! Exposes the core modules used by the binary:
//! - [`config`] — Environment-based configuration.
//! - [`crypto`] — HMAC signing and AES-GCM encryption for URL security.
//! - [`event`] — [`TrackingEvent`](event::TrackingEvent) struct and serialization.
//! - [`producer`] — High-throughput Iggy producer with background send mode.
//! - [`routes`] — Axum HTTP endpoint handlers.

pub mod config;
pub mod crypto;
pub mod event;
pub mod health;
pub mod ingest_token_cache;
pub mod producer;
pub mod rate_limiter;
pub mod routes;
pub mod tenant_cache;
pub mod tracking_url_cache;
