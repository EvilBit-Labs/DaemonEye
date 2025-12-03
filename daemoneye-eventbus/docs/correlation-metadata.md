# Correlation Metadata and Multi-Collector Workflow Support

## Overview

This document describes the correlation metadata implementation in daemoneye-eventbus, which enables comprehensive tracking of events across multiple collectors and complex multi-step workflows.

## Core Features

### 1. Hierarchical Correlation Tracking

The `CorrelationMetadata` structure supports hierarchical event relationships:

```rust,ignore
pub struct CorrelationMetadata {
    /// Primary correlation ID for tracking related events
    pub correlation_id: String,
    /// Parent correlation ID for hierarchical relationships
    pub parent_correlation_id: Option<String>,
    /// Root correlation ID for the entire workflow
    pub root_correlation_id: String,
    /// Event sequence number within correlation group
    pub sequence_number: u64,
    /// Workflow stage identifier
    pub workflow_stage: Option<String>,
    /// Custom correlation tags for flexible grouping
    pub correlation_tags: HashMap<String, String>,
    /// Timestamp when correlation was created
    pub created_at: SystemTime,
}
```

### 2. Workflow Stage Tracking

Correlation metadata supports workflow stage tracking for complex multi-step operations:

```rust,ignore
let metadata = CorrelationMetadata::new(correlation_id)
    .with_stage("collection".to_string())
    .with_tag("collector".to_string(), "procmond".to_string());
```

Stages can represent different phases of analysis:

- `collection` - Initial data gathering
- `analysis` - Processing and analysis
- `enrichment` - Adding context from other sources
- `alerting` - Generating alerts
- `forensic_analysis` - Detailed investigation

### 3. Correlation Tags

Flexible tagging system for grouping and filtering events:

```rust,ignore
let metadata = CorrelationMetadata::new(correlation_id)
    .with_tag("investigation_id".to_string(), investigation_id)
    .with_tag("analyst".to_string(), "security_team".to_string())
    .with_tag("severity".to_string(), "critical".to_string())
    .with_tag("incident_type".to_string(), "malware_detection".to_string());
```

### 4. Sequence Numbering

Automatic sequence numbering for ordering events within a correlation group:

```rust,ignore
let mut metadata = CorrelationMetadata::new(correlation_id);
metadata.increment_sequence(); // sequence_number = 1
metadata.increment_sequence(); // sequence_number = 2
```

### 5. Child Correlation Creation

Create child correlations that inherit properties from parent:

```rust,ignore
let parent_metadata = CorrelationMetadata::new(parent_id)
    .with_stage("collection".to_string())
    .with_tag("workflow".to_string(), "threat_analysis".to_string());

let child_metadata = parent_metadata.create_child(child_id);
// Child inherits workflow stage and tags from parent
// Child's parent_correlation_id = parent_id
// Child's root_correlation_id = parent_id
```

## Advanced Filtering

### CorrelationFilter

Comprehensive filtering capabilities for correlation-based event routing:

```rust,ignore
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
```

### Filter Examples

#### Filter by Root Correlation ID

Track all events in a workflow:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_root_id(root_correlation_id);
```

#### Filter by Workflow Stage

Get events from specific workflow stages:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_stage("analysis".to_string());
```

#### Filter by Required Tags

All tags must match:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_required_tag("severity".to_string(), "critical".to_string())
    .with_required_tag("incident_type".to_string(), "malware".to_string());
```

#### Filter by Any Tags

At least one tag must match:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_any_tag("collector".to_string(), "procmond".to_string())
    .with_any_tag("collector".to_string(), "netmond".to_string());
```

#### Filter by Sequence Range

Get events within a specific sequence range:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_sequence_range(Some(10), Some(100));
```

#### Complex Filter

Combine multiple filter criteria:

```rust,ignore
let filter = CorrelationFilter::new()
    .with_root_id(root_id)
    .with_stage("analysis".to_string())
    .with_required_tag("collector".to_string(), "procmond".to_string())
    .with_sequence_range(Some(40), Some(50));
```

## Pattern Matching

Correlation metadata supports wildcard pattern matching:

```rust,ignore
let metadata = CorrelationMetadata::new("test-correlation-123".to_string());

// Exact match
assert!(metadata.matches_pattern("test-correlation-123"));

// Wildcard match
assert!(metadata.matches_pattern("test-correlation-*"));
assert!(metadata.matches_pattern("test-*"));
assert!(metadata.matches_pattern("*-123"));
```

## Integration with EventBus

### Message Structure

Messages now include full correlation metadata:

```rust,ignore
pub struct Message {
    pub id: Uuid,
    pub topic: String,
    pub correlation_metadata: CorrelationMetadata,
    pub payload: Vec<u8>,
    pub sequence: u64,
    pub timestamp: SystemTime,
    pub message_type: MessageType,
}
```

### BusEvent Structure

Bus events received by subscribers include correlation metadata:

```rust,ignore
pub struct BusEvent {
    pub event_id: String,
    pub event: CollectionEvent,
    pub correlation_metadata: CorrelationMetadata,
    pub bus_timestamp: SystemTime,
    pub matched_pattern: String,
    pub subscriber_id: String,
}
```

### Publishing with Correlation Metadata

```rust,ignore
// Create correlation metadata
let correlation_metadata = CorrelationMetadata::new(correlation_id)
    .with_stage("collection".to_string())
    .with_tag("source".to_string(), "procmond".to_string());

