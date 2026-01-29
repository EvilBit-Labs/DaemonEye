//! Message types and serialization for the EventBus

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

/// Correlation metadata for multi-collector workflow support and forensic tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationMetadata {
    /// Primary correlation ID for tracking related events across collectors
    pub correlation_id: String,
    /// Parent correlation ID for hierarchical event relationships
    pub parent_correlation_id: Option<String>,
    /// Root correlation ID for the entire workflow
    pub root_correlation_id: String,
    /// Event sequence number within correlation group
    pub sequence_number: u64,
    /// Workflow stage identifier (e.g., "collection", "analysis", "alerting")
    pub workflow_stage: Option<String>,
    /// Custom correlation tags for flexible grouping and filtering
    pub correlation_tags: HashMap<String, String>,
    /// Timestamp when correlation was created
    pub created_at: SystemTime,
}

impl CorrelationMetadata {
    /// Create a new correlation metadata instance
    ///
    /// # Security
    /// - Validates correlation ID length to prevent resource exhaustion
    pub fn new(correlation_id: String) -> Self {
        // Security: Limit correlation ID length
        const MAX_CORRELATION_ID_LENGTH: usize = 256;
        let bounded_id = if correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
            correlation_id[..MAX_CORRELATION_ID_LENGTH].to_string()
        } else {
            correlation_id
        };

        Self {
            root_correlation_id: bounded_id.clone(),
            correlation_id: bounded_id,
            parent_correlation_id: None,
            sequence_number: 0,
            workflow_stage: None,
            correlation_tags: HashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    /// Create correlation metadata with parent relationship
    ///
    /// # Security
    /// - Validates all ID lengths to prevent resource exhaustion
    pub fn with_parent(
        correlation_id: String,
        parent_correlation_id: String,
        root_correlation_id: String,
    ) -> Self {
        // Security: Limit correlation ID lengths
        const MAX_CORRELATION_ID_LENGTH: usize = 256;

        let bounded_id = if correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
            correlation_id[..MAX_CORRELATION_ID_LENGTH].to_string()
        } else {
            correlation_id
        };

        let bounded_parent = if parent_correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
            parent_correlation_id[..MAX_CORRELATION_ID_LENGTH].to_string()
        } else {
            parent_correlation_id
        };

        let bounded_root = if root_correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
            root_correlation_id[..MAX_CORRELATION_ID_LENGTH].to_string()
        } else {
            root_correlation_id
        };

        Self {
            correlation_id: bounded_id,
            parent_correlation_id: Some(bounded_parent),
            root_correlation_id: bounded_root,
            sequence_number: 0,
            workflow_stage: None,
            correlation_tags: HashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    /// Set workflow stage
    ///
    /// # Security
    /// - Limits workflow stage length to prevent resource exhaustion
    pub fn with_stage(mut self, stage: String) -> Self {
        // Security: Limit workflow stage length
        const MAX_STAGE_LENGTH: usize = 128;

        let bounded_stage = if stage.len() > MAX_STAGE_LENGTH {
            stage[..MAX_STAGE_LENGTH].to_string()
        } else {
            stage
        };

        self.workflow_stage = Some(bounded_stage);
        self
    }

    /// Add correlation tag
    ///
    /// # Security
    /// - Limits number of tags to prevent resource exhaustion
    /// - Bounds tag key and value lengths
    pub fn with_tag(mut self, key: String, value: String) -> Self {
        // Security: Limit number of tags to prevent resource exhaustion
        const MAX_TAGS: usize = 64;
        const MAX_TAG_KEY_LENGTH: usize = 128;
        const MAX_TAG_VALUE_LENGTH: usize = 1024;

        if self.correlation_tags.len() < MAX_TAGS
            && key.len() <= MAX_TAG_KEY_LENGTH
            && value.len() <= MAX_TAG_VALUE_LENGTH
        {
            self.correlation_tags.insert(key, value);
        }
        self
    }

    /// Set sequence number
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence_number = sequence;
        self
    }

    /// Increment sequence number
    pub fn increment_sequence(&mut self) {
        self.sequence_number = self.sequence_number.saturating_add(1);
    }

    /// Create a child correlation from this one
    pub fn create_child(&self, child_id: String) -> Self {
        Self {
            correlation_id: child_id,
            parent_correlation_id: Some(self.correlation_id.clone()),
            root_correlation_id: self.root_correlation_id.clone(),
            sequence_number: 0,
            workflow_stage: self.workflow_stage.clone(),
            correlation_tags: self.correlation_tags.clone(),
            created_at: SystemTime::now(),
        }
    }

    /// Check if this correlation matches a filter pattern
    ///
    /// # Security
    /// - Limits regex pattern length to prevent ReDoS attacks
    /// - Escapes regex special characters to prevent regex injection
    /// - Uses anchored full-string matching for security
    pub fn matches_pattern(&self, pattern: &str) -> bool {
        // Security: Limit pattern length to prevent ReDoS attacks
        const MAX_PATTERN_LENGTH: usize = 256;
        if pattern.len() > MAX_PATTERN_LENGTH {
            return false;
        }

        // Support wildcard matching for correlation IDs using glob-style matching
        if pattern.contains('*') {
            // Escape all regex special characters except *
            let escaped_pattern = regex::escape(pattern);
            // Replace escaped \* with .* for wildcard matching
            let regex_pattern = escaped_pattern.replace("\\*", ".*");
            // Anchor with ^ and $ for full-string matching
            let anchored_pattern = format!("^{}$", regex_pattern);
            if let Ok(regex) = regex::Regex::new(&anchored_pattern) {
                return regex.is_match(&self.correlation_id)
                    || self
                        .parent_correlation_id
                        .as_ref()
                        .map(|p| regex.is_match(p))
                        .unwrap_or(false)
                    || regex.is_match(&self.root_correlation_id);
            }
        }

        // Exact match
        self.correlation_id == pattern
            || self
                .parent_correlation_id
                .as_ref()
                .map(|p| p == pattern)
                .unwrap_or(false)
            || self.root_correlation_id == pattern
    }

    /// Check if this correlation has a specific tag
    pub fn has_tag(&self, key: &str, value: &str) -> bool {
        self.correlation_tags
            .get(key)
            .map(|v| v == value)
            .unwrap_or(false)
    }

    /// Check if this correlation is in a specific workflow stage
    pub fn in_stage(&self, stage: &str) -> bool {
        self.workflow_stage
            .as_ref()
            .map(|s| s == stage)
            .unwrap_or(false)
    }
}

