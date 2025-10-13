//! Embedded busrt broker implementation for collector-core runtime.
//!
//! This module provides embedded busrt broker functionality that runs within
//! the collector-core process, enabling multi-process communication without
//! requiring external broker infrastructure.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                  Embedded Busrt Broker                          │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │   Broker    │  │  Transport  │  │    Message Router       │ │
//! │  │   Core      │  │   Layer     │  │                         │ │
//! │  └─────────────┘  └─────────────┘  │  - Topic Management     │ │
//! │         │                │         │  - Client Management    │ │
//! │         └────────────────┼─────────│  - Message Distribution │ │
//! │                          │         │  - Health Monitoring    │ │
//! │                          │         └─────────────────────────┘ │
//! │                          │                   │                 │
//! │                          └───────────────────┘                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::busrt_types::{
    BrokerStats, EmbeddedBrokerConfig, SecurityConfig, TransportConfig, TransportType,
};
use anyhow::{Context, Result};
use base64::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock, mpsc},
    task::JoinHandle,
    time::{interval, timeout},
};
use tracing::{debug, error, info, instrument, warn};

/// Embedded busrt broker for collector-core runtime.
///
/// This broker runs within the collector-core process and provides
/// message routing, client management, and health monitoring capabilities
/// for multi-collector communication.
///
/// # Features
///
/// - **Multi-Transport Support**: Unix sockets, named pipes, TCP, in-process
/// - **Client Management**: Connection tracking and lifecycle management
/// - **Message Routing**: Topic-based message distribution
/// - **Health Monitoring**: Broker statistics and health reporting
/// - **Graceful Shutdown**: Coordinated shutdown with client notification
/// - **Security**: Optional TLS and authentication support
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::{EmbeddedBroker, EmbeddedBrokerConfig, TransportConfig, TransportType};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = EmbeddedBrokerConfig {
///         max_connections: 100,
///         message_buffer_size: 10000,
///         transport: TransportConfig {
///             transport_type: TransportType::UnixSocket,
///             path: Some("/tmp/daemoneye-broker.sock".to_string()),
///             address: None,
///             port: None,
///         },
///         security: Default::default(),
///     };
///
///     let mut broker = EmbeddedBroker::new(config).await?;
///     broker.start().await?;
///
///     // Broker is now running and accepting connections
///
///     broker.shutdown().await?;
///     Ok(())
/// }
/// ```
pub struct EmbeddedBroker {
    /// Broker configuration
    config: EmbeddedBrokerConfig,
    /// Broker runtime state
    state: Arc<BrokerState>,
    /// Transport listener handles
    listener_handles: Vec<JoinHandle<Result<()>>>,
    /// Message routing task handle
    routing_handle: Option<JoinHandle<Result<()>>>,
    /// Health monitoring task handle
    health_handle: Option<JoinHandle<Result<()>>>,
    /// Client cleanup task handle
    cleanup_handle: Option<JoinHandle<Result<()>>>,
    /// Shutdown coordination
    shutdown_signal: Arc<AtomicBool>,
    /// Broker statistics
    statistics: Arc<RwLock<BrokerStats>>,
}

/// Internal broker state for managing clients and messages.
#[derive(Debug)]
struct BrokerState {
    /// Connected clients
    clients: RwLock<HashMap<String, ClientInfo>>,
    /// Topic subscriptions
    subscriptions: RwLock<HashMap<String, Vec<String>>>,
    /// Message routing channels
    message_router: Mutex<Option<mpsc::Sender<RoutingMessage>>>,
    /// Broker startup timestamp
    startup_time: SystemTime,
    /// Connection counters
    total_connections: AtomicU64,
    active_connections: AtomicU64,
    messages_published: AtomicU64,
    messages_delivered: AtomicU64,
}

/// Client connection information.
#[derive(Debug)]
#[allow(dead_code)]
struct ClientInfo {
    /// Unique client identifier
    client_id: String,
    /// Client connection timestamp
    connected_at: SystemTime,
    /// Last activity timestamp
    last_activity: SystemTime,
    /// Message sender for this client
    sender: mpsc::Sender<BrokerMessage>,
    /// Message receiver for this client (kept alive to prevent channel drops)
    receiver: Option<mpsc::Receiver<BrokerMessage>>,
    /// Client subscriptions
    subscriptions: Vec<String>,
    /// Client metadata
    metadata: HashMap<String, String>,
}

/// Internal message routing structure.
#[derive(Debug, Clone)]
struct RoutingMessage {
    /// Message identifier
    message_id: String,
    /// Source client identifier
    source_client_id: String,
    /// Target topic
    topic: String,
    /// Message payload
    payload: Vec<u8>,
    /// Message timestamp
    timestamp: SystemTime,
    /// Message metadata
    metadata: HashMap<String, String>,
}

/// Broker message for client communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerMessage {
    /// Message type
    pub message_type: BrokerMessageType,
    /// Message payload
    pub payload: serde_json::Value,
    /// Message timestamp
    pub timestamp: i64,
    /// Message metadata
    pub metadata: HashMap<String, String>,
}

