//! ai-query — Unified AI query service for the Business OS.
//!
//! Combines Text-to-SQL (SLM on Baseten) + plugin-runtime Turso proxy +
//! Delta Lake DataFusion into a single service. Provides natural language
//! querying across all data sources: raw event history (Delta Lake) and
//! structured business data from plugins (Stripe, Shopify, etc. via Turso).
//!
//! Endpoints:
//!   POST /query/nl      — natural language → SQL → results
//!   POST /query/similar  — vector similarity search (LanceDB Cloud)
//!   POST /chat           — multi-turn conversation with SLM
//!   GET  /health         — health check

mod config;
mod delta;
mod schema;
mod slm;
mod turso_proxy;
mod vectordb;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use config::Config;
use slm::{QueryTarget, SlmResult};

// ─── Shared State ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    config: Config,
    lancedb_ready: bool,
}

// ─── Request / Response Types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NlQueryRequest {
    /// Natural language question.
    prompt: String,
    /// Tenant key_prefix for scoping.
    key_prefix: String,
    /// Max rows to return (default: 100).
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct NlQueryResponse {
    /// The SQL query that was generated and executed.
    sql: String,
    /// Which data source was queried: "delta" or "turso".
    source: String,
    /// Result rows.
    rows: Vec<serde_json::Value>,
    /// Number of rows returned.
    count: usize,
    /// Query execution time in milliseconds.
    query_ms: u64,
}

#[derive(Debug, Deserialize)]
struct SimilarQueryRequest {
    /// Event ID to find similar events for.
    event_id: Option<String>,
    /// Or raw text/embedding to search for.
    query: Option<String>,
    /// Tenant key_prefix for scoping.
    key_prefix: String,
    /// Max results (default: 10).
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    /// Conversation messages.
    messages: Vec<ChatMessage>,
    /// Tenant key_prefix for scoping.
    key_prefix: String,
    /// Max rows per query (default: 100).
    limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatResponse {
    /// Assistant's response message.
    message: ChatMessage,
    /// If a query was executed, the results.
    query_result: Option<NlQueryResponse>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// POST /query/nl — Natural language → SQL → results.
async fn handle_nl_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NlQueryRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "NL query: key_prefix={}, prompt='{}'",
        req.key_prefix, req.prompt
    );

    let limit = req.limit.unwrap_or(100);

    // Build schema context (Delta Lake + plugin tables)
    let schema_context = schema::build_schema_context(&state.config, &req.key_prefix).await;

    // Generate SQL via SLM
    let slm_result = slm::generate_sql(&state.config, &schema_context, &req.prompt)
        .await
        .map_err(|e| {
            error!("SLM error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?;

    match slm_result {
        SlmResult::Sql { query, target } => {
            let (source_name, result) = match target {
                QueryTarget::Delta => {
                    let (rows, query_ms) =
                        delta::execute_sql(&state.config, &req.key_prefix, &query, limit)
                            .await
                            .map_err(|e| {
                                error!("Delta query error: {}", e);
                                (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    Json(ErrorResponse { error: e }),
                                )
                            })?;
                    ("delta", (rows, query_ms))
                }
                QueryTarget::Turso => {
                    let (rows, query_ms) =
                        turso_proxy::execute_turso_sql(&state.config, &req.key_prefix, &query)
                            .await
                            .map_err(|e| {
                                error!("Turso query error: {}", e);
                                (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    Json(ErrorResponse { error: e }),
                                )
                            })?;
                    ("turso", (rows, query_ms))
                }
            };

            let (rows, query_ms) = result;
            let count = rows.len();

            info!(
                "NL query complete: source={}, {} rows, {}ms",
                source_name, count, query_ms
            );

            Ok(Json(serde_json::json!({
                "sql": query,
                "source": source_name,
                "rows": rows,
                "count": count,
                "query_ms": query_ms,
            })))
        }
        SlmResult::Explanation(explanation) => {
            info!("SLM returned explanation: {}", explanation);
            Ok(Json(serde_json::json!({
                "explanation": explanation,
            })))
        }
    }
}

/// POST /query/similar — Vector similarity search via LanceDB.
async fn handle_similar_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SimilarQueryRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Similar query: key_prefix={}, event_id={:?}, query={:?}",
        req.key_prefix, req.event_id, req.query
    );

    if !state.lancedb_ready {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "LanceDB not configured — set LANCEDB_URI and LANCEDB_API_KEY".into(),
            }),
        ));
    }

    let limit = req.limit.unwrap_or(10);

    // Build query vector from text or event_id
    let query_vector = if let Some(ref text) = req.query {
        vectordb::placeholder_embedding(text)
    } else if let Some(ref event_id) = req.event_id {
        // TODO: look up the event's vector from LanceDB and use it as the query
        vectordb::placeholder_embedding(event_id)
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Either 'query' (text) or 'event_id' is required".into(),
            }),
        ));
    };

    let (rows, query_ms) = vectordb::search_similar(
        &state.config, query_vector, &req.key_prefix, limit,
    )
    .await
    .map_err(|e| {
        error!("LanceDB search error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    let count = rows.len();
    info!("Similar query complete: {} results in {}ms", count, query_ms);

    Ok(Json(serde_json::json!({
        "source": "lancedb",
        "rows": rows,
        "count": count,
        "query_ms": query_ms,
        "key_prefix": req.key_prefix,
    })))
}

/// POST /chat — Multi-turn conversation with SLM.
async fn handle_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Chat: key_prefix={}, {} messages",
        req.key_prefix,
        req.messages.len()
    );

    // Extract the last user message as the prompt
    let user_prompt = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "No user message found in conversation".into(),
                }),
            )
        })?;

    let limit = req.limit.unwrap_or(100);

    // Build schema context
    let schema_context = schema::build_schema_context(&state.config, &req.key_prefix).await;

    // Generate SQL via SLM
    let slm_result = slm::generate_sql(&state.config, &schema_context, &user_prompt)
        .await
        .map_err(|e| {
            error!("SLM error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?;

    match slm_result {
        SlmResult::Sql { query, target } => {
            let (source_name, result) = match target {
                QueryTarget::Delta => {
                    let r = delta::execute_sql(&state.config, &req.key_prefix, &query, limit)
                        .await
                        .map_err(|e| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(ErrorResponse { error: e }),
                            )
                        })?;
                    ("delta", r)
                }
                QueryTarget::Turso => {
                    let r = turso_proxy::execute_turso_sql(
                        &state.config,
                        &req.key_prefix,
                        &query,
                    )
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse { error: e }),
                        )
                    })?;
                    ("turso", r)
                }
            };

            let (rows, query_ms) = result;
            let count = rows.len();

            // Build a natural language summary
            let summary = format!(
                "I queried {} and found {} result(s) in {}ms. Here's what I found:",
                source_name, count, query_ms
            );

            Ok(Json(ChatResponse {
                message: ChatMessage {
                    role: "assistant".into(),
                    content: summary,
                },
                query_result: Some(NlQueryResponse {
                    sql: query,
                    source: source_name.into(),
                    rows,
                    count,
                    query_ms,
                }),
            }))
        }
        SlmResult::Explanation(explanation) => Ok(Json(ChatResponse {
            message: ChatMessage {
                role: "assistant".into(),
                content: explanation,
            },
            query_result: None,
        })),
    }
}

