//! Tracking event definition and serialization.
//!
//! Every HTTP request to `/t`, `/p`, or `/i` produces a single [`TrackingEvent`]
//! that captures the full request context. The event is serialized to JSON and
//! published to the Iggy stream as an opaque payload.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// A single tracking event captured from an inbound HTTP request.
///
/// The struct has two logical sections:
/// - **Envelope fields** (fixed): `event_id`, `event_type`, `timestamp`, IP,
///   headers, path, and host — always present, populated by the core.
/// - **Params** (opaque): arbitrary key-value pairs forwarded from the query
///   string. The core never reads or validates these; downstream consumers
///   interpret them according to the business domain.
///
/// Serialized as JSON via `serde_json` for the Iggy message payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingEvent {
    /// Unique event identifier (UUIDv7 — time-sortable).
    pub event_id: String,
    /// Event type: `"click"`, `"postback"`, or `"impression"`.
    pub event_type: String,
    /// Unix timestamp in milliseconds when the event was captured.
    pub timestamp: u64,
    /// Client IP address (from X-Forwarded-For, X-Real-IP, or socket peer).
    pub ip: String,
    /// Client User-Agent header value.
    pub user_agent: String,
    /// HTTP Referer header, if present.
    pub referer: Option<String>,
    /// Accept-Language header, if present.
    pub accept_language: Option<String>,
    /// The endpoint path that handled this request (`/t`, `/p`, or `/i`).
    pub request_path: String,
    /// The Host header value from the inbound request.
    pub request_host: String,
    /// All remaining query parameters, passed through opaquely.
    pub params: HashMap<String, String>,
    /// Optional full nested JSON payload for external/imported events.
    /// Tracking routes set this to None. The `/ingest` endpoint and future
    /// plugin adapters populate it with the original source payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_payload: Option<serde_json::Value>,
}

impl TrackingEvent {
    /// Construct a new tracking event from the HTTP request context.
    ///
    /// Automatically generates a UUIDv7 event ID and captures the current
    /// timestamp in milliseconds.
    pub fn new(
        event_type: &str,
        ip: String,
        user_agent: String,
        referer: Option<String>,
        accept_language: Option<String>,
        request_path: &str,
        request_host: String,
        params: HashMap<String, String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            event_id: Uuid::now_v7().to_string(),
            event_type: event_type.to_string(),
            timestamp: now,
            ip,
            user_agent,
            referer,
            accept_language,
            request_path: request_path.to_string(),
            request_host,
            params,
            raw_payload: None,
        }
    }

    /// Serialize the event to a JSON byte vector for the Iggy message payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("Failed to serialize TrackingEvent")
    }
}
