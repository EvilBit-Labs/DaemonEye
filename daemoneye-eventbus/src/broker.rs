//! Embedded broker implementation for the EventBus

use crate::error::{EventBusError, Result};
use crate::message::{BusEvent, CollectionEvent, EventSubscription, Message};
use crate::queue_manager::QueueManager;
use crate::rate_limiter::RateLimiter;
use crate::topic::TopicMatcher;
use crate::transport::{
    ClientConfig, ClientConnectionManager, SocketConfig, TransportClient, TransportServer,
};
use crate::{EventBus, EventBusStatistics};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Embedded broker that manages topics and subscriptions
pub struct DaemoneyeBroker {
    /// Topic matcher for routing messages
    topic_matcher: Arc<RwLock<TopicMatcher>>,
    /// Client connection manager
    client_manager: Arc<Mutex<ClientConnectionManager>>,
    /// Transport server for accepting connections
    transport_server: Arc<Mutex<Option<TransportServer>>>,
    /// Message sequence counter
    sequence: Arc<Mutex<u64>>,
    /// Statistics
    stats: Arc<Mutex<EventBusStatistics>>,
    /// Broker start time
    start_time: Instant,
    /// Shutdown signal broadcaster
    shutdown_tx: broadcast::Sender<()>,
    /// Socket configuration
    config: SocketConfig,
    /// Subscriber senders for message delivery
    subscriber_senders:
        Arc<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<BusEvent>>>>,
    /// Raw message subscribers for RPC (receive Message directly)
    raw_subscriber_senders:
        Arc<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<Message>>>>,
    /// Reverse mapping from original subscriber IDs to broker UUIDs
    subscriber_id_mapping: Arc<Mutex<std::collections::HashMap<String, Uuid>>>,
    /// Routing table for one-to-one messaging (client_id -> transport client)
    direct_routing: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Global semaphore for publish() backpressure
    publish_semaphore: Arc<tokio::sync::Semaphore>,
    /// Rate limiter with token bucket algorithm
    rate_limiter: Arc<RateLimiter>,
    /// Authentication enabled flag
    auth_enabled: bool,
    /// Queue capacity for queued messages
    #[allow(dead_code)]
    queue_capacity: usize,
    /// Queue manager for handling client message queues
    queue_manager: Arc<QueueManager>,
}

impl DaemoneyeBroker {
    /// Create a new embedded broker
    pub async fn new(socket_path: &str) -> Result<Self> {
        Self::new_with_config(socket_path, false, 1000).await
    }

    /// Create a new embedded broker with configuration
    pub async fn new_with_config(
        socket_path: &str,
        auth_enabled: bool,
        queue_capacity: usize,
    ) -> Result<Self> {
        let instance_id = Uuid::new_v4().to_string();
        let mut config = SocketConfig::new(&instance_id);

        // Override with provided socket path if different
        if socket_path != config.get_socket_path() {
            config = SocketConfig {
                unix_path: socket_path.to_string(),
                windows_pipe: socket_path.to_string(),
                connection_limit: 100, // Default connection limit
                #[cfg(target_os = "freebsd")]
                freebsd_path: None,
                auth_token: if auth_enabled {
                    Some(Uuid::new_v4().to_string())
                } else {
                    None
                },
                per_client_byte_limit: 10 * 1024 * 1024,
                rate_limit_config: None,
            };
        }

        // Create broadcast channel for shutdown signaling (capacity 1 is sufficient)
        let (shutdown_tx, _) = broadcast::channel(1);

        // Create client connection manager
        let client_config = ClientConfig::default();
        let client_manager = ClientConnectionManager::new(client_config);

        // Create queue manager with configured capacity
        let queue_manager = Arc::new(QueueManager::new(queue_capacity, queue_capacity * 10));

        // Create rate limiter with configuration from SocketConfig or default
        let rate_limit_config = config.rate_limit_config.clone().unwrap_or_default();
        let rate_limiter = Arc::new(RateLimiter::new(rate_limit_config));

        let broker = Self {
            topic_matcher: Arc::new(RwLock::new(TopicMatcher::new())),
            client_manager: Arc::new(Mutex::new(client_manager)),
            transport_server: Arc::new(Mutex::new(None)),
            sequence: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(EventBusStatistics::default())),
            start_time: Instant::now(),
            shutdown_tx,
            config,
            subscriber_senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
            raw_subscriber_senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
            subscriber_id_mapping: Arc::new(Mutex::new(std::collections::HashMap::new())),
            direct_routing: Arc::new(Mutex::new(std::collections::HashMap::new())),
            publish_semaphore: Arc::new(tokio::sync::Semaphore::new(1000)),
            rate_limiter,
            auth_enabled,
            queue_capacity,
            queue_manager,
        };

