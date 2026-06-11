//! Local in-process event bus implementation with topic-based routing.
//!
//! This submodule contains [`LocalEventBus`], a topic-based event bus using
//! daemoneye-eventbus wildcard semantics, along with its subscriber tracking,
//! pattern matching, and filtering logic.

#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::pattern_type_mismatch)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::option_if_let_else)]
#![allow(clippy::match_same_arms)]

use super::EventBus;
use super::types::{
    BusEvent, CorrelationFilter, CorrelationMetadata, CrossCollectorCorrelation, EventBusConfig,
    EventBusCorrelation, EventBusFilters, EventBusStatistics, EventFilter, EventSubscription,
    SequenceCorrelation, TemporalCorrelation, TopicCorrelation, TopicFilter,
};
use crate::{event::CollectionEvent, source::SourceCaps};
use anyhow::Result;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

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
    /// Correlation filtering configuration for daemoneye-eventbus
    pub correlation_filter: Option<CorrelationFilter>,
    /// Event filtering configuration for daemoneye-eventbus
    pub event_filter: Option<EventFilter>,
    /// Event sender channel
    pub sender: tokio::sync::mpsc::UnboundedSender<Arc<BusEvent>>,
}

/// Local event bus implementation using topic-based routing with daemoneye-eventbus semantics
pub struct LocalEventBus {
    /// Event bus configuration
    #[allow(dead_code)]
    config: EventBusConfig,
    /// Subscriber information with topic patterns
    subscribers: Arc<RwLock<HashMap<String, SubscriberInfo>>>,
    /// Statistics (used only for uptime and `active_subscribers`; hot-path counters use atomics)
    stats: Arc<Mutex<EventBusStatistics>>,
    /// Atomic counter for total events published (avoids Mutex on hot path)
    events_published: Arc<AtomicU64>,
    /// Atomic counter for total events delivered (avoids Mutex on hot path)
    events_delivered: Arc<AtomicU64>,
    /// Start time
    start_time: Instant,
}