impl Default for CorrelationMetadata {
    fn default() -> Self {
        Self::new(Uuid::new_v4().to_string())
    }
}

/// A message sent through the event bus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier
    pub id: Uuid,
    /// Topic this message is published to
    pub topic: String,
    /// Correlation metadata for tracking related events and workflows
    pub correlation_metadata: CorrelationMetadata,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MessageType {
    /// Regular event message
    #[default]
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
    /// Correlation metadata for tracking related events and workflows
    pub correlation_metadata: CorrelationMetadata,
    /// Timestamp when event was published to the bus
    pub bus_timestamp: SystemTime,
    /// Topic pattern that matched this message
    pub matched_pattern: String,
    /// Subscriber ID that received this event
    pub subscriber_id: String,
}

impl Message {
    /// Create a new message with correlation metadata
    pub fn new(
        topic: String,
        correlation_metadata: CorrelationMetadata,
        payload: Vec<u8>,
        sequence: u64,
        message_type: MessageType,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            topic,
            correlation_metadata,
            payload,
            sequence,
            timestamp: SystemTime::now(),
            message_type,
        }
    }

    /// Create a new message with simple correlation ID (for backward compatibility)
    pub fn new_with_correlation_id(
        topic: String,
        correlation_id: String,
        payload: Vec<u8>,
        sequence: u64,
        message_type: MessageType,
    ) -> Self {
        Self::new(
            topic,
            CorrelationMetadata::new(correlation_id),
            payload,
            sequence,
            message_type,
        )
    }

    /// Create a new event message
    pub fn event(topic: String, correlation_id: String, payload: Vec<u8>, sequence: u64) -> Self {
        Self::new_with_correlation_id(topic, correlation_id, payload, sequence, MessageType::Event)
    }

    /// Create a new event message with full correlation metadata
    pub fn event_with_metadata(
        topic: String,
        correlation_metadata: CorrelationMetadata,
        payload: Vec<u8>,
        sequence: u64,
    ) -> Self {
        Self::new(
            topic,
            correlation_metadata,
            payload,
            sequence,
            MessageType::Event,
        )
    }

    /// Create a new control message
    pub fn control(topic: String, correlation_id: String, payload: Vec<u8>, sequence: u64) -> Self {
        Self::new_with_correlation_id(
            topic,
            correlation_id,
            payload,
            sequence,
            MessageType::Control,
        )
    }

    /// Create a new control message with full correlation metadata
    pub fn control_with_metadata(
        topic: String,
        correlation_metadata: CorrelationMetadata,
        payload: Vec<u8>,
        sequence: u64,
    ) -> Self {
        Self::new(
            topic,
            correlation_metadata,
            payload,
            sequence,
            MessageType::Control,
        )
    }

    /// Create a heartbeat message
    pub fn heartbeat(sequence: u64) -> Self {
        Self::new_with_correlation_id(
            "heartbeat".to_string(),
            "system".to_string(),
            Vec::new(),
            sequence,
            MessageType::Heartbeat,
        )
    }

    /// Create a shutdown message
    pub fn shutdown() -> Self {
        Self::new_with_correlation_id(
            "shutdown".to_string(),
            "system".to_string(),
            Vec::new(),
            0,
            MessageType::Shutdown,
        )
    }

    /// Create an RPC request message
    pub fn rpc_request(
        topic: String,
        request: &crate::rpc::RpcRequest,
    ) -> Result<Self, crate::error::EventBusError> {
        let payload = postcard::to_allocvec(request)
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))?;

        // Convert RpcCorrelationMetadata to CorrelationMetadata
        let correlation_metadata = CorrelationMetadata {
            correlation_id: request.correlation_metadata.correlation_id.clone(),
            parent_correlation_id: request.correlation_metadata.parent_correlation_id.clone(),
            root_correlation_id: request.correlation_metadata.root_correlation_id.clone(),
            sequence_number: request.correlation_metadata.sequence_number,
            workflow_stage: request.correlation_metadata.workflow_stage.clone(),
            correlation_tags: request.correlation_metadata.correlation_tags.clone(),
            created_at: SystemTime::now(),
        };

        Ok(Self::control_with_metadata(
            topic,
            correlation_metadata,
            payload,
            0, // Sequence will be set by broker
        ))
    }

    /// Create an RPC response message
    pub fn rpc_response(
        topic: String,
        response: &crate::rpc::RpcResponse,
    ) -> Result<Self, crate::error::EventBusError> {
        let payload = postcard::to_allocvec(response)
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))?;

        // Convert RpcCorrelationMetadata to CorrelationMetadata
        let correlation_metadata = CorrelationMetadata {
            correlation_id: response.correlation_metadata.correlation_id.clone(),
            parent_correlation_id: response.correlation_metadata.parent_correlation_id.clone(),
            root_correlation_id: response.correlation_metadata.root_correlation_id.clone(),
            sequence_number: response.correlation_metadata.sequence_number,
            workflow_stage: response.correlation_metadata.workflow_stage.clone(),
            correlation_tags: response.correlation_metadata.correlation_tags.clone(),
            created_at: SystemTime::now(),
        };

        Ok(Self::control_with_metadata(
            topic,
            correlation_metadata,
            payload,
            0, // Sequence will be set by broker
        ))
    }

    /// Serialize message to bytes
    pub fn serialize(&self) -> Result<Vec<u8>, crate::error::EventBusError> {
        postcard::to_allocvec(self)
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))
    }

    /// Deserialize message from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self, crate::error::EventBusError> {
        postcard::from_bytes(data)
            .map_err(|e| crate::error::EventBusError::serialization(e.to_string()))
    }
}

