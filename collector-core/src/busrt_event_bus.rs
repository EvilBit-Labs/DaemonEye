//! Busrt-based event bus implementation for collector-core framework.
//!
//! This module provides a busrt message broker-based implementation of the EventBus trait,
//! enabling migration from crossbeam channels to industrial-grade IPC capabilities while
//! maintaining backward compatibility with existing EventBus interfaces.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    BusrtEventBus                                │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │   Embedded  │  │   Client    │  │    Topic Router         │ │
//! │  │   Broker    │  │  Manager    │  │                         │ │
//! │  └─────────────┘  └─────────────┘  │  - Event Distribution   │ │
//! │         │                │         │  - Capability Matching  │ │
//! │         └────────────────┼─────────│  - Correlation Tracking │ │
//! │                          │         │  - Backpressure Control │ │
//! │                          │         └─────────────────────────┘ │
//! │                          │                   │                 │
//! │                          └───────────────────┘                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    busrt_client::CollectorBusrtClient,
    busrt_types::{
        BrokerConfig, BrokerMode, BusrtClient, BusrtError, BusrtEvent, CrossbeamCompatibilityLayer,
        EmbeddedBrokerConfig, EventPayload, ProcessEventData, ProcessEventType, SecurityConfig,
        TransportConfig, TransportType, topics,
    },
    embedded_broker::EmbeddedBroker,
    event::CollectionEvent,
    event_bus::{BusEvent, EventBus, EventBusConfig, EventBusStatistics, EventSubscription},
    source::SourceCaps,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use busrt::QoS;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{RwLock, mpsc},
    task::JoinHandle,
    time::interval,
};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// Busrt-based event bus implementation providing backward compatibility with EventBus trait.
///
/// This implementation wraps busrt broker functionality to provide the same interface
/// as the existing LocalEventBus while leveraging industrial-grade message broker
/// capabilities for multi-process communication and future scalability.
///
/// # Features
///
/// - **Embedded Broker**: Runs busrt broker within collector-core runtime
/// - **Topic-Based Routing**: Uses hierarchical topics for event distribution
/// - **Capability Matching**: Routes events based on subscriber capabilities
/// - **Correlation Tracking**: Maintains correlation IDs across event chains
/// - **Backward Compatibility**: Implements existing EventBus trait interface
/// - **Migration Support**: Provides compatibility layer for crossbeam migration
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::{BusrtEventBus, EventBusConfig, EventSubscription, SourceCaps};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = EventBusConfig::default();
///     let mut event_bus = BusrtEventBus::new(config).await?;
///
///     // Start the embedded broker
///     event_bus.start().await?;
///
///     // Subscribe to events
///     let subscription = EventSubscription {
///         subscriber_id: "test-subscriber".to_string(),
///         capabilities: SourceCaps::PROCESS,
///         event_filter: None,
///         correlation_filter: None,
///         topic_patterns: None,
///         enable_wildcards: false,
///     };
///
///     let receiver = event_bus.subscribe(subscription).await?;
///
///     Ok(())
/// }
/// ```
pub struct BusrtEventBus {
    /// Event bus configuration
    config: EventBusConfig,
    /// Busrt broker configuration
    broker_config: BrokerConfig,
    /// Enable crossbeam compatibility mode
    enable_compatibility_mode: bool,
    /// Embedded broker instance (if running embedded mode)
    embedded_broker: Option<EmbeddedBroker>,
    /// Busrt client for publishing and subscribing
    busrt_client: Option<Arc<dyn BusrtClient>>,
    /// Subscriber management
    subscribers: Arc<RwLock<HashMap<String, BusrtSubscriberInfo>>>,
    /// Event statistics tracking
    statistics: Arc<RwLock<EventBusStatistics>>,
    /// Shutdown coordination
    shutdown_signal: Arc<AtomicBool>,
    /// Event counters for statistics
    event_counter: Arc<AtomicU64>,
    delivery_counter: Arc<AtomicU64>,
    drop_counter: Arc<AtomicU64>,
    /// Background task handles
    routing_handle: Option<JoinHandle<Result<()>>>,
    cleanup_handle: Option<JoinHandle<Result<()>>>,
    /// Crossbeam compatibility layer
    #[allow(dead_code)] // Placeholder for future implementation
    compatibility_layer: Option<CrossbeamCompatibilityLayer>,
    /// Topic subscription mapping
    topic_subscriptions: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

/// Internal subscriber information for busrt integration.
#[derive(Debug)]
#[allow(dead_code)]
struct BusrtSubscriberInfo {
    /// Original subscription configuration
    subscription: EventSubscription,
    /// Channel sender for delivering events to subscriber
    sender: mpsc::Sender<BusEvent>,
    /// Busrt topic patterns this subscriber is interested in
    topic_patterns: Vec<String>,
    /// Last delivery timestamp for statistics
    last_delivery: SystemTime,
    /// Number of events received by this subscriber
    events_received: u64,
}

/// Configuration for busrt event bus operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusrtEventBusConfig {
    /// Base event bus configuration
    pub event_bus: EventBusConfig,
    /// Busrt broker configuration
    pub broker: BrokerConfig,
    /// Enable crossbeam compatibility mode
    pub enable_compatibility_mode: bool,
    /// Topic prefix for event routing
    pub topic_prefix: String,
    /// QoS level for event messages
    pub default_qos: BusrtQoS,
    /// Maximum message size in bytes
    pub max_message_size: usize,
    /// Connection timeout for busrt client
    #[serde(with = "humantime_serde")]
    pub connection_timeout: Duration,
    /// Reconnection interval on connection failure
    #[serde(with = "humantime_serde")]
    pub reconnection_interval: Duration,
}