impl LocalEventBus {
    /// Create a new local event bus with topic-based routing
    pub fn new(config: EventBusConfig) -> Self {
        Self {
            config,
            subscribers: Arc::new(RwLock::new(HashMap::with_capacity(16))), // Pre-allocate for typical subscriber count
            stats: Arc::new(Mutex::new(EventBusStatistics {
                events_published: 0,
                events_delivered: 0,
                active_subscribers: 0,
                uptime: Duration::from_secs(0),
            })),
            events_published: Arc::new(AtomicU64::new(0)),
            events_delivered: Arc::new(AtomicU64::new(0)),
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
                    "Empty segment at position {i} in pattern '{pattern}'"
                ));
            }
        }

        // Validate multi-level wildcard placement (# must be at end)
        for (i, segment) in segments.iter().enumerate() {
            if *segment == "#" && i != segments.len() - 1 {
                return Err(anyhow::anyhow!(
                    "Multi-level wildcard '#' must be at the end of pattern '{pattern}'"
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

    // Topic constants for better performance
    const TOPIC_PROCESS_NEW: &'static str = "events.process.new";
    const TOPIC_NETWORK_NEW: &'static str = "events.network.new";
    const TOPIC_FILESYSTEM_NEW: &'static str = "events.filesystem.new";
    const TOPIC_PERFORMANCE_NEW: &'static str = "events.performance.new";
    const TOPIC_TRIGGER_REQUEST: &'static str = "control.trigger.request";

    /// Generate topic for an event based on its type
    const fn generate_topic_for_event(event: &CollectionEvent) -> &'static str {
        match event {
            CollectionEvent::Process(_) => Self::TOPIC_PROCESS_NEW,
            CollectionEvent::Network(_) => Self::TOPIC_NETWORK_NEW,
            CollectionEvent::Filesystem(_) => Self::TOPIC_FILESYSTEM_NEW,
            CollectionEvent::Performance(_) => Self::TOPIC_PERFORMANCE_NEW,
            CollectionEvent::TriggerRequest(_) => Self::TOPIC_TRIGGER_REQUEST,
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
    async fn publish(
        &self,
        event: CollectionEvent,
        correlation_metadata: CorrelationMetadata,
    ) -> Result<()> {
        let topic = Self::generate_topic_for_event(&event);

        tracing::debug!(
            topic = %topic,
            event_type = event.event_type(),
            correlation_id = %correlation_metadata.correlation_id,
            parent_correlation_id = ?correlation_metadata.parent_correlation_id,
            root_correlation_id = %correlation_metadata.root_correlation_id,
            workflow_stage = ?correlation_metadata.workflow_stage,
            "Publishing event to LocalEventBus with daemoneye-eventbus correlation metadata"
        );

        // Create bus event with enhanced correlation metadata and wrap in Arc once.
        // Each subscriber receives a cheap Arc clone rather than a deep copy of the event.
        let bus_event = Arc::new(BusEvent {
            id: Uuid::new_v4(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            event,
            correlation_metadata,
            routing_metadata: {
                let mut metadata = HashMap::with_capacity(1);
                metadata.insert("topic".to_owned(), topic.to_owned());
                metadata
            },
        });

        // Find matching subscribers and deliver events
        let mut delivered_count: u64 = 0;
        let mut failed_subscribers = Vec::with_capacity(4); // Pre-allocate for typical failure scenarios

        {
            let subscribers = self.subscribers.read().await;
            for (subscriber_id, subscriber_info) in subscribers.iter() {
                // Check if any of the subscriber's topic patterns match
                let topic_matches = subscriber_info.topic_patterns.iter().any(|pattern| {
                    Self::topic_matches_pattern(topic, pattern, subscriber_info.enable_wildcards)
                });

                if topic_matches {
                    // Apply additional topic filtering if configured
                    let topic_filter_passed =
                        if let Some(topic_filter) = &subscriber_info.topic_filter {
                            Self::apply_topic_filter(topic, topic_filter)
                        } else {
                            true
                        };

                    // Apply correlation filtering if configured
                    let correlation_filter_passed =
                        if let Some(correlation_filter) = &subscriber_info.correlation_filter {
                            Self::apply_correlation_filter(
                                &bus_event.correlation_metadata,
                                correlation_filter,
                            )
                        } else {
                            true
                        };

                    // Apply event filtering if configured
                    let event_filter_passed =
                        if let Some(event_filter) = &subscriber_info.event_filter {
                            Self::apply_event_filter(&bus_event.event, event_filter)
                        } else {
                            true
                        };

                    if topic_filter_passed && correlation_filter_passed && event_filter_passed {
                        if subscriber_info.sender.send(Arc::clone(&bus_event)).is_ok() {
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

        // Increment atomic counters on the hot path (no lock required)
        self.events_published.fetch_add(1, Ordering::Relaxed);
        self.events_delivered
            .fetch_add(delivered_count, Ordering::Relaxed);

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
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<Arc<BusEvent>>> {
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
            // Pre-allocate with exact capacity based on capability count to avoid reallocations
            let capability_count = [
                subscription.capabilities.contains(SourceCaps::PROCESS),
                subscription.capabilities.contains(SourceCaps::NETWORK),
                subscription.capabilities.contains(SourceCaps::FILESYSTEM),
                subscription.capabilities.contains(SourceCaps::PERFORMANCE),
            ]
            .iter()
            .filter(|&&x| x)
            .count();

            let mut patterns = Vec::with_capacity(capability_count.max(1));
            if subscription.capabilities.contains(SourceCaps::PROCESS) {
                patterns.push("events.process.+".to_owned()); // Single-level wildcard
            }
            if subscription.capabilities.contains(SourceCaps::NETWORK) {
                patterns.push("events.network.+".to_owned());
            }
            if subscription.capabilities.contains(SourceCaps::FILESYSTEM) {
                patterns.push("events.filesystem.+".to_owned());
            }
            if subscription.capabilities.contains(SourceCaps::PERFORMANCE) {
                patterns.push("events.performance.+".to_owned());
            }
            if patterns.is_empty() {
                patterns.push("events.#".to_owned()); // Multi-level wildcard for all events
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
            correlation_filter: subscription.correlation_filter,
            event_filter: subscription.event_filter,
            sender: tx,
        };

        {
            let mut subscribers = self.subscribers.write().await;
            subscribers.insert(subscriber_id.clone(), subscriber_info)
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            let subscribers = self.subscribers.read().await;
            stats.active_subscribers = subscribers.len()
        };

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
            stats.active_subscribers = active_subscribers
        };

        tracing::info!(
            subscriber_id = %subscriber_id,
            "Successfully unsubscribed from LocalEventBus"
        );

        Ok(())
    }

    async fn get_statistics(&self) -> Result<EventBusStatistics> {
        // Flush atomic counters into the stats struct on read.
        // This is the only point where the Mutex is acquired for counter values,
        // keeping the publish hot path free of lock contention.
        let active_subscribers = {
            let subscribers = self.subscribers.read().await;
            subscribers.len()
        };
        let mut stats = self.stats.lock().await;
        stats.events_published = self.events_published.load(Ordering::Acquire);
        stats.events_delivered = self.events_delivered.load(Ordering::Acquire);
        stats.active_subscribers = active_subscribers;
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
            subscribers.clear()
        };

        tracing::info!("LocalEventBus shutdown completed");
        Ok(())
    }
}

impl LocalEventBus {
    /// Apply topic filtering rules to determine if an event should be delivered
    fn apply_topic_filter(topic: &str, filter: &TopicFilter) -> bool {
        // Check exclude topics first (exact match) - avoid string allocation
        if filter.exclude_topics.iter().any(|t| t == topic) {
            return false;
        }

        // Check exclude patterns
        for exclude_pattern in &filter.exclude_patterns {
            if Self::topic_matches_pattern(topic, exclude_pattern, true) {
                return false;
            }
        }

        // Check include topics (exact match) - avoid string allocation
        if !filter.include_topics.is_empty() {
            if !filter.include_topics.iter().any(|t| t == topic) {
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
        if !filter.priority_topics.is_empty()
            && let Some(_priority) = filter.priority_topics.get(topic)
        {
            // Priority-based filtering logic can be extended here
            // For now, just allow the event through
            return true;
        }

        true
    }

    /// Apply correlation filtering rules to determine if an event should be delivered
    fn apply_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        filter: &CorrelationFilter,
    ) -> bool {
        // Check correlation ID filter
        if let Some(filter_correlation_id) = &filter.correlation_id
            && correlation_metadata.correlation_id != *filter_correlation_id
        {
            return false;
        }

        // Check parent correlation ID filter
        if let Some(filter_parent_id) = &filter.parent_correlation_id {
            match &correlation_metadata.parent_correlation_id {
                Some(parent_id) if parent_id == filter_parent_id => {}
                None if filter_parent_id.is_empty() => {} // Allow empty parent filter to match None
                _ => return false,
            }
        }

        // Check root correlation ID filter
        if let Some(filter_root_id) = &filter.root_correlation_id
            && correlation_metadata.root_correlation_id != *filter_root_id
        {
            return false;
        }

        // Check workflow stage filter
        if let Some(filter_stage) = &filter.workflow_stage {
            match &correlation_metadata.workflow_stage {
                Some(stage) if stage == filter_stage => {}
                None if filter_stage.is_empty() => {} // Allow empty stage filter to match None
                _ => return false,
            }
        }

        // Check correlation tags filter
        for (filter_key, filter_value) in &filter.correlation_tags {
            match correlation_metadata.correlation_tags.get(filter_key) {
                Some(value) if value == filter_value => {}
                _ => return false,
            }
        }

        // Legacy process ID filtering (for backward compatibility)
        if !filter.process_ids.is_empty() {
            // Extract PID from correlation tags if available
            if let Some(pid_str) = correlation_metadata.correlation_tags.get("pid") {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if !filter.process_ids.contains(&pid) {
                        return false;
                    }
                } else {
                    return false; // Invalid PID format
                }
            } else {
                return false; // No PID in correlation tags
            }
        }

        // Apply daemoneye-eventbus specific correlation filtering
        if let Some(eventbus_correlation) = &filter.eventbus_correlation
            && !Self::apply_eventbus_correlation_filter(correlation_metadata, eventbus_correlation)
        {
            return false;
        }

        true
    }

    /// Apply daemoneye-eventbus specific correlation filtering
    fn apply_eventbus_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        eventbus_correlation: &EventBusCorrelation,
    ) -> bool {
        // Check sequence correlation
        if let Some(sequence_correlation) = &eventbus_correlation.sequence_correlation
            && !Self::apply_sequence_correlation_filter(correlation_metadata, sequence_correlation)
        {
            return false;
        }

        // Check topic correlation
        if let Some(topic_correlation) = &eventbus_correlation.topic_correlation
            && !Self::apply_topic_correlation_filter(correlation_metadata, topic_correlation)
        {
            return false;
        }

        // Check temporal correlation
        if let Some(temporal_correlation) = &eventbus_correlation.temporal_correlation
            && !Self::apply_temporal_correlation_filter(correlation_metadata, temporal_correlation)
        {
            return false;
        }

        // Check cross-collector correlation
        if let Some(cross_collector_correlation) = &eventbus_correlation.cross_collector_correlation
            && !Self::apply_cross_collector_correlation_filter(
                correlation_metadata,
                cross_collector_correlation,
            )
        {
            return false;
        }

        true
    }

    /// Apply sequence-based correlation filtering
    const fn apply_sequence_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        sequence_correlation: &SequenceCorrelation,
    ) -> bool {
        let sequence = correlation_metadata.sequence_number;
        let base = sequence_correlation.base_sequence;
        let window = sequence_correlation.window_size;

        // Check if sequence is within the correlation window
        // Use subtraction-based comparison to avoid overflow
        if sequence < base {
            return false;
        }
        let delta = sequence - base;
        delta <= window
    }

    /// Apply topic-based correlation filtering
    fn apply_topic_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        topic_correlation: &TopicCorrelation,
    ) -> bool {
        // Check if any topic chains match the correlation patterns
        if let Some(eventbus_metadata) = &correlation_metadata.eventbus_metadata {
            for topic_chain in &eventbus_metadata.topic_chains {
                // Check source patterns
                let source_matches = topic_correlation.source_patterns.iter().any(|pattern| {
                    Self::topic_matches_pattern(&topic_chain.source_topic, pattern, true)
                });

                // Check target patterns
                let target_matches = topic_correlation.target_patterns.iter().any(|pattern| {
                    Self::topic_matches_pattern(&topic_chain.target_topic, pattern, true)
                });

                if source_matches && target_matches {
                    return true;
                }
            }
            return false; // No matching topic chains found
        }

        // If no eventbus metadata, allow through (backward compatibility)
        true
    }

    /// Apply temporal correlation filtering
    fn apply_temporal_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        temporal_correlation: &TemporalCorrelation,
    ) -> bool {
        if let Some(eventbus_metadata) = &correlation_metadata.eventbus_metadata {
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let delivery_time = eventbus_metadata.delivery_timestamp;
            let time_diff = current_time.saturating_sub(delivery_time);

            // Check if event is within the temporal correlation window
            time_diff <= temporal_correlation.window_seconds
        } else {
            // If no eventbus metadata, allow through (backward compatibility)
            true
        }
    }

    /// Apply cross-collector correlation filtering
    fn apply_cross_collector_correlation_filter(
        correlation_metadata: &CorrelationMetadata,
        cross_collector_correlation: &CrossCollectorCorrelation,
    ) -> bool {
        if let Some(eventbus_metadata) = &correlation_metadata.eventbus_metadata
            && let Some(collector_coordination) = &eventbus_metadata.collector_coordination
        {
            // Check if initiator collector matches source collectors
            let source_matches = cross_collector_correlation
                .source_collectors
                .contains(&collector_coordination.initiator_collector);
            // Check if target collector matches - check if any target collectors match
            let target_matches = collector_coordination
                .target_collectors
                .iter()
                .any(|tc| cross_collector_correlation.target_collectors.contains(tc))
                || cross_collector_correlation.target_collectors.is_empty();

            return source_matches && target_matches;
        }

        // If no collector coordination metadata, allow through (backward compatibility)
        true
    }

    /// Apply event filtering rules to determine if an event should be delivered
    fn apply_event_filter(event: &CollectionEvent, filter: &EventFilter) -> bool {
        // Check event type filtering
        if !filter.event_types.is_empty() {
            let event_type = event.event_type().to_owned();
            if !filter.event_types.contains(&event_type) {
                return false;
            }
        }

        // Check process ID filtering (for process events)
        if !filter.pids.is_empty() {
            #[allow(clippy::wildcard_enum_match_arm)]
            match event {
                CollectionEvent::Process(process_event) => {
                    if !filter.pids.contains(&process_event.pid) {
                        return false;
                    }
                }
                _ => {
                    // Non-process events don't have PIDs, so filter them out if PID filter is specified
                    return false;
                }
            }
        }

        // Check minimum priority filtering
        if let Some(min_priority) = filter.min_priority {
            let event_priority = Self::extract_event_priority(event);
            if event_priority < min_priority {
                return false;
            }
        }

        // Check metadata filtering
        if !filter.metadata_filters.is_empty() {
            let event_metadata = Self::extract_event_metadata(event);
            for (filter_key, filter_value) in &filter.metadata_filters {
                match event_metadata.get(filter_key) {
                    Some(value) if value == filter_value => {}
                    _ => return false,
                }
            }
        }

        // Check source collector filtering - avoid string allocation by using static strings
        if !filter.source_collectors.is_empty() {
            let source_collector = Self::extract_source_collector(event);
            if !filter
                .source_collectors
                .iter()
                .any(|sc| sc == source_collector)
            {
                return false;
            }
        }

        // Apply daemoneye-eventbus specific filtering
        if let Some(eventbus_filters) = &filter.eventbus_filters
            && !Self::apply_eventbus_filters(event, eventbus_filters)
        {
            return false;
        }

        true
    }

    /// Extract event priority for filtering
    #[allow(clippy::wildcard_enum_match_arm)]
    const fn extract_event_priority(event: &CollectionEvent) -> u8 {
        #[allow(clippy::wildcard_enum_match_arm)]
        match event {
            CollectionEvent::TriggerRequest(trigger) => match trigger.priority {
                crate::event::TriggerPriority::Low => 1,
                crate::event::TriggerPriority::Normal => 5,
                crate::event::TriggerPriority::High => 8,
                crate::event::TriggerPriority::Critical => 10,
            },
            _ => 5, // Default priority for other event types
        }
    }

    /// Extract event metadata for filtering
    fn extract_event_metadata(event: &CollectionEvent) -> HashMap<String, String> {
        match event {
            CollectionEvent::Process(process_event) => {
                // Pre-allocate HashMap with estimated capacity to reduce reallocations
                let mut metadata = HashMap::with_capacity(8);
                metadata.insert("pid".to_owned(), process_event.pid.to_string());
                if let Some(ppid) = process_event.ppid {
                    metadata.insert("ppid".to_owned(), ppid.to_string());
                }
                metadata.insert("name".to_owned(), process_event.name.clone());
                if let Some(executable_path) = &process_event.executable_path {
                    metadata.insert("executable_path".to_owned(), executable_path.clone());
                }
                if let Some(user_id) = &process_event.user_id {
                    metadata.insert("user_id".to_owned(), user_id.clone());
                }
                metadata.insert(
                    "accessible".to_owned(),
                    process_event.accessible.to_string(),
                );
                metadata.insert(
                    "file_exists".to_owned(),
                    process_event.file_exists.to_string(),
                );
                metadata
            }
            CollectionEvent::Network(_) => HashMap::new(), // Future implementation
            CollectionEvent::Filesystem(_) => HashMap::new(), // Future implementation
            CollectionEvent::Performance(_) => HashMap::new(), // Future implementation
            CollectionEvent::TriggerRequest(trigger) => {
                // Pre-allocate HashMap with estimated capacity based on trigger metadata size
                let estimated_capacity = 5 + trigger.metadata.len();
                let mut metadata = HashMap::with_capacity(estimated_capacity);
                metadata.insert("trigger_id".to_owned(), trigger.trigger_id.clone());
                metadata.insert(
                    "target_collector".to_owned(),
                    trigger.target_collector.clone(),
                );
                metadata.insert(
                    "analysis_type".to_owned(),
                    format!("{:?}", trigger.analysis_type),
                );
                metadata.insert("priority".to_owned(), format!("{:?}", trigger.priority));
                if let Some(target_pid) = trigger.target_pid {
                    metadata.insert("target_pid".to_owned(), target_pid.to_string());
                }
                metadata.extend(trigger.metadata.clone());
                metadata
            }
        }
    }

    /// Extract source collector for filtering - use static strings to avoid allocations
    const fn extract_source_collector(event: &CollectionEvent) -> &'static str {
        match event {
            CollectionEvent::Process(_) => "procmond",
            CollectionEvent::Network(_) => "netmond",
            CollectionEvent::Filesystem(_) => "fsmond",
            CollectionEvent::Performance(_) => "perfmond",
            CollectionEvent::TriggerRequest(_) => "trigger-manager",
        }
    }

    /// Apply daemoneye-eventbus specific filtering
    fn apply_eventbus_filters(event: &CollectionEvent, filters: &EventBusFilters) -> bool {
        // Early return if no filters are set to avoid unnecessary work
        if filters.message_types.is_empty()
            && filters.topic_domains.is_empty()
            && filters.capability_filters.is_empty()
        {
            return true;
        }

        // Check message type filtering (events are always MessageType::Event) - avoid string allocation
        if !filters.message_types.is_empty()
            && !filters.message_types.iter().any(|mt| mt == "Event")
        {
            return false;
        }

        // Check topic domain filtering - avoid string allocation by comparing directly
        if !filters.topic_domains.is_empty() {
            let topic = Self::generate_topic_for_event(event);
            let domain = topic.split('.').next().unwrap_or("");
            // Avoid string allocation by using iterator instead of collecting to String
            if !filters.topic_domains.iter().any(|d| d == domain) {
                return false;
            }
        }

        // Check capability filtering - only extract capabilities if needed
        if !filters.capability_filters.is_empty() {
            let event_capabilities = Self::extract_event_capabilities(event);
            for (filter_key, filter_value) in &filters.capability_filters {
                match event_capabilities.get(filter_key) {
                    Some(value) if value == filter_value => {}
                    _ => return false,
                }
            }
        }

        // Sequence and timestamp filtering would be applied at the message level
        // in a full daemoneye-eventbus integration, but for LocalEventBus we skip these

        true
    }

    /// Extract event capabilities for filtering
    fn extract_event_capabilities(event: &CollectionEvent) -> HashMap<String, String> {
        // Pre-allocate HashMap with fixed capacity since we know the maximum size
        let mut capabilities = HashMap::with_capacity(3);

        match event {
            CollectionEvent::Process(_) => {
                capabilities.insert("domain".to_owned(), "process".to_owned());
                capabilities.insert("realtime".to_owned(), "true".to_owned());
                capabilities.insert("system_wide".to_owned(), "true".to_owned());
            }
            CollectionEvent::Network(_) => {
                capabilities.insert("domain".to_owned(), "network".to_owned());
                capabilities.insert("realtime".to_owned(), "true".to_owned());
            }
            CollectionEvent::Filesystem(_) => {
                capabilities.insert("domain".to_owned(), "filesystem".to_owned());
                capabilities.insert("realtime".to_owned(), "true".to_owned());
            }
            CollectionEvent::Performance(_) => {
                capabilities.insert("domain".to_owned(), "performance".to_owned());
                capabilities.insert("realtime".to_owned(), "false".to_owned());
            }
            CollectionEvent::TriggerRequest(_) => {
                capabilities.insert("domain".to_owned(), "control".to_owned());
                capabilities.insert("realtime".to_owned(), "true".to_owned());
            }
        }

        capabilities
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;
    use crate::event_bus::{
        CollectorCoordination, CrossCollectorCorrelation, EventBusCorrelation, EventBusFilters,
        EventBusMetadata, SequenceCorrelation, TemporalCorrelation, TopicChain, TopicCorrelation,
    };
    use std::time::Duration;
    use std::time::SystemTime;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_local_event_bus() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(5678),
            name: "test".to_owned(),
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned(), "command".to_owned()],
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_metadata = CorrelationMetadata::new("test-correlation".to_owned());
        bus.publish(event.clone(), correlation_metadata)
            .await
            .unwrap();

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
            subscriber_id: "process-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        // Subscribe to network events with wildcard pattern
        let network_subscription = EventSubscription {
            subscriber_id: "network-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::NETWORK,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.network.+".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut process_receiver = bus.subscribe(process_subscription).await.unwrap();
        let mut network_receiver = bus.subscribe(network_subscription).await.unwrap();

        // Publish a process event
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(5678),
            name: "test_process".to_owned(),
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned()],
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_metadata = CorrelationMetadata::new("test-correlation".to_owned())
            .with_tag("event_type".to_owned(), "process".to_owned());
        bus.publish(process_event.clone(), correlation_metadata)
            .await
            .unwrap();

        // Process subscriber should receive the event
        let received_process_event = timeout(Duration::from_secs(3), process_receiver.recv())
            .await
            .expect("timed out waiting for process event")
            .expect("process event channel closed");

        assert_eq!(received_process_event.event, process_event);
        assert_eq!(
            received_process_event.correlation_metadata.correlation_id,
            "test-correlation"
        );
        assert_eq!(
            received_process_event
                .correlation_metadata
                .correlation_tags
                .get("event_type"),
            Some(&"process".to_owned())
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
            subscriber_id: "process-only".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let trigger_subscription = EventSubscription {
            subscriber_id: "trigger-only".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["control.trigger.+".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut process_receiver = bus.subscribe(process_subscription).await.unwrap();
        let mut trigger_receiver = bus.subscribe(trigger_subscription).await.unwrap();

        // Publish a process event (should go to events.process.new topic)
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Publish a trigger request (should go to control.trigger.request topic)
        let trigger_event = CollectionEvent::TriggerRequest(crate::event::TriggerRequest {
            trigger_id: "test-trigger".to_owned(),
            target_collector: "test-collector".to_owned(),
            analysis_type: crate::event::AnalysisType::BinaryHash,
            priority: crate::event::TriggerPriority::Normal,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "test-correlation".to_owned(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        });

        // Publish both events with correlation metadata
        let process_correlation = CorrelationMetadata::new("process-correlation".to_owned())
            .with_tag("event_type".to_owned(), "process".to_owned());
        let trigger_correlation = CorrelationMetadata::new("trigger-correlation".to_owned())
            .with_tag("event_type".to_owned(), "trigger".to_owned());

        bus.publish(process_event.clone(), process_correlation)
            .await
            .unwrap();
        bus.publish(trigger_event.clone(), trigger_correlation)
            .await
            .unwrap();

        // Process subscriber should only receive the process event
        let received_process = timeout(Duration::from_secs(2), process_receiver.recv())
            .await
            .expect("timeout waiting for process event")
            .expect("process channel closed");

        assert!(matches!(
            &received_process.event,
            CollectionEvent::Process(_)
        ));
        assert_eq!(
            received_process.correlation_metadata.correlation_id,
            "process-correlation"
        );

        // Trigger subscriber should only receive the trigger event
        let received_trigger = timeout(Duration::from_secs(2), trigger_receiver.recv())
            .await
            .expect("timeout waiting for trigger event")
            .expect("trigger channel closed");

        assert!(matches!(
            &received_trigger.event,
            CollectionEvent::TriggerRequest(_)
        ));
        assert_eq!(
            received_trigger.correlation_metadata.correlation_id,
            "trigger-correlation"
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
            include_topics: vec!["events.process.lifecycle".to_owned()],
            exclude_topics: vec!["events.process.metadata".to_owned()],
            include_patterns: vec!["events.process.+".to_owned()],
            exclude_patterns: vec!["events.network.+".to_owned()],
            priority_topics: HashMap::new(),
        };

        let subscription = EventSubscription {
            subscriber_id: "filtered-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#".to_owned()]), // Subscribe to all events
            enable_wildcards: true,
            topic_filter: Some(topic_filter),
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        // Test include topic (should be delivered)
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // This should be delivered (matches include_topics)
        let correlation_metadata = CorrelationMetadata::new("test-1".to_owned());
        bus.publish(process_event.clone(), correlation_metadata)
            .await
            .unwrap();

        // Should receive the event
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for filtered event")
            .expect("channel closed");

        assert_eq!(received.correlation_metadata.correlation_id, "test-1");

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

    #[tokio::test]
    async fn test_correlation_filtering() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Create a subscription with correlation filtering
        let correlation_filter = CorrelationFilter {
            correlation_id: Some("target-correlation".to_owned()),
            parent_correlation_id: None,
            root_correlation_id: Some("root-workflow".to_owned()),
            workflow_stage: Some("analysis".to_owned()),
            correlation_tags: {
                let mut tags = HashMap::new();
                tags.insert("priority".to_owned(), "high".to_owned());
                tags
            },
            process_ids: vec![],
            eventbus_correlation: None, // No daemoneye-eventbus specific correlation
        };

        let subscription = EventSubscription {
            subscriber_id: "correlation-filtered-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: Some(correlation_filter),
            topic_patterns: Some(vec!["events.#".to_owned()]), // Subscribe to all events
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        // Create test events with different correlation metadata
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Event that should match the correlation filter
        let matching_correlation = CorrelationMetadata::with_parent(
            "target-correlation".to_owned(),
            "parent-id".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("analysis".to_owned())
        .with_tag("priority".to_owned(), "high".to_owned());

        // Event that should NOT match the correlation filter (wrong correlation ID)
        let non_matching_correlation = CorrelationMetadata::with_parent(
            "other-correlation".to_owned(),
            "parent-id".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("analysis".to_owned())
        .with_tag("priority".to_owned(), "high".to_owned());

        // Publish matching event
        bus.publish(process_event.clone(), matching_correlation)
            .await
            .unwrap();

        // Should receive the matching event
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for matching event")
            .expect("channel closed");

        assert_eq!(
            received.correlation_metadata.correlation_id,
            "target-correlation"
        );
        assert_eq!(
            received.correlation_metadata.root_correlation_id,
            "root-workflow"
        );
        assert_eq!(
            received.correlation_metadata.workflow_stage,
            Some("analysis".to_owned())
        );

        // Publish non-matching event
        bus.publish(process_event.clone(), non_matching_correlation)
            .await
            .unwrap();

        // Should NOT receive the non-matching event
        let no_event = timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(
            no_event.is_err(),
            "Should not receive event with non-matching correlation ID"
        );

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_hierarchical_correlation_tracking() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Subscribe to all events
        let subscription = EventSubscription {
            subscriber_id: "hierarchy-tracker".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Create a hierarchical correlation chain
        let root_correlation = CorrelationMetadata::new("root-workflow".to_owned())
            .with_stage("initial_detection".to_owned())
            .with_sequence(1)
            .with_tag(
                "workflow".to_owned(),
                "suspicious_process_analysis".to_owned(),
            );

        let child_correlation = CorrelationMetadata::with_parent(
            "child-analysis".to_owned(),
            "root-workflow".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("binary_analysis".to_owned())
        .with_sequence(2)
        .with_tag("analysis_type".to_owned(), "yara_scan".to_owned());

        let grandchild_correlation = CorrelationMetadata::with_parent(
            "grandchild-enrichment".to_owned(),
            "child-analysis".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("memory_analysis".to_owned())
        .with_sequence(3)
        .with_tag("analysis_type".to_owned(), "memory_dump".to_owned());

        // Publish events in hierarchical order
        bus.publish(process_event.clone(), root_correlation.clone())
            .await
            .unwrap();
        bus.publish(process_event.clone(), child_correlation.clone())
            .await
            .unwrap();
        bus.publish(process_event.clone(), grandchild_correlation.clone())
            .await
            .unwrap();

        // Verify root event
        let root_event = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for root event")
            .expect("channel closed");

        assert_eq!(
            root_event.correlation_metadata.correlation_id,
            "root-workflow"
        );
        assert_eq!(root_event.correlation_metadata.parent_correlation_id, None);
        assert_eq!(
            root_event.correlation_metadata.root_correlation_id,
            "root-workflow"
        );
        assert_eq!(root_event.correlation_metadata.sequence_number, 1);
        assert_eq!(
            root_event.correlation_metadata.workflow_stage,
            Some("initial_detection".to_owned())
        );

        // Verify child event
        let child_event = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for child event")
            .expect("channel closed");

        assert_eq!(
            child_event.correlation_metadata.correlation_id,
            "child-analysis"
        );
        assert_eq!(
            child_event.correlation_metadata.parent_correlation_id,
            Some("root-workflow".to_owned())
        );
        assert_eq!(
            child_event.correlation_metadata.root_correlation_id,
            "root-workflow"
        );
        assert_eq!(child_event.correlation_metadata.sequence_number, 2);
        assert_eq!(
            child_event.correlation_metadata.workflow_stage,
            Some("binary_analysis".to_owned())
        );

        // Verify grandchild event
        let grandchild_event = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for grandchild event")
            .expect("channel closed");

        assert_eq!(
            grandchild_event.correlation_metadata.correlation_id,
            "grandchild-enrichment"
        );
        assert_eq!(
            grandchild_event.correlation_metadata.parent_correlation_id,
            Some("child-analysis".to_owned())
        );
        assert_eq!(
            grandchild_event.correlation_metadata.root_correlation_id,
            "root-workflow"
        );
        assert_eq!(grandchild_event.correlation_metadata.sequence_number, 3);
        assert_eq!(
            grandchild_event.correlation_metadata.workflow_stage,
            Some("memory_analysis".to_owned())
        );

        bus.shutdown().await.unwrap();
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

    #[tokio::test]
    async fn test_enhanced_event_filtering_with_daemoneye_eventbus() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Create enhanced event filter with daemoneye-eventbus support
        let eventbus_filters = EventBusFilters {
            message_types: vec!["Event".to_owned()],
            sequence_range: None,
            timestamp_range: None,
            topic_domains: vec!["events".to_owned()],
            capability_filters: {
                let mut filters = HashMap::new();
                filters.insert("domain".to_owned(), "process".to_owned());
                filters.insert("realtime".to_owned(), "true".to_owned());
                filters
            },
        };

        let event_filter = EventFilter {
            event_types: vec!["process".to_owned()],
            pids: vec![1234, 5678],
            min_priority: Some(3),
            metadata_filters: {
                let mut filters = HashMap::new();
                filters.insert("name".to_owned(), "test_process".to_owned());
                filters
            },
            topic_filters: vec![],
            source_collectors: vec!["procmond".to_owned()],
            eventbus_filters: Some(eventbus_filters),
        };

        let subscription = EventSubscription {
            subscriber_id: "enhanced-filter-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: Some(event_filter),
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        // Create a process event that should match all filters
        let matching_process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234, // Matches PID filter
            ppid: None,
            name: "test_process".to_owned(), // Matches metadata filter
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned()],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Create a process event that should NOT match (wrong PID)
        let non_matching_process_event = CollectionEvent::Process(ProcessEvent {
            pid: 9999, // Does NOT match PID filter
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned()],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_metadata = CorrelationMetadata::new("filter-test".to_owned());

        // Publish matching event
        bus.publish(matching_process_event.clone(), correlation_metadata.clone())
            .await
            .unwrap();

        // Should receive the matching event
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for matching event")
            .expect("channel closed");

        assert!(matches!(&received.event, CollectionEvent::Process(_)));
        assert_eq!(received.correlation_metadata.correlation_id, "filter-test");

        // Publish non-matching event
        bus.publish(
            non_matching_process_event.clone(),
            correlation_metadata.clone(),
        )
        .await
        .unwrap();

        // Should NOT receive the non-matching event
        let no_event = timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(
            no_event.is_err(),
            "Should not receive event that doesn't match PID filter"
        );

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_enhanced_correlation_filtering_with_daemoneye_eventbus() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        // Create enhanced correlation filter with daemoneye-eventbus support
        let sequence_correlation = SequenceCorrelation {
            base_sequence: 100,
            window_size: 50,
            group_id: "test-group".to_owned(),
        };

        let topic_correlation = TopicCorrelation {
            source_patterns: vec!["events.process.+".to_owned()],
            target_patterns: vec!["events.#".to_owned()],
            correlation_keys: HashMap::new(),
        };

        let temporal_correlation = TemporalCorrelation {
            window_seconds: 60,
            max_events: Some(100),
            trigger_conditions: vec!["high_priority".to_owned()],
        };

        let cross_collector_correlation = CrossCollectorCorrelation {
            source_collectors: vec!["procmond".to_owned()],
            target_collectors: vec!["procmond".to_owned(), "netmond".to_owned()],
            correlation_metadata: HashMap::new(),
            stage_progression: vec!["detection".to_owned(), "analysis".to_owned()],
        };

        let eventbus_correlation = EventBusCorrelation {
            sequence_correlation: Some(sequence_correlation),
            topic_correlation: Some(topic_correlation),
            temporal_correlation: Some(temporal_correlation),
            cross_collector_correlation: Some(cross_collector_correlation),
        };

        let correlation_filter = CorrelationFilter {
            correlation_id: Some("enhanced-correlation".to_owned()),
            parent_correlation_id: None,
            root_correlation_id: Some("root-workflow".to_owned()),
            workflow_stage: Some("analysis".to_owned()),
            correlation_tags: {
                let mut tags = HashMap::new();
                tags.insert("priority".to_owned(), "high".to_owned());
                tags
            },
            process_ids: vec![],
            eventbus_correlation: Some(eventbus_correlation),
        };

        let subscription = EventSubscription {
            subscriber_id: "enhanced-correlation-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: Some(correlation_filter),
            topic_patterns: Some(vec!["events.#".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Create correlation metadata that should match the enhanced filters
        let collector_coordination = CollectorCoordination {
            initiator_collector: "procmond".to_owned(),
            target_collectors: vec!["procmond".to_owned(), "netmond".to_owned()],
            workflow_id: "test-workflow".to_owned(),
            coordination_stage: "analysis".to_owned(),
            coordination_data: HashMap::new(),
        };

        let eventbus_metadata = EventBusMetadata {
            broker_id: Some("test-broker".to_owned()),
            routing_path: vec!["events.process.new".to_owned()],
            delivery_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            delivery_tracking: HashMap::new(),
            topic_chains: vec![TopicChain {
                source_topic: "events.process.new".to_owned(),
                target_topic: "events.process.analysis".to_owned(),
                chain_id: "test-chain".to_owned(),
                chain_sequence: 125, // Within sequence window (100-150)
            }],
            collector_coordination: Some(collector_coordination),
        };

        let matching_correlation = CorrelationMetadata::with_parent(
            "enhanced-correlation".to_owned(),
            "parent-id".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("analysis".to_owned())
        .with_sequence(125) // Within sequence window
        .with_tag("priority".to_owned(), "high".to_owned())
        .with_eventbus_metadata(eventbus_metadata);

        // Publish matching event
        bus.publish(process_event.clone(), matching_correlation)
            .await
            .unwrap();

        // Should receive the matching event
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for enhanced correlation event")
            .expect("channel closed");

        assert_eq!(
            received.correlation_metadata.correlation_id,
            "enhanced-correlation"
        );
        assert_eq!(
            received.correlation_metadata.root_correlation_id,
            "root-workflow"
        );
        assert_eq!(received.correlation_metadata.sequence_number, 125);

        // Create correlation metadata that should NOT match (sequence out of range)
        let non_matching_correlation = CorrelationMetadata::with_parent(
            "enhanced-correlation".to_owned(),
            "parent-id".to_owned(),
            "root-workflow".to_owned(),
        )
        .with_stage("analysis".to_owned())
        .with_sequence(200) // Outside sequence window (100-150)
        .with_tag("priority".to_owned(), "high".to_owned());

        // Publish non-matching event
        bus.publish(process_event.clone(), non_matching_correlation)
            .await
            .unwrap();

        // Should NOT receive the non-matching event
        let no_event = timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(
            no_event.is_err(),
            "Should not receive event with sequence outside correlation window"
        );

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_correlation_metadata_tracking() {
        let config = EventBusConfig::default();
        let mut bus = LocalEventBus::new(config);

        let subscription = EventSubscription {
            subscriber_id: "tracking-subscriber".to_owned(),
            capabilities: crate::source::SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#".to_owned()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = bus.subscribe(subscription).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test_process".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        // Create correlation metadata with topic chains and delivery tracking
        let mut correlation_metadata = CorrelationMetadata::new("tracking-test".to_owned())
            .with_stage("initial_detection".to_owned())
            .with_sequence(1)
            .add_topic_chain(
                "events.process.new".to_owned(),
                "events.process.analysis".to_owned(),
                "chain-1".to_owned(),
            );

        // Track delivery
        correlation_metadata.track_delivery("tracking-subscriber".to_owned(), true, None);

        bus.publish(process_event.clone(), correlation_metadata.clone())
            .await
            .unwrap();

        // Receive and verify the event with tracking metadata
        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("timeout waiting for tracking event")
            .expect("channel closed");

        assert_eq!(
            received.correlation_metadata.correlation_id,
            "tracking-test"
        );
        assert_eq!(received.correlation_metadata.sequence_number, 1);
        assert_eq!(
            received.correlation_metadata.workflow_stage,
            Some("initial_detection".to_owned())
        );

        // Verify eventbus metadata was created
        assert!(received.correlation_metadata.eventbus_metadata.is_some());
        let eventbus_metadata = received
            .correlation_metadata
            .eventbus_metadata
            .clone()
            .unwrap();

        // Verify topic chains
        assert_eq!(eventbus_metadata.topic_chains.len(), 1);
        assert_eq!(
            eventbus_metadata.topic_chains[0].source_topic,
            "events.process.new"
        );
        assert_eq!(
            eventbus_metadata.topic_chains[0].target_topic,
            "events.process.analysis"
        );
        assert_eq!(eventbus_metadata.topic_chains[0].chain_id, "chain-1");

        // Verify delivery tracking
        assert!(
            eventbus_metadata
                .delivery_tracking
                .contains_key("tracking-subscriber")
        );
        let delivery_status = &eventbus_metadata.delivery_tracking["tracking-subscriber"];
        assert!(delivery_status.success);
        assert_eq!(delivery_status.retry_count, 0);

        bus.shutdown().await.unwrap();
    }
}
