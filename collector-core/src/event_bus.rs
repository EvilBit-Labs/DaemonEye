//! Event bus system for collector coordination and communication.
//!
//! This module provides a unified event bus interface that supports both
//! local in-process communication and distributed communication through
//! message brokers. The event bus enables collectors to publish events,
//! subscribe to event patterns, and coordinate analysis workflows.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Event Bus Interface                         │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  LocalEventBus  │  DistributedEventBus  │  HybridEventBus      │
//! │  (In-Process)   │  (Message Broker)     │  (Combined)          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::{event::CollectionEvent, source::SourceCaps};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

/// Event bus interface for collector coordination
#[async_trait]
pub trait EventBus: Send + Sync {
    /// Publish an event to the bus
    async fn publish(&self, event: CollectionEvent, correlation_id: Option<String>) -> Result<()>;

    /// Subscribe to events matching a pattern
    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>>;

    /// Unsubscribe from events
    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()>;

    /// Get bus statistics
    async fn get_statistics(&self) -> Result<EventBusStatistics>;

    /// Get a reference to the underlying type for downcasting
    fn as_any(&self) -> &dyn std::any::Any;

    /// Shutdown the event bus and perform any necessary cleanup.
    ///
    /// This method should be called when the event bus is no longer needed
    /// to ensure proper resource cleanup and graceful shutdown of any
    /// background tasks or connections.
    ///
    /// # Errors
    ///
    /// Returns an error if shutdown fails or cleanup cannot be completed.
    async fn shutdown(&self) -> Result<()>;
}

/// Event bus configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusConfig {
    /// Maximum number of subscribers
    pub max_subscribers: usize,
    /// Event buffer size
    pub buffer_size: usize,
    /// Enable statistics collection
    pub enable_statistics: bool,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            max_subscribers: 1000,
            buffer_size: 10000,
            enable_statistics: true,
        }
    }
}

/// Event subscription configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    /// Unique subscriber identifier
    pub subscriber_id: String,
    /// Source capabilities
    pub capabilities: SourceCaps,
    /// Event filter
    pub event_filter: Option<EventFilter>,
    /// Correlation filter
    pub correlation_filter: Option<String>,
    /// Topic patterns
    pub topic_patterns: Option<Vec<String>>,
    /// Enable wildcards
    pub enable_wildcards: bool,
}

/// Event filtering criteria for subscribers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    /// Event types
    pub event_types: Vec<String>,
    /// Process IDs
    pub pids: Vec<u32>,
    /// Minimum priority
    pub min_priority: Option<u8>,
    /// Metadata filters
    pub metadata_filters: HashMap<String, String>,
    /// Topic filters
    pub topic_filters: Vec<String>,
    /// Source collectors
    pub source_collectors: Vec<String>,
}

/// Event correlation information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    /// Correlation ID pattern
    pub correlation_id: Option<String>,
    /// Process ID filters
    pub process_ids: Vec<u32>,
}

/// Bus event wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    /// Event ID
    pub id: Uuid,
    /// Event timestamp (Unix timestamp in seconds)
    pub timestamp: u64,
    /// Event payload
    pub event: CollectionEvent,
    /// Correlation ID
    pub correlation_id: Option<String>,
    /// Routing metadata
    pub routing_metadata: HashMap<String, String>,
}

/// Event bus statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusStatistics {
    /// Total events published
    pub events_published: u64,
    /// Total events delivered
    pub events_delivered: u64,
    /// Active subscribers
    pub active_subscribers: usize,
    /// Bus uptime
    pub uptime: Duration,
}

/// Local event bus implementation using in-process channels
pub struct LocalEventBus {
    /// Event bus configuration
    #[allow(dead_code)]
    config: EventBusConfig,
    /// Event publisher
    event_tx: broadcast::Sender<CollectionEvent>,
    /// Subscriber management
    subscribers: Arc<RwLock<HashMap<String, broadcast::Receiver<CollectionEvent>>>>,
    /// Forwarding task handles
    forwarding_tasks: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Statistics
    stats: Arc<Mutex<EventBusStatistics>>,
    /// Start time
    start_time: Instant,
}