/// QoS levels for busrt messages (wrapper around busrt::QoS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BusrtQoS {
    /// No delivery guarantees
    No,
    /// At least once delivery
    Processed,
    /// Real-time delivery
    Realtime,
    /// Real-time with processing confirmation
    RealtimeProcessed,
}

impl From<BusrtQoS> for QoS {
    fn from(qos: BusrtQoS) -> Self {
        match qos {
            BusrtQoS::No => QoS::No,
            BusrtQoS::Processed => QoS::Processed,
            BusrtQoS::Realtime => QoS::Realtime,
            BusrtQoS::RealtimeProcessed => QoS::RealtimeProcessed,
        }
    }
}

impl Default for BusrtEventBusConfig {
    fn default() -> Self {
        Self {
            event_bus: EventBusConfig::default(),
            broker: BrokerConfig {
                mode: BrokerMode::Embedded,
                embedded: Some(EmbeddedBrokerConfig {
                    max_connections: 100,
                    message_buffer_size: 10000,
                    transport: TransportConfig {
                        transport_type: TransportType::UnixSocket,
                        path: Some(if cfg!(test) {
                            use std::time::{SystemTime, UNIX_EPOCH};
                            let timestamp = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_nanos();
                            let thread_id = std::thread::current().id();
                            format!("/tmp/daemoneye-busrt-{:?}-{}.sock", thread_id, timestamp)
                        } else {
                            "/tmp/daemoneye-busrt.sock".to_string()
                        }),
                        address: None,
                        port: None,
                    },
                    security: SecurityConfig {
                        authentication_enabled: false,
                        tls_enabled: false,
                        cert_file: None,
                        key_file: None,
                        ca_file: None,
                    },
                }),
                standalone: None,
            },
            enable_compatibility_mode: true,
            topic_prefix: "daemoneye".to_string(),
            default_qos: BusrtQoS::Processed,
            max_message_size: 1024 * 1024, // 1MB
            connection_timeout: Duration::from_secs(10),
            reconnection_interval: Duration::from_secs(5),
        }
    }
}

impl BusrtEventBus {
    /// Creates a new busrt event bus with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for event bus operation
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::{BusrtEventBus, EventBusConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = EventBusConfig::default();
    ///     let event_bus = BusrtEventBus::new(config).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new(config: EventBusConfig) -> Result<Self> {
        let busrt_config = BusrtEventBusConfig {
            event_bus: config.clone(),
            ..Default::default()
        };

        Self::new_with_busrt_config(busrt_config).await
    }

