//! High-performance event bus implementation using crossbeam channels.
//!
//! This module provides a lock-free, high-throughput event bus that can handle
//! millions of events per second with minimal latency using crossbeam's optimized
//! lock-free channels and proper synchronization primitives.

use crate::{event::CollectionEvent, source::SourceCaps};
use anyhow::Result;
use async_trait::async_trait;
use crossbeam::{
    channel::{Receiver, Sender, bounded, unbounded},
    utils::Backoff,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::SystemTime,
};
use tracing::{info, warn};
use uuid::Uuid;

/// High-performance event bus configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighPerformanceEventBusConfig {
    /// Maximum number of events in the broadcast channel buffer
    pub channel_capacity: usize,
    /// Maximum number of concurrent subscribers
    pub max_subscribers: usize,
    /// Backpressure strategy when subscribers are slow
    pub backpressure_strategy: BackpressureStrategy,
    /// Enable event persistence for replay
    pub enable_persistence: bool,
    /// Maximum number of persisted events
    pub max_persisted_events: usize,
    /// Event correlation tracking
    pub enable_correlation_tracking: bool,
}

impl Default for HighPerformanceEventBusConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 1024 * 1024, // 1M events
            max_subscribers: 1000,
            backpressure_strategy: BackpressureStrategy::Blocking,
            enable_persistence: false,
            max_persisted_events: 10000,
            enable_correlation_tracking: true,
        }
    }
}

/// Backpressure handling strategies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackpressureStrategy {
    /// Block publishers when ring buffer is full
    Blocking,
    /// Drop oldest events when ring buffer is full
    DropOldest,
    /// Drop newest events when ring buffer is full
    DropNewest,
}

/// Event bus trait for high-performance inter-collector communication.
#[async_trait]
pub trait HighPerformanceEventBus: Send + Sync {
    /// Publish an event to the bus.
    async fn publish(&self, event: CollectionEvent, correlation_id: String) -> Result<()>;

    /// Subscribe to events with the given capabilities.
    async fn subscribe(&self, subscription: EventSubscription) -> Result<Receiver<BusEvent>>;

    /// Get current bus statistics.
    async fn get_statistics(&self) -> EventBusStatistics;

    /// Shutdown the event bus.
    async fn shutdown(&mut self) -> Result<()>;
}

/// Event subscription configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    /// Unique identifier for the subscriber
    pub subscriber_id: String,
    /// Capabilities this subscriber is interested in
    pub capabilities: SourceCaps,
    /// Optional event filtering criteria
    pub event_filter: Option<EventFilter>,
    /// Optional correlation ID filtering
    pub correlation_filter: Option<String>,
}

/// Event filtering criteria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    /// Event types to include
    pub event_types: Vec<String>,
    /// Minimum priority level
    pub min_priority: Option<u8>,
    /// Source filtering
    pub source_filter: Option<String>,
}

/// Bus event wrapper with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    /// Unique event identifier
    pub event_id: String,
    /// The actual collection event
    pub event: CollectionEvent,
    /// Correlation ID for event chaining
    pub correlation_id: String,
    /// Publisher identifier
    pub publisher_id: String,
    /// Event timestamp
    pub timestamp: SystemTime,
    /// Event priority (higher = more important)
    pub priority: u8,
}

/// Event bus statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusStatistics {
    /// Total events published
    pub events_published: u64,
    /// Total events delivered to subscribers
    pub events_delivered: u64,
    /// Total events dropped due to backpressure
    pub events_dropped: u64,
    /// Current number of active subscribers
    pub active_subscribers: usize,
    /// Average delivery latency in milliseconds
    pub avg_delivery_latency_ms: f64,
    /// Current memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Last statistics update time
    pub last_updated: SystemTime,
}

/// High-performance event bus implementation using crossbeam channels.
pub struct HighPerformanceEventBusImpl {
    config: HighPerformanceEventBusConfig,
    publisher: Sender<BusEvent>,
    subscribers: Arc<parking_lot::RwLock<HashMap<String, SubscriberInfo>>>,
    statistics: Arc<parking_lot::RwLock<EventBusStatistics>>,
    shutdown_signal: Arc<AtomicBool>,
    event_counter: Arc<AtomicU64>,
    delivery_counter: Arc<AtomicU64>,
    drop_counter: Arc<AtomicU64>,
    routing_handle: Option<thread::JoinHandle<()>>,
}

/// Internal subscriber information.
#[derive(Debug)]
struct SubscriberInfo {
    subscription: EventSubscription,
    sender: Sender<BusEvent>,
    #[allow(dead_code)]
    last_delivery: SystemTime,
    #[allow(dead_code)]
    events_received: u64,
}

