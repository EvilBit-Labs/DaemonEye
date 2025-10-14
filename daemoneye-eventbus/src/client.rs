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
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
    /// Background task handles
    task_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Subscription information
#[derive(Debug, Clone)]
struct SubscriptionInfo {
    /// Topic patterns
    patterns: Vec<String>,
    /// Event sender
    sender: mpsc::UnboundedSender<BusEvent>,
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
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();

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
        mut shutdown_rx: mpsc::UnboundedReceiver<()>,
    ) -> Result<()> {
        // Health monitoring task
        let health_task = {
            let transport = Arc::clone(&self.transport);
            let stats = Arc::clone(&self.stats);
            let config = self.config.clone();
            let client_id = self.client_id.clone();

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
                        _ = shutdown_rx.recv() => {
                            info!("Health monitoring task shutting down for client: {}", client_id);
                            break;
                        }
                    }
                }
            })
        };

        self.task_handles.push(health_task);
        Ok(())
    }

    /// Publish a message to a topic
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
        let payload = bincode::serde::encode_to_vec(&event, bincode::config::standard())
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Create message
        let message = Message::event(topic.to_string(), correlation, payload, sequence);
        let message_data = message.serialize()?;

        // Send message
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
            topic, message.correlation_id
        );
        Ok(())
    }

    /// Subscribe to topic patterns
    pub async fn subscribe(
        &self,
        subscription: EventSubscription,
    ) -> Result<mpsc::UnboundedReceiver<BusEvent>> {
        let subscription_id = subscription.subscriber_id.clone();
        let patterns = subscription.topic_patterns.clone().unwrap_or_default();

        // Validate topic patterns
        for pattern in &patterns {
            TopicPattern::new(pattern).map_err(|e| {
                EventBusError::topic(format!("Invalid topic pattern '{}': {}", pattern, e))
            })?;
        }

        // Create channel for events
        let (tx, rx) = mpsc::unbounded_channel();

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

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.messages_received += 1;
        }

        Ok(())
    }

    /// Handle event messages
    async fn handle_event_message(&self, message: Message) -> Result<()> {
        // Deserialize event
        let event: CollectionEvent =
            bincode::serde::decode_from_slice(&message.payload, bincode::config::standard())
                .map_err(|e| EventBusError::serialization(e.to_string()))?
                .0;

        // Create bus event
        let bus_event = BusEvent {
            event_id: message.id.to_string(),
            event,
            correlation_id: message.correlation_id.clone(),
            bus_timestamp: message.timestamp,
            matched_pattern: message.topic.clone(),
            subscriber_id: "".to_string(), // Will be set per subscription
        };

        // Find matching subscriptions
        let subscriptions = self.subscriptions.read().await;
        let mut delivered_count = 0;

        for (subscription_id, subscription_info) in subscriptions.iter() {
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

                if subscription_info.sender.send(event_copy).is_ok() {
                    delivered_count += 1;
                } else {
                    warn!(
                        "Failed to deliver message to subscription: {}",
                        subscription_id
                    );
                }
            }
        }

        debug!("Delivered message to {} subscriptions", delivered_count);
        Ok(())
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
