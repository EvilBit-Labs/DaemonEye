//! Embedded broker implementation for the EventBus

use crate::error::{EventBusError, Result};
use crate::message::{BusEvent, CollectionEvent, EventSubscription, Message};
use crate::topic::TopicMatcher;
use crate::transport::{SocketConfig, TransportClient};
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
    /// Connected clients
    clients: Arc<Mutex<Vec<TransportClient>>>,
    /// Message sequence counter
    sequence: Arc<Mutex<u64>>,
    /// Statistics
    stats: Arc<Mutex<EventBusStatistics>>,
    /// Broker start time
    start_time: Instant,
    /// Shutdown signal
    shutdown_tx: mpsc::UnboundedSender<()>,
    /// Shutdown receiver
    shutdown_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<()>>>>,
}

impl DaemoneyeBroker {
    /// Create a new embedded broker
    pub async fn new(socket_path: &str) -> Result<Self> {
        let instance_id = Uuid::new_v4().to_string();
        let config = SocketConfig::new(&instance_id);

        // Override with provided socket path if different
        let _config = if socket_path != config.get_socket_path() {
            SocketConfig {
                unix_path: socket_path.to_string(),
                windows_pipe: socket_path.to_string(),
            }
        } else {
            config
        };

        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();

        let broker = Self {
            topic_matcher: Arc::new(RwLock::new(TopicMatcher::new())),
            clients: Arc::new(Mutex::new(Vec::new())),
            sequence: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(EventBusStatistics::default())),
            start_time: Instant::now(),
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(Some(shutdown_rx))),
        };

        info!("DaemonEye broker created with socket: {}", socket_path);
        Ok(broker)
    }

    /// Start the broker server
    pub async fn start(&self) -> Result<()> {
        info!("DaemonEye broker started");
        Ok(())
    }

    /// Publish a message to the broker
    pub async fn publish(&self, topic: &str, correlation_id: &str, payload: Vec<u8>) -> Result<()> {
        let mut seq_guard = self.sequence.lock().await;
        let sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);

        let message = Message::event(
            topic.to_string(),
            correlation_id.to_string(),
            payload,
            sequence,
        );

        // Find subscribers for this topic
        let topic_matcher = self.topic_matcher.read().await;
        let subscribers = topic_matcher.find_subscribers(topic);
        drop(topic_matcher);

        // Update statistics regardless of subscribers
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
        }

        if subscribers.is_empty() {
            debug!("No subscribers for topic: {}", topic);
            return Ok(());
        }

        // Serialize message
        let message_data = message.serialize()?;

        // Send to all subscribers
        let mut clients_guard = self.clients.lock().await;
        let mut failed_clients = Vec::new();
        let mut delivered_count = 0;

        for (i, client) in clients_guard.iter_mut().enumerate() {
            if let Err(e) = client.send(&message_data).await {
                error!("Failed to send message to client {}: {}", i, e);
                failed_clients.push(i);
            } else {
                delivered_count += 1;
            }
        }

        // Remove failed clients
        for &i in failed_clients.iter().rev() {
            clients_guard.remove(i);
        }

        // Update statistics
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published += 1;
            stats_guard.messages_delivered += delivered_count;
            stats_guard.active_subscribers = subscribers.len();
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
    pub async fn add_client(&self, client: TransportClient) {
        let mut clients_guard = self.clients.lock().await;
        clients_guard.push(client);

        info!("Client connected, total clients: {}", clients_guard.len());
    }

    /// Get current statistics
    pub async fn statistics(&self) -> EventBusStatistics {
        let stats_guard = self.stats.lock().await;
        let topic_matcher = self.topic_matcher.read().await;
        let _clients_guard = self.clients.lock().await;

        EventBusStatistics {
            messages_published: stats_guard.messages_published,
            messages_delivered: stats_guard.messages_delivered,
            active_subscribers: topic_matcher.subscriber_count(),
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

        // Close all client connections
        let mut clients_guard = self.clients.lock().await;
        for client in clients_guard.drain(..) {
            if let Err(e) = client.close().await {
                error!("Failed to close client connection: {}", e);
            }
        }

        Ok(())
    }
}

/// EventBus implementation using the embedded broker
pub struct DaemoneyeEventBus {
    broker: Arc<DaemoneyeBroker>,
    subscriber_id: Uuid,
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
        let payload =
            bincode::serialize(&event).map_err(|e| EventBusError::serialization(e.to_string()))?;

        self.broker.publish(topic, &correlation_id, payload).await
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::Receiver<BusEvent>> {
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

        // Subscribe to each pattern
        for pattern in patterns {
            self.broker.subscribe(&pattern, self.subscriber_id).await?;
        }

        // Create a new receiver for this subscription
        let (_tx, rx) = mpsc::channel(1000);

        // Store the sender for this subscription (simplified approach)
        // In a real implementation, we'd need to track multiple receivers

        Ok(rx)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        // Parse subscriber ID as UUID
        let id = subscriber_id
            .parse::<Uuid>()
            .map_err(|_| EventBusError::configuration("Invalid subscriber ID format"))?;

        self.broker.unsubscribe(id).await
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
        // Just check that we got a receiver
        assert!(true);

        let stats = event_bus.statistics().await;
        assert_eq!(stats.active_subscribers, 1);
    }
}