    /// Creates a new busrt event bus with full busrt configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Full busrt event bus configuration
    pub async fn new_with_busrt_config(config: BusrtEventBusConfig) -> Result<Self> {
        let statistics = EventBusStatistics {
            events_published: 0,
            events_delivered: 0,
            events_persisted: 0,
            active_subscribers: 0,
            events_dropped: 0,
            avg_delivery_latency_ms: 0.0,
            memory_usage_bytes: 0,
            last_updated: SystemTime::now(),
        };

        Ok(Self {
            config: config.event_bus,
            broker_config: config.broker,
            enable_compatibility_mode: config.enable_compatibility_mode,
            embedded_broker: None,
            busrt_client: None,
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            statistics: Arc::new(RwLock::new(statistics)),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            event_counter: Arc::new(AtomicU64::new(0)),
            delivery_counter: Arc::new(AtomicU64::new(0)),
            drop_counter: Arc::new(AtomicU64::new(0)),
            routing_handle: None,
            cleanup_handle: None,
            compatibility_layer: None,
            topic_subscriptions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Starts the busrt event bus with embedded broker if configured.
    ///
    /// This method must be called before using the event bus for publishing
    /// or subscribing to events.
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting busrt event bus");

        // Start embedded broker if configured
        if matches!(self.broker_config.mode, BrokerMode::Embedded) {
            self.start_embedded_broker().await?;
        }

        // Create busrt client
        self.create_busrt_client().await?;

        // Start event routing task
        self.start_event_routing().await?;

        // Start cleanup task if persistence is enabled
        if self.config.enable_persistence {
            self.start_cleanup_task().await?;
        }

        // Initialize compatibility layer if enabled
        // Note: We cannot directly clone Arc<dyn BusrtClient> into Box<dyn BusrtClient>
        // The compatibility layer will need to work with the Arc directly
        // For now, leave this as a placeholder until the compatibility layer is refactored
        // to accept Arc<dyn BusrtClient> instead of Box<dyn BusrtClient>
        if self.enable_compatibility_mode && self.busrt_client.is_some() {
            warn!(
                "Compatibility mode requested but requires refactoring to support Arc<dyn BusrtClient>"
            );
            // self.compatibility_layer = Some(CrossbeamCompatibilityLayer::new(...));
        }

        info!("Busrt event bus started successfully");
        Ok(())
    }

    /// Starts the embedded busrt broker.
    async fn start_embedded_broker(&mut self) -> Result<()> {
        info!("Starting embedded busrt broker");

        let embedded_config = self
            .broker_config
            .embedded
            .as_ref()
            .context("Embedded broker configuration not found")?
            .clone();

        let mut broker = EmbeddedBroker::new(embedded_config).await?;
        broker.start().await?;

        self.embedded_broker = Some(broker);

        info!("Embedded busrt broker started successfully");
        Ok(())
    }

    /// Creates and connects the busrt client.
    async fn create_busrt_client(&mut self) -> Result<()> {
        info!("Creating busrt client");

        // Create transport config from broker config
        let transport_config = if let Some(embedded_config) = &self.broker_config.embedded {
            embedded_config.transport.clone()
        } else {
            // Default transport config for standalone mode
            TransportConfig {
                transport_type: TransportType::UnixSocket,
                path: Some(if cfg!(test) {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos();
                    let thread_id = std::thread::current().id();
                    format!("/tmp/daemoneye-busrt-{:?}-{}.sock", thread_id, timestamp)
                } else {
                    "/tmp/daemoneye-busrt.sock".to_string()
                }),
                address: None,
                port: None,
            }
        };

        let mut client = CollectorBusrtClient::new(transport_config)
            .await
            .context("Failed to create busrt client")?;

        // Connect to broker
        client
            .connect()
            .await
            .context("Failed to connect busrt client")?;

        self.busrt_client = Some(Arc::new(client));

        info!("Busrt client created and connected successfully");
        Ok(())
    }

    /// Starts the event routing background task.
    async fn start_event_routing(&mut self) -> Result<()> {
        let subscribers = Arc::clone(&self.subscribers);
        let statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let event_counter = Arc::clone(&self.event_counter);
        let delivery_counter = Arc::clone(&self.delivery_counter);
        let drop_counter = Arc::clone(&self.drop_counter);
        let _config = self.config.clone();

        let handle = tokio::spawn(async move {
            info!("Busrt event routing task started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                // Placeholder for event routing logic
                // In the real implementation, this would handle busrt message routing
                tokio::time::sleep(Duration::from_millis(100)).await;

                // Update statistics periodically
                {
                    let mut stats = statistics.write().await;
                    stats.events_published = event_counter.load(Ordering::Relaxed);
                    stats.events_delivered = delivery_counter.load(Ordering::Relaxed);
                    stats.events_dropped = drop_counter.load(Ordering::Relaxed);
                    stats.active_subscribers = subscribers.read().await.len();
                    stats.last_updated = SystemTime::now();
                }
            }

            info!("Busrt event routing task stopped");
            Ok(())
        });

        self.routing_handle = Some(handle);
        Ok(())
    }

    /// Starts the cleanup task for removing old persisted events.
    async fn start_cleanup_task(&mut self) -> Result<()> {
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let cleanup_interval = self.config.cleanup_interval;

        let handle = tokio::spawn(async move {
            let mut cleanup_timer = interval(cleanup_interval);

            while !shutdown_signal.load(Ordering::Relaxed) {
                cleanup_timer.tick().await;

                // Placeholder for cleanup logic
                debug!("Performing busrt event cleanup");
            }

            Ok(())
        });

        self.cleanup_handle = Some(handle);
        Ok(())
    }

    /// Maps a CollectionEvent to appropriate busrt topic.
    fn map_event_to_topic(&self, event: &CollectionEvent) -> String {
        match event {
            CollectionEvent::Process(_) => topics::EVENTS_PROCESS_ENUMERATION.to_string(),
            CollectionEvent::Network(_) => topics::EVENTS_NETWORK_CONNECTIONS.to_string(),
            CollectionEvent::Filesystem(_) => topics::EVENTS_FILESYSTEM_OPERATIONS.to_string(),
            CollectionEvent::Performance(_) => topics::EVENTS_PERFORMANCE_METRICS.to_string(),
            CollectionEvent::TriggerRequest(_) => topics::CONTROL_AGENT_TASKS.to_string(),
        }
    }

    /// Converts CollectionEvent to BusrtEvent for message broker.
    fn convert_to_busrt_event(
        &self,
        event: CollectionEvent,
        correlation_id: String,
    ) -> Result<BusrtEvent> {
        let event_id = Uuid::new_v4().to_string();
        let timestamp_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get timestamp")?
            .as_millis() as i64;

        let topic = self.map_event_to_topic(&event);

        let payload = match event {
            CollectionEvent::Process(process_event) => {
                // Convert process event to busrt format
                EventPayload::Process(ProcessEventData {
                    process: crate::busrt_types::ProcessRecord {
                        pid: process_event.pid,
                        name: process_event.name.clone(),
                        executable_path: process_event.executable_path.clone(),
                    },
                    collection_cycle_id: Uuid::new_v4().to_string(),
                    event_type: ProcessEventType::Discovery,
                })
            }
            CollectionEvent::Network(_) => {
                // Placeholder for network event conversion
                EventPayload::Network(crate::busrt_types::NetworkEventData {
                    connection_id: Uuid::new_v4().to_string(),
                    event_type: crate::busrt_types::NetworkEventType::ConnectionEstablished,
                })
            }
            CollectionEvent::Filesystem(_) => {
                // Placeholder for filesystem event conversion
                EventPayload::Filesystem(crate::busrt_types::FilesystemEventData {
                    file_path: "/placeholder".to_string(),
                    event_type: crate::busrt_types::FilesystemEventType::FileCreated,
                })
            }
            CollectionEvent::Performance(_) => {
                // Placeholder for performance event conversion
                EventPayload::Performance(crate::busrt_types::PerformanceEventData {
                    metric_name: "placeholder".to_string(),
                    event_type: crate::busrt_types::PerformanceEventType::MetricUpdate,
                })
            }
            CollectionEvent::TriggerRequest(_) => {
                // Placeholder for trigger request conversion
                EventPayload::Process(ProcessEventData {
                    process: crate::busrt_types::ProcessRecord {
                        pid: 0,
                        name: "trigger".to_string(),
                        executable_path: None,
                    },
                    collection_cycle_id: Uuid::new_v4().to_string(),
                    event_type: ProcessEventType::Anomaly,
                })
            }
        };

        Ok(BusrtEvent {
            event_id,
            correlation_id: Some(correlation_id),
            timestamp_ms,
            source_collector: "collector-core".to_string(),
            topic,
            payload,
            correlation: None,
        })
    }

    /// Converts BusrtEvent back to BusEvent for EventBus compatibility.
    #[allow(dead_code)]
    fn convert_from_busrt_event(&self, busrt_event: BusrtEvent) -> Result<BusEvent> {
        Self::convert_from_busrt_event_static(busrt_event)
    }

    /// Static version of convert_from_busrt_event for use in spawned tasks.
    fn convert_from_busrt_event_static(busrt_event: BusrtEvent) -> Result<BusEvent> {
        // Convert busrt event back to CollectionEvent
        let collection_event = match busrt_event.payload {
            EventPayload::Process(process_data) => {
                CollectionEvent::Process(crate::event::ProcessEvent {
                    pid: process_data.process.pid,
                    ppid: None,
                    name: process_data.process.name,
                    executable_path: process_data.process.executable_path,
                    command_line: Vec::new(),
                    start_time: None,
                    cpu_usage: None,
                    memory_usage: None,
                    executable_hash: None,
                    user_id: None,
                    accessible: true,
                    file_exists: true,
                    timestamp: SystemTime::UNIX_EPOCH
                        + Duration::from_millis(busrt_event.timestamp_ms as u64),
                    platform_metadata: None,
                })
            }
            EventPayload::Network(_) => {
                // Placeholder conversion
                CollectionEvent::Network(crate::event::NetworkEvent {
                    connection_id: "placeholder".to_string(),
                    source_addr: "0.0.0.0:0".to_string(),
                    dest_addr: "0.0.0.0:0".to_string(),
                    protocol: "TCP".to_string(),
                    state: "ESTABLISHED".to_string(),
                    pid: None,
                    bytes_sent: 0,
                    bytes_received: 0,
                    timestamp: SystemTime::UNIX_EPOCH
                        + Duration::from_millis(busrt_event.timestamp_ms as u64),
                })
            }
            EventPayload::Filesystem(_) => {
                // Placeholder conversion
                CollectionEvent::Filesystem(crate::event::FilesystemEvent {
                    path: "placeholder".to_string(),
                    operation: "access".to_string(),
                    pid: None,
                    size: None,
                    permissions: None,
                    file_hash: None,
                    timestamp: SystemTime::UNIX_EPOCH
                        + Duration::from_millis(busrt_event.timestamp_ms as u64),
                })
            }
            EventPayload::Performance(_) => {
                // Placeholder conversion
                CollectionEvent::Performance(crate::event::PerformanceEvent {
                    metric_name: "placeholder".to_string(),
                    value: 0.0,
                    unit: "count".to_string(),
                    pid: None,
                    component: "system".to_string(),
                    timestamp: SystemTime::UNIX_EPOCH
                        + Duration::from_millis(busrt_event.timestamp_ms as u64),
                })
            }
        };

        Ok(BusEvent {
            event_id: busrt_event.event_id,
            event: collection_event,
            correlation_id: busrt_event.correlation_id.unwrap_or_default(),
            bus_timestamp: SystemTime::UNIX_EPOCH
                + Duration::from_millis(busrt_event.timestamp_ms as u64),
            publisher_id: busrt_event.source_collector,
            routing_metadata: HashMap::new(),
        })
    }

    /// Determines topic patterns for a subscription based on capabilities.
    fn get_topic_patterns_for_subscription(&self, subscription: &EventSubscription) -> Vec<String> {
        let mut patterns = Vec::new();

        if subscription.capabilities.contains(SourceCaps::PROCESS) {
            patterns.push(format!("{}/*", topics::EVENTS_PROCESS_ENUMERATION));
        }
        if subscription.capabilities.contains(SourceCaps::NETWORK) {
            patterns.push(format!("{}/*", topics::EVENTS_NETWORK_CONNECTIONS));
        }
        if subscription.capabilities.contains(SourceCaps::FILESYSTEM) {
            patterns.push(format!("{}/*", topics::EVENTS_FILESYSTEM_OPERATIONS));
        }
        if subscription.capabilities.contains(SourceCaps::PERFORMANCE) {
            patterns.push(format!("{}/*", topics::EVENTS_PERFORMANCE_METRICS));
        }

        // If no specific capabilities, subscribe to all events
        if patterns.is_empty() {
            patterns.push("events/*".to_string());
        }

        patterns
    }
}

#[async_trait]
impl EventBus for BusrtEventBus {
    #[instrument(skip_all, fields(correlation_id = %correlation_id))]
    async fn publish(&mut self, event: CollectionEvent, correlation_id: String) -> Result<()> {
        let busrt_client = self
            .busrt_client
            .as_ref()
            .context("Busrt client not initialized - call start() first")?;

        // Convert to busrt event
        let busrt_event = self.convert_to_busrt_event(event, correlation_id)?;
        let topic = busrt_event.topic.clone();

        // Publish via busrt client
        busrt_client
            .publish(&topic, busrt_event)
            .await
            .context("Failed to publish event via busrt")?;

        // Update statistics
        self.event_counter.fetch_add(1, Ordering::Relaxed);

        debug!(topic = %topic, "Event published via busrt");
        Ok(())
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<mpsc::Receiver<BusEvent>> {
        let busrt_client = self
            .busrt_client
            .as_ref()
            .context("Busrt client not initialized - call start() first")?
            .clone();

        // Create channel for delivering events to subscriber
        let (sender, receiver) = mpsc::channel(self.config.max_buffer_size);

        // Determine topic patterns for this subscription
        let topic_patterns = self.get_topic_patterns_for_subscription(&subscription);

        // Subscribe to busrt topics and spawn forwarding tasks
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        for pattern in &topic_patterns {
            let pattern_clone = pattern.clone();
            let subscriber_id = subscription.subscriber_id.clone();
            let sender_clone = sender.clone();
            let subscribers_clone = Arc::clone(&self.subscribers);
            let topic_subs_clone = Arc::clone(&self.topic_subscriptions);
            let shutdown_clone = Arc::clone(&shutdown_signal);

            // Subscribe to the busrt broker for this pattern
            let mut busrt_receiver = match busrt_client.subscribe(pattern).await {
                Ok(rx) => {
                    debug!(
                        subscriber_id = %subscriber_id,
                        pattern = %pattern,
                        "Subscribed to busrt topic pattern"
                    );
                    rx
                }
                Err(e) => {
                    error!(
                        subscriber_id = %subscriber_id,
                        pattern = %pattern,
                        error = %e,
                        "Failed to subscribe to busrt topic pattern"
                    );
                    continue;
                }
            };

            // Spawn task to forward events from busrt to subscriber
            tokio::spawn(async move {
                debug!(
                    subscriber_id = %subscriber_id,
                    pattern = %pattern_clone,
                    "Starting event forwarding task"
                );

                loop {
                    // Check shutdown signal
                    if shutdown_clone.load(Ordering::Relaxed) {
                        debug!(subscriber_id = %subscriber_id, "Shutdown signal received, stopping forwarding");
                        break;
                    }

                    // Wait for event with timeout
                    let busrt_event = tokio::select! {
                        event = busrt_receiver.recv() => {
                            match event {
                                Some(e) => e,
                                None => {
                                    debug!(subscriber_id = %subscriber_id, "Busrt receiver closed");
                                    break;
                                }
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            continue;
                        }
                    };
                    // Convert BusrtEvent to BusEvent
                    let bus_event = match Self::convert_from_busrt_event_static(busrt_event) {
                        Ok(event) => event,
                        Err(e) => {
                            warn!(
                                subscriber_id = %subscriber_id,
                                error = %e,
                                "Failed to convert busrt event"
                            );
                            continue;
                        }
                    };

                    // Try to send event to subscriber
                    if let Err(e) = sender_clone.try_send(bus_event) {
                        match e {
                            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                warn!(
                                    subscriber_id = %subscriber_id,
                                    "Subscriber channel full, dropping event"
                                );
                            }
                            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                                debug!(
                                    subscriber_id = %subscriber_id,
                                    "Subscriber channel closed, stopping forwarding"
                                );

                                // Clean up subscriber registration
                                let mut subscribers = subscribers_clone.write().await;
                                subscribers.remove(&subscriber_id);

                                let mut topic_subs = topic_subs_clone.write().await;
                                if let Some(subs) = topic_subs.get_mut(&pattern_clone) {
                                    subs.retain(|id| id != &subscriber_id);
                                }

                                break;
                            }
                        }
                    } else {
                        // Update subscriber statistics
                        let mut subscribers = subscribers_clone.write().await;
                        if let Some(info) = subscribers.get_mut(&subscriber_id) {
                            info.last_delivery = SystemTime::now();
                            info.events_received = info.events_received.saturating_add(1);
                        }
                    }
                }

                debug!(
                    subscriber_id = %subscriber_id,
                    pattern = %pattern_clone,
                    "Event forwarding task stopped"
                );
            });
        }

        // Store subscriber information
        let subscriber_info = BusrtSubscriberInfo {
            subscription: subscription.clone(),
            sender,
            topic_patterns: topic_patterns.clone(),
            last_delivery: SystemTime::now(),
            events_received: 0,
        };

        {
            let mut subscribers = self.subscribers.write().await;
            subscribers.insert(subscription.subscriber_id.clone(), subscriber_info);
        }

        // Update topic subscription mapping
        {
            let mut topic_subs = self.topic_subscriptions.write().await;
            for pattern in topic_patterns {
                topic_subs
                    .entry(pattern)
                    .or_insert_with(Vec::new)
                    .push(subscription.subscriber_id.clone());
            }
        }

        info!(
            subscriber_id = %subscription.subscriber_id,
            capabilities = ?subscription.capabilities,
            "Subscriber registered with busrt event bus"
        );

        Ok(receiver)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        let _busrt_client = self
            .busrt_client
            .as_ref()
            .context("Busrt client not initialized")?;

        // Remove subscriber
        let subscriber_info = {
            let mut subscribers = self.subscribers.write().await;
            subscribers.remove(subscriber_id)
        };

        if let Some(info) = subscriber_info {
            // Unsubscribe from busrt topics
            for pattern in &info.topic_patterns {
                // Note: In real implementation, we would unsubscribe from busrt topics here
                debug!(
                    subscriber_id = %subscriber_id,
                    pattern = %pattern,
                    "Unsubscribing from busrt topic pattern"
                );
            }

            // Update topic subscription mapping
            {
                let mut topic_subs = self.topic_subscriptions.write().await;
                for pattern in &info.topic_patterns {
                    if let Some(subscribers) = topic_subs.get_mut(pattern) {
                        subscribers.retain(|id| id != subscriber_id);
                        if subscribers.is_empty() {
                            topic_subs.remove(pattern);
                        }
                    }
                }
            }

            info!(subscriber_id = %subscriber_id, "Subscriber unregistered from busrt event bus");
        }

        Ok(())
    }

    async fn statistics(&self) -> EventBusStatistics {
        self.statistics.read().await.clone()
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down busrt event bus");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for background tasks to complete
        if let Some(handle) = self.routing_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Routing task join error");
            }
        }

        if let Some(handle) = self.cleanup_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Cleanup task join error");
            }
        }

        // Shutdown embedded broker if running
        if let Some(mut broker) = self.embedded_broker.take() {
            if let Err(e) = broker.shutdown().await {
                warn!(error = %e, "Embedded broker shutdown error");
            }
        }

        info!("Busrt event bus shutdown complete");
        Ok(())
    }

    async fn replay_events(&mut self, correlation_id: &str, subscriber_id: &str) -> Result<()> {
        // Placeholder implementation for event replay
        // In the real implementation, this would query busrt persistence
        info!(
            correlation_id = %correlation_id,
            subscriber_id = %subscriber_id,
            "Event replay requested (placeholder implementation)"
        );
        Ok(())
    }
}