impl HighPerformanceEventBusImpl {
    /// Creates a new high-performance event bus.
    pub async fn new(config: HighPerformanceEventBusConfig) -> Result<Self> {
        info!(
            "Creating high-performance event bus with channel capacity: {}",
            config.channel_capacity
        );

        // Create crossbeam unbounded channel for high-performance event distribution
        let (publisher, receiver) = unbounded::<BusEvent>();

        let statistics = EventBusStatistics {
            events_published: 0,
            events_delivered: 0,
            events_dropped: 0,
            active_subscribers: 0,
            avg_delivery_latency_ms: 0.0,
            memory_usage_bytes: 0,
            last_updated: SystemTime::now(),
        };

        let subscribers = Arc::new(parking_lot::RwLock::new(
            HashMap::<String, SubscriberInfo>::new(),
        ));
        let statistics_arc = Arc::new(parking_lot::RwLock::new(statistics));
        let shutdown_signal = Arc::new(AtomicBool::new(false));
        let event_counter = Arc::new(AtomicU64::new(0));
        let delivery_counter = Arc::new(AtomicU64::new(0));
        let drop_counter = Arc::new(AtomicU64::new(0));

        // Clone the Arc references for the thread
        let subscribers_clone = Arc::clone(&subscribers);
        let statistics_clone = Arc::clone(&statistics_arc);
        let shutdown_signal_clone = Arc::clone(&shutdown_signal);
        let delivery_counter_clone = Arc::clone(&delivery_counter);
        let drop_counter_clone = Arc::clone(&drop_counter);

        // Start the event routing task using crossbeam scope for safe concurrency
        let routing_handle = thread::spawn(move || {
            let backoff = Backoff::new();

            while !shutdown_signal_clone.load(Ordering::Acquire) {
                // Use crossbeam's backoff for efficient spinning
                if let Ok(bus_event) = receiver.try_recv() {
                    // Route to all subscribers
                    let subs = subscribers_clone.read();
                    let mut delivered = 0;
                    let mut dropped = 0;

                    for (subscriber_id, subscriber_info) in subs.iter() {
                        // Apply capability filtering
                        if !matches_capabilities(
                            &bus_event.event,
                            &subscriber_info.subscription.capabilities,
                        ) {
                            continue;
                        }

                        // Apply event filtering if configured
                        if let Some(filter) = &subscriber_info.subscription.event_filter {
                            if !matches_filter(&bus_event.event, filter) {
                                continue;
                            }
                        }

                        // Apply correlation filtering if configured
                        if let Some(correlation_id) =
                            &subscriber_info.subscription.correlation_filter
                        {
                            if bus_event.correlation_id != *correlation_id {
                                continue;
                            }
                        }

                        // Send to subscriber using crossbeam channel
                        match subscriber_info.sender.try_send(bus_event.clone()) {
                            Ok(_) => {
                                delivered += 1;
                                delivery_counter_clone.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => {
                                dropped += 1;
                                drop_counter_clone.fetch_add(1, Ordering::Relaxed);
                                if tracing::enabled!(tracing::Level::WARN) {
                                    warn!(
                                        subscriber_id = %subscriber_id,
                                        "Failed to deliver event to subscriber - channel full"
                                    );
                                }
                            }
                        }
                    }

                    // Update statistics
                    let mut stats = statistics_clone.write();
                    stats.events_delivered += delivered;
                    stats.events_dropped += dropped;
                    stats.last_updated = SystemTime::now();
                } else {
                    // No events available, use backoff for efficient waiting
                    backoff.snooze();
                }
            }
        });

        // Create the bus instance
        Ok(Self {
            config,
            publisher,
            subscribers,
            statistics: statistics_arc,
            shutdown_signal,
            event_counter,
            delivery_counter,
            drop_counter,
            routing_handle: Some(routing_handle),
        })
    }

    /// Starts the event bus background tasks.
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting high-performance event bus");
        info!("High-performance event bus started successfully");
        Ok(())
    }

    /// Creates a bus event from a collection event.
    fn create_bus_event(&self, event: CollectionEvent, correlation_id: String) -> BusEvent {
        BusEvent {
            event_id: Uuid::new_v4().to_string(),
            event,
            correlation_id,
            publisher_id: "high-performance-bus".to_string(),
            timestamp: SystemTime::now(),
            priority: 5, // Default priority
        }
    }

    /// Checks if an event matches the given capabilities.
    #[allow(dead_code)]
    fn matches_capabilities(&self, event: &CollectionEvent, capabilities: &SourceCaps) -> bool {
        match event {
            CollectionEvent::Process(_) => capabilities.contains(SourceCaps::PROCESS),
            CollectionEvent::Network(_) => capabilities.contains(SourceCaps::NETWORK),
            CollectionEvent::Filesystem(_) => capabilities.contains(SourceCaps::FILESYSTEM),
            CollectionEvent::Performance(_) => capabilities.contains(SourceCaps::PERFORMANCE),
            CollectionEvent::TriggerRequest(_) => capabilities.contains(SourceCaps::REALTIME),
        }
    }

    /// Checks if an event matches the given filter.
    #[allow(dead_code)]
    fn matches_filter(&self, _event: &CollectionEvent, _filter: &EventFilter) -> bool {
        // TODO: Implement actual filtering logic
        true
    }

