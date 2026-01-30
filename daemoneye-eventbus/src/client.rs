//! EventBus client implementation with topic management and reconnection logic.
//!
//! This module provides a high-level client interface for connecting to the
//! daemoneye-eventbus broker with automatic reconnection, health monitoring,
//! and topic-based subscription management.

use crate::{
    error::{EventBusError, Result},
    message::{BusEvent, CollectionEvent, EventSubscription, Message, MessageType},
    topic::{Topic, TopicPattern},
    transport::{ClientConfig, SocketConfig, TransportClient},
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// High-level EventBus client with topic management and reconnection
pub struct EventBusClient {
    /// Client identifier
    client_id: String,
    /// Transport client
    transport: Arc<Mutex<TransportClient>>,
    /// Client configuration
    config: ClientConfig,
    /// Socket configuration (stored for reconnection)
    #[allow(dead_code)]
    socket_config: SocketConfig,
    /// Active subscriptions
    subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
    /// Message sequence counter
    sequence: Arc<Mutex<u64>>,
    /// Client statistics
    stats: Arc<Mutex<ClientStats>>,
    /// Shutdown signal
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Background task handles
    task_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Subscription information
#[derive(Debug, Clone)]
struct SubscriptionInfo {
    /// Topic patterns
    patterns: Vec<String>,
    /// Event sender (bounded for backpressure)
    sender: mpsc::Sender<BusEvent>,
    /// Subscription timestamp (for future metrics)
    #[allow(dead_code)]
    created_at: Instant,
    /// Last message received (for future metrics)
    #[allow(dead_code)]
    last_message: Option<Instant>,
}

/// Client statistics
#[derive(Debug, Clone, Default)]
pub struct ClientStats {
    /// Messages published
    pub messages_published: u64,
    /// Messages received
    pub messages_received: u64,
    /// Subscription count
    pub active_subscriptions: usize,
    /// Connection uptime
    pub uptime: Duration,
    /// Reconnection attempts
    pub reconnection_attempts: u64,
    /// Last successful connection
    pub last_connected: Option<Instant>,
    /// Health check failures
    pub health_check_failures: u64,
}

impl EventBusClient {
    /// Create a new EventBus client
    pub async fn new(
        client_id: String,
        socket_config: SocketConfig,
        config: ClientConfig,
    ) -> Result<Self> {
        info!("Creating EventBus client: {}", client_id);

        let transport =
            TransportClient::connect_with_config(&socket_config, config.clone()).await?;
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        let shutdown_rx = shutdown_tx.subscribe();

        let mut client = Self {
            client_id: client_id.clone(),
            transport: Arc::new(Mutex::new(transport)),
            config,
            socket_config,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(ClientStats {
                last_connected: Some(Instant::now()),
                ..Default::default()
            })),
            shutdown_tx: Some(shutdown_tx),
            task_handles: Vec::new(),
        };

        // Start background tasks
        client.start_background_tasks(shutdown_rx).await?;

        info!("EventBus client created: {}", client_id);
        Ok(client)
    }

    /// Start background tasks for health monitoring and message processing
    async fn start_background_tasks(
        &mut self,
        shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<()> {
        // Health monitoring task
        let health_task = {
            let transport = Arc::clone(&self.transport);
            let stats = Arc::clone(&self.stats);
            let config = self.config.clone();
            let client_id = self.client_id.clone();
            let mut shutdown_rx_health = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(config.health_check_interval);
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let mut transport_guard = transport.lock().await;
                            match transport_guard.health_check().await {
                                Ok(true) => {
                                    debug!("Health check passed for client: {}", client_id);
                                }
                                Ok(false) => {
                                    warn!("Health check failed for client: {}", client_id);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.health_check_failures += 1;
                                }
                                Err(e) => {
                                    error!("Health check error for client {}: {}", client_id, e);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.health_check_failures += 1;
                                }
                            }
                        }
                        _ = shutdown_rx_health.recv() => {
                            info!("Health monitoring task shutting down for client: {}", client_id);
                            break;
                        }
                    }
                }
            })
        };

        // Message processing task - continuously calls process_messages()
        let message_task = {
            let transport = Arc::clone(&self.transport);
            let subscriptions = Arc::clone(&self.subscriptions);
            let stats = Arc::clone(&self.stats);
            let client_id = self.client_id.clone();
            let mut shutdown_rx_msg = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(10));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            // Try to receive a message (non-blocking with timeout)
                            let message_result = {
                                let mut transport_guard = transport.lock().await;
                                tokio::time::timeout(
                                    tokio::time::Duration::from_millis(100),
                                    transport_guard.receive()
                                ).await
                            };

                            match message_result {
                                Ok(Ok(message_data)) => {
                                    // Deserialize and process message
                                    match crate::message::Message::deserialize(&message_data) {
                                        Ok(message) => {
                                            // Handle different message types
                                            match message.message_type {
                                                crate::message::MessageType::Event => {
                                                    // Process event message (stats incremented in handle_event_message_internal)
                                                    if let Err(e) = Self::handle_event_message_internal(
                                                        &subscriptions,
                                                        &stats,
                                                        &message,
                                                        &client_id
                                                    ).await {
                                                        debug!("Error handling event message for client {}: {}", client_id, e);
                                                    }
                                                }
                                                crate::message::MessageType::Control => {
                                                    // Update statistics for control messages
                                                    {
                                                        let mut stats_guard = stats.lock().await;
                                                        stats_guard.messages_received += 1;
                                                    }
                                                    debug!("Received control message for client: {}", client_id);
                                                }
                                                crate::message::MessageType::Heartbeat => {
                                                    // Update statistics for heartbeat messages
                                                    {
                                                        let mut stats_guard = stats.lock().await;
                                                        stats_guard.messages_received += 1;
                                                    }
                                                    debug!("Received heartbeat for client: {}", client_id);
                                                }
                                                crate::message::MessageType::Shutdown => {
                                                    // Update statistics before handling shutdown (to count shutdown messages)
                                                    {
                                                        let mut stats_guard = stats.lock().await;
                                                        stats_guard.messages_received += 1;
                                                    }
                                                    info!("Received shutdown message for client: {}", client_id);
                                                    break;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            debug!("Failed to deserialize message for client {}: {}", client_id, e);
                                        }
                                    }
                                }
                                Ok(Err(EventBusError::Transport(msg))) if msg.contains("Server shutdown") => {
                                    info!("Server shutdown detected for client: {}", client_id);
                                    break;
                                }
                                Ok(Err(e)) => {
                                    // Log error but continue processing
                                    debug!("Error receiving message for client {}: {}", client_id, e);
                                    // Yield to avoid busy looping on errors
                                    tokio::task::yield_now().await;
                                }
                                Err(_) => {
                                    // Timeout - no message available, continue loop
                                }
                            }
                        }
                        _ = shutdown_rx_msg.recv() => {
                            info!("Message processing task shutting down for client: {}", client_id);
                            break;
                        }
                    }
                }
            })
        };

        self.task_handles.push(health_task);
        self.task_handles.push(message_task);
        Ok(())
    }

    /// Publish a message to a topic with backpressure control
    pub async fn publish(
        &self,
        topic: &str,
        event: CollectionEvent,
        correlation_id: Option<String>,
    ) -> Result<()> {
        // Validate topic
        let _topic_obj = Topic::new(topic)
            .map_err(|e| EventBusError::topic(format!("Invalid topic '{}': {}", topic, e)))?;

        let correlation = correlation_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        // Get next sequence number
        let sequence = {
            let mut seq_guard = self.sequence.lock().await;
            let seq = *seq_guard;
            *seq_guard += 1;
            seq
        };

        // Serialize event
        let payload = postcard::to_allocvec(&event)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Validate payload size
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::serialization(
                "Payload exceeds 1MB limit".to_string(),
            ));
        }

        // Create message
        let message = Message::event(topic.to_string(), correlation, payload, sequence);
        let message_data = message.serialize()?;

        // Acquire permit for backpressure before sending
        {
            let transport = self.transport.lock().await;
            let _permit = transport.acquire_permit().await?;
            drop(_permit);
            drop(transport);
        }
        {
            let mut transport = self.transport.lock().await;
            transport.send(&message_data).await?;
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.messages_published += 1;
        }

        debug!(
            "Published message to topic: {} (correlation: {})",
            topic, message.correlation_metadata.correlation_id
        );
        Ok(())
    }

    /// Send a direct one-to-one message to a specific client
    pub async fn send_direct(&self, client_id: &str, payload: &[u8]) -> Result<()> {
        // Validate payload
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_string(),
            ));
        }

        // Route via broker control topic
        let topic = format!("control.direct.{}", client_id);
        let correlation_id = Uuid::new_v4().to_string();

        // Serialize message for broker routing
        let message =
            crate::message::Message::control(topic.clone(), correlation_id, payload.to_vec(), 0);
        let message_data = message.serialize()?;

        // Acquire permit for backpressure
        {
            let transport = self.transport.lock().await;
            let _permit = transport.acquire_permit().await?;
            drop(_permit);
            drop(transport);
        }
        {
            let mut transport = self.transport.lock().await;
            transport.send(&message_data).await?;
        }

        debug!("Sent direct message to client: {} via broker", client_id);
        Ok(())
    }

    /// Enqueue a message for a client (queue support)
    pub async fn enqueue_message(
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

        // Route via broker queue topic: queue.{client_id}.{topic}
        let queue_topic = format!("queue.{}.{}", client_id, topic);
        let correlation_id = Uuid::new_v4().to_string();

        // Serialize message for broker routing
        let message = crate::message::Message::control(
            queue_topic.clone(),
            correlation_id,
            payload.to_vec(),
            0,
        );
        let message_data = message.serialize()?;

        // Acquire permit
        {
            let transport = self.transport.lock().await;
            let _permit = transport.acquire_permit().await?;
            drop(_permit);
            drop(transport);
        }
        {
            let mut transport = self.transport.lock().await;
            transport.send(&message_data).await?;
        }

        debug!(
            "Enqueued message for client: {} on topic: {} via broker",
            client_id, topic
        );
        Ok(())
    }

    /// Subscribe to topic patterns with backpressure support
    pub async fn subscribe(
        &self,
        subscription: EventSubscription,
    ) -> Result<mpsc::Receiver<BusEvent>> {
        let subscription_id = subscription.subscriber_id.clone();
        let patterns = subscription.topic_patterns.clone().unwrap_or_default();

        // Validate topic patterns
        for pattern in &patterns {
            TopicPattern::new(pattern).map_err(|e| {
                EventBusError::topic(format!("Invalid topic pattern '{}': {}", pattern, e))
            })?;
        }

        // Create bounded channel for backpressure (default: 1000 messages)
        let (tx, rx) = mpsc::channel(1000);

        // Store subscription info
        let subscription_info = SubscriptionInfo {
            patterns: patterns.clone(),
            sender: tx,
            created_at: Instant::now(),
            last_message: None,
        };

        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(subscription_id.clone(), subscription_info);
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.active_subscriptions = {
                let subscriptions = self.subscriptions.read().await;
                subscriptions.len()
            };
        }

        info!(
            "Subscribed to patterns: {:?} (subscription: {})",
            patterns, subscription_id
        );
        Ok(rx)
    }

    /// Unsubscribe from topics
    pub async fn unsubscribe(&self, subscription_id: &str) -> Result<()> {
        let removed = {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.remove(subscription_id).is_some()
        };

        if removed {
            // Update statistics
            let mut stats = self.stats.lock().await;
            stats.active_subscriptions = {
                let subscriptions = self.subscriptions.read().await;
                subscriptions.len()
            };

            info!("Unsubscribed: {}", subscription_id);
            Ok(())
        } else {
            Err(EventBusError::topic(format!(
                "Subscription not found: {}",
                subscription_id
            )))
        }
    }

    /// Process incoming messages (should be called in a loop)
    pub async fn process_messages(&self) -> Result<()> {
        let message_data = {
            let mut transport = self.transport.lock().await;
            transport.receive().await?
        };

        // Deserialize message
        let message = Message::deserialize(&message_data)?;

        // Update statistics before handling message (to count shutdown messages)
        {
            let mut stats = self.stats.lock().await;
            stats.messages_received += 1;
        }

        // Handle different message types
        match message.message_type {
            MessageType::Event => {
                self.handle_event_message(message).await?;
            }
            MessageType::Control => {
                self.handle_control_message(message).await?;
            }
            MessageType::Heartbeat => {
                debug!("Received heartbeat message");
            }
            MessageType::Shutdown => {
                info!("Received shutdown message");
                return Err(EventBusError::transport("Server shutdown"));
            }
        }

        Ok(())
    }

    /// Handle event messages (internal helper for background task)
    async fn handle_event_message_internal(
        subscriptions: &Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
        stats: &Arc<Mutex<ClientStats>>,
        message: &Message,
        _client_id: &str,
    ) -> Result<()> {
        // Deserialize event
        let event: CollectionEvent = postcard::from_bytes(&message.payload)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Create bus event
        let bus_event = BusEvent {
            event_id: message.id.to_string(),
            event,
            correlation_metadata: message.correlation_metadata.clone(),
            bus_timestamp: message.timestamp,
            matched_pattern: message.topic.clone(),
            subscriber_id: "".to_string(), // Will be set per subscription
        };

        // Find matching subscriptions
        let subscriptions_guard = subscriptions.read().await;
        let mut delivered_count = 0;

        for (subscription_id, subscription_info) in subscriptions_guard.iter() {
            // Check if any pattern matches the topic
            let matches = subscription_info.patterns.iter().any(|pattern| {
                if let Ok(topic_pattern) = TopicPattern::new(pattern) {
                    if let Ok(topic_obj) = Topic::new(&message.topic) {
                        topic_pattern.matches(&topic_obj)
                    } else {
                        false
                    }
                } else {
                    false
                }
            });

            if matches {
                let mut event_copy = bus_event.clone();
                event_copy.subscriber_id = subscription_id.clone();

                match subscription_info.sender.try_send(event_copy) {
                    Ok(()) => {
                        delivered_count += 1;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            "Subscription {} queue full, message dropped",
                            subscription_id
                        );
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        warn!(
                            "Subscription {} closed, removing from active subscriptions",
                            subscription_id
                        );
                    }
                }
            }
        }
        drop(subscriptions_guard);

        // Update statistics
        {
            let mut stats_guard = stats.lock().await;
            stats_guard.messages_received += 1;
        }

        debug!("Delivered message to {} subscriptions", delivered_count);
        Ok(())
    }

    /// Handle event messages
    async fn handle_event_message(&self, message: Message) -> Result<()> {
        Self::handle_event_message_internal(
            &self.subscriptions,
            &self.stats,
            &message,
            &self.client_id,
        )
        .await
    }

    /// Handle control messages
    async fn handle_control_message(&self, message: Message) -> Result<()> {
        debug!("Received control message: {}", message.topic);
        // Control message handling can be extended as needed
        Ok(())
    }

    /// Reconnect to the broker
    pub async fn reconnect(&self) -> Result<()> {
        info!("Reconnecting client: {}", self.client_id);

        {
            let mut transport = self.transport.lock().await;
            transport.reconnect().await?;
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.reconnection_attempts += 1;
            stats.last_connected = Some(Instant::now());
        }

        info!("Successfully reconnected client: {}", self.client_id);
        Ok(())
    }

    /// Get client statistics
    pub async fn get_stats(&self) -> ClientStats {
        let mut stats = self.stats.lock().await;

        // Update uptime
        if let Some(last_connected) = stats.last_connected {
            stats.uptime = last_connected.elapsed();
        }

        // Update subscription count
        stats.active_subscriptions = {
            let subscriptions = self.subscriptions.read().await;
            subscriptions.len()
        };

        stats.clone()
    }

    /// Get client ID
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Check if client is connected
    pub async fn is_connected(&self) -> bool {
        let mut transport = self.transport.lock().await;
        transport.is_alive().await
    }

    /// Shutdown the client
    pub async fn shutdown(mut self) -> Result<()> {
        info!("Shutting down EventBus client: {}", self.client_id);

        // Send shutdown signal to background tasks
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for background tasks to complete
        for handle in self.task_handles {
            let _ = handle.await;
        }

        // Close transport connection
        {
            let _transport = self.transport.lock().await;
            // Transport will be closed when dropped
        }

        info!("EventBus client shutdown complete: {}", self.client_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ProcessEvent;
    use crate::transport::TransportServer;
    use std::time::SystemTime;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_client_creation() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-client.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let result = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await;

        // Should succeed with real server
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_topic_validation() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-validation.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        // Test invalid topic
        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test".to_string(),
            command_line: None,
            executable_path: None,
            ppid: None,
            start_time: Some(SystemTime::now()),
            metadata: std::collections::HashMap::new(),
        });

        let result = client.publish("invalid..topic", event, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_subscription_management() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-subscription.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        // Test subscription with invalid pattern
        let subscription = EventSubscription {
            subscriber_id: "test-sub".to_string(),
            capabilities: crate::message::SourceCaps {
                event_types: vec!["process".to_string()],
                collectors: vec![],
                max_priority: 5,
            },
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#.invalid".to_string()]),
            enable_wildcards: true,
        };

        let result = client.subscribe(subscription).await;
        assert!(result.is_err());
    }
}
