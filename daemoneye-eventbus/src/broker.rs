//! Embedded broker implementation for the EventBus

use crate::error::{EventBusError, Result};
use crate::message::{BusEvent, CollectionEvent, EventSubscription, Message};
use crate::topic::TopicMatcher;
use crate::transport::{ClientConfig, ClientConnectionManager, SocketConfig, TransportServer};
use crate::{EventBus, EventBusStatistics};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, mpsc};
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
    /// Shutdown signal
    shutdown_tx: mpsc::UnboundedSender<()>,
    /// Shutdown receiver
    #[allow(dead_code)]
    shutdown_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<()>>>>,
    /// Socket configuration
    config: SocketConfig,
    /// Subscriber senders for message delivery
    subscriber_senders:
        Arc<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<BusEvent>>>>,
    /// Reverse mapping from original subscriber IDs to broker UUIDs
    subscriber_id_mapping: Arc<Mutex<std::collections::HashMap<String, Uuid>>>,
}

impl DaemoneyeBroker {
    /// Create a new embedded broker
    pub async fn new(socket_path: &str) -> Result<Self> {
        let instance_id = Uuid::new_v4().to_string();
        let config = SocketConfig::new(&instance_id);

        // Override with provided socket path if different
        let config = if socket_path != config.get_socket_path() {
            SocketConfig {
                unix_path: socket_path.to_string(),
                windows_pipe: socket_path.to_string(),
            }
        } else {
            config
        };

        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();

        // Create client connection manager
        let client_config = ClientConfig::default();
        let client_manager = ClientConnectionManager::new(client_config);

        let broker = Self {
            topic_matcher: Arc::new(RwLock::new(TopicMatcher::new())),
            client_manager: Arc::new(Mutex::new(client_manager)),
            transport_server: Arc::new(Mutex::new(None)),
            sequence: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(EventBusStatistics::default())),
            start_time: Instant::now(),
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(Some(shutdown_rx))),
            config,
            subscriber_senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
            subscriber_id_mapping: Arc::new(Mutex::new(std::collections::HashMap::new())),
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
        let _client_manager = Arc::clone(&self.client_manager);
        let _socket_config = self.config.clone();

        tokio::spawn(async move {
            loop {
                let server_guard = server.lock().await;
                if let Some(ref transport_server) = *server_guard {
                    match transport_server.accept().await {
                        Ok(_client) => {
                            let client_id = Uuid::new_v4().to_string();
                            info!("Accepted new client connection: {}", client_id);

                            // Add client to manager (this will fail since we need to refactor the manager)
                            // For now, we'll just log the connection
                            debug!("Client {} connected but not yet managed", client_id);
                        }
                        Err(e) => {
                            error!("Failed to accept client connection: {}", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    }
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });

        Ok(())
    }

    /// Start health monitoring background task
    async fn start_health_monitoring_task(&self) -> Result<()> {
        let client_manager = Arc::clone(&self.client_manager);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;

                let mut manager = client_manager.lock().await;
                if let Err(e) = manager.health_check_all().await {
                    error!("Health check failed: {}", e);
                }
            }
        });

        Ok(())
    }

    /// Get the socket configuration
    pub fn config(&self) -> &SocketConfig {
        &self.config
    }

    /// Publish a message to the broker
    pub async fn publish(&self, topic: &str, correlation_id: &str, payload: Vec<u8>) -> Result<()> {
        let mut seq_guard = self.sequence.lock().await;
        let sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);

        // Find subscribers for this topic
        let topic_matcher = self.topic_matcher.read().await;
        let subscribers = topic_matcher.find_subscribers(topic)?;
        drop(topic_matcher);

        if subscribers.is_empty() {
            debug!("No subscribers for topic: {}", topic);
            // Update statistics even when no subscribers
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
            return Ok(());
        }

        // Deserialize payload to CollectionEvent before creating message
        let collection_event: CollectionEvent =
            bincode::serde::decode_from_slice(&payload, bincode::config::standard())
                .map_err(|e| EventBusError::serialization(e.to_string()))?
                .0;

        let message = Message::event(
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
                        "Delivered message to {} managed clients",
                        delivered_clients.len()
                    );
                }
                Err(e) => {
                    error!("Failed to broadcast to managed clients: {}", e);
                }
            }
        }

        // Send to internal subscribers
        let mut senders_guard = self.subscriber_senders.lock().await;
        let mut failed_senders = Vec::new();

        for (subscriber_id, sender) in senders_guard.iter() {
            let bus_event = BusEvent {
                event_id: Uuid::new_v4().to_string(),
                event: collection_event.clone(),
                correlation_id: correlation_id.to_string(),
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

    /// Subscribe to a topic pattern
    pub async fn subscribe(&self, pattern: &str, subscriber_id: Uuid) -> Result<()> {
        let mut topic_matcher = self.topic_matcher.write().await;
        topic_matcher.subscribe(pattern, subscriber_id)?;

        debug!("Subscribed {} to pattern: {}", subscriber_id, pattern);
        Ok(())
    }

    /// Unsubscribe from all patterns
    pub async fn unsubscribe(&self, subscriber_id: Uuid) -> Result<()> {
        let mut topic_matcher = self.topic_matcher.write().await;
        topic_matcher.unsubscribe(subscriber_id)?;

        debug!("Unsubscribed: {}", subscriber_id);
        Ok(())
    }

    /// Add a client connection
    pub async fn add_client(&self, client_id: String, socket_config: &SocketConfig) -> Result<()> {
        let mut client_manager = self.client_manager.lock().await;
        client_manager
            .add_client(client_id.clone(), socket_config)
            .await?;

        info!(
            "Client connected: {}, total clients: {}",
            client_id,
            client_manager.get_stats().total_clients
        );
        Ok(())
    }

    /// Remove a client connection
    pub async fn remove_client(&self, client_id: &str) -> Result<()> {
        let mut client_manager = self.client_manager.lock().await;
        client_manager.remove_client(client_id).await?;

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
        let payload = bincode::serde::encode_to_vec(&event, bincode::config::standard())
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
}
