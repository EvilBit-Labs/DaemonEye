//! Event bus configuration, subscription, filter, and correlation types.
//!
//! This submodule holds the data types used by the event bus: configuration,
//! subscriptions, filters, correlation metadata, bus events, and statistics.

use crate::{event::CollectionEvent, source::SourceCaps};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

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
    /// Correlation filter with daemoneye-eventbus support
    pub correlation_filter: Option<CorrelationFilter>,
    /// Topic patterns using daemoneye-eventbus syntax
    /// Supports hierarchical patterns like "events.process.*", "control.#", etc.
    /// Wildcards: + (single-level), # (multi-level, must be at end)
    pub topic_patterns: Option<Vec<String>>,
    /// Enable wildcards (always true for daemoneye-eventbus compatibility)
    pub enable_wildcards: bool,
    /// Topic filter for backward compatibility with existing event filtering
    pub topic_filter: Option<TopicFilter>,
}

/// Event filtering criteria for subscribers with daemoneye-eventbus integration
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
    /// Topic filters (deprecated - use `topic_patterns` in `EventSubscription`)
    pub topic_filters: Vec<String>,
    /// Source collectors
    pub source_collectors: Vec<String>,
    /// daemoneye-eventbus specific filters
    pub eventbus_filters: Option<EventBusFilters>,
}

/// daemoneye-eventbus specific filtering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusFilters {
    /// Message type filters (Event, Control, Heartbeat, Shutdown)
    pub message_types: Vec<String>,
    /// Sequence number range filtering
    pub sequence_range: Option<(u64, u64)>,
    /// Timestamp range filtering (Unix timestamps)
    pub timestamp_range: Option<(u64, u64)>,
    /// Topic domain filters (events, control)
    pub topic_domains: Vec<String>,
    /// Subscriber capability filters
    pub capability_filters: HashMap<String, String>,
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

/// Event correlation information with daemoneye-eventbus support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    /// Correlation ID pattern
    pub correlation_id: Option<String>,
    /// Parent correlation ID for hierarchical relationships
    pub parent_correlation_id: Option<String>,
    /// Root correlation ID for entire workflow
    pub root_correlation_id: Option<String>,
    /// Workflow stage filter
    pub workflow_stage: Option<String>,
    /// Correlation tags for flexible filtering
    pub correlation_tags: HashMap<String, String>,
    /// Process ID filters (legacy compatibility)
    pub process_ids: Vec<u32>,
    /// daemoneye-eventbus correlation patterns
    pub eventbus_correlation: Option<EventBusCorrelation>,
}

/// daemoneye-eventbus specific correlation tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusCorrelation {
    /// Message sequence correlation (track related messages by sequence)
    pub sequence_correlation: Option<SequenceCorrelation>,
    /// Topic-based correlation (correlate events across topics)
    pub topic_correlation: Option<TopicCorrelation>,
    /// Temporal correlation (correlate events within time windows)
    pub temporal_correlation: Option<TemporalCorrelation>,
    /// Cross-collector correlation (correlate events from different collectors)
    pub cross_collector_correlation: Option<CrossCollectorCorrelation>,
}

/// Sequence-based correlation for message ordering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceCorrelation {
    /// Base sequence number for correlation group
    pub base_sequence: u64,
    /// Sequence window size for correlation
    pub window_size: u64,
    /// Correlation group identifier
    pub group_id: String,
}

/// Topic-based correlation for cross-topic event tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicCorrelation {
    /// Source topic patterns to correlate from
    pub source_patterns: Vec<String>,
    /// Target topic patterns to correlate to
    pub target_patterns: Vec<String>,
    /// Correlation key extraction rules
    pub correlation_keys: HashMap<String, String>,
}

/// Temporal correlation for time-based event grouping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalCorrelation {
    /// Time window for correlation (seconds)
    pub window_seconds: u64,
    /// Maximum events per correlation group
    pub max_events: Option<usize>,
    /// Correlation trigger conditions
    pub trigger_conditions: Vec<String>,
}

/// Cross-collector correlation for multi-collector workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossCollectorCorrelation {
    /// Source collector types
    pub source_collectors: Vec<String>,
    /// Target collector types
    pub target_collectors: Vec<String>,
    /// Correlation metadata keys
    pub correlation_metadata: HashMap<String, String>,
    /// Workflow stage progression
    pub stage_progression: Vec<String>,
}

/// Bus event wrapper with daemoneye-eventbus correlation metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    /// Event ID
    pub id: Uuid,
    /// Event timestamp (Unix timestamp in seconds)
    pub timestamp: u64,
    /// Event payload
    pub event: CollectionEvent,
    /// Correlation metadata for daemoneye-eventbus integration
    pub correlation_metadata: CorrelationMetadata,
    /// Routing metadata
    pub routing_metadata: HashMap<String, String>,
}