impl BusEvent {
    /// Create a new bus event with correlation metadata
    pub fn new(
        event: CollectionEvent,
        correlation_metadata: CorrelationMetadata,
        matched_pattern: String,
        subscriber_id: String,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            event,
            correlation_metadata,
            bus_timestamp: SystemTime::now(),
            matched_pattern,
            subscriber_id,
        }
    }

    /// Create a new bus event with simple correlation ID (for backward compatibility)
    pub fn new_with_correlation_id(
        event: CollectionEvent,
        correlation_id: String,
        matched_pattern: String,
        subscriber_id: String,
    ) -> Self {
        Self::new(
            event,
            CorrelationMetadata::new(correlation_id),
            matched_pattern,
            subscriber_id,
        )
    }

    /// Get the correlation ID for backward compatibility
    pub fn correlation_id(&self) -> &str {
        &self.correlation_metadata.correlation_id
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

/// Correlation ID filtering with support for hierarchical and workflow-based filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    /// Specific correlation IDs to include
    pub correlation_ids: Vec<String>,
    /// Correlation ID patterns to match (supports wildcards)
    pub correlation_patterns: Vec<String>,
    /// Filter by parent correlation IDs
    pub parent_correlation_ids: Vec<String>,
    /// Filter by root correlation IDs (entire workflow)
    pub root_correlation_ids: Vec<String>,
    /// Filter by workflow stage
    pub workflow_stages: Vec<String>,
    /// Filter by correlation tags (all tags must match)
    pub required_tags: HashMap<String, String>,
    /// Filter by any of these correlation tags (at least one must match)
    pub any_tags: HashMap<String, String>,
    /// Minimum sequence number
    pub min_sequence: Option<u64>,
    /// Maximum sequence number
    pub max_sequence: Option<u64>,
}