impl LocalEventBus {
    /// Create a new local event bus
    pub fn new(config: EventBusConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.buffer_size);

        Self {
            config,
            event_tx,
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            forwarding_tasks: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(Mutex::new(EventBusStatistics {
                events_published: 0,
                events_delivered: 0,
                active_subscribers: 0,
                uptime: Duration::from_secs(0),
            })),
            start_time: Instant::now(),
        }
    }
}

#[async_trait]
impl EventBus for LocalEventBus {
    async fn publish(&self, event: CollectionEvent, _correlation_id: Option<String>) -> Result<()> {
        let _ = self.event_tx.send(event);

        let mut stats = self.stats.lock().await;
        stats.events_published += 1;

        Ok(())
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>> {
        // Create a channel for the subscriber
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Prepare a receiver before returning to avoid a race where publish happens
        // before the forwarding task is actually listening, which would drop events.
        let mut forwarding_rx = self.event_tx.subscribe();
        let subscriber_id = subscription.subscriber_id.clone();

        // Store the receiver for potential future use
        let mut subscribers = self.subscribers.write().await;
        subscribers.insert(subscriber_id.clone(), self.event_tx.subscribe());

        let mut stats = self.stats.lock().await;
        stats.active_subscribers = subscribers.len();

        // Spawn forwarding task that forwards broadcast messages into the receiver-specific channel
        let task_handle = tokio::spawn(async move {
            loop {
                match forwarding_rx.recv().await {
                    Ok(event) => {
                        // Convert CollectionEvent to BusEvent
                        let bus_event = BusEvent {
                            id: Uuid::new_v4(),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                            event,
                            correlation_id: None,
                            routing_metadata: HashMap::new(),
                        };
                        let _ = tx.send(bus_event);
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Log lag and continue
                        tracing::warn!("Subscriber lagged behind, continuing");
                        continue;
                    }
                    Err(RecvError::Closed) => {
                        // Channel closed, terminate task
                        break;
                    }
                }
            }
        });

        // Store the task handle and shutdown sender
        let mut forwarding_tasks = self.forwarding_tasks.write().await;
        forwarding_tasks.insert(subscriber_id.clone(), task_handle);

        Ok(rx)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        // Remove subscriber tracking entry
        let mut subscribers = self.subscribers.write().await;
        subscribers.remove(subscriber_id);
        let active_subscribers = subscribers.len();
        drop(subscribers);

        // Abort the forwarding task so it doesn't continue to process events
        if let Some(task_handle) = self.forwarding_tasks.write().await.remove(subscriber_id) {
            task_handle.abort();
            if let Err(err) = task_handle.await {
                if !err.is_cancelled() {
                    tracing::warn!(
                        subscriber_id = %subscriber_id,
                        error = %err,
                        "Forwarding task ended with error during unsubscribe"
                    );
                }
            }
        }

        let mut stats = self.stats.lock().await;
        stats.active_subscribers = active_subscribers;

        Ok(())
    }

    async fn get_statistics(&self) -> Result<EventBusStatistics> {
        let mut stats = self.stats.lock().await;
        stats.uptime = self.start_time.elapsed();
        Ok(stats.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn shutdown(&self) -> Result<()> {
        // Abort all forwarding tasks
        let forwarding_tasks = self.forwarding_tasks.read().await;
        for task_handle in forwarding_tasks.values() {
            task_handle.abort();
        }
        drop(forwarding_tasks);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;
    use std::time::Duration;
    use std::time::SystemTime;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_local_event_bus() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_string(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["process".to_string()]),
            enable_wildcards: false,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(5678),
            name: "test".to_string(),
            executable_path: Some("/bin/test".to_string()),
            command_line: vec!["test".to_string(), "command".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        bus.publish(event.clone(), None).await.unwrap();

        let received_event = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("timed out waiting for event");

        match received_event {
            Some(received) => {
                assert_eq!(received.event, event);
            }
            None => {
                panic!("Channel was closed - forwarding task may have terminated");
            }
        }
    }
}
