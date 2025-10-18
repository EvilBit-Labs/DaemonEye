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
//! │  (Topic-based)  │  (Message Broker)     │  (Combined)          │
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
use tokio::sync::{Mutex, RwLock};
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

/// Event subscription configuration with daemoneye-eventbus topic pattern support
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
    /// Topic patterns using daemoneye-eventbus syntax
    /// Supports hierarchical patterns like "events.process.*", "control.#", etc.
    /// Wildcards: + (single-level), # (multi-level, must be at end)
    pub topic_patterns: Option<Vec<String>>,
    /// Enable wildcards (always true for daemoneye-eventbus compatibility)
    pub enable_wildcards: bool,
    /// Topic filter for backward compatibility with existing event filtering
    pub topic_filter: Option<TopicFilter>,
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
    /// Topic filters (deprecated - use topic_patterns in EventSubscription)
    pub topic_filters: Vec<String>,
    /// Source collectors
    pub source_collectors: Vec<String>,
}

/// Topic-based filtering configuration for daemoneye-eventbus integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicFilter {
    /// Specific topics to include (exact match)
    pub include_topics: Vec<String>,
    /// Specific topics to exclude (exact match)
    pub exclude_topics: Vec<String>,
    /// Topic patterns to include (with wildcards)
    pub include_patterns: Vec<String>,
    /// Topic patterns to exclude (with wildcards)
    pub exclude_patterns: Vec<String>,
    /// Priority-based topic filtering
    pub priority_topics: HashMap<String, u8>,
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

/// Subscriber information for topic-based routing with daemoneye-eventbus support
#[derive(Debug, Clone)]
struct SubscriberInfo {
    /// Subscriber ID
    #[allow(dead_code)]
    pub subscriber_id: String,
    /// Topic patterns to match using daemoneye-eventbus syntax
    pub topic_patterns: Vec<String>,
    /// Enable wildcard matching (always true for daemoneye-eventbus compatibility)
    pub enable_wildcards: bool,
    /// Additional topic filtering configuration
    pub topic_filter: Option<TopicFilter>,
    /// Event sender channel
    pub sender: tokio::sync::mpsc::UnboundedSender<BusEvent>,
}

/// Local event bus implementation using topic-based routing with daemoneye-eventbus semantics
pub struct LocalEventBus {
    /// Event bus configuration
    #[allow(dead_code)]
    config: EventBusConfig,
    /// Subscriber information with topic patterns
    subscribers: Arc<RwLock<HashMap<String, SubscriberInfo>>>,
    /// Statistics
    stats: Arc<Mutex<EventBusStatistics>>,
    /// Start time
    start_time: Instant,
}

impl LocalEventBus {
    /// Create a new local event bus with topic-based routing
    pub fn new(config: EventBusConfig) -> Self {
        Self {
            config,
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(Mutex::new(EventBusStatistics {
                events_published: 0,
                events_delivered: 0,
                active_subscribers: 0,
                uptime: Duration::from_secs(0),
            })),
            start_time: Instant::now(),
        }
    }

    /// Validate topic patterns for daemoneye-eventbus compatibility
    fn validate_topic_patterns(patterns: &[String]) -> Result<()> {
        for pattern in patterns {
            Self::validate_single_pattern(pattern)?;
        }
        Ok(())
    }

    /// Validate a single topic pattern
    fn validate_single_pattern(pattern: &str) -> Result<()> {
        if pattern.is_empty() {
            return Err(anyhow::anyhow!("Empty topic pattern"));
        }

        let segments: Vec<&str> = pattern.split('.').collect();

        // Check for empty segments
        for (i, segment) in segments.iter().enumerate() {
            if segment.is_empty() {
                return Err(anyhow::anyhow!(
                    "Empty segment at position {} in pattern '{}'",
                    i,
                    pattern
                ));
            }
        }

        // Validate multi-level wildcard placement (# must be at end)
        for (i, segment) in segments.iter().enumerate() {
            if *segment == "#" && i != segments.len() - 1 {
                return Err(anyhow::anyhow!(
                    "Multi-level wildcard '#' must be at the end of pattern '{}'",
                    pattern
                ));
            }
        }

        // Validate topic structure (should start with known domains)
        if let Some(first_segment) = segments.first() {
            match *first_segment {
                "events" | "control" | "+" | "#" => {
                    // Valid domain or wildcard
                }
                _ => {
                    tracing::warn!(
                        pattern = %pattern,
                        "Topic pattern uses non-standard domain '{}', consider using 'events' or 'control'",
                        first_segment
                    );
                }
            }
        }

        Ok(())
    }