impl CorrelationFilter {
    /// Create a new empty correlation filter
    pub fn new() -> Self {
        Self {
            correlation_ids: Vec::new(),
            correlation_patterns: Vec::new(),
            parent_correlation_ids: Vec::new(),
            root_correlation_ids: Vec::new(),
            workflow_stages: Vec::new(),
            required_tags: HashMap::new(),
            any_tags: HashMap::new(),
            min_sequence: None,
            max_sequence: None,
        }
    }

    /// Add a correlation ID to filter
    pub fn with_correlation_id(mut self, id: String) -> Self {
        self.correlation_ids.push(id);
        self
    }

    /// Add a correlation pattern to filter
    pub fn with_pattern(mut self, pattern: String) -> Self {
        self.correlation_patterns.push(pattern);
        self
    }

    /// Add a parent correlation ID to filter
    pub fn with_parent_id(mut self, id: String) -> Self {
        self.parent_correlation_ids.push(id);
        self
    }

    /// Add a root correlation ID to filter
    pub fn with_root_id(mut self, id: String) -> Self {
        self.root_correlation_ids.push(id);
        self
    }

    /// Add a workflow stage to filter
    pub fn with_stage(mut self, stage: String) -> Self {
        self.workflow_stages.push(stage);
        self
    }

    /// Add a required tag to filter
    pub fn with_required_tag(mut self, key: String, value: String) -> Self {
        self.required_tags.insert(key, value);
        self
    }

    /// Add an optional tag to filter (any match)
    pub fn with_any_tag(mut self, key: String, value: String) -> Self {
        self.any_tags.insert(key, value);
        self
    }

    /// Set sequence range
    pub fn with_sequence_range(mut self, min: Option<u64>, max: Option<u64>) -> Self {
        self.min_sequence = min;
        self.max_sequence = max;
        self
    }

    /// Check if correlation metadata matches this filter
    pub fn matches(&self, metadata: &CorrelationMetadata) -> bool {
        // Check correlation IDs
        if !self.correlation_ids.is_empty()
            && !self.correlation_ids.contains(&metadata.correlation_id)
        {
            return false;
        }

        // Check correlation patterns
        if !self.correlation_patterns.is_empty() {
            let matches_pattern = self
                .correlation_patterns
                .iter()
                .any(|pattern| metadata.matches_pattern(pattern));
            if !matches_pattern {
                return false;
            }
        }

        // Check parent correlation IDs
        if !self.parent_correlation_ids.is_empty() {
            if let Some(ref parent_id) = metadata.parent_correlation_id {
                if !self.parent_correlation_ids.contains(parent_id) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check root correlation IDs
        if !self.root_correlation_ids.is_empty()
            && !self
                .root_correlation_ids
                .contains(&metadata.root_correlation_id)
        {
            return false;
        }

        // Check workflow stages
        if !self.workflow_stages.is_empty() {
            if let Some(ref stage) = metadata.workflow_stage {
                if !self.workflow_stages.contains(stage) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check required tags (all must match)
        for (key, value) in &self.required_tags {
            if !metadata.has_tag(key, value) {
                return false;
            }
        }

        // Check any tags (at least one must match)
        if !self.any_tags.is_empty() {
            let has_any_tag = self
                .any_tags
                .iter()
                .any(|(key, value)| metadata.has_tag(key, value));
            if !has_any_tag {
                return false;
            }
        }

        // Check sequence range
        if let Some(min_seq) = self.min_sequence
            && metadata.sequence_number < min_seq
        {
            return false;
        }
        if let Some(max_seq) = self.max_sequence
            && metadata.sequence_number > max_seq
        {
            return false;
        }

        true
    }
}

impl Default for CorrelationFilter {
    fn default() -> Self {
        Self::new()
    }
}