/// Broker message types for client communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrokerMessageType {
    /// Connection acknowledgment
    ConnectionAck,
    /// Subscription confirmation
    SubscriptionAck,
    /// Published message delivery
    MessageDelivery,
    /// Health check request
    HealthCheck,
    /// Shutdown notification
    Shutdown,
    /// Error notification
    Error,
}

/// Broker configuration validation and defaults.
impl Default for EmbeddedBrokerConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            message_buffer_size: 10000,
            transport: TransportConfig {
                transport_type: TransportType::UnixSocket,
                path: Some("/tmp/daemoneye-broker.sock".to_string()),
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
        }
    }
}

impl EmbeddedBroker {
    /// Creates a new embedded broker with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Embedded broker configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::{EmbeddedBroker, EmbeddedBrokerConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = EmbeddedBrokerConfig::default();
    ///     let broker = EmbeddedBroker::new(config).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new(config: EmbeddedBrokerConfig) -> Result<Self> {
        // Validate configuration
        Self::validate_config(&config)?;

        let startup_time = SystemTime::now();
        let state = Arc::new(BrokerState {
            clients: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
            message_router: Mutex::new(None),
            startup_time,
            total_connections: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            messages_published: AtomicU64::new(0),
            messages_delivered: AtomicU64::new(0),
        });

        let statistics = Arc::new(RwLock::new(BrokerStats {
            uptime_seconds: 0,
            total_connections: 0,
            active_connections: 0,
            messages_published: 0,
            messages_delivered: 0,
            topics_count: 0,
            memory_usage_bytes: 0,
        }));