/// Correlation metadata for daemoneye-eventbus integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationMetadata {
    /// Primary correlation ID for tracking related events across collectors
    pub correlation_id: String,
    /// Parent correlation ID for hierarchical event relationships
    pub parent_correlation_id: Option<String>,
    /// Root correlation ID for the entire workflow
    pub root_correlation_id: String,
    /// Trace ID for distributed tracing integration
    pub trace_id: Option<String>,
    /// Span ID for distributed tracing integration
    pub span_id: Option<String>,
    /// Event sequence number within correlation group
    pub sequence_number: u64,
    /// Workflow stage identifier
    pub workflow_stage: Option<String>,
    /// Custom correlation tags for flexible grouping
    pub correlation_tags: HashMap<String, String>,
    /// daemoneye-eventbus specific correlation metadata
    pub eventbus_metadata: Option<EventBusMetadata>,
}

/// daemoneye-eventbus specific correlation metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusMetadata {
    /// Message broker instance ID
    pub broker_id: Option<String>,
    /// Topic routing path through the broker
    pub routing_path: Vec<String>,
    /// Message delivery timestamp (Unix timestamp)
    pub delivery_timestamp: u64,
    /// Subscriber delivery tracking
    pub delivery_tracking: HashMap<String, DeliveryStatus>,
    /// Cross-topic correlation chains
    pub topic_chains: Vec<TopicChain>,
    /// Collector coordination metadata
    pub collector_coordination: Option<CollectorCoordination>,
}

/// Message delivery status tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryStatus {
    /// Delivery attempt timestamp
    pub timestamp: u64,
    /// Delivery success status
    pub success: bool,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// Retry count
    pub retry_count: u32,
}

/// Topic correlation chain for cross-topic event tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicChain {
    /// Source topic
    pub source_topic: String,
    /// Target topic
    pub target_topic: String,
    /// Chain correlation ID
    pub chain_id: String,
    /// Chain sequence number
    pub chain_sequence: u64,
}

/// Collector coordination metadata for multi-collector workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorCoordination {
    /// Initiating collector ID
    pub initiator_collector: String,
    /// Target collectors for coordination
    pub target_collectors: Vec<String>,
    /// Coordination workflow ID
    pub workflow_id: String,
    /// Coordination stage
    pub coordination_stage: String,
    /// Coordination metadata
    pub coordination_data: HashMap<String, String>,
}

impl CorrelationMetadata {
    /// Create a new correlation metadata instance
    pub fn new(correlation_id: String) -> Self {
        let root_id = correlation_id.clone();
        Self {
            correlation_id,
            parent_correlation_id: None,
            root_correlation_id: root_id,
            trace_id: None,
            span_id: None,
            sequence_number: 0,
            workflow_stage: None,
            correlation_tags: HashMap::new(),
            eventbus_metadata: None,
        }
    }

    /// Create correlation metadata with parent relationship
    pub fn with_parent(
        correlation_id: String,
        parent_correlation_id: String,
        root_correlation_id: String,
    ) -> Self {
        Self {
            correlation_id,
            parent_correlation_id: Some(parent_correlation_id),
            root_correlation_id,
            trace_id: None,
            span_id: None,
            sequence_number: 0,
            workflow_stage: None,
            correlation_tags: HashMap::new(),
            eventbus_metadata: None,
        }
    }

    /// Set workflow stage
    #[must_use]
    pub fn with_stage(mut self, stage: String) -> Self {
        self.workflow_stage = Some(stage);
        self
    }

    /// Add correlation tag
    #[must_use]
    pub fn with_tag(mut self, key: String, value: String) -> Self {
        self.correlation_tags.insert(key, value);
        self
    }

