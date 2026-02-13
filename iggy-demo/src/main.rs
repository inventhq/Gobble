// Apache Iggy Demo - Dynamic Producers and Consumers with Axum
//
// This demo shows:
// 1. Running an axum HTTP server
// 2. Dynamically creating producers for different topics
// 3. Dynamically creating consumers that process messages
// 4. Simple REST API to send and receive messages

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::StreamExt;
use iggy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const STREAM_NAME: &str = "demo-stream";

// Application state shared across handlers
struct AppState {
    client: IggyClient,
    producers: RwLock<HashMap<String, Arc<IggyProducer>>>,
    active_consumers: RwLock<HashSet<String>>,  // Track which topics have consumers
    message_buffer: RwLock<Vec<ReceivedMessageInfo>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SendMessageRequest {
    topic: String,
    payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SendMessageResponse {
    success: bool,
    message_id: String,
    topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceivedMessageInfo {
    id: String,
    topic: String,
    offset: u64,
    payload: String,
    timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TopicInfo {
    name: String,
    partitions: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    Registry::default()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new("INFO")))
        .init();

    info!("Starting Iggy Demo Application...");

    // Connect to Iggy server
    let client = IggyClientBuilder::new()
        .with_tcp()
        .with_server_address("127.0.0.1:8090".to_string())
        .build()?;

    client.connect().await?;
    client
        .login_user(DEFAULT_ROOT_USERNAME, DEFAULT_ROOT_PASSWORD)
        .await?;
    info!("Connected to Iggy server");

    // Create the demo stream if it doesn't exist
    match client.create_stream(STREAM_NAME).await {
        Ok(_) => info!("Created stream: {}", STREAM_NAME),
        Err(_) => info!("Stream already exists: {}", STREAM_NAME),
    }

    // Create app state - NO hardcoded topics!
    let state = Arc::new(AppState {
        client,
        producers: RwLock::new(HashMap::new()),
        active_consumers: RwLock::new(HashSet::new()),
        message_buffer: RwLock::new(Vec::new()),
    });

    // Build axum router
    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/topics", get(list_topics))
        .route("/topics/{topic}", post(create_topic))
        .route("/messages", post(send_message))
        .route("/messages/{topic}", get(get_messages))
        .with_state(state);

    // Start server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4000").await?;
    info!("Axum server listening on http://0.0.0.0:4000");
    info!("");
    info!("Available endpoints:");
    info!("  GET  /              - API info");
    info!("  GET  /health        - Health check");
    info!("  GET  /topics        - List all topics");
    info!("  POST /topics/:name  - Create a new topic (also starts dynamic consumer)");
    info!("  POST /messages      - Send a message (JSON: {{\"topic\": \"...\", \"payload\": \"...\"}})");
    info!("  GET  /messages/:topic - Get received messages for a topic");
    info!("");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn create_topic_if_not_exists(client: &IggyClient, topic_name: &str) -> Result<()> {
    match client
        .create_topic(
            &Identifier::named(STREAM_NAME).unwrap(),
            topic_name,
            1, // partitions
            CompressionAlgorithm::default(),
            None,
            IggyExpiry::NeverExpire,
            MaxTopicSize::ServerDefault,
        )
        .await
    {
        Ok(_) => info!("Created topic: {}", topic_name),
        Err(_) => info!("Topic already exists: {}", topic_name),
    }
    Ok(())
}

// ============================================================
// DYNAMIC PRODUCER - Created on-demand per topic
// ============================================================
async fn get_or_create_producer(
    state: &AppState,
    topic: &str,
) -> Result<Arc<IggyProducer>, StatusCode> {
    // Check if producer already exists
    {
        let producers = state.producers.read().await;
        if let Some(producer) = producers.get(topic) {
            return Ok(producer.clone());
        }
    }

    // Create new producer dynamically
    let producer = state
        .client
        .producer(STREAM_NAME, topic)
        .map_err(|e| {
            error!("Failed to create producer builder: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .direct(
            DirectConfig::builder()
                .batch_length(100)
                .linger_time(IggyDuration::from_str("10ms").unwrap())
                .build(),
        )
        .create_topic_if_not_exists(1, None, IggyExpiry::NeverExpire, MaxTopicSize::ServerDefault)
        .build();

    // Initialize the producer
    producer.init().await.map_err(|e| {
        error!("Failed to init producer: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let producer = Arc::new(producer);

    // Store for reuse
    {
        let mut producers = state.producers.write().await;
        producers.insert(topic.to_string(), producer.clone());
    }

    info!("🚀 Created DYNAMIC producer for topic: {}", topic);
    Ok(producer)
}

// ============================================================
// DYNAMIC CONSUMER - Created on-demand per topic
// ============================================================
async fn get_or_create_consumer(state: Arc<AppState>, topic: &str) -> Result<(), StatusCode> {
    // Check if consumer already exists for this topic
    {
        let consumers = state.active_consumers.read().await;
        if consumers.contains(topic) {
            return Ok(());
        }
    }

    // Mark as active before spawning to prevent duplicates
    {
        let mut consumers = state.active_consumers.write().await;
        if consumers.contains(topic) {
            return Ok(()); // Another task beat us to it
        }
        consumers.insert(topic.to_string());
    }

    // Spawn consumer task
    let topic_owned = topic.to_string();
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        if let Err(e) = consume_topic(state_clone, &topic_owned).await {
            error!("Dynamic consumer for topic {} failed: {}", topic_owned, e);
        }
    });

    info!("🎧 Created DYNAMIC consumer for topic: {}", topic);
    Ok(())
}

async fn consume_topic(state: Arc<AppState>, topic: &str) -> Result<()> {
    info!("Starting dynamic consumer for topic: {}", topic);

    // Retry loop for consumer initialization
    let mut retries = 0;
    let max_retries = 10;
    let mut consumer = loop {
        match state
            .client
            .consumer(&format!("demo-consumer-{}", topic), STREAM_NAME, topic, 0)
        {
            Ok(builder) => {
                let mut c = builder
                    .auto_commit(AutoCommit::When(AutoCommitWhen::PollingMessages))
                    .polling_strategy(PollingStrategy::next())
                    .poll_interval(IggyDuration::from_str("500ms")?)
                    .batch_length(10)
                    .init_retries(5, IggyDuration::from_str("1s")?)
                    .build();

                match c.init().await {
                    Ok(_) => {
                        info!("Consumer initialized for topic: {}", topic);
                        break c;
                    }
                    Err(e) => {
                        retries += 1;
                        if retries >= max_retries {
                            error!("Failed to init consumer after {} retries: {}", max_retries, e);
                            // Remove from active set on failure
                            let mut consumers = state.active_consumers.write().await;
                            consumers.remove(topic);
                            return Err(e.into());
                        }
                        info!("Consumer init failed for {}, retry {}/{}: {}", topic, retries, max_retries, e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    }
                }
            }
            Err(e) => {
                retries += 1;
                if retries >= max_retries {
                    error!("Failed to create consumer after {} retries: {}", max_retries, e);
                    // Remove from active set on failure
                    let mut consumers = state.active_consumers.write().await;
                    consumers.remove(topic);
                    return Err(e.into());
                }
                info!("Consumer creation failed for {}, retry {}/{}: {}", topic, retries, max_retries, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    };

    while let Some(result) = consumer.next().await {
        match result {
            Ok(message) => {
                let payload = String::from_utf8_lossy(&message.message.payload).to_string();
                let info = ReceivedMessageInfo {
                    id: format!("{}", message.message.header.id),
                    topic: topic.to_string(),
                    offset: message.message.header.offset,
                    payload: payload.clone(),
                    timestamp: message.message.header.timestamp,
                };

                info!(
                    "📨 Received message on {}: offset={}, payload={}",
                    topic, message.message.header.offset, payload
                );

                // Store in buffer
                let mut buffer = state.message_buffer.write().await;
                buffer.push(info);
                // Keep only last 100 messages
                if buffer.len() > 100 {
                    buffer.remove(0);
                }
            }
            Err(e) => {
                error!("Error consuming message: {}", e);
            }
        }
    }

    Ok(())
}

// --- HTTP Handlers ---

async fn root() -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "Iggy Demo API",
        "version": "2.0.0",
        "description": "FULLY DYNAMIC producer/consumer demo with Iggy and Axum",
        "features": {
            "dynamic_producers": "Created on-demand when sending messages to a topic",
            "dynamic_consumers": "Created on-demand when a topic is created or accessed"
        },
        "endpoints": {
            "GET /health": "Health check",
            "GET /topics": "List all topics",
            "POST /topics/:name": "Create a new topic + spawn dynamic consumer",
            "POST /messages": "Send a message (creates dynamic producer)",
            "GET /messages/:topic": "Get received messages for a topic"
        }
    }))
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn list_topics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stream = state
        .client
        .get_stream(&Identifier::named(STREAM_NAME).unwrap())
        .await;

    let active_consumers = state.active_consumers.read().await;

    match stream {
        Ok(Some(stream)) => {
            let topics: Vec<serde_json::Value> = stream
                .topics
                .iter()
                .map(|t| serde_json::json!({
                    "name": t.name,
                    "partitions": t.partitions_count,
                    "has_consumer": active_consumers.contains(&t.name)
                }))
                .collect();
            (StatusCode::OK, Json(serde_json::json!({"topics": topics})))
        }
        _ => (
            StatusCode::OK,
            Json(serde_json::json!({"topics": [], "error": "Could not fetch topics"})),
        ),
    }
}

async fn create_topic(
    State(state): State<Arc<AppState>>,
    Path(topic): Path<String>,
) -> impl IntoResponse {
    match create_topic_if_not_exists(&state.client, &topic).await {
        Ok(_) => {
            // DYNAMIC: Start consumer for this topic
            let _ = get_or_create_consumer(state.clone(), &topic).await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "success": true, 
                    "topic": topic,
                    "consumer_started": true
                })),
            )
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"success": false, "error": e.to_string()})),
        ),
    }
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // DYNAMIC: Get or create producer for this topic
    let producer = match get_or_create_producer(&state, &request.topic).await {
        Ok(p) => p,
        Err(status) => {
            return (
                status,
                Json(SendMessageResponse {
                    success: false,
                    message_id: String::new(),
                    topic: request.topic,
                }),
            )
        }
    };

    // DYNAMIC: Ensure consumer exists for this topic  
    let _ = get_or_create_consumer(state.clone(), &request.topic).await;

    // Create and send message
    let message_id = Uuid::new_v4().to_string();
    let message = match IggyMessage::from_str(&request.payload) {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to create message: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(SendMessageResponse {
                    success: false,
                    message_id: String::new(),
                    topic: request.topic,
                }),
            );
        }
    };

    match producer.send(vec![message]).await {
        Ok(_) => {
            info!("📤 Sent message to topic {}: {}", request.topic, request.payload);
            (
                StatusCode::OK,
                Json(SendMessageResponse {
                    success: true,
                    message_id,
                    topic: request.topic,
                }),
            )
        }
        Err(e) => {
            error!("Failed to send message: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SendMessageResponse {
                    success: false,
                    message_id: String::new(),
                    topic: request.topic,
                }),
            )
        }
    }
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(topic): Path<String>,
) -> impl IntoResponse {
    let buffer = state.message_buffer.read().await;
    let messages: Vec<_> = buffer
        .iter()
        .filter(|m| m.topic == topic)
        .cloned()
        .collect();

    let has_consumer = state.active_consumers.read().await.contains(&topic);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "topic": topic,
            "has_consumer": has_consumer,
            "count": messages.len(),
            "messages": messages
        })),
    )
}
