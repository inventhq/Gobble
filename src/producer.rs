//! Iggy producer module.
//!
//! Wraps the Apache Iggy SDK producer in a high-throughput, fire-and-forget
//! interface. Uses **background send mode** with sharded workers so that
//! `send()` enqueues the message into an in-memory buffer and returns
//! immediately — background threads batch and flush to Iggy in parallel.
//!
//! Falls back to NOOP mode (events counted but not persisted) when the
//! Iggy server is unreachable at startup. A background reconnection task
//! periodically attempts to connect and swap NOOP → Iggy without restart.

use iggy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::event::TrackingEvent;

/// Application-level backpressure timeout. If the Iggy producer's internal
/// `Block` mode cannot enqueue within this window (consumers dead / buffer
/// full), the event is shed and the HTTP handler still returns success.
/// Users can resync historical data — shedding is acceptable.
const BACKPRESSURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Maximum bytes held in the producer's in-memory buffer across all shards.
/// Bounds memory usage even under sustained backpressure.
const MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB

/// Internal enum to distinguish between a live Iggy producer and a
/// no-op fallback when the Iggy server is unavailable.
enum ProducerInner {
    /// Connected to Iggy — messages are batched and sent in the background.
    Iggy(IggyProducer),
    /// No Iggy connection — events are counted but silently dropped.
    Noop,
}

/// High-throughput event producer backed by Apache Iggy.
///
/// In **background mode**, calling [`send()`](Self::send) serializes the event,
/// enqueues it into a sharded in-memory buffer, and returns immediately.
/// Background worker threads batch messages and flush them to Iggy over TCP,
/// achieving throughput close to the raw HTTP layer ceiling.
///
/// The producer tracks a monotonic event counter via [`events_sent()`](Self::events_sent)
/// for health-check reporting.
///
/// If started in NOOP mode (Iggy unreachable), a background task periodically
/// attempts reconnection and swaps to Iggy mode without requiring a restart.
pub struct EventProducer {
    inner: RwLock<ProducerInner>,
    /// Monotonic counter of events accepted (including NOOP mode).
    events_sent: AtomicU64,
    /// Counter of events shed due to backpressure (buffer full / timeout).
    /// Users can resync historical data — shedding oldest is acceptable.
    events_dropped: AtomicU64,
    /// Connection params stored for NOOP → Iggy reconnection attempts.
    conn_params: ConnParams,
}

/// Stored connection parameters for reconnection attempts.
struct ConnParams {
    server_addr: String,
    stream_name: String,
    topic_name: String,
    partitions: u32,
}