    /// Generate topic for an event based on its type
    fn generate_topic_for_event(event: &CollectionEvent) -> String {
        match event {
            CollectionEvent::Process(_) => "events.process.new".to_string(),
            CollectionEvent::Network(_) => "events.network.new".to_string(),
            CollectionEvent::Filesystem(_) => "events.filesystem.new".to_string(),
            CollectionEvent::Performance(_) => "events.performance.new".to_string(),
            CollectionEvent::TriggerRequest(_) => "control.trigger.request".to_string(),
        }
    }

    /// Check if a topic matches a pattern with daemoneye-eventbus wildcard support
    /// Supports: + (single-level wildcard), # (multi-level wildcard, must be at end)
    fn topic_matches_pattern(topic: &str, pattern: &str, enable_wildcards: bool) -> bool {
        if !enable_wildcards {
            return topic == pattern;
        }

        // daemoneye-eventbus style wildcard matching: + (single-level), # (multi-level)
        let topic_parts: Vec<&str> = topic.split('.').collect();
        let pattern_parts: Vec<&str> = pattern.split('.').collect();

        Self::match_parts_eventbus(&topic_parts, &pattern_parts)
    }

    /// Recursive pattern matching for topic segments with daemoneye-eventbus semantics
    fn match_parts_eventbus(topic_parts: &[&str], pattern_parts: &[&str]) -> bool {
        let mut pattern_idx = 0;
        let mut topic_idx = 0;

        while pattern_idx < pattern_parts.len() && topic_idx < topic_parts.len() {
            let pattern_part = pattern_parts[pattern_idx];
            let topic_part = topic_parts[topic_idx];

            match pattern_part {
                "+" => {
                    // Single-level wildcard: matches exactly one segment
                    pattern_idx += 1;
                    topic_idx += 1;
                }
                "#" => {
                    // Multi-level wildcard: matches remaining segments (must be at end)
                    if pattern_idx != pattern_parts.len() - 1 {
                        // # must be the last segment in pattern
                        return false;
                    }
                    // # matches all remaining topic segments
                    return true;
                }
                "*" => {
                    // Legacy wildcard support: treat as single-level wildcard
                    pattern_idx += 1;
                    topic_idx += 1;
                }
                literal => {
                    // Exact match required
                    if literal != topic_part {
                        return false;
                    }
                    pattern_idx += 1;
                    topic_idx += 1;
                }
            }
        }

        // Check if we consumed all pattern segments and topic segments
        pattern_idx == pattern_parts.len() && topic_idx == topic_parts.len()
    }
}

