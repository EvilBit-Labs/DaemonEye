//! Message types and serialization for the EventBus

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

/// A message sent through the event bus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier
    pub id: Uuid,
    /// Topic this message is published to
    pub topic: String,
    /// Correlation ID for tracking related events
    pub correlation_id: String,
    /// Message payload (serialized data)
    pub payload: Vec<u8>,
    /// Message sequence number for ordering
    pub sequence: u64,
    /// Timestamp when message was created
    pub timestamp: SystemTime,
    /// Message type for routing
    pub message_type: MessageType,
}

/// Types of messages for routing and handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// Regular event message
    Event,
    /// Control message (subscribe, unsubscribe, etc.)
    Control,
    /// Heartbeat/keepalive message
    Heartbeat,
    /// Shutdown message
    Shutdown,
}

/// A bus event that subscribers receive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    /// Unique identifier for this bus event
    pub event_id: String,
    /// Original collection event
    pub event: CollectionEvent,
    /// Correlation ID for tracking related events
    pub correlation_id: String,
    /// Timestamp when event was published to the bus
    pub bus_timestamp: SystemTime,
    /// Topic pattern that matched this message
    pub matched_pattern: String,
    /// Subscriber ID that received this event
    pub subscriber_id: String,
}

impl Message {
    /// Create a new message
    pub fn new(
        topic: String,
        correlation_id: String,
        payload: Vec<u8>,
        sequence: u64,
        message_type: MessageType,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            topic,
            correlation_id,
            payload,
            sequence,
            timestamp: SystemTime::now(),
            message_type,
        }
    }

    /// Create a new event message
    pub fn event(topic: String, correlation_id: String, payload: Vec<u8>, sequence: u64) -> Self {
        Self::new(topic, correlation_id, payload, sequence, MessageType::Event)
    }

    /// Create a new control message
    pub fn control(topic: String, correlation_id: String, payload: Vec<u8>, sequence: u64) -> Self {
        Self::new(
            topic,
            correlation_id,
            payload,
            sequence,
            MessageType::Control,
        )
    }

    /// Create a heartbeat message
    pub fn heartbeat(sequence: u64) -> Self {
        Self::new(
            "heartbeat".to_string(),
            "system".to_string(),
            Vec::new(),
            sequence,
            MessageType::Heartbeat,
        )
    }

    /// Create a shutdown message
    pub fn shutdown() -> Self {
        Self::new(
            "shutdown".to_string(),
            "system".to_string(),
            Vec::new(),
            0,
            MessageType::Shutdown,
        )
    }

    /// Serialize message to bytes
    pub fn serialize(&self) -> Result<Vec<u8>, crate::error::EventBusError> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))
    }

    /// Deserialize message from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self, crate::error::EventBusError> {
        bincode::serde::decode_from_slice(data, bincode::config::standard())
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))
            .map(|(result, _)| result)
    }
}

impl BusEvent {
    /// Create a new bus event
    pub fn new(
        event: CollectionEvent,
        correlation_id: String,
        matched_pattern: String,
        subscriber_id: String,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            event,
            correlation_id,
            bus_timestamp: SystemTime::now(),
            matched_pattern,
            subscriber_id,
        }
    }
}

impl Default for MessageType {
    fn default() -> Self {
        Self::Event
    }
}

/// Collection event types for compatibility with collector-core
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectionEvent {
    /// Process monitoring events
    Process(ProcessEvent),
    /// Network monitoring events (future extension)
    Network(NetworkEvent),
    /// Filesystem monitoring events (future extension)
    Filesystem(FilesystemEvent),
    /// Performance monitoring events (future extension)
    Performance(PerformanceEvent),
    /// Analysis collector trigger requests
    TriggerRequest(TriggerRequest),
}

/// Process monitoring event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEvent {
    /// Process identifier
    pub pid: u32,
    /// Process name
    pub name: String,
    /// Command line arguments
    pub command_line: Option<String>,
    /// Executable path
    pub executable_path: Option<String>,
    /// Parent process ID
    pub ppid: Option<u32>,
    /// Process start time
    pub start_time: Option<SystemTime>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Network monitoring event data (placeholder)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEvent {
    /// Connection identifier
    pub connection_id: String,
    /// Source address
    pub source_address: String,
    /// Destination address
    pub destination_address: String,
    /// Protocol
    pub protocol: String,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Filesystem monitoring event data (placeholder)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemEvent {
    /// File path
    pub path: String,
    /// Event type (create, modify, delete, etc.)
    pub event_type: String,
    /// File size
    pub size: Option<u64>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Performance monitoring event data (placeholder)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceEvent {
    /// Metric name
    pub metric_name: String,
    /// Metric value
    pub value: f64,
    /// Unit
    pub unit: String,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Trigger request for analysis collectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerRequest {
    /// Request identifier
    pub request_id: String,
    /// Target collector type
    pub collector_type: String,
    /// Priority level
    pub priority: u8,
    /// Request payload
    pub payload: Vec<u8>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Event subscription configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    /// Unique identifier for the subscriber
    pub subscriber_id: String,
    /// Capabilities that this subscriber can handle
    pub capabilities: SourceCaps,
    /// Optional event type filter
    pub event_filter: Option<EventFilter>,
    /// Optional correlation ID filter
    pub correlation_filter: Option<CorrelationFilter>,
    /// Optional explicit topic patterns
    pub topic_patterns: Option<Vec<String>>,
    /// Enable wildcarding support for topic patterns
    pub enable_wildcards: bool,
}

/// Source capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCaps {
    /// Supported event types
    pub event_types: Vec<String>,
    /// Supported collectors
    pub collectors: Vec<String>,
    /// Maximum priority level
    pub max_priority: u8,
}

/// Event filtering criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    /// Event types to include
    pub event_types: Vec<String>,
    /// Process IDs to include
    pub pids: Vec<u32>,
    /// Minimum priority level
    pub min_priority: Option<u8>,
    /// Custom metadata filters
    pub metadata_filters: HashMap<String, String>,
    /// Topic pattern filters
    pub topic_filters: Vec<String>,
    /// Source collector filters
    pub source_collectors: Vec<String>,
}

/// Correlation ID filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    /// Specific correlation IDs to include
    pub correlation_ids: Vec<String>,
    /// Correlation ID patterns to match
    pub correlation_patterns: Vec<String>,
}