        Ok(Self {
            config,
            state,
            listener_handles: Vec::new(),
            routing_handle: None,
            health_handle: None,
            cleanup_handle: None,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            statistics,
        })
    }

    /// Validates broker configuration.
    fn validate_config(config: &EmbeddedBrokerConfig) -> Result<()> {
        if config.max_connections == 0 {
            anyhow::bail!("max_connections must be greater than 0");
        }

        if config.message_buffer_size == 0 {
            anyhow::bail!("message_buffer_size must be greater than 0");
        }

        // Validate transport configuration
        match config.transport.transport_type {
            TransportType::UnixSocket => {
                if config.transport.path.is_none() {
                    anyhow::bail!("Unix socket transport requires path configuration");
                }
            }
            TransportType::Tcp => {
                if config.transport.address.is_none() || config.transport.port.is_none() {
                    anyhow::bail!("TCP transport requires address and port configuration");
                }
            }
            TransportType::NamedPipe => {
                if config.transport.path.is_none() {
                    anyhow::bail!("Named pipe transport requires path configuration");
                }
            }
            TransportType::InProcess => {
                // In-process transport doesn't require additional configuration
            }
        }

        // Validate security configuration
        if config.security.tls_enabled
            && (config.security.cert_file.is_none() || config.security.key_file.is_none())
        {
            anyhow::bail!("TLS requires cert_file and key_file configuration");
        }

        Ok(())
    }

    /// Starts the embedded broker with all configured transports.
    ///
    /// This method initializes the message routing system, starts transport
    /// listeners, and begins health monitoring.
    #[instrument(skip(self))]
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting embedded busrt broker");

        // Initialize message routing
        self.start_message_routing().await?;

        // Start transport listeners
        self.start_transport_listeners().await?;

        // Start health monitoring
        self.start_health_monitoring().await?;

        // Start client cleanup
        self.start_client_cleanup().await?;

        info!(
            transport = ?self.config.transport.transport_type,
            max_connections = self.config.max_connections,
            "Embedded busrt broker started successfully"
        );

        Ok(())
    }

    /// Starts the message routing background task.
    async fn start_message_routing(&mut self) -> Result<()> {
        let (routing_tx, mut routing_rx) = mpsc::channel(self.config.message_buffer_size);

        // Store routing sender in broker state
        {
            let mut router = self.state.message_router.lock().await;
            *router = Some(routing_tx);
        }

        let state = Arc::clone(&self.state);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let statistics = Arc::clone(&self.statistics);

        let handle = tokio::spawn(async move {
            info!("Message routing task started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::select! {
                    message = routing_rx.recv() => {
                        match message {
                            Some(routing_message) => {
                                if let Err(e) = Self::route_message(routing_message, &state, &statistics).await {
                                    error!(error = %e, "Failed to route message");
                                }
                            }
                            None => {
                                debug!("Message routing channel closed");
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Periodic maintenance
                    }
                }
            }

            info!("Message routing task stopped");
            Ok(())
        });

        self.routing_handle = Some(handle);
        Ok(())
    }

    /// Routes a message to appropriate subscribers.
    async fn route_message(
        message: RoutingMessage,
        state: &Arc<BrokerState>,
        statistics: &Arc<RwLock<BrokerStats>>,
    ) -> Result<()> {
        // Find subscribers for the topic
        let subscribers = {
            let subscriptions = state.subscriptions.read().await;
            subscriptions
                .get(&message.topic)
                .cloned()
                .unwrap_or_default()
        };

        if subscribers.is_empty() {
            debug!(topic = %message.topic, "No subscribers for topic");
            return Ok(());
        }

        // Get client information for subscribers
        let clients = state.clients.read().await;
        let mut delivery_count = 0;
        let subscriber_count = subscribers.len();

        for subscriber_id in &subscribers {
            if let Some(client_info) = clients.get(subscriber_id) {
                let broker_message = BrokerMessage {
                    message_type: BrokerMessageType::MessageDelivery,
                    payload: serde_json::json!({
                        "message_id": message.message_id,
                        "topic": message.topic,
                        "payload": base64::prelude::BASE64_STANDARD.encode(&message.payload),
                        "source_client": message.source_client_id,
                        "timestamp": message.timestamp.duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default().as_millis() as i64
                    }),
                    timestamp: SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64,
                    metadata: message.metadata.clone(),
                };

                // Attempt delivery with timeout
                match timeout(
                    Duration::from_secs(5),
                    client_info.sender.send(broker_message),
                )
                .await
                {
                    Ok(Ok(())) => {
                        delivery_count += 1;
                        debug!(
                            subscriber = %subscriber_id,
                            topic = %message.topic,
                            "Message delivered to subscriber"
                        );
                    }
                    Ok(Err(_)) => {
                        warn!(
                            subscriber = %subscriber_id,
                            "Subscriber channel closed"
                        );
                    }
                    Err(_) => {
                        warn!(
                            subscriber = %subscriber_id,
                            "Message delivery timeout"
                        );
                    }
                }
            }
        }

        // Update statistics
        state
            .messages_delivered
            .fetch_add(delivery_count, Ordering::Relaxed);

        {
            let mut stats = statistics.write().await;
            stats.messages_delivered = state.messages_delivered.load(Ordering::Relaxed);
        }

        debug!(
            topic = %message.topic,
            subscribers = subscriber_count,
            delivered = delivery_count,
            "Message routing completed"
        );

        Ok(())
    }

    /// Starts transport listeners based on configuration.
    async fn start_transport_listeners(&mut self) -> Result<()> {
        match self.config.transport.transport_type {
            TransportType::UnixSocket => {
                self.start_unix_socket_listener().await?;
            }
            TransportType::Tcp => {
                self.start_tcp_listener().await?;
            }
            TransportType::NamedPipe => {
                self.start_named_pipe_listener().await?;
            }
            TransportType::InProcess => {
                self.start_in_process_listener().await?;
            }
        }

        Ok(())
    }

    /// Starts Unix socket listener.
    #[cfg(unix)]
    async fn start_unix_socket_listener(&mut self) -> Result<()> {
        let socket_path = self
            .config
            .transport
            .path
            .as_ref()
            .context("Unix socket path not configured")?
            .clone();

        // Remove existing socket file if it exists
        if let Err(e) = std::fs::remove_file(&socket_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %socket_path, error = %e, "Failed to remove existing socket file");
            }
        }

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind Unix socket: {}", socket_path))?;

        let state = Arc::clone(&self.state);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let max_connections = self.config.max_connections;

        let handle = tokio::spawn(async move {
            info!(path = %socket_path, "Unix socket listener started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, _addr)) => {
                                let current_connections = state.active_connections.load(Ordering::Relaxed);
                                if current_connections >= max_connections as u64 {
                                    warn!("Maximum connections reached, rejecting new connection");
                                    continue;
                                }

                                // Handle new client connection
                                if let Err(e) = Self::handle_new_client(stream, &state).await {
                                    error!(error = %e, "Failed to handle new client");
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to accept Unix socket connection");
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Check shutdown signal
                    }
                }
            }

            info!("Unix socket listener stopped");
            Ok(())
        });

        self.listener_handles.push(handle);
        Ok(())
    }

    #[cfg(not(unix))]
    async fn start_unix_socket_listener(&mut self) -> Result<()> {
        anyhow::bail!("Unix sockets are not supported on this platform");
    }

    /// Starts TCP listener.
    async fn start_tcp_listener(&mut self) -> Result<()> {
        let address = self
            .config
            .transport
            .address
            .as_ref()
            .context("TCP address not configured")?;
        let port = self
            .config
            .transport
            .port
            .context("TCP port not configured")?;

        let bind_addr = format!("{}:{}", address, port);
        let listener = TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind TCP listener: {}", bind_addr))?;

        let state = Arc::clone(&self.state);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let max_connections = self.config.max_connections;

        let handle = tokio::spawn(async move {
            info!(address = %bind_addr, "TCP listener started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((_stream, addr)) => {
                                let current_connections = state.active_connections.load(Ordering::Relaxed);
                                if current_connections >= max_connections as u64 {
                                    warn!(client_addr = %addr, "Maximum connections reached, rejecting new connection");
                                    continue;
                                }

                                debug!(client_addr = %addr, "New TCP connection accepted");

                                // Increment counters
                                state.total_connections.fetch_add(1, Ordering::Relaxed);
                                state.active_connections.fetch_add(1, Ordering::Relaxed);

                                // Spawn a task to handle the connection and ensure counter is decremented
                                let state_clone = Arc::clone(&state);
                                tokio::spawn(async move {
                                    // Placeholder connection handler
                                    // Note: TCP stream handling would be implemented here
                                    // For now, we just log and immediately close
                                    debug!(client_addr = %addr, "Processing TCP connection (placeholder)");

                                    // When connection completes (success or error), decrement counter
                                    state_clone.active_connections.fetch_sub(1, Ordering::Relaxed);
                                    debug!(client_addr = %addr, "TCP connection closed, active connections decremented");
                                });
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to accept TCP connection");
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Check shutdown signal
                    }
                }
            }

            info!("TCP listener stopped");
            Ok(())
        });

        self.listener_handles.push(handle);
        Ok(())
    }

    /// Starts named pipe listener (Windows).
    async fn start_named_pipe_listener(&mut self) -> Result<()> {
        // Placeholder for named pipe implementation
        info!("Named pipe listener started (placeholder implementation)");
        Ok(())
    }

    /// Starts in-process listener.
    async fn start_in_process_listener(&mut self) -> Result<()> {
        // Placeholder for in-process implementation
        info!("In-process listener started (placeholder implementation)");
        Ok(())
    }

    /// Handles a new client connection.
    #[cfg(unix)]
    async fn handle_new_client(
        _stream: tokio::net::UnixStream,
        state: &Arc<BrokerState>,
    ) -> Result<()> {
        // Placeholder for client connection handling
        // In a real implementation, this would:
        // 1. Perform authentication if enabled
        // 2. Create client session
        // 3. Start message handling loop
        // 4. Register client in broker state

        let client_id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(1000);

        let client_info = ClientInfo {
            client_id: client_id.clone(),
            connected_at: SystemTime::now(),
            last_activity: SystemTime::now(),
            sender,
            receiver: Some(receiver),
            subscriptions: Vec::new(),
            metadata: HashMap::new(),
        };

        {
            let mut clients = state.clients.write().await;
            clients.insert(client_id.clone(), client_info);
        }

        state.total_connections.fetch_add(1, Ordering::Relaxed);
        state.active_connections.fetch_add(1, Ordering::Relaxed);

        info!(client_id = %client_id, "New client connected");

        Ok(())
    }

    #[cfg(not(unix))]
    #[allow(dead_code)]
    async fn handle_new_client(_unused: (), _state: &Arc<BrokerState>) -> Result<()> {
        // Not used on non-Unix platforms
        Ok(())
    }

    /// Starts health monitoring background task.
    async fn start_health_monitoring(&mut self) -> Result<()> {
        let state = Arc::clone(&self.state);
        let statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let handle = tokio::spawn(async move {
            let mut health_timer = interval(Duration::from_secs(30));

            while !shutdown_signal.load(Ordering::Relaxed) {
                health_timer.tick().await;

                // Update broker statistics
                let uptime = state.startup_time.elapsed().unwrap_or_default().as_secs();

                let clients_count = state.clients.read().await.len();
                let topics_count = state.subscriptions.read().await.len();

                {
                    let mut stats = statistics.write().await;
                    stats.uptime_seconds = uptime;
                    stats.total_connections = state.total_connections.load(Ordering::Relaxed);
                    stats.active_connections = state.active_connections.load(Ordering::Relaxed);
                    stats.messages_published = state.messages_published.load(Ordering::Relaxed);
                    stats.messages_delivered = state.messages_delivered.load(Ordering::Relaxed);
                    stats.topics_count = topics_count as u64;
                    stats.memory_usage_bytes =
                        Self::estimate_memory_usage(clients_count, topics_count);
                }

                debug!(
                    uptime_seconds = uptime,
                    active_connections = clients_count,
                    topics_count = topics_count,
                    "Health monitoring update"
                );
            }

            Ok(())
        });

        self.health_handle = Some(handle);
        Ok(())
    }

    /// Starts client cleanup background task.
    async fn start_client_cleanup(&mut self) -> Result<()> {
        let state = Arc::clone(&self.state);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let handle = tokio::spawn(async move {
            let mut cleanup_timer = interval(Duration::from_secs(60));

            while !shutdown_signal.load(Ordering::Relaxed) {
                cleanup_timer.tick().await;

                // Clean up inactive clients
                let inactive_threshold = Duration::from_secs(300); // 5 minutes
                let now = SystemTime::now();
                let mut clients_to_remove = Vec::new();

                {
                    let clients = state.clients.read().await;
                    for (client_id, client_info) in clients.iter() {
                        if let Ok(inactive_duration) = now.duration_since(client_info.last_activity)
                        {
                            if inactive_duration > inactive_threshold {
                                clients_to_remove.push(client_id.clone());
                            }
                        }
                    }
                }

                if !clients_to_remove.is_empty() {
                    let mut clients = state.clients.write().await;
                    for client_id in &clients_to_remove {
                        clients.remove(client_id);
                        state.active_connections.fetch_sub(1, Ordering::Relaxed);
                    }

                    info!(
                        removed_clients = clients_to_remove.len(),
                        "Cleaned up inactive clients"
                    );
                }
            }

            Ok(())
        });

        self.cleanup_handle = Some(handle);
        Ok(())
    }

    /// Estimates memory usage for statistics.
    fn estimate_memory_usage(clients_count: usize, topics_count: usize) -> u64 {
        // Rough estimation of memory usage
        let base_overhead = 1024 * 1024; // 1MB base
        let per_client = 1024; // 1KB per client
        let per_topic = 512; // 512B per topic

        (base_overhead + (clients_count * per_client) + (topics_count * per_topic)) as u64
    }

    /// Returns current broker statistics.
    pub async fn statistics(&self) -> BrokerStats {
        self.statistics.read().await.clone()
    }

    /// Initiates graceful shutdown of the embedded broker.
    ///
    /// This method signals all background tasks to shut down gracefully,
    /// notifies connected clients, and waits for cleanup to complete.
    #[instrument(skip(self))]
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down embedded busrt broker");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Notify all connected clients
        self.notify_clients_shutdown().await?;

        // Wait for background tasks to complete
        if let Some(handle) = self.routing_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Message routing task join error");
            }
        }

        if let Some(handle) = self.health_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Health monitoring task join error");
            }
        }

        if let Some(handle) = self.cleanup_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Client cleanup task join error");
            }
        }

        // Wait for transport listeners to complete
        for handle in self.listener_handles.drain(..) {
            if let Err(e) = handle.await {
                warn!(error = %e, "Transport listener task join error");
            }
        }

        // Clean up socket file if using Unix socket
        if matches!(
            self.config.transport.transport_type,
            TransportType::UnixSocket
        ) {
            if let Some(socket_path) = &self.config.transport.path {
                if let Err(e) = std::fs::remove_file(socket_path) {
                    warn!(path = %socket_path, error = %e, "Failed to remove socket file");
                }
            }
        }

        info!("Embedded busrt broker shutdown complete");
        Ok(())
    }

    /// Notifies all connected clients about broker shutdown.
    async fn notify_clients_shutdown(&self) -> Result<()> {
        let clients = self.state.clients.read().await;

        let shutdown_message = BrokerMessage {
            message_type: BrokerMessageType::Shutdown,
            payload: serde_json::json!({
                "reason": "broker_shutdown",
                "message": "Broker is shutting down gracefully"
            }),
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            metadata: HashMap::new(),
        };

        for (client_id, client_info) in clients.iter() {
            if let Err(e) = client_info.sender.try_send(shutdown_message.clone()) {
                warn!(
                    client_id = %client_id,
                    error = %e,
                    "Failed to notify client of shutdown"
                );
            }
        }

        info!(client_count = clients.len(), "Notified clients of shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_embedded_broker_creation() {
        let config = EmbeddedBrokerConfig::default();
        let broker = EmbeddedBroker::new(config).await;
        assert!(broker.is_ok());
    }

    #[tokio::test]
    async fn test_config_validation() {
        // Test invalid max_connections
        let config = EmbeddedBrokerConfig {
            max_connections: 0,
            ..Default::default()
        };
        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_err());

        // Test invalid message_buffer_size
        let config = EmbeddedBrokerConfig {
            message_buffer_size: 0,
            ..Default::default()
        };
        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_err());

        // Test valid config
        let config = EmbeddedBrokerConfig::default();
        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unix_socket_config() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test_broker.sock");

        let config = EmbeddedBrokerConfig {
            transport: TransportConfig {
                transport_type: TransportType::UnixSocket,
                path: Some(socket_path.to_string_lossy().to_string()),
                address: None,
                port: None,
            },
            ..Default::default()
        };

        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tcp_config() {
        let config = EmbeddedBrokerConfig {
            transport: TransportConfig {
                transport_type: TransportType::Tcp,
                path: None,
                address: Some("127.0.0.1".to_string()),
                port: Some(0), // Use 0 for automatic port assignment
            },
            ..Default::default()
        };

        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_broker_startup_and_shutdown() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test_broker.sock");

        let config = EmbeddedBrokerConfig {
            transport: TransportConfig {
                transport_type: TransportType::UnixSocket,
                path: Some(socket_path.to_string_lossy().to_string()),
                address: None,
                port: None,
            },
            ..Default::default()
        };

        let mut broker = EmbeddedBroker::new(config).await.unwrap();

        // Start broker
        let start_result = broker.start().await;
        assert!(start_result.is_ok());

        // Get statistics
        let stats = broker.statistics().await;
        assert_eq!(stats.active_connections, 0);

        // Shutdown broker
        let shutdown_result = broker.shutdown().await;
        assert!(shutdown_result.is_ok());
    }

    #[tokio::test]
    async fn test_memory_usage_estimation() {
        let memory_usage = EmbeddedBroker::estimate_memory_usage(10, 5);
        assert!(memory_usage > 0);

        // More clients should use more memory
        let memory_usage_more = EmbeddedBroker::estimate_memory_usage(100, 50);
        assert!(memory_usage_more > memory_usage);
    }

    #[tokio::test]
    async fn test_broker_message_types() {
        let message = BrokerMessage {
            message_type: BrokerMessageType::ConnectionAck,
            payload: serde_json::json!({"status": "connected"}),
            timestamp: 1234567890,
            metadata: HashMap::new(),
        };

        // Test serialization
        let serialized = serde_json::to_string(&message);
        assert!(serialized.is_ok());

        // Test deserialization
        let deserialized: Result<BrokerMessage, _> = serde_json::from_str(&serialized.unwrap());
        assert!(deserialized.is_ok());
    }

    #[tokio::test]
    async fn test_security_config_validation() {
        // Test TLS config without cert files
        let config = EmbeddedBrokerConfig {
            security: SecurityConfig {
                tls_enabled: true,
                cert_file: None,
                key_file: None,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_err());

        // Test valid TLS config
        let config = EmbeddedBrokerConfig {
            security: SecurityConfig {
                tls_enabled: true,
                cert_file: Some("/path/to/cert.pem".to_string()),
                key_file: Some("/path/to/key.pem".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = EmbeddedBroker::validate_config(&config);
        assert!(result.is_ok());
    }
}
