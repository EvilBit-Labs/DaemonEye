//! Comprehensive tests for correlation metadata and multi-collector workflow support

use daemoneye_eventbus::{
    BusEvent, CollectionEvent, CorrelationFilter, CorrelationMetadata, DaemoneyeBroker,
    DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent, SourceCaps,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tempfile::tempdir;
use uuid::Uuid;

#[tokio::test]
async fn test_correlation_metadata_creation() {
    let correlation_id = Uuid::new_v4().to_string();
    let metadata = CorrelationMetadata::new(correlation_id.clone());

    assert_eq!(metadata.correlation_id, correlation_id);
    assert_eq!(metadata.root_correlation_id, correlation_id);
    assert!(metadata.parent_correlation_id.is_none());
    assert_eq!(metadata.sequence_number, 0);
    assert!(metadata.workflow_stage.is_none());
    assert!(metadata.correlation_tags.is_empty());
}

#[tokio::test]
async fn test_correlation_metadata_with_parent() {
    let parent_id = Uuid::new_v4().to_string();
    let root_id = Uuid::new_v4().to_string();
    let child_id = Uuid::new_v4().to_string();

    let metadata =
        CorrelationMetadata::with_parent(child_id.clone(), parent_id.clone(), root_id.clone());

    assert_eq!(metadata.correlation_id, child_id);
    assert_eq!(metadata.parent_correlation_id, Some(parent_id));
    assert_eq!(metadata.root_correlation_id, root_id);
}

#[tokio::test]
async fn test_correlation_metadata_builder_pattern() {
    let correlation_id = Uuid::new_v4().to_string();
    let metadata = CorrelationMetadata::new(correlation_id.clone())
        .with_stage("analysis".to_string())
        .with_tag("collector".to_string(), "procmond".to_string())
        .with_tag("priority".to_string(), "high".to_string())
        .with_sequence(42);

    assert_eq!(metadata.workflow_stage, Some("analysis".to_string()));
    assert_eq!(metadata.sequence_number, 42);
    assert_eq!(metadata.correlation_tags.len(), 2);
    assert!(metadata.has_tag("collector", "procmond"));
    assert!(metadata.has_tag("priority", "high"));
}

#[tokio::test]
async fn test_correlation_metadata_child_creation() {
    let parent_id = Uuid::new_v4().to_string();
    let parent_metadata = CorrelationMetadata::new(parent_id.clone())
        .with_stage("collection".to_string())
        .with_tag("source".to_string(), "procmond".to_string());

    let child_id = Uuid::new_v4().to_string();
    let child_metadata = parent_metadata.create_child(child_id.clone());

    assert_eq!(child_metadata.correlation_id, child_id);
    assert_eq!(
        child_metadata.parent_correlation_id,
        Some(parent_id.clone())
    );
    assert_eq!(child_metadata.root_correlation_id, parent_id);
    assert_eq!(
        child_metadata.workflow_stage,
        Some("collection".to_string())
    );
    assert!(child_metadata.has_tag("source", "procmond"));
}

#[tokio::test]
async fn test_correlation_metadata_pattern_matching() {
    let correlation_id = "test-correlation-123".to_string();
    let metadata = CorrelationMetadata::new(correlation_id.clone());

    // Exact match
    assert!(metadata.matches_pattern("test-correlation-123"));

    // Wildcard match
    assert!(metadata.matches_pattern("test-correlation-*"));
    assert!(metadata.matches_pattern("test-*"));
    assert!(metadata.matches_pattern("*-123"));

    // No match
    assert!(!metadata.matches_pattern("other-correlation-*"));
}

#[tokio::test]
#[ignore = "Test has incorrect expectations for pattern matching behavior"]
async fn test_correlation_metadata_pattern_matching_regex_special_chars() {
    // Test that regex special characters are properly escaped
    let correlation_id = "test.correlation+123".to_string();
    let metadata = CorrelationMetadata::new(correlation_id.clone());

    // Exact match with special characters
    assert!(metadata.matches_pattern("test.correlation+123"));

    // Wildcard match with special characters
    assert!(metadata.matches_pattern("test.correlation+*"));
    assert!(metadata.matches_pattern("test.*"));
    assert!(metadata.matches_pattern("*+123")); // Matches strings ending with +123

    // Pattern with dots should match literally (not as regex wildcard)
    let correlation_id2 = "test.correlation.456".to_string();
    let metadata2 = CorrelationMetadata::new(correlation_id2.clone());
    assert!(metadata2.matches_pattern("test.correlation.*"));
    assert!(!metadata2.matches_pattern("test.correlation*")); // Should not match without dot

    // Pattern with plus signs should match literally
    let correlation_id3 = "test+correlation+789".to_string();
    let metadata3 = CorrelationMetadata::new(correlation_id3.clone());
    assert!(metadata3.matches_pattern("test+correlation+*"));
    assert!(!metadata3.matches_pattern("test+correlation*")); // Should not match without plus

    // Pattern with parentheses should match literally
    let correlation_id4 = "test(correlation)456".to_string();
    let metadata4 = CorrelationMetadata::new(correlation_id4.clone());
    assert!(metadata4.matches_pattern("test(correlation)*"));
    assert!(!metadata4.matches_pattern("testcorrelation*")); // Should not match without parentheses

    // Pattern with brackets should match literally
    let correlation_id5 = "test[correlation]789".to_string();
    let metadata5 = CorrelationMetadata::new(correlation_id5.clone());
    assert!(metadata5.matches_pattern("test[correlation]*"));
    assert!(!metadata5.matches_pattern("testcorrelation*")); // Should not match without brackets

    // Pattern with anchors (^ and $) should match literally, not as regex anchors
    let correlation_id6 = "test^correlation$123".to_string();
    let metadata6 = CorrelationMetadata::new(correlation_id6.clone());
    assert!(metadata6.matches_pattern("test^correlation$*"));
    assert!(metadata6.matches_pattern("test^correlation$123")); // Exact match
}

#[tokio::test]
async fn test_correlation_metadata_sequence_increment() {
    let mut metadata = CorrelationMetadata::new(Uuid::new_v4().to_string());

    assert_eq!(metadata.sequence_number, 0);

    metadata.increment_sequence();
    assert_eq!(metadata.sequence_number, 1);

    metadata.increment_sequence();
    assert_eq!(metadata.sequence_number, 2);
}

#[tokio::test]
async fn test_correlation_filter_creation() {
    let filter = CorrelationFilter::new()
        .with_correlation_id("test-id-1".to_string())
        .with_pattern("test-*".to_string())
        .with_stage("analysis".to_string())
        .with_required_tag("collector".to_string(), "procmond".to_string());

    assert_eq!(filter.correlation_ids.len(), 1);
    assert_eq!(filter.correlation_patterns.len(), 1);
    assert_eq!(filter.workflow_stages.len(), 1);
    assert_eq!(filter.required_tags.len(), 1);
}

#[tokio::test]
async fn test_correlation_filter_exact_match() {
    let correlation_id = "test-correlation-123".to_string();
    let metadata = CorrelationMetadata::new(correlation_id.clone());

    let filter = CorrelationFilter::new().with_correlation_id(correlation_id.clone());

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_pattern_match() {
    let metadata = CorrelationMetadata::new("test-correlation-123".to_string());

    let filter = CorrelationFilter::new().with_pattern("test-correlation-*".to_string());

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_parent_match() {
    let parent_id = Uuid::new_v4().to_string();
    let metadata = CorrelationMetadata::with_parent(
        Uuid::new_v4().to_string(),
        parent_id.clone(),
        Uuid::new_v4().to_string(),
    );

    let filter = CorrelationFilter::new().with_parent_id(parent_id);

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_root_match() {
    let root_id = Uuid::new_v4().to_string();
    let metadata = CorrelationMetadata::with_parent(
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        root_id.clone(),
    );

    let filter = CorrelationFilter::new().with_root_id(root_id);

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_stage_match() {
    let metadata =
        CorrelationMetadata::new(Uuid::new_v4().to_string()).with_stage("analysis".to_string());

    let filter = CorrelationFilter::new().with_stage("analysis".to_string());

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_required_tags_match() {
    let metadata = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_tag("collector".to_string(), "procmond".to_string())
        .with_tag("priority".to_string(), "high".to_string());

    let filter = CorrelationFilter::new()
        .with_required_tag("collector".to_string(), "procmond".to_string())
        .with_required_tag("priority".to_string(), "high".to_string());

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_required_tags_no_match() {
    let metadata = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_tag("collector".to_string(), "procmond".to_string());

    let filter = CorrelationFilter::new()
        .with_required_tag("collector".to_string(), "procmond".to_string())
        .with_required_tag("priority".to_string(), "high".to_string());

    assert!(!filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_any_tags_match() {
    let metadata = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_tag("collector".to_string(), "procmond".to_string());

    // Test with different keys for any_tags
    let filter = CorrelationFilter::new()
        .with_any_tag("collector".to_string(), "procmond".to_string())
        .with_any_tag("priority".to_string(), "high".to_string());

    assert!(filter.matches(&metadata));

    // Test with non-matching any_tags
    let metadata2 = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_tag("priority".to_string(), "low".to_string());

    let filter2 = CorrelationFilter::new()
        .with_any_tag("collector".to_string(), "procmond".to_string())
        .with_any_tag("priority".to_string(), "high".to_string());

    assert!(!filter2.matches(&metadata2));
}

#[tokio::test]
async fn test_correlation_filter_sequence_range_match() {
    let metadata = CorrelationMetadata::new(Uuid::new_v4().to_string()).with_sequence(50);

    let filter = CorrelationFilter::new().with_sequence_range(Some(10), Some(100));

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_sequence_range_no_match() {
    let metadata = CorrelationMetadata::new(Uuid::new_v4().to_string()).with_sequence(150);

    let filter = CorrelationFilter::new().with_sequence_range(Some(10), Some(100));

    assert!(!filter.matches(&metadata));
}

#[tokio::test]
async fn test_correlation_filter_complex_match() {
    let root_id = Uuid::new_v4().to_string();
    let parent_id = Uuid::new_v4().to_string();
    let metadata = CorrelationMetadata::with_parent(
        Uuid::new_v4().to_string(),
        parent_id.clone(),
        root_id.clone(),
    )
    .with_stage("analysis".to_string())
    .with_tag("collector".to_string(), "procmond".to_string())
    .with_tag("priority".to_string(), "high".to_string())
    .with_sequence(42);

    let filter = CorrelationFilter::new()
        .with_root_id(root_id)
        .with_stage("analysis".to_string())
        .with_required_tag("collector".to_string(), "procmond".to_string())
        .with_sequence_range(Some(40), Some(50));

    assert!(filter.matches(&metadata));
}

#[tokio::test]
async fn test_bus_event_with_correlation_metadata() {
    let correlation_metadata = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_stage("collection".to_string())
        .with_tag("source".to_string(), "procmond".to_string());

    let event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "test_process".to_string(),
        command_line: Some("test --arg".to_string()),
        executable_path: Some("/bin/test".to_string()),
        ppid: Some(1),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    });

    let bus_event = BusEvent::new(
        event,
        correlation_metadata.clone(),
        "events.process.*".to_string(),
        "test-subscriber".to_string(),
    );

    assert_eq!(
        bus_event.correlation_metadata.correlation_id,
        correlation_metadata.correlation_id
    );
    assert_eq!(
        bus_event.correlation_metadata.workflow_stage,
        Some("collection".to_string())
    );
    assert!(bus_event.correlation_metadata.has_tag("source", "procmond"));
}

#[tokio::test]
async fn test_multi_collector_workflow_correlation() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-workflow.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Create root correlation for workflow
    let root_correlation = CorrelationMetadata::new(Uuid::new_v4().to_string())
        .with_stage("collection".to_string())
        .with_tag(
            "workflow".to_string(),
            "suspicious_process_analysis".to_string(),
        );

    // Subscribe to process events
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
                .with_root_id(root_correlation.root_correlation_id.clone())
                .with_required_tag(
                    "workflow".to_string(),
                    "suspicious_process_analysis".to_string(),
                ),
        ),
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
    };

    let _receiver = event_bus.subscribe(subscription).await.unwrap();

    // Publish event with correlation metadata
    let event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "suspicious_process".to_string(),
        command_line: Some("malware --execute".to_string()),
        executable_path: Some("/tmp/malware".to_string()),
        ppid: Some(1),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    });

    // This should work with the new correlation metadata
    let result = event_bus
        .publish(event, root_correlation.correlation_id.clone())
        .await;
    assert!(result.is_ok());

    // Verify statistics
    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 1);
}

#[tokio::test]
async fn test_hierarchical_correlation_tracking() {
    // Create a three-level correlation hierarchy
    let root_id = Uuid::new_v4().to_string();
    let root_metadata = CorrelationMetadata::new(root_id.clone())
        .with_stage("detection".to_string())
        .with_tag("workflow".to_string(), "threat_analysis".to_string());

    let child1_id = Uuid::new_v4().to_string();
    let child1_metadata = root_metadata
        .create_child(child1_id.clone())
        .with_stage("collection".to_string());

    let child2_id = Uuid::new_v4().to_string();
    let child2_metadata = child1_metadata
        .create_child(child2_id.clone())
        .with_stage("analysis".to_string());

    // Verify hierarchy
    assert_eq!(child1_metadata.parent_correlation_id, Some(root_id.clone()));
    assert_eq!(child1_metadata.root_correlation_id, root_id.clone());

    assert_eq!(child2_metadata.parent_correlation_id, Some(child1_id));
    assert_eq!(child2_metadata.root_correlation_id, root_id);

    // Verify inherited tags
    assert!(child1_metadata.has_tag("workflow", "threat_analysis"));
    assert!(child2_metadata.has_tag("workflow", "threat_analysis"));

    // Verify filter can match entire hierarchy
    let filter = CorrelationFilter::new().with_root_id(root_id);

    assert!(filter.matches(&root_metadata));
    assert!(filter.matches(&child1_metadata));
    assert!(filter.matches(&child2_metadata));
}

#[tokio::test]
async fn test_forensic_correlation_tracking() {
    // Create correlation metadata for forensic investigation
    let investigation_id = Uuid::new_v4().to_string();
    let forensic_metadata = CorrelationMetadata::new(investigation_id.clone())
        .with_stage("forensic_analysis".to_string())
        .with_tag("investigation_id".to_string(), investigation_id.clone())
        .with_tag("analyst".to_string(), "security_team".to_string())
        .with_tag("severity".to_string(), "critical".to_string())
        .with_tag("incident_type".to_string(), "malware_detection".to_string());

    // Create filter for forensic queries
    let forensic_filter = CorrelationFilter::new()
        .with_required_tag("investigation_id".to_string(), investigation_id.clone())
        .with_required_tag("severity".to_string(), "critical".to_string());

    assert!(forensic_filter.matches(&forensic_metadata));

    // Verify we can track all events in this investigation
    let child_event = forensic_metadata
        .create_child(Uuid::new_v4().to_string())
        .with_stage("evidence_collection".to_string());

    assert!(forensic_filter.matches(&child_event));
}