    /// Updates event bus statistics.
    #[allow(dead_code)]
    fn update_statistics(&self, delivered: u64, dropped: u64) {
        let mut stats = self.statistics.write();
        stats.events_delivered += delivered;
        stats.events_dropped += dropped;
        stats.last_updated = SystemTime::now();
    }
}

#[async_trait]
impl HighPerformanceEventBus for HighPerformanceEventBusImpl {
    #[tracing::instrument(skip(self, event))]
    async fn publish(&self, event: CollectionEvent, correlation_id: String) -> Result<()> {
        // Create bus event
        let bus_event = self.create_bus_event(event, correlation_id);

        // Send to the main event channel using crossbeam
        self.publisher
            .send(bus_event)
            .map_err(|_| anyhow::anyhow!("Failed to send event - channel disconnected"))?;

        // Update counters with proper memory ordering
        self.event_counter.fetch_add(1, Ordering::Release);

        // Update statistics (batched to reduce lock contention)
        let mut stats = self.statistics.write();
        stats.events_published += 1;
        stats.last_updated = SystemTime::now();

        Ok(())
    }

    async fn subscribe(&self, subscription: EventSubscription) -> Result<Receiver<BusEvent>> {
        let mut subscribers = self.subscribers.write();

        if subscribers.len() >= self.config.max_subscribers {
            anyhow::bail!(
                "Maximum number of subscribers ({}) reached",
                self.config.max_subscribers
            );
        }

        if subscribers.contains_key(&subscription.subscriber_id) {
            anyhow::bail!("Subscriber '{}' already exists", subscription.subscriber_id);
        }

        // Create crossbeam bounded channel for this subscriber
        let (sender, receiver) = bounded::<BusEvent>(self.config.channel_capacity);

        let subscriber_info = SubscriberInfo {
            subscription: subscription.clone(),
            sender,
            last_delivery: SystemTime::now(),
            events_received: 0,
        };

        let subscriber_id = subscription.subscriber_id.clone();
        subscribers.insert(subscription.subscriber_id, subscriber_info);

        if tracing::enabled!(tracing::Level::INFO) {
            info!(
                subscriber_id = %subscriber_id,
                capabilities = ?subscription.capabilities,
                "Subscriber registered"
            );
        }

        // Update statistics
        let mut stats = self.statistics.write();
        stats.active_subscribers = subscribers.len();
        stats.last_updated = SystemTime::now();

        Ok(receiver)
    }

    async fn get_statistics(&self) -> EventBusStatistics {
        self.statistics.read().clone()
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down high-performance event bus");

        // Signal shutdown with proper memory ordering
        self.shutdown_signal.store(true, Ordering::Release);

        // Wait for routing task to complete
        if let Some(handle) = self.routing_handle.take() {
            let _ = handle.join();
        }

        info!("High-performance event bus shutdown complete");
        Ok(())
    }
}

/// Helper function to check if an event matches capabilities.
fn matches_capabilities(event: &CollectionEvent, capabilities: &SourceCaps) -> bool {
    match event {
        CollectionEvent::Process(_) => capabilities.contains(SourceCaps::PROCESS),
        CollectionEvent::Network(_) => capabilities.contains(SourceCaps::NETWORK),
        CollectionEvent::Filesystem(_) => capabilities.contains(SourceCaps::FILESYSTEM),
        CollectionEvent::Performance(_) => capabilities.contains(SourceCaps::PERFORMANCE),
        CollectionEvent::TriggerRequest(_) => capabilities.contains(SourceCaps::REALTIME),
    }
}

/// Helper function to check if an event matches a filter.
fn matches_filter(_event: &CollectionEvent, _filter: &EventFilter) -> bool {
    // TODO: Implement actual filtering logic
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;
    use std::time::Duration;

    #[tokio::test]
    async fn test_high_performance_event_bus_creation() {
        let config = HighPerformanceEventBusConfig::default();
        let event_bus = HighPerformanceEventBusImpl::new(config).await.unwrap();

        assert_eq!(event_bus.config.channel_capacity, 1024 * 1024);
    }

    #[tokio::test]
    async fn test_event_publishing_and_subscription() {
        let config = HighPerformanceEventBusConfig {
            channel_capacity: 1024,
            max_subscribers: 10,
            ..Default::default()
        };

        let mut event_bus = HighPerformanceEventBusImpl::new(config).await.unwrap();
        event_bus.start().await.unwrap();

        // Subscribe to process events
        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
        };

        let event_queue = event_bus.subscribe(subscription).await.unwrap();

        // Publish a process event
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(1000),
            name: "test-process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test-process".to_string(), "--arg".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(0.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        event_bus
            .publish(process_event, "test-correlation".to_string())
            .await
            .unwrap();

        // Wait for the event to be delivered using crossbeam channel
        tokio::time::timeout(Duration::from_millis(1000), async {
            loop {
                if let Ok(event) = event_queue.try_recv() {
                    break event;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let stats = event_bus.get_statistics().await;
        assert_eq!(stats.events_published, 1);
        assert_eq!(stats.active_subscribers, 1);
    }
}