#[async_trait]
impl EventBus for LocalEventBus {
    async fn publish(&self, event: CollectionEvent, correlation_id: Option<String>) -> Result<()> {
        let topic = Self::generate_topic_for_event(&event);

        tracing::debug!(
            topic = %topic,
            event_type = event.event_type(),
            correlation_id = ?correlation_id,
            "Publishing event to LocalEventBus with topic-based routing"
        );

        // Create bus event
        let bus_event = BusEvent {
            id: Uuid::new_v4(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            event,
            correlation_id,
            routing_metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("topic".to_string(), topic.clone());
                metadata
            },
        };

        // Find matching subscribers and deliver events
        let mut delivered_count = 0;
        let mut failed_subscribers = Vec::new();

        {
            let subscribers = self.subscribers.read().await;
            for (subscriber_id, subscriber_info) in subscribers.iter() {
                // Check if any of the subscriber's topic patterns match
                let topic_matches = subscriber_info.topic_patterns.iter().any(|pattern| {
                    Self::topic_matches_pattern(&topic, pattern, subscriber_info.enable_wildcards)
                });

                if topic_matches {
                    // Apply additional topic filtering if configured
                    let should_deliver = if let Some(topic_filter) = &subscriber_info.topic_filter {
                        Self::apply_topic_filter(&topic, topic_filter)
                    } else {
                        true
                    };

                    if should_deliver {
                        if subscriber_info.sender.send(bus_event.clone()).is_ok() {
                            delivered_count += 1;
                            tracing::debug!(
                                subscriber_id = %subscriber_id,
                                topic = %topic,
                                matched_pattern = ?subscriber_info.topic_patterns,
                                "Event delivered to subscriber"
                            );
                        } else {
                            failed_subscribers.push(subscriber_id.clone());
                            tracing::debug!(
                                subscriber_id = %subscriber_id,
                                "Failed to deliver event - subscriber channel closed"
                            );
                        }
                    } else {
                        tracing::debug!(
                            subscriber_id = %subscriber_id,
                            topic = %topic,
                            "Event filtered out by topic filter"
                        );
                    }
                }
            }
        }

        // Remove failed subscribers
        if !failed_subscribers.is_empty() {
            let mut subscribers = self.subscribers.write().await;
            for subscriber_id in failed_subscribers {
                subscribers.remove(&subscriber_id);
            }
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.events_published += 1;
            stats.events_delivered += delivered_count;
            let subscribers = self.subscribers.read().await;
            stats.active_subscribers = subscribers.len();
        }

        tracing::debug!(
            topic = %topic,
            delivered_count = delivered_count,
            "Event published successfully"
        );

        Ok(())
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>> {
        let subscriber_id = subscription.subscriber_id.clone();

        // Check if we've reached the maximum number of subscribers
        {
            let subscribers = self.subscribers.read().await;
            if subscribers.len() >= self.config.max_subscribers {
                return Err(anyhow::anyhow!(
                    "Maximum number of subscribers ({}) reached",
                    self.config.max_subscribers
                ));
            }
        }

        // Generate default topic patterns if none provided, using daemoneye-eventbus syntax
        let topic_patterns = if let Some(patterns) = subscription.topic_patterns {
            Self::validate_topic_patterns(&patterns)?;
            patterns
        } else {
            let mut patterns = Vec::new();
            if subscription.capabilities.contains(SourceCaps::PROCESS) {
                patterns.push("events.process.+".to_string()); // Single-level wildcard
            }
            if subscription.capabilities.contains(SourceCaps::NETWORK) {
                patterns.push("events.network.+".to_string());
            }
            if subscription.capabilities.contains(SourceCaps::FILESYSTEM) {
                patterns.push("events.filesystem.+".to_string());
            }
            if subscription.capabilities.contains(SourceCaps::PERFORMANCE) {
                patterns.push("events.performance.+".to_string());
            }
            if patterns.is_empty() {
                patterns.push("events.#".to_string()); // Multi-level wildcard for all events
            }
            patterns
        };

        tracing::debug!(
            subscriber_id = %subscriber_id,
            topic_patterns = ?topic_patterns,
            enable_wildcards = subscription.enable_wildcards,
            "Subscribing to LocalEventBus with topic patterns"
        );

        // Create a channel for the subscriber
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Store subscriber information
        let subscriber_info = SubscriberInfo {
            subscriber_id: subscriber_id.clone(),
            topic_patterns,
            enable_wildcards: subscription.enable_wildcards,
            topic_filter: subscription.topic_filter,
            sender: tx,
        };

        {
            let mut subscribers = self.subscribers.write().await;
            subscribers.insert(subscriber_id.clone(), subscriber_info);
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            let subscribers = self.subscribers.read().await;
            stats.active_subscribers = subscribers.len();
        }

        tracing::info!(
            subscriber_id = %subscriber_id,
            "Successfully subscribed to LocalEventBus with topic-based routing"
        );

        Ok(rx)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        tracing::debug!(
            subscriber_id = %subscriber_id,
            "Unsubscribing from LocalEventBus"
        );

        // Remove subscriber
        let active_subscribers = {
            let mut subscribers = self.subscribers.write().await;
            subscribers.remove(subscriber_id);
            subscribers.len()
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.active_subscribers = active_subscribers;
        }

        tracing::info!(
            subscriber_id = %subscriber_id,
            "Successfully unsubscribed from LocalEventBus"
        );

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
        tracing::info!("Shutting down LocalEventBus");

        // Clear all subscribers (this will close their channels)
        {
            let mut subscribers = self.subscribers.write().await;
            subscribers.clear();
        }

        tracing::info!("LocalEventBus shutdown completed");
        Ok(())
    }
}

impl LocalEventBus {
    /// Apply topic filtering rules to determine if an event should be delivered
    fn apply_topic_filter(topic: &str, filter: &TopicFilter) -> bool {
        // Check exclude topics first (exact match)
        if filter.exclude_topics.contains(&topic.to_string()) {
            return false;
        }

        // Check exclude patterns
        for exclude_pattern in &filter.exclude_patterns {
            if Self::topic_matches_pattern(topic, exclude_pattern, true) {
                return false;
            }
        }

        // Check include topics (exact match)
        if !filter.include_topics.is_empty() {
            if !filter.include_topics.contains(&topic.to_string()) {
                // Check include patterns
                let pattern_match = filter.include_patterns.iter().any(|include_pattern| {
                    Self::topic_matches_pattern(topic, include_pattern, true)
                });
                if !pattern_match {
                    return false;
                }
            }
        } else if !filter.include_patterns.is_empty() {
            // Only include patterns specified, check them
            let pattern_match = filter
                .include_patterns
                .iter()
                .any(|include_pattern| Self::topic_matches_pattern(topic, include_pattern, true));
            if !pattern_match {
                return false;
            }
        }

        // Check priority-based filtering
        if !filter.priority_topics.is_empty() {
            if let Some(_priority) = filter.priority_topics.get(topic) {
                // Priority-based filtering logic can be extended here
                // For now, just allow the event through
                return true;
            }
        }

        true
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
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
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

        let received_event = timeout(Duration::from_secs(5), receiver.recv())
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

        // Test shutdown
        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_local_event_bus_topic_patterns() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Subscribe to process events with wildcard pattern
        let process_subscription = EventSubscription {
            subscriber_id: "process-subscriber".to_string(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        // Subscribe to network events with wildcard pattern
        let network_subscription = EventSubscription {
            subscriber_id: "network-subscriber".to_string(),
            capabilities: crate::source::SourceCaps::NETWORK,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.network.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut process_receiver = bus.subscribe(process_subscription).await.unwrap();
        let mut network_receiver = bus.subscribe(network_subscription).await.unwrap();

        // Publish a process event
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(5678),
            name: "test_process".to_string(),
            executable_path: Some("/bin/test".to_string()),
            command_line: vec!["test".to_string()],
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

        bus.publish(process_event.clone(), Some("test-correlation".to_string()))
            .await
            .unwrap();

        // Process subscriber should receive the event
        let received_process_event = timeout(Duration::from_secs(3), process_receiver.recv())
            .await
            .expect("timed out waiting for process event")
            .expect("process event channel closed");

        assert_eq!(received_process_event.event, process_event);
        assert_eq!(
            received_process_event.correlation_id,
            Some("test-correlation".to_string())
        );

        // Network subscriber should not receive the process event (verify topic isolation)
        let network_result = timeout(Duration::from_millis(500), network_receiver.recv()).await;
        assert!(
            network_result.is_err(),
            "Network subscriber should not receive process events"
        );

        // Test statistics
        let stats = bus.get_statistics().await.unwrap();
        assert_eq!(stats.events_published, 1);
        assert_eq!(stats.active_subscribers, 2);

        // Test unsubscribe
        bus.unsubscribe("process-subscriber").await.unwrap();
        let updated_stats = bus.get_statistics().await.unwrap();
        assert_eq!(updated_stats.active_subscribers, 1);

        // Shutdown
        bus.shutdown().await.unwrap();
    }

    #[test]
    fn test_topic_pattern_matching() {
        // Test exact matches
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.process.new",
            false
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.network.new",
            false
        ));

        // Test single-level wildcard (+) - daemoneye-eventbus syntax
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.process.+",
            true
        ));
        assert!(LocalEventBus::topic_matches_pattern(
            "events.network.update",
            "events.network.+",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.network.+",
            true
        ));