// ─── Admin Endpoints (LanceDB management) ───────────────────────────────────

/// POST /vectordb/create-table — Create the events table in LanceDB Cloud.
async fn handle_create_table(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!("Creating LanceDB events table...");

    let result = vectordb::create_table(&state.config).await.map_err(|e| {
        error!("Create table error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "created",
        "table": "events",
        "response": result,
    })))
}

/// POST /vectordb/ingest — Insert events into LanceDB with placeholder embeddings.
/// Body: { "events": [{ "event_id", "tenant_id", "event_type", "timestamp_ms", "text" }] }
async fn handle_vectordb_ingest(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let events = body["events"]
        .as_array()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Missing 'events' array".into(),
                }),
            )
        })?;

    let mut event_ids = Vec::new();
    let mut tenant_ids = Vec::new();
    let mut event_types = Vec::new();
    let mut timestamps = Vec::new();
    let mut texts = Vec::new();
    let mut vectors = Vec::new();

    for ev in events {
        let eid = ev["event_id"].as_str().unwrap_or("").to_string();
        let tid = ev["tenant_id"].as_str().unwrap_or("").to_string();
        let etype = ev["event_type"].as_str().unwrap_or("").to_string();
        let ts = ev["timestamp_ms"].as_i64().unwrap_or(0);
        let text = ev["text"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                vectordb::event_to_text(
                    &etype,
                    ev["ip"].as_str().unwrap_or(""),
                    ev["user_agent"].as_str().unwrap_or(""),
                    &ev["params"].to_string(),
                )
            });

        let vec = vectordb::placeholder_embedding(&text);

        event_ids.push(eid);
        tenant_ids.push(tid);
        event_types.push(etype);
        timestamps.push(ts);
        texts.push(text);
        vectors.push(vec);
    }

    let count = vectordb::insert_events(
        &state.config,
        event_ids,
        tenant_ids,
        event_types,
        timestamps,
        texts,
        vectors,
    )
    .await
    .map_err(|e| {
        error!("Ingest error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "inserted",
        "count": count,
    })))
}

/// POST /vectordb/create-index — Create a vector index for fast ANN search.
async fn handle_create_index(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!("Creating vector index on LanceDB events table...");

    let result = vectordb::create_index(&state.config).await.map_err(|e| {
        error!("Create index error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "index_created",
        "response": result,
    })))
}

/// GET /health — Health check.
async fn handle_health(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "ai-query",
        "slm_configured": state.config.slm_url.is_some(),
        "plugin_runtime_configured": state.config.plugin_runtime_url.is_some(),
        "lancedb_configured": state.lancedb_ready,
    }))
}

// ─── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    let addr = format!("{}:{}", config.host, config.port);

    info!("Starting ai-query on {}...", addr);
    info!("Delta table: s3://{}/events", config.r2_bucket);
    info!(
        "SLM endpoint: {}",
        config.slm_url.as_deref().unwrap_or("NOT CONFIGURED (mock mode)")
    );
    info!(
        "Plugin runtime: {}",
        config
            .plugin_runtime_url
            .as_deref()
            .unwrap_or("NOT CONFIGURED")
    );

    // Check LanceDB Cloud configuration
    let lancedb_ready = vectordb::check_connection(&config).await;
    info!(
        "LanceDB: {}",
        if lancedb_ready { "CONNECTED" } else { "NOT CONFIGURED" }
    );

    let state = Arc::new(AppState { config, lancedb_ready });

    let app = Router::new()
        .route("/query/nl", post(handle_nl_query))
        .route("/query/similar", post(handle_similar_query))
        .route("/chat", post(handle_chat))
        .route("/vectordb/create-table", post(handle_create_table))
        .route("/vectordb/ingest", post(handle_vectordb_ingest))
        .route("/vectordb/create-index", post(handle_create_index))
        .route("/health", get(handle_health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    info!("ai-query listening on {}", addr);

    axum::serve(listener, app).await.expect("Server error");
}