// Publish event with metadata
let message = Message::event_with_metadata(
    topic,
    correlation_metadata,
    payload,
    sequence,
);
```

### Subscribing with Correlation Filter

```rust,ignore
let subscription = EventSubscription {
    subscriber_id: "workflow-subscriber".to_string(),
    capabilities: SourceCaps {
        event_types: vec!["process".to_string()],
        collectors: vec!["procmond".to_string()],
        max_priority: 5,
    },
    event_filter: None,
    correlation_filter: Some(
        CorrelationFilter::new()
            .with_root_id(root_correlation_id)
            .with_required_tag("workflow".to_string(), "suspicious_process_analysis".to_string())
    ),
    topic_patterns: Some(vec!["events.process.*".to_string()]),
    enable_wildcards: true,
};

let receiver = event_bus.subscribe(subscription).await?;
```

## Use Cases

### 1. Multi-Collector Workflows

Track events across multiple collectors in a cascading analysis workflow:

```rust,ignore
// Root correlation for entire workflow
let root_correlation = CorrelationMetadata::new(workflow_id)
    .with_stage("detection".to_string())
    .with_tag("workflow".to_string(), "suspicious_process_analysis".to_string());

// Process collection stage
let collection_correlation = root_correlation.create_child(collection_id)
    .with_stage("collection".to_string());

// Binary analysis stage
let analysis_correlation = collection_correlation.create_child(analysis_id)
    .with_stage("analysis".to_string());

// Alert generation stage
let alert_correlation = analysis_correlation.create_child(alert_id)
    .with_stage("alerting".to_string());
```

### 2. Forensic Investigation Tracking

Track all events related to a security investigation:

```rust,ignore
let investigation_id = Uuid::new_v4().to_string();
let forensic_metadata = CorrelationMetadata::new(investigation_id.clone())
    .with_stage("forensic_analysis".to_string())
    .with_tag("investigation_id".to_string(), investigation_id.clone())
    .with_tag("analyst".to_string(), "security_team".to_string())
    .with_tag("severity".to_string(), "critical".to_string())
    .with_tag("incident_type".to_string(), "malware_detection".to_string());

// Filter for all events in this investigation
let forensic_filter = CorrelationFilter::new()
    .with_required_tag("investigation_id".to_string(), investigation_id)
    .with_required_tag("severity".to_string(), "critical".to_string());
```

### 3. Distributed Tracing

Track events across distributed system components:

```rust,ignore
let trace_metadata = CorrelationMetadata::new(trace_id)
    .with_stage("distributed_collection".to_string())
    .with_tag("service".to_string(), "daemoneye-agent".to_string())
    .with_tag("host".to_string(), hostname)
    .with_tag("region".to_string(), "us-west-2".to_string());
```

### 4. Performance Analysis

Track performance metrics across workflow stages:

```rust,ignore
let perf_metadata = CorrelationMetadata::new(perf_id)
    .with_stage("performance_monitoring".to_string())
    .with_tag("metric_type".to_string(), "latency".to_string())
    .with_tag("component".to_string(), "collector".to_string());

// Filter for performance events
let perf_filter = CorrelationFilter::new()
    .with_required_tag("metric_type".to_string(), "latency".to_string())
    .with_sequence_range(Some(0), Some(1000));
```

## Backward Compatibility

The implementation maintains backward compatibility with existing code:

### Simple Correlation ID

For simple use cases, you can still use just a correlation ID string:

```rust,ignore
// Old style (still works)
let message = Message::event(topic, correlation_id, payload, sequence);

// New style (recommended)
let message = Message::event_with_metadata(
    topic,
    CorrelationMetadata::new(correlation_id),
    payload,
    sequence,
);
```

### BusEvent Compatibility

BusEvent provides a helper method for backward compatibility:

```rust,ignore
// Get correlation ID from metadata
let correlation_id = bus_event.correlation_id();
```

## Testing

Comprehensive test suite in `tests/correlation_metadata_tests.rs` covers:

- Correlation metadata creation and builder pattern
- Hierarchical correlation tracking
- Pattern matching and filtering
- Workflow stage tracking
- Forensic investigation tracking
- Multi-collector workflow coordination
- Sequence numbering and ordering
- Tag-based filtering

Run tests:

```bash
cargo test -p daemoneye-eventbus --test correlation_metadata_tests
```

## Performance Considerations

- Correlation metadata is cloned when creating messages and events
- Pattern matching uses regex for wildcard support (cached internally)
- Filtering is performed in-memory with O(1) hash lookups for tags
- Sequence numbering uses saturating arithmetic to prevent overflow
- Child correlation creation is O(1) with shallow cloning of tags

## Future Enhancements

Potential future improvements:

1. **Distributed Tracing Integration**: OpenTelemetry trace/span ID support
2. **Correlation Persistence**: Store correlation metadata in database for historical queries
3. **Correlation Analytics**: Aggregate statistics on workflow performance
4. **Correlation Visualization**: Generate workflow diagrams from correlation data
5. **Correlation Compression**: Optimize storage for large correlation hierarchies