        // Test single-level wildcard - + matches exactly one segment
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process",
            "events.+",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.+",
            true
        )); // + doesn't match multiple segments
        assert!(!LocalEventBus::topic_matches_pattern(
            "control.trigger.request",
            "events.+",
            true
        ));

        // Test multi-level wildcard (#) - daemoneye-eventbus syntax
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.#",
            true
        ));
        assert!(LocalEventBus::topic_matches_pattern(
            "events.network.connections.tcp",
            "events.#",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "control.trigger.request",
            "events.#",
            true
        ));

        // Test legacy wildcard support (*) - treated as single-level
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.process.*",
            true
        ));
        assert!(LocalEventBus::topic_matches_pattern(
            "events.network.update",
            "events.network.*",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.process.new",
            "events.network.*",
            true
        ));
    }

    #[tokio::test]
    async fn test_local_event_bus_topic_based_routing() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Subscribe to specific topic patterns
        let process_subscription = EventSubscription {
            subscriber_id: "process-only".to_string(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let trigger_subscription = EventSubscription {
            subscriber_id: "trigger-only".to_string(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["control.trigger.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut process_receiver = bus.subscribe(process_subscription).await.unwrap();
        let mut trigger_receiver = bus.subscribe(trigger_subscription).await.unwrap();

        // Publish a process event (should go to events.process.new topic)
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_string(),
            executable_path: None,
            command_line: vec![],
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

        // Publish a trigger request (should go to control.trigger.request topic)
        let trigger_event = CollectionEvent::TriggerRequest(crate::event::TriggerRequest {
            trigger_id: "test-trigger".to_string(),
            target_collector: "test-collector".to_string(),
            analysis_type: crate::event::AnalysisType::BinaryHash,
            priority: crate::event::TriggerPriority::Normal,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "test-correlation".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        });

        // Publish both events
        bus.publish(
            process_event.clone(),
            Some("process-correlation".to_string()),
        )
        .await
        .unwrap();
        bus.publish(
            trigger_event.clone(),
            Some("trigger-correlation".to_string()),
        )
        .await
        .unwrap();

        // Process subscriber should only receive the process event
        let received_process = timeout(Duration::from_secs(2), process_receiver.recv())
            .await
            .expect("timeout waiting for process event")
            .expect("process channel closed");

        assert!(matches!(
            received_process.event,
            CollectionEvent::Process(_)
        ));
        assert_eq!(
            received_process.correlation_id,
            Some("process-correlation".to_string())
        );

        // Trigger subscriber should only receive the trigger event
        let received_trigger = timeout(Duration::from_secs(2), trigger_receiver.recv())
            .await
            .expect("timeout waiting for trigger event")
            .expect("trigger channel closed");

        assert!(matches!(
            received_trigger.event,
            CollectionEvent::TriggerRequest(_)
        ));
        assert_eq!(
            received_trigger.correlation_id,
            Some("trigger-correlation".to_string())
        );

        // Verify topic-based isolation - process subscriber should not receive trigger events
        let no_trigger_for_process =
            timeout(Duration::from_millis(200), process_receiver.recv()).await;
        assert!(
            no_trigger_for_process.is_err(),
            "Process subscriber should not receive trigger events"
        );

        // Verify topic-based isolation - trigger subscriber should not receive process events
        let no_process_for_trigger =
            timeout(Duration::from_millis(200), trigger_receiver.recv()).await;
        assert!(
            no_process_for_trigger.is_err(),
            "Trigger subscriber should not receive process events"
        );

        // Verify statistics
        let stats = bus.get_statistics().await.unwrap();
        assert_eq!(stats.events_published, 2);
        assert_eq!(stats.events_delivered, 2); // Each event delivered to exactly one subscriber
        assert_eq!(stats.active_subscribers, 2);

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_topic_filter_functionality() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Create a subscription with topic filtering
        let topic_filter = TopicFilter {
            include_topics: vec!["events.process.lifecycle".to_string()],
            exclude_topics: vec!["events.process.metadata".to_string()],
            include_patterns: vec!["events.process.+".to_string()],
            exclude_patterns: vec!["events.network.+".to_string()],
            priority_topics: HashMap::new(),
        };

        let subscription = EventSubscription {
            subscriber_id: "filtered-subscriber".to_string(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#".to_string()]), // Subscribe to all events
            enable_wildcards: true,
            topic_filter: Some(topic_filter),
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        // Test include topic (should be delivered)
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_string(),
            executable_path: None,
            command_line: vec![],
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

        // This should be delivered (matches include_topics)
        bus.publish(process_event.clone(), Some("test-1".to_string()))
            .await
            .unwrap();

        // Should receive the event
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for filtered event")
            .expect("channel closed");

        assert_eq!(received.correlation_id, Some("test-1".to_string()));

        bus.shutdown().await.unwrap();
    }

    #[test]
    fn test_topic_pattern_validation() {
        // Test valid patterns
        assert!(LocalEventBus::validate_single_pattern("events.process.+").is_ok());
        assert!(LocalEventBus::validate_single_pattern("events.#").is_ok());
        assert!(LocalEventBus::validate_single_pattern("control.collector.lifecycle").is_ok());

        // Test invalid patterns
        assert!(LocalEventBus::validate_single_pattern("").is_err());
        assert!(LocalEventBus::validate_single_pattern("events..process").is_err());
        assert!(LocalEventBus::validate_single_pattern("events.#.invalid").is_err()); // # must be at end
    }

    #[test]
    fn test_daemoneye_eventbus_wildcard_semantics() {
        // Test + (single-level wildcard)
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.lifecycle",
            "events.process.+",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.process.lifecycle.detail",
            "events.process.+",
            true
        )); // + doesn't match multiple levels

        // Test # (multi-level wildcard)
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.lifecycle",
            "events.#",
            true
        ));
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.lifecycle.detail.extra",
            "events.#",
            true
        )); // # matches multiple levels

        // Test mixed patterns
        assert!(LocalEventBus::topic_matches_pattern(
            "events.process.lifecycle",
            "events.+.lifecycle",
            true
        ));
        assert!(!LocalEventBus::topic_matches_pattern(
            "events.network.connections",
            "events.+.lifecycle",
            true
        ));
    }
}