/// Placeholder client for compatibility layer
#[allow(dead_code)] // Placeholder for future implementation
struct PlaceholderCompatibilityClient;

#[async_trait]
impl BusrtClient for PlaceholderCompatibilityClient {
    async fn publish(&self, _topic: &str, _message: BusrtEvent) -> Result<(), BusrtError> {
        Ok(())
    }

    async fn subscribe(&self, _pattern: &str) -> Result<mpsc::Receiver<BusrtEvent>, BusrtError> {
        let (_tx, rx) = mpsc::channel(100);
        Ok(rx)
    }

    async fn unsubscribe(&self, _pattern: &str) -> Result<(), BusrtError> {
        Ok(())
    }

    async fn rpc_call_json(
        &self,
        _service: &str,
        _method: &str,
        _request: serde_json::Value,
    ) -> Result<serde_json::Value, BusrtError> {
        Ok(serde_json::json!({"status": "ok"}))
    }

    async fn get_broker_stats(&self) -> Result<crate::busrt_types::BrokerStats, BusrtError> {
        Ok(crate::busrt_types::BrokerStats {
            uptime_seconds: 0,
            total_connections: 0,
            active_connections: 0,
            messages_published: 0,
            messages_delivered: 0,
            topics_count: 0,
            memory_usage_bytes: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;

    #[tokio::test]
    async fn test_busrt_event_bus_creation() {
        let config = EventBusConfig::default();
        let event_bus = BusrtEventBus::new(config).await;
        assert!(event_bus.is_ok());
    }

    #[tokio::test]
    async fn test_busrt_event_bus_startup() {
        let config = EventBusConfig::default();
        let mut event_bus = BusrtEventBus::new(config).await.unwrap();

        let result = event_bus.start().await;
        assert!(result.is_ok());

        // Cleanup
        let _ = event_bus.shutdown().await;
    }

    #[tokio::test]
    async fn test_event_to_topic_mapping() {
        let config = EventBusConfig::default();
        let event_bus = BusrtEventBus::new(config).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: Vec::new(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let topic = event_bus.map_event_to_topic(&process_event);
        assert_eq!(topic, topics::EVENTS_PROCESS_ENUMERATION);
    }

    #[tokio::test]
    async fn test_subscription_topic_patterns() {
        let config = EventBusConfig::default();
        let event_bus = BusrtEventBus::new(config).await.unwrap();

        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_string(),
            capabilities: SourceCaps::PROCESS | SourceCaps::NETWORK,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
        };

        let patterns = event_bus.get_topic_patterns_for_subscription(&subscription);
        assert!(patterns.len() >= 2);
        assert!(
            patterns
                .iter()
                .any(|p| p.contains(topics::EVENTS_PROCESS_ENUMERATION))
        );
        assert!(
            patterns
                .iter()
                .any(|p| p.contains(topics::EVENTS_NETWORK_CONNECTIONS))
        );
    }

    #[tokio::test]
    async fn test_busrt_event_conversion() {
        let config = EventBusConfig::default();
        let event_bus = BusrtEventBus::new(config).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: Vec::new(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_id = "test-correlation-123".to_string();
        let busrt_event = event_bus
            .convert_to_busrt_event(process_event, correlation_id.clone())
            .unwrap();

        assert_eq!(busrt_event.correlation_id, Some(correlation_id));
        assert_eq!(busrt_event.topic, topics::EVENTS_PROCESS_ENUMERATION);
        assert!(matches!(busrt_event.payload, EventPayload::Process(_)));
    }

    #[tokio::test]
    async fn test_busrt_client_integration() {
        let transport = TransportConfig {
            transport_type: TransportType::UnixSocket,
            path: Some("/tmp/test_integration.sock".to_string()),
            address: None,
            port: None,
        };

        let client = CollectorBusrtClient::new(transport).await;
        assert!(client.is_ok());

        let client = client.unwrap();
        assert!(!client.is_connected()); // Should not be connected initially
    }

    #[tokio::test]
    async fn test_busrt_config_defaults() {
        let config = BusrtEventBusConfig::default();

        assert!(matches!(config.broker.mode, BrokerMode::Embedded));
        assert!(config.enable_compatibility_mode);
        assert_eq!(config.topic_prefix, "daemoneye");
        assert!(matches!(config.default_qos, BusrtQoS::Processed));
    }

    #[tokio::test]
    async fn test_qos_conversion() {
        let qos_no: QoS = BusrtQoS::No.into();
        let qos_processed: QoS = BusrtQoS::Processed.into();
        let qos_realtime: QoS = BusrtQoS::Realtime.into();
        let qos_realtime_processed: QoS = BusrtQoS::RealtimeProcessed.into();

        assert!(matches!(qos_no, QoS::No));
        assert!(matches!(qos_processed, QoS::Processed));
        assert!(matches!(qos_realtime, QoS::Realtime));
        assert!(matches!(qos_realtime_processed, QoS::RealtimeProcessed));
    }
}