impl EventProducer {
    /// Create a new producer and connect to the Iggy server.
    ///
    /// - Connects via TCP to `server_addr` (e.g. `"127.0.0.1:8090"`).
    /// - Authenticates with the default root credentials.
    /// - Creates the stream if it doesn't exist.
    /// - Creates the topic with the given number of partitions if it doesn't exist.
    /// - Configures **background send mode** with aggressive batching
    ///   (1000 messages or 1ms linger, whichever fires first).
    ///
    /// If the connection times out (5s) or fails, the producer falls back
    /// to NOOP mode and the server starts without Iggy persistence.
    pub async fn new(
        server_addr: &str,
        stream_name: &str,
        topic_name: &str,
        partitions: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Resolve DNS hostname to IP if needed (Iggy SDK only accepts IP addresses)
        let resolved_addr = resolve_server_addr(server_addr).await;
        info!("Iggy server address: {} (resolved from {})", resolved_addr, server_addr);

        // Build a TCP client pointed at the Iggy server with auto-login enabled.
        // The Iggy SDK's background producer creates its own internal TCP connections
        // that don't inherit the parent client's auth — auto-login ensures all
        // connections authenticate automatically.
        let client = IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(resolved_addr.clone())
            .with_auto_sign_in(AutoLogin::Enabled(Credentials::UsernamePassword(
                DEFAULT_ROOT_USERNAME.to_string(),
                DEFAULT_ROOT_PASSWORD.to_string(),
            )))
            .build()?;

        // Attempt connection with a 5-second timeout to avoid blocking
        // indefinitely if Iggy is unreachable
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.connect(),
        )
        .await
        {
            Ok(Ok(_)) => {
                info!("Connected to Iggy at {} (auto-login enabled)", server_addr);

                // Ensure the stream exists (idempotent)
                match client.create_stream(stream_name).await {
                    Ok(_) => info!("Created stream: {}", stream_name),
                    Err(_) => info!("Stream already exists: {}", stream_name),
                }

                // Build the producer in BACKGROUND mode for maximum throughput.
                // Background mode uses sharded workers that batch messages and
                // flush them asynchronously — send() returns immediately.
                let producer = client
                    .producer(stream_name, topic_name)?
                    .background(
                        BackgroundConfig::builder()
                            .batch_length(1000)
                            .linger_time(IggyDuration::from(1))
                            .max_buffer_size(IggyByteSize::from(MAX_BUFFER_BYTES))
                            .build(),
                    )
                    .partitioning(Partitioning::balanced())
                    .create_topic_if_not_exists(
                        partitions,
                        None,
                        IggyExpiry::NeverExpire,
                        MaxTopicSize::ServerDefault,
                    )
                    .build();

                producer.init().await?;

                info!(
                    "Iggy producer initialized (background mode) for stream '{}', topic '{}'",
                    stream_name, topic_name
                );

                Ok(Self {
                    inner: RwLock::new(ProducerInner::Iggy(producer)),
                    events_sent: AtomicU64::new(0),
                    events_dropped: AtomicU64::new(0),
                    conn_params: ConnParams {
                        server_addr: server_addr.to_string(),
                        stream_name: stream_name.to_string(),
                        topic_name: topic_name.to_string(),
                        partitions,
                    },
                })
            }
            Ok(Err(e)) => {
                warn!(
                    "Could not connect to Iggy ({}): {} — running in NOOP mode",
                    server_addr, e
                );
                Ok(Self {
                    inner: RwLock::new(ProducerInner::Noop),
                    events_sent: AtomicU64::new(0),
                    events_dropped: AtomicU64::new(0),
                    conn_params: ConnParams {
                        server_addr: server_addr.to_string(),
                        stream_name: stream_name.to_string(),
                        topic_name: topic_name.to_string(),
                        partitions,
                    },
                })
            }
            Err(_) => {
                warn!(
                    "Iggy connection timed out ({}) — running in NOOP mode",
                    server_addr
                );
                Ok(Self {
                    inner: RwLock::new(ProducerInner::Noop),
                    events_sent: AtomicU64::new(0),
                    events_dropped: AtomicU64::new(0),
                    conn_params: ConnParams {
                        server_addr: server_addr.to_string(),
                        stream_name: stream_name.to_string(),
                        topic_name: topic_name.to_string(),
                        partitions,
                    },
                })
            }
        }
    }

    /// Enqueue a tracking event for delivery to Iggy.
    ///
    /// In background mode this serializes the event to JSON, wraps it in an
    /// `IggyMessage`, and pushes it into the producer's internal buffer.
    /// The call returns immediately — actual network I/O happens on background
    /// worker threads. In NOOP mode the event is simply counted.
    ///
    /// When `partition_key` is provided, events with the same key are routed
    /// to the same Iggy partition (consistent hashing). This ensures all
    /// events for a given tenant land on the same partition.
    pub async fn send(&self, event: &TrackingEvent, partition_key: Option<&str>) {
        self.events_sent.fetch_add(1, Ordering::Relaxed);

        let inner = self.inner.read().await;
        match &*inner {
            ProducerInner::Iggy(producer) => {
                let payload = event.to_bytes();
                let message = match IggyMessage::builder()
                    .payload(bytes::Bytes::from(payload))
                    .build()
                {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!("Failed to build Iggy message: {}", e);
                        return;
                    }
                };
                let partitioning = partition_key
                    .and_then(|k| Partitioning::messages_key_str(k).ok())
                    .map(Arc::new);
                match tokio::time::timeout(
                    BACKPRESSURE_TIMEOUT,
                    producer.send_with_partitioning(vec![message], partitioning),
                ).await {
                    Ok(Err(e)) => {
                        let dropped = self.events_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        warn!("Event shed (send error): {} [total dropped: {}]", e, dropped);
                    }
                    Err(_) => {
                        let dropped = self.events_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        warn!("Event shed (backpressure timeout {}ms) [total dropped: {}]", BACKPRESSURE_TIMEOUT.as_millis(), dropped);
                    }
                    Ok(Ok(())) => {}
                }
            }
            ProducerInner::Noop => {}
        }
    }

    /// Enqueue a batch of tracking events for delivery to Iggy.
    ///
    /// Groups events by their `key_prefix` param (tenant ID) and sends each
    /// group with the appropriate partition key. Events without a `key_prefix`
    /// are sent with balanced (round-robin) partitioning.
    /// Returns the number of events successfully enqueued.
    pub async fn send_batch(&self, events: &[TrackingEvent]) -> usize {
        let count = events.len();
        self.events_sent.fetch_add(count as u64, Ordering::Relaxed);

        let inner = self.inner.read().await;
        match &*inner {
            ProducerInner::Iggy(producer) => {
                // Group events by key_prefix for partition-key routing
                let mut groups: HashMap<Option<String>, Vec<IggyMessage>> = HashMap::new();
                for event in events {
                    let payload = event.to_bytes();
                    match IggyMessage::builder()
                        .payload(bytes::Bytes::from(payload))
                        .build()
                    {
                        Ok(msg) => {
                            let key = event.params.get("key_prefix").cloned();
                            groups.entry(key).or_default().push(msg);
                        }
                        Err(e) => {
                            error!("Failed to build Iggy message: {}", e);
                        }
                    }
                }

                let mut sent = 0;
                for (key, messages) in groups {
                    let partitioning = key
                        .as_deref()
                        .and_then(|k| Partitioning::messages_key_str(k).ok())
                        .map(Arc::new);
                    let batch_size = messages.len();
                    match tokio::time::timeout(
                        BACKPRESSURE_TIMEOUT,
                        producer.send_with_partitioning(messages, partitioning),
                    ).await {
                        Ok(Err(e)) => {
                            let dropped = self.events_dropped.fetch_add(batch_size as u64, Ordering::Relaxed) + batch_size as u64;
                            warn!("Batch shed (send error): {} [{} events, total dropped: {}]", e, batch_size, dropped);
                        }
                        Err(_) => {
                            let dropped = self.events_dropped.fetch_add(batch_size as u64, Ordering::Relaxed) + batch_size as u64;
                            warn!("Batch shed (backpressure timeout {}ms) [{} events, total dropped: {}]", BACKPRESSURE_TIMEOUT.as_millis(), batch_size, dropped);
                        }
                        Ok(Ok(())) => {
                            sent += batch_size;
                        }
                    }
                }
                sent
            }
            ProducerInner::Noop => count,
        }
    }

    /// Returns the total number of events accepted since startup.
    /// Includes both Iggy-delivered and NOOP-counted events.
    pub fn events_sent(&self) -> u64 {
        self.events_sent.load(Ordering::Relaxed)
    }

    /// Returns the total number of events shed due to backpressure since startup.
    /// Non-zero means consumers are slow or down — users can resync historical data.
    pub fn events_dropped(&self) -> u64 {
        self.events_dropped.load(Ordering::Relaxed)
    }

    /// Returns `true` if the producer is connected to Iggy,
    /// `false` if running in NOOP fallback mode.
    pub async fn is_connected(&self) -> bool {
        matches!(&*self.inner.read().await, ProducerInner::Iggy(_))
    }

    /// Attempt to connect to Iggy and swap from NOOP → Iggy mode.
    /// Returns `true` if the swap succeeded, `false` if already connected
    /// or if the connection attempt failed.
    async fn try_reconnect(&self) -> bool {
        // Only attempt if currently in NOOP mode
        {
            let inner = self.inner.read().await;
            if matches!(&*inner, ProducerInner::Iggy(_)) {
                return false;
            }
        }

        let p = &self.conn_params;
        info!("Attempting NOOP → Iggy reconnection to {}...", p.server_addr);

        let resolved = resolve_server_addr(&p.server_addr).await;
        let client = match IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(resolved)
            .with_auto_sign_in(AutoLogin::Enabled(Credentials::UsernamePassword(
                DEFAULT_ROOT_USERNAME.to_string(),
                DEFAULT_ROOT_PASSWORD.to_string(),
            )))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("Reconnect: failed to build client: {}", e);
                return false;
            }
        };

        match tokio::time::timeout(std::time::Duration::from_secs(5), client.connect()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                warn!("Reconnect: connection failed: {}", e);
                return false;
            }
            Err(_) => {
                warn!("Reconnect: connection timed out");
                return false;
            }
        }

        // Ensure stream exists
        match client.create_stream(&p.stream_name).await {
            Ok(_) => info!("Reconnect: created stream: {}", p.stream_name),
            Err(_) => {}
        }

        let producer = match client
            .producer(&p.stream_name, &p.topic_name)
        {
            Ok(b) => b,
            Err(e) => {
                warn!("Reconnect: failed to build producer: {}", e);
                return false;
            }
        };

        let producer = producer
            .background(
                BackgroundConfig::builder()
                    .batch_length(1000)
                    .linger_time(IggyDuration::from(1))
                    .max_buffer_size(IggyByteSize::from(MAX_BUFFER_BYTES))
                    .build(),
            )
            .partitioning(Partitioning::balanced())
            .create_topic_if_not_exists(
                p.partitions,
                None,
                IggyExpiry::NeverExpire,
                MaxTopicSize::ServerDefault,
            )
            .build();

        if let Err(e) = producer.init().await {
            warn!("Reconnect: producer init failed: {}", e);
            return false;
        }

        // Swap NOOP → Iggy
        let mut inner = self.inner.write().await;
        *inner = ProducerInner::Iggy(producer);
        info!("NOOP → Iggy reconnection successful! Events are now being persisted.");
        true
    }

    /// Start a background task that periodically attempts to reconnect
    /// to Iggy when running in NOOP mode. Once connected, the task stops
    /// retrying. Call this after creating the producer.
    pub fn start_reconnect_task(self: &Arc<Self>) {
        let producer = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.tick().await; // skip immediate first tick
            loop {
                interval.tick().await;
                if producer.is_connected().await {
                    // Already connected — stop trying
                    break;
                }
                if producer.try_reconnect().await {
                    // Successfully reconnected — stop trying
                    break;
                }
            }
        });
    }
}

/// Resolve a `host:port` address, converting DNS hostnames to IP addresses.
/// The Iggy SDK only accepts raw IP addresses, so K8s service DNS names
/// like `iggy.tracker.svc.cluster.local:8090` must be resolved first.
/// If the host is already an IP address, it is returned unchanged.
pub async fn resolve_server_addr(addr: &str) -> String {
    use tokio::net::lookup_host;

    // Try parsing as-is first (already an IP:port)
    if addr.parse::<std::net::SocketAddr>().is_ok() {
        return addr.to_string();
    }

    // Resolve DNS hostname to IP
    match lookup_host(addr).await {
        Ok(mut addrs) => {
            if let Some(resolved) = addrs.next() {
                format!("{}:{}", resolved.ip(), resolved.port())
            } else {
                warn!("DNS resolution returned no addresses for {}, using as-is", addr);
                addr.to_string()
            }
        }
        Err(e) => {
            warn!("DNS resolution failed for {}: {} — using as-is", addr, e);
            addr.to_string()
        }
    }
}

/// Thread-safe shared handle to the event producer.
pub type SharedProducer = Arc<EventProducer>;