        info!("DaemonEye broker created with socket: {}", socket_path);
        Ok(broker)
    }

    /// Start the broker server
    pub async fn start(&self) -> Result<()> {
        info!("Starting DaemonEye broker");

        // Create and start transport server
        let server = TransportServer::new(self.config.clone()).await?;
        {
            let mut server_guard = self.transport_server.lock().await;
            *server_guard = Some(server);
        }

        // Start client connection acceptance task
        self.start_client_acceptance_task().await?;

        // Start health monitoring task
        self.start_health_monitoring_task().await?;

        info!("DaemonEye broker started successfully");
        Ok(())
    }

    /// Start client acceptance background task
    async fn start_client_acceptance_task(&self) -> Result<()> {
        let server = Arc::clone(&self.transport_server);
        let client_manager = Arc::clone(&self.client_manager);
        let direct_routing = Arc::clone(&self.direct_routing);
        let queue_manager = Arc::clone(&self.queue_manager);
        let auth_enabled = self.auth_enabled;
        let auth_token = self.config.auth_token.clone();
        let shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            loop {
                tokio::select! {
                    // Accept new connections with timeout
                    result = {
                        let server_clone = Arc::clone(&server);
                        async move {
                            let server_guard = server_clone.lock().await;
                            if let Some(ref transport_server) = *server_guard {
                                // We need to call accept() but can't hold the guard across await.
                                // Since TransportServer doesn't implement Clone, we'll need to
                                // restructure this. For now, we'll accept the connection while
                                // holding the guard, which means this will block other operations.
                                // TODO: Consider restructuring to avoid holding guard across await
                                tokio::time::timeout(
                                    tokio::time::Duration::from_millis(100),
                                    transport_server.accept()
                                ).await
                            } else {
                                Ok(Err(EventBusError::transport("Server not listening")))
                            }
                        }
                    } => {
                        match result {
                            Ok(Ok(mut accepted_client)) => {
                                let client_id = Uuid::new_v4().to_string();
                                info!("Accepted new client connection: {}", client_id);

                                // Authenticate if enabled (read first frame before inserting)
                                if auth_enabled && let Err(e) = Self::authenticate_client(&mut accepted_client, &auth_token).await {
                                    error!("Authentication failed for client {}: {}", client_id, e);
                                    let _ = accepted_client.close().await;
                                    continue;
                                }

                                // Insert client into manager
                                {
                                    let mut manager = client_manager.lock().await;
                                    if let Err(e) = manager.insert_accepted_client(client_id.clone(), accepted_client).await {
                                        error!("Failed to insert accepted client {}: {}", client_id, e);
                                        continue;
                                    }
                                }

                                // Update direct routing
                                {
                                    let mut routing = direct_routing.lock().await;
                                    routing.insert(client_id.clone(), client_id.clone());
                                }

                                // Drain queued messages for reconnected client
                                {
                                    let queue_manager_drain = Arc::clone(&queue_manager);
                                    let client_id_drain = client_id.clone();
                                    let client_manager_drain = Arc::clone(&client_manager);
                                    tokio::spawn(async move {
                                        let queued_messages = queue_manager_drain.drain_queue(&client_id_drain).await;
                                        if let Ok(messages) = queued_messages && !messages.is_empty() {
                                            info!("Draining {} queued messages for client: {}", messages.len(), client_id_drain);
                                            let mut manager = client_manager_drain.lock().await;
                                            for msg in messages {
                                                if let Err(e) = manager.send_to_client(&client_id_drain, &msg).await {
                                                    warn!("Failed to send drained message to client {}: {}", client_id_drain, e);
                                                    // Re-enqueue if send fails
                                                    let _ = queue_manager_drain.enqueue(&client_id_drain, msg).await;
                                                    break; // Stop draining if send fails
                                                }
                                            }
                                        }
                                    });
                                }

                                // Spawn per-connection task to monitor stream and handle messages
                                let client_id_task = client_id.clone();
                                let client_manager_task = Arc::clone(&client_manager);
                                let direct_routing_task = Arc::clone(&direct_routing);
                                let mut shutdown_rx_task = shutdown_rx.resubscribe();

                                tokio::spawn(async move {
                                    loop {
                                        tokio::select! {
                                            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                                                // Periodic health check
                                                let mut client_guard = client_manager_task.lock().await;
                                                if let Some(managed_client) = client_guard.get_managed_client_mut(&client_id_task) {
                                                    if !managed_client.health_check().await.unwrap_or(false) {
                                                        warn!("Client {} failed health check, removing", client_id_task);
                                                        drop(client_guard);
                                                        break;
                                                    }
                                                } else {
                                                    drop(client_guard);
                                                    break;
                                                }
                                            }
                                            _ = shutdown_rx_task.recv() => {
                                                break;
                                            }
                                        }
                                    }

                                    // Remove client on disconnect
                                    {
                                        let mut client_guard = client_manager_task.lock().await;
                                        if let Err(e) = client_guard.remove_client(&client_id_task).await {
                                            error!("Failed to remove client {}: {}", client_id_task, e);
                                        }
                                    }

                                    // Remove from direct routing
                                    {
                                        let mut routing = direct_routing_task.lock().await;
                                        routing.remove(&client_id_task);
                                    }

                                    info!("Client {} connection task completed", client_id_task);
                                });

                                debug!("Client {} connected and managed", client_id);
                            }
                            Ok(Err(e)) => {
                                error!("Failed to accept client connection: {}", e);
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            }
                            Err(_timeout) => {
                                // Timeout is normal when no clients are connecting
                                // Just continue the loop
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = shutdown_rx.recv() => {
                        info!("Client acceptance task shutting down");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Authenticate a client connection
    async fn authenticate_client(
        transport_client: &mut TransportClient,
        expected_token: &Option<String>,
    ) -> Result<()> {
        if let Some(expected) = expected_token {
            // Read first frame (authentication message)
            let auth_frame = transport_client.receive().await?;

            // Parse and verify token
            let auth_string = String::from_utf8_lossy(&auth_frame);
            if let Some((token_hash, _)) = auth_string.split_once(':') {
                // Verify token hash matches expected
                use blake3::Hasher;
                let mut hasher = Hasher::new();
                hasher.update(expected.as_bytes());
                hasher.update(b"PING");
                let expected_hash = hasher.finalize().to_hex().to_string();

                if token_hash == expected_hash {
                    // Send authentication success
                    transport_client.send(b"PONG").await?;
                    Ok(())
                } else {
                    Err(EventBusError::transport(
                        "Authentication failed: invalid token".to_string(),
                    ))
                }
            } else {
                Err(EventBusError::transport(
                    "Authentication failed: malformed message".to_string(),
                ))
            }
        } else {
            Ok(())
        }
    }

    /// Start health monitoring background task
    async fn start_health_monitoring_task(&self) -> Result<()> {
        let client_manager = Arc::clone(&self.client_manager);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    // Health check interval
                    _ = interval.tick() => {
                        let mut manager = client_manager.lock().await;
                        if let Err(e) = manager.health_check_all().await {
                            error!("Health check failed: {}", e);
                        }
                    }

                    // Handle shutdown signal
                    _ = shutdown_rx.recv() => {
                        info!("Health monitoring task shutting down");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Get the socket configuration
    pub fn config(&self) -> &SocketConfig {
        &self.config
    }

    /// Route a one-to-one message to a specific client
    pub async fn route_one_to_one(&self, client_id: &str, payload: &[u8]) -> Result<()> {
        // Validate payload
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_string(),
            ));
        }

        let transport_client_id = {
            let routing = self.direct_routing.lock().await;
            routing.get(client_id).cloned()
        };
        if let Some(transport_client_id) = transport_client_id {
            let mut client_manager = self.client_manager.lock().await;
            match client_manager
                .send_to_client(&transport_client_id, payload)
                .await
            {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Client disconnected or backpressure - enqueue message
                    warn!(
                        "Failed to send to client {}: {}, enqueueing message",
                        client_id, e
                    );
                    if let Err(queue_err) = self
                        .queue_manager
                        .enqueue(client_id, payload.to_vec())
                        .await
                    {
                        error!(
                            "Failed to enqueue message for client {}: {}",
                            client_id, queue_err
                        );
                    }
                    Err(e)
                }
            }
        } else {
            // Client not found - try to enqueue for later delivery
            warn!(
                "Client {} not found for direct routing, enqueueing message",
                client_id
            );
            if let Err(queue_err) = self
                .queue_manager
                .enqueue(client_id, payload.to_vec())
                .await
            {
                error!(
                    "Failed to enqueue message for client {}: {}",
                    client_id, queue_err
                );
            }
            Err(EventBusError::transport(format!(
                "Client {} not found for direct routing",
                client_id
            )))
        }
    }

    /// Enqueue a message for a client (queue support)
    pub async fn enqueue_for_client(
        &self,
        client_id: &str,
        topic: &str,
        payload: &[u8],
    ) -> Result<()> {
        // Validate payload
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_string(),
            ));
        }

        // Queue topic format: queue.{client_id}.{topic}
        let queue_topic = format!("queue.{}.{}", client_id, topic);
        self.publish(&queue_topic, &Uuid::new_v4().to_string(), payload.to_vec())
            .await
    }

    /// Publish a message to the broker with backpressure control
    pub async fn publish(&self, topic: &str, correlation_id: &str, payload: Vec<u8>) -> Result<()> {
        // Acquire global semaphore for backpressure
        let _permit = self
            .publish_semaphore
            .acquire()
            .await
            .map_err(|_| EventBusError::transport("Publish semaphore closed".to_string()))?;

        // Rate limit check with proper token bucket algorithm
        let client_id = self.extract_client_id_from_topic(topic);
        if !self
            .rate_limiter
            .check_rate_limit(client_id.as_deref(), Some(topic))
            .await
        {
            return Err(EventBusError::transport("Rate limit exceeded".to_string()));
        }
        // Validate payload size
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_string(),
            ));
        }

        // Security audit: log all publishes
        debug!(
            "Publishing message to topic: {} (size: {} bytes)",
            topic,
            payload.len()
        );

        // Check if this is a control message (based on topic prefix)
        if topic.starts_with("control.") {
            return self
                .publish_control_message(topic, correlation_id, payload)
                .await;
        }

        let mut seq_guard = self.sequence.lock().await;
        let sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);

        // Find subscribers for this topic
        let topic_matcher = self.topic_matcher.read().await;
        let subscribers = topic_matcher.find_subscribers(topic)?;
        drop(topic_matcher);

        // Build Message regardless of subscriber type (no CollectionEvent decoding required for routing)
        let message = Message::event(
            topic.to_string(),
            correlation_id.to_string(),
            payload.clone(),
            sequence,
        );

        // Serialize message
        let message_data = message.serialize()?;

        // Send to managed clients via client manager (no CollectionEvent decoding needed)
        let mut delivered_count = 0;
        {
            let mut client_manager = self.client_manager.lock().await;
            match client_manager
                .broadcast_to_topic(topic, &message_data)
                .await
            {
                Ok(delivered_clients) => {
                    delivered_count += delivered_clients.len() as u64;
                    debug!(
                        "Delivered message to {} managed clients",
                        delivered_clients.len()
                    );
                }
                Err(e) => {
                    error!("Failed to broadcast to managed clients: {}", e);
                }
            }

            // Handle failed clients by enqueueing messages
            // Note: broadcast_to_topic already removes failed clients, but we can
            // check for specific client failures and enqueue for them
            // For now, we'll enqueue on individual send_to_client failures in route_one_to_one
        }

        // Send to internal subscribers (only those matching the topic)
        // For internal subscribers, attempt CollectionEvent decoding but handle errors gracefully
        let mut senders_guard = self.subscriber_senders.lock().await;
        let mut failed_senders = Vec::new();

        // Attempt to decode CollectionEvent for internal subscribers
        let collection_event_result: Result<CollectionEvent> =
            postcard::from_bytes(&payload).map_err(|e| EventBusError::serialization(e.to_string()));

        for subscriber_id in &subscribers {
            if let Some(sender) = senders_guard.get(subscriber_id) {
                match &collection_event_result {
                    Ok(collection_event) => {
                        // Successfully decoded - send BusEvent
                        let bus_event = BusEvent {
                            event_id: Uuid::new_v4().to_string(),
                            event: collection_event.clone(),
                            correlation_metadata: message.correlation_metadata.clone(),
                            bus_timestamp: std::time::SystemTime::now(),
                            matched_pattern: topic.to_string(),
                            subscriber_id: subscriber_id.clone(),
                        };

                        if sender.send(bus_event).is_err() {
                            failed_senders.push(subscriber_id.clone());
                        } else {
                            delivered_count += 1;
                        }
                    }
                    Err(e) => {
                        // Decode failed - log and skip this subscriber
                        warn!(
                            "Failed to decode CollectionEvent for subscriber {}: {}. Skipping delivery.",
                            subscriber_id, e
                        );
                        // Could optionally deliver raw message here if needed
                    }
                }
            }
        }

        // Remove failed senders
        for subscriber_id in failed_senders {
            senders_guard.remove(&subscriber_id);
        }

        // Get total subscriber count
        let total_subscribers = {
            let client_manager = self.client_manager.lock().await;
            client_manager.get_stats().total_clients + senders_guard.len()
        };
        drop(senders_guard);

        // Update statistics: increment messages_published exactly once
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
            stats_guard.messages_delivered += delivered_count;
            stats_guard.active_subscribers = total_subscribers;
        }

        debug!(
            "Published message to {} subscribers on topic: {}",
            delivered_count, topic
        );
        Ok(())
    }

    /// Publish a control message without CollectionEvent deserialization
    async fn publish_control_message(
        &self,
        topic: &str,
        correlation_id: &str,
        payload: Vec<u8>,
    ) -> Result<()> {
        let mut seq_guard = self.sequence.lock().await;
        let sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);

        // Find subscribers for this topic
        let topic_matcher = self.topic_matcher.read().await;
        let subscribers = topic_matcher.find_subscribers(topic)?;
        drop(topic_matcher);

        if subscribers.is_empty() {
            debug!("No subscribers for control topic: {}", topic);
            // Update statistics even when no subscribers
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
            return Ok(());
        }

        // Create control message directly without CollectionEvent deserialization
        let message = Message::control(
            topic.to_string(),
            correlation_id.to_string(),
            payload,
            sequence,
        );

        // Serialize message
        let message_data = message.serialize()?;

        // Send to managed clients via client manager
        let mut delivered_count = 0;
        {
            let mut client_manager = self.client_manager.lock().await;
            match client_manager
                .broadcast_to_topic(topic, &message_data)
                .await
            {
                Ok(delivered_clients) => {
                    delivered_count += delivered_clients.len() as u64;
                    debug!(
                        "Delivered control message to {} managed clients",
                        delivered_clients.len()
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to broadcast control message to managed clients: {}",
                        e
                    );
                }
            }
        }

        // Send raw messages to RPC subscribers (only those matching the topic)
        let mut raw_senders_guard = self.raw_subscriber_senders.lock().await;
        let mut raw_failed_senders = Vec::new();
        let mut raw_delivered_count = 0;

        for subscriber_id in &subscribers {
            if let Some(sender) = raw_senders_guard.get(subscriber_id) {
                if sender.send(message.clone()).is_err() {
                    warn!(
                        "Failed to send raw message to subscriber: {}",
                        subscriber_id
                    );
                    raw_failed_senders.push(subscriber_id.clone());
                } else {
                    raw_delivered_count += 1;
                }
            }
        }

        // Remove failed raw senders
        for subscriber_id in raw_failed_senders {
            raw_senders_guard.remove(&subscriber_id);
        }
        drop(raw_senders_guard);

        // Update statistics
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
            stats_guard.messages_delivered += delivered_count + raw_delivered_count;
        }

        debug!(
            "Published control message to {} subscribers on topic: {}",
            delivered_count + raw_delivered_count,
            topic
        );
        Ok(())
    }

    /// Subscribe to a topic pattern
    pub async fn subscribe(&self, pattern: &str, subscriber_id: Uuid) -> Result<()> {
        let mut topic_matcher = self.topic_matcher.write().await;
        topic_matcher.subscribe(pattern, subscriber_id)?;

        debug!("Subscribed {} to pattern: {}", subscriber_id, pattern);
        Ok(())
    }

    /// Subscribe to raw messages (for RPC clients)
    pub async fn subscribe_raw(
        &self,
        pattern: &str,
        subscriber_id: Uuid,
    ) -> Result<mpsc::UnboundedReceiver<Message>> {
        // Subscribe to topic pattern
        let mut topic_matcher = self.topic_matcher.write().await;
        topic_matcher.subscribe(pattern, subscriber_id)?;
        drop(topic_matcher);

        // Create channel for raw messages
        let (tx, rx) = mpsc::unbounded_channel();

        // Store sender for raw message delivery
        let mut raw_senders = self.raw_subscriber_senders.lock().await;
        raw_senders.insert(subscriber_id.to_string(), tx);

        debug!(
            "Subscribed {} to raw messages for pattern: {}",
            subscriber_id, pattern
        );
        Ok(rx)
    }

    /// Unsubscribe from all patterns
    pub async fn unsubscribe(&self, subscriber_id: Uuid) -> Result<()> {
        let mut topic_matcher = self.topic_matcher.write().await;
        topic_matcher.unsubscribe(subscriber_id)?;
        drop(topic_matcher);

        // Remove from both regular and raw subscribers
        let subscriber_str = subscriber_id.to_string();
        {
            let mut senders = self.subscriber_senders.lock().await;
            senders.remove(&subscriber_str);
        }
        {
            let mut raw_senders = self.raw_subscriber_senders.lock().await;
            raw_senders.remove(&subscriber_str);
        }

        debug!("Unsubscribed: {}", subscriber_id);
        Ok(())
    }

    /// Add a client connection with authentication
    pub async fn add_client(&self, client_id: String, socket_config: &SocketConfig) -> Result<()> {
        // Authenticate if enabled
        if self.auth_enabled
            && let Some(ref expected_token) = self.config.auth_token
            && socket_config.auth_token.as_ref() != Some(expected_token)
        {
            return Err(EventBusError::transport(
                "Authentication failed: invalid token".to_string(),
            ));
        }

        let mut client_manager = self.client_manager.lock().await;
        client_manager
            .add_client(client_id.clone(), socket_config)
            .await?;

        // Add to direct routing table
        {
            let mut routing = self.direct_routing.lock().await;
            routing.insert(client_id.clone(), client_id.clone());
        }

        info!(
            "Client connected: {}, total clients: {}",
            client_id,
            client_manager.get_stats().total_clients
        );
        Ok(())
    }

    /// Extract client ID from topic (helper for rate limiting)
    fn extract_client_id_from_topic(&self, topic: &str) -> Option<String> {
        if topic.starts_with("direct.") {
            topic.strip_prefix("direct.").map(|s| s.to_string())
        } else if topic.starts_with("queue.") {
            topic
                .strip_prefix("queue.")
                .and_then(|s| s.split('.').next())
                .map(|s| s.to_string())
        } else {
            None
        }
    }

    /// Remove a client connection
    pub async fn remove_client(&self, client_id: &str) -> Result<()> {
        let mut client_manager = self.client_manager.lock().await;
        client_manager.remove_client(client_id).await?;

        // Remove from direct routing table
        {
            let mut routing = self.direct_routing.lock().await;
            routing.remove(client_id);
        }

        // Remove from rate limiter
        self.rate_limiter.remove_client(client_id).await;

        info!(
            "Client disconnected: {}, remaining clients: {}",
            client_id,
            client_manager.get_stats().total_clients
        );
        Ok(())
    }

    /// Subscribe a client to topic patterns
    pub async fn subscribe_client(
        &self,
        client_id: &str,
        topic_patterns: Vec<String>,
    ) -> Result<()> {
        let mut client_manager = self.client_manager.lock().await;
        client_manager
            .subscribe_client(client_id, topic_patterns)
            .await
    }

    /// Unsubscribe a client from all topics
    pub async fn unsubscribe_client(&self, client_id: &str) -> Result<()> {
        let mut client_manager = self.client_manager.lock().await;
        client_manager.unsubscribe_client(client_id).await
    }

    /// Get current statistics
    pub async fn statistics(&self) -> EventBusStatistics {
        let stats_guard = self.stats.lock().await;
        let topic_matcher = self.topic_matcher.read().await;
        let client_manager = self.client_manager.lock().await;

        EventBusStatistics {
            messages_published: stats_guard.messages_published,
            messages_delivered: stats_guard.messages_delivered,
            active_subscribers: topic_matcher.subscriber_count()
                + client_manager.get_stats().total_clients,
            active_topics: topic_matcher.pattern_count(),
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }
    }

    /// Shutdown the broker
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down DaemonEye broker");

        // Send shutdown signal
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Failed to send shutdown signal: {}", e);
        }

        // Shutdown client manager
        {
            let mut client_manager = self.client_manager.lock().await;
            if let Err(e) = client_manager.shutdown().await {
                error!("Failed to shutdown client manager: {}", e);
            }
        }

        // Shutdown transport server
        {
            let mut server_guard = self.transport_server.lock().await;
            if let Some(mut server) = server_guard.take()
                && let Err(e) = server.shutdown().await
            {
                error!("Failed to shutdown transport server: {}", e);
            }
        }

        info!("DaemonEye broker shutdown complete");
        Ok(())
    }
}