    /// Set sequence number
    #[must_use]
    pub const fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence_number = sequence;
        self
    }

    /// Set trace ID for distributed tracing
    #[must_use]
    pub fn with_trace_id(mut self, trace_id: String) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Set span ID for distributed tracing
    #[must_use]
    pub fn with_span_id(mut self, span_id: String) -> Self {
        self.span_id = Some(span_id);
        self
    }

    /// Set daemoneye-eventbus metadata
    #[must_use]
    pub fn with_eventbus_metadata(mut self, metadata: EventBusMetadata) -> Self {
        self.eventbus_metadata = Some(metadata);
        self
    }

    /// Add topic chain for cross-topic correlation
    #[must_use]
    pub fn add_topic_chain(
        mut self,
        source_topic: String,
        target_topic: String,
        chain_id: String,
    ) -> Self {
        let chain = TopicChain {
            source_topic,
            target_topic,
            chain_id,
            chain_sequence: self.sequence_number,
        };

        if let Some(ref mut eventbus_metadata) = self.eventbus_metadata {
            eventbus_metadata.topic_chains.push(chain);
        } else {
            let metadata = EventBusMetadata {
                broker_id: None,
                routing_path: Vec::new(),
                delivery_timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                delivery_tracking: HashMap::new(),
                topic_chains: vec![chain],
                collector_coordination: None,
            };
            self.eventbus_metadata = Some(metadata);
        }
        self
    }

    /// Set collector coordination metadata
    #[must_use]
    pub fn with_collector_coordination(mut self, coordination: CollectorCoordination) -> Self {
        if let Some(ref mut eventbus_metadata) = self.eventbus_metadata {
            eventbus_metadata.collector_coordination = Some(coordination);
        } else {
            let metadata = EventBusMetadata {
                broker_id: None,
                routing_path: Vec::new(),
                delivery_timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                delivery_tracking: HashMap::new(),
                topic_chains: Vec::new(),
                collector_coordination: Some(coordination),
            };
            self.eventbus_metadata = Some(metadata);
        }
        self
    }

    /// Track message delivery to a subscriber
    pub fn track_delivery(
        &mut self,
        subscriber_id: String,
        success: bool,
        error_message: Option<String>,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let delivery_status = DeliveryStatus {
            timestamp: now,
            success,
            error_message,
            retry_count: 0,
        };

        if let Some(ref mut eventbus_metadata) = self.eventbus_metadata {
            // Refresh the delivery timestamp on every tracked delivery so that
            // pre-existing metadata does not carry a stale value into later
            // temporal-correlation / delivery-window checks.
            eventbus_metadata.delivery_timestamp = now;
            eventbus_metadata
                .delivery_tracking
                .insert(subscriber_id, delivery_status);
        } else {
            let mut delivery_tracking = HashMap::new();
            delivery_tracking.insert(subscriber_id, delivery_status);

            let metadata = EventBusMetadata {
                broker_id: None,
                routing_path: Vec::new(),
                delivery_timestamp: now,
                delivery_tracking,
                topic_chains: Vec::new(),
                collector_coordination: None,
            };
            self.eventbus_metadata = Some(metadata);
        }
    }
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreadable_literal,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    /// A clearly stale Unix timestamp (2001-09-09) used to seed pre-existing
    /// metadata so a refresh is observable.
    const STALE_TIMESTAMP: u64 = 1_000_000_000;

    fn current_unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    #[test]
    fn track_delivery_refreshes_stale_delivery_timestamp_on_existing_metadata() {
        // Arrange: correlation metadata that already carries eventbus metadata
        // with a stale delivery timestamp (the regression scenario).
        let mut metadata = CorrelationMetadata::new("corr-1".to_owned());
        metadata.eventbus_metadata = Some(EventBusMetadata {
            broker_id: None,
            routing_path: Vec::new(),
            delivery_timestamp: STALE_TIMESTAMP,
            delivery_tracking: HashMap::new(),
            topic_chains: Vec::new(),
            collector_coordination: None,
        });

        // Act: record a delivery against the pre-existing metadata.
        metadata.track_delivery("subscriber-1".to_owned(), true, None);

        // Assert: the delivery timestamp must advance past the stale value so
        // temporal-correlation/delivery-window checks see a fresh timestamp.
        let eventbus_metadata = metadata
            .eventbus_metadata
            .as_ref()
            .expect("eventbus metadata should be present");
        assert!(
            eventbus_metadata.delivery_timestamp > STALE_TIMESTAMP,
            "delivery_timestamp must be refreshed on every tracked delivery (was {}, expected > {})",
            eventbus_metadata.delivery_timestamp,
            STALE_TIMESTAMP
        );
        assert!(eventbus_metadata.delivery_timestamp >= current_unix_secs().saturating_sub(5));
    }

    #[test]
    fn track_delivery_records_status_on_first_create() {
        let mut metadata = CorrelationMetadata::new("corr-2".to_owned());
        assert!(metadata.eventbus_metadata.is_none());

        metadata.track_delivery("subscriber-1".to_owned(), false, Some("boom".to_owned()));

        let eventbus_metadata = metadata
            .eventbus_metadata
            .as_ref()
            .expect("eventbus metadata should be created");
        assert!(eventbus_metadata.delivery_timestamp >= current_unix_secs().saturating_sub(5));
        let status = eventbus_metadata
            .delivery_tracking
            .get("subscriber-1")
            .expect("delivery status should be recorded");
        assert!(!status.success);
        assert_eq!(status.error_message.as_deref(), Some("boom"));
    }
}