impl std::fmt::Debug for DaemoneyeBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemoneyeBroker")
            .field("start_time", &self.start_time)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// EventBus implementation using the embedded broker
pub struct DaemoneyeEventBus {
    broker: Arc<DaemoneyeBroker>,
    #[allow(dead_code)]
    subscriber_id: Uuid,
    #[allow(dead_code)]
    event_sender: mpsc::UnboundedSender<BusEvent>,
}

impl DaemoneyeEventBus {
    /// Create a new EventBus from a broker
    pub async fn from_broker(broker: DaemoneyeBroker) -> Result<Self> {
        let broker = Arc::new(broker);
        let subscriber_id = Uuid::new_v4();
        let (event_sender, _event_receiver) = mpsc::unbounded_channel();

        let event_bus = Self {
            broker: Arc::clone(&broker),
            subscriber_id,
            event_sender,
        };

        // Start the broker
        event_bus.broker.start().await?;

        Ok(event_bus)
    }

    /// Get the broker reference
    pub fn broker(&self) -> &Arc<DaemoneyeBroker> {
        &self.broker
    }
}

impl EventBus for DaemoneyeEventBus {
    async fn publish(&mut self, event: CollectionEvent, correlation_id: String) -> Result<()> {
        // Determine topic based on event type
        let topic = match &event {
            CollectionEvent::Process(_) => "events.process.new",
            CollectionEvent::Network(_) => "events.network.new",
            CollectionEvent::Filesystem(_) => "events.filesystem.new",
            CollectionEvent::Performance(_) => "events.performance.new",
            CollectionEvent::TriggerRequest(_) => "control.trigger.request",
        };

        // Serialize event to payload
        let payload = postcard::to_allocvec(&event)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        self.broker.publish(topic, &correlation_id, payload).await
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>> {
        // Extract topic patterns from subscription
        let patterns = if let Some(topic_patterns) = &subscription.topic_patterns {
            topic_patterns.clone()
        } else {
            // Generate patterns based on capabilities
            let mut patterns = Vec::new();
            for event_type in &subscription.capabilities.event_types {
                match event_type.as_str() {
                    "process" => patterns.push("events.process.*".to_string()),
                    "network" => patterns.push("events.network.*".to_string()),
                    "filesystem" => patterns.push("events.filesystem.*".to_string()),
                    "performance" => patterns.push("events.performance.*".to_string()),
                    _ => patterns.push(format!("events.{}.*", event_type)),
                }
            }
            patterns
        };

        // Parse subscriber ID from subscription, generate UUID if needed
        let subscriber_id = if subscription.subscriber_id.is_empty() {
            Uuid::new_v4()
        } else {
            subscription
                .subscriber_id
                .parse::<Uuid>()
                .unwrap_or_else(|_| Uuid::new_v4())
        };

        // Subscribe to each pattern using the parsed subscriber ID
        for pattern in patterns {
            self.broker.subscribe(&pattern, subscriber_id).await?;
        }

        // Create a new receiver for this subscription
        let (tx, rx) = mpsc::unbounded_channel();

        // Store the sender for this subscription using the broker-side UUID
        // and maintain the mapping from original subscriber ID to broker UUID
        {
            let mut senders = self.broker.subscriber_senders.lock().await;
            let mut mapping = self.broker.subscriber_id_mapping.lock().await;

            senders.insert(subscriber_id.to_string(), tx);
            mapping.insert(subscription.subscriber_id.clone(), subscriber_id);
        }

        Ok(rx)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        // First try to parse subscriber ID as UUID
        match subscriber_id.parse::<Uuid>() {
            Ok(id) => {
                // Direct UUID lookup - unsubscribe from broker and remove sender
                self.broker.unsubscribe(id).await?;
                let mut senders = self.broker.subscriber_senders.lock().await;
                let mut mapping = self.broker.subscriber_id_mapping.lock().await;

                senders.remove(&id.to_string());
                // Remove from mapping by finding the key that maps to this UUID
                mapping.retain(|_, &mut uuid| uuid != id);
                Ok(())
            }
            Err(_) => {
                // String-based lookup - find the corresponding broker UUID
                let mapping = self.broker.subscriber_id_mapping.lock().await;
                if let Some(&broker_uuid) = mapping.get(subscriber_id) {
                    // Found the mapping, now unsubscribe from broker and clean up
                    drop(mapping); // Release mapping lock before calling broker
                    self.broker.unsubscribe(broker_uuid).await?;

                    let mut senders = self.broker.subscriber_senders.lock().await;
                    let mut mapping = self.broker.subscriber_id_mapping.lock().await;

                    senders.remove(&broker_uuid.to_string());
                    mapping.remove(subscriber_id);
                } else {
                    // No mapping found - this might be a direct UUID string that wasn't parsed
                    // Try to find it in the senders map directly
                    let mut senders = self.broker.subscriber_senders.lock().await;
                    if senders.contains_key(subscriber_id) {
                        // This is a direct string key, remove it
                        senders.remove(subscriber_id);
                    } else {
                        warn!("No subscription found for subscriber ID: {}", subscriber_id);
                    }
                }
                Ok(())
            }
        }
    }

    async fn statistics(&self) -> EventBusStatistics {
        self.broker.statistics().await
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.broker.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{EventSubscription, SourceCaps};

    #[tokio::test]
    async fn test_broker_creation() {
        let broker = DaemoneyeBroker::new("/tmp/test-broker.sock").await.unwrap();
        assert!(broker.start().await.is_ok());
    }

    #[tokio::test]
    async fn test_event_bus_creation() {
        let broker = DaemoneyeBroker::new("/tmp/test-eventbus.sock")
            .await
            .unwrap();
        let event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

        let stats = event_bus.statistics().await;
        assert_eq!(stats.messages_published, 0);
        assert_eq!(stats.active_subscribers, 0);
    }

    #[tokio::test]
    async fn test_topic_subscription() {
        let broker = DaemoneyeBroker::new("/tmp/test-subscription.sock")
            .await
            .unwrap();
        let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

        // Subscribe to a pattern
        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_string(),
            capabilities: SourceCaps {
                event_types: vec!["process".to_string()],
                collectors: vec!["procmond".to_string()],
                max_priority: 5,
            },
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.*".to_string()]),
            enable_wildcards: true,
        };
        let receiver = event_bus.subscribe(subscription).await.unwrap();
        // Verify receiver is open and ready to receive messages
        assert!(
            !receiver.is_closed(),
            "Receiver should not be closed immediately after subscription"
        );

        let stats = event_bus.statistics().await;
        assert_eq!(stats.active_subscribers, 1);
    }

    #[tokio::test]
    async fn test_queue_manager_enqueue_on_disconnect() {
        let broker = DaemoneyeBroker::new_with_config("/tmp/test-queue-enqueue.sock", false, 100)
            .await
            .unwrap();
        assert!(broker.start().await.is_ok());

        // Try to route to non-existent client - should enqueue
        let payload = b"test message".to_vec();
        let result = broker
            .route_one_to_one("non-existent-client", &payload)
            .await;
        assert!(result.is_err()); // Routing fails

        // Verify message was enqueued
        let stats = broker.queue_manager.get_stats("non-existent-client").await;
        assert!(stats.is_some());
        if let Some(queue_stats) = stats {
            assert_eq!(queue_stats.messages_enqueued, 1);
            assert_eq!(queue_stats.current_depth, 1);
        }
    }

    #[tokio::test]
    async fn test_queue_manager_drain_on_reconnect() {
        let broker = DaemoneyeBroker::new_with_config("/tmp/test-queue-drain.sock", false, 100)
            .await
            .unwrap();
        assert!(broker.start().await.is_ok());

        let client_id = "test-client-drain";

        // Enqueue a message for a client that doesn't exist yet
        let payload1 = b"queued message 1".to_vec();
        let _ = broker
            .queue_manager
            .enqueue(client_id, payload1.clone())
            .await;

        let payload2 = b"queued message 2".to_vec();
        let _ = broker
            .queue_manager
            .enqueue(client_id, payload2.clone())
            .await;

        // Verify messages are queued
        let stats = broker.queue_manager.get_stats(client_id).await;
        assert!(stats.is_some());
        if let Some(queue_stats) = stats {
            assert_eq!(queue_stats.messages_enqueued, 2);
            assert_eq!(queue_stats.current_depth, 2);
        }

        // Simulate client connection by draining the queue
        let drained = broker.queue_manager.drain_queue(client_id).await.unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], payload1);
        assert_eq!(drained[1], payload2);

        // Verify queue is now empty
        let stats_after = broker.queue_manager.get_stats(client_id).await;
        if let Some(queue_stats) = stats_after {
            assert_eq!(queue_stats.messages_dequeued, 2);
            assert_eq!(queue_stats.current_depth, 0);
        }
    }
}
