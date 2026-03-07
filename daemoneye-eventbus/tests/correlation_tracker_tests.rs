//! Tests for CorrelationTracker workflow tracking

use std::time::Duration;

use daemoneye_eventbus::CorrelationMetadata;
use daemoneye_eventbus::correlation::{CorrelationTracker, CorrelationTrackerConfig};

#[tokio::test]
async fn test_correlation_tracker_creation() {
    let config = CorrelationTrackerConfig::default();
    let tracker = CorrelationTracker::new(config);

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 0);
    assert_eq!(stats.total_events_tracked, 0);
    assert_eq!(stats.history_size, 0);
}

#[tokio::test]
async fn test_track_single_event() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata =
        CorrelationMetadata::new("workflow-1".to_owned()).with_stage("collection".to_owned());

    tracker
        .track_event("events.process.new", &metadata)
        .await
        .expect("track_event should succeed");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 1);
    assert_eq!(stats.total_events_tracked, 1);
    assert_eq!(stats.history_size, 1);
}

#[tokio::test]
async fn test_track_multiple_events_same_workflow() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata1 = CorrelationMetadata::new("workflow-1".to_owned())
        .with_stage("collection".to_owned())
        .with_sequence(0);

    let metadata2 = CorrelationMetadata::new("workflow-1".to_owned())
        .with_stage("analysis".to_owned())
        .with_sequence(1);

    tracker
        .track_event("events.process.new", &metadata1)
        .await
        .expect("first track should succeed");
    tracker
        .track_event("events.process.analyzed", &metadata2)
        .await
        .expect("second track should succeed");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 1, "same workflow, one entry");
    assert_eq!(stats.total_events_tracked, 2);
    assert_eq!(stats.history_size, 2);
}

#[tokio::test]
async fn test_track_events_different_workflows() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let meta_a = CorrelationMetadata::new("workflow-a".to_owned());
    let meta_b = CorrelationMetadata::new("workflow-b".to_owned());

    tracker
        .track_event("events.process.new", &meta_a)
        .await
        .expect("a");
    tracker
        .track_event("events.network.new", &meta_b)
        .await
        .expect("b");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 2);
    assert_eq!(stats.total_events_tracked, 2);
}

#[tokio::test]
async fn test_event_history_bounded() {
    let config = CorrelationTrackerConfig {
        max_history_size: 3,
        max_active_workflows: 100,
        workflow_timeout: Duration::from_secs(300),
    };
    let tracker = CorrelationTracker::new(config);

    for i in 0..5 {
        let meta = CorrelationMetadata::new(format!("wf-{i}"));
        tracker
            .track_event("events.process.new", &meta)
            .await
            .expect("track should succeed");
    }

    let stats = tracker.stats().await;
    assert_eq!(
        stats.history_size, 3,
        "history should be bounded to max_history_size"
    );
    assert_eq!(
        stats.total_events_tracked, 5,
        "total should count all events"
    );
}

#[tokio::test]
async fn test_get_workflow_state() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata =
        CorrelationMetadata::new("wf-lookup".to_owned()).with_stage("collection".to_owned());

    tracker
        .track_event("events.process.new", &metadata)
        .await
        .expect("track");

    let state = tracker.get_workflow_state("wf-lookup").await;
    assert!(state.is_some());
    let state = state.expect("should exist");
    assert_eq!(state.correlation_id, "wf-lookup");
    assert_eq!(state.total_events, 1);
    assert!(state.stages.contains_key("collection"));

    let missing = tracker.get_workflow_state("nonexistent").await;
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_find_events_by_tag() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let meta1 = CorrelationMetadata::new("inc-001".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-12345".to_owned())
        .with_tag("severity".to_owned(), "critical".to_owned());

    let meta2 = CorrelationMetadata::new("inc-002".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-12345".to_owned())
        .with_tag("severity".to_owned(), "low".to_owned());

    let meta3 = CorrelationMetadata::new("inc-003".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-99999".to_owned());

    tracker
        .track_event("events.process.new", &meta1)
        .await
        .expect("1");
    tracker
        .track_event("events.process.new", &meta2)
        .await
        .expect("2");
    tracker
        .track_event("events.network.new", &meta3)
        .await
        .expect("3");

    let results = tracker
        .find_events_by_tag("investigation_id", "INC-12345")
        .await;
    assert_eq!(results.len(), 2);

    let results = tracker.find_events_by_tag("severity", "critical").await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].correlation_id, "inc-001");

    let results = tracker.find_events_by_tag("nonexistent", "value").await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_get_workflow_timeline() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let root_id = "root-timeline".to_owned();

    let meta1 = CorrelationMetadata::new(root_id.clone())
        .with_stage("enumeration".to_owned())
        .with_sequence(0);

    let child_meta = meta1
        .create_child("child-1".to_owned())
        .with_stage("analysis".to_owned())
        .with_sequence(1);

    tracker
        .track_event("events.process.new", &meta1)
        .await
        .expect("1");
    tracker
        .track_event("events.process.analyzed", &child_meta)
        .await
        .expect("2");

    let timeline = tracker.get_workflow_timeline(&root_id).await;
    assert_eq!(timeline.root_correlation_id, root_id);
    assert_eq!(timeline.events.len(), 2);
    assert_eq!(timeline.events[0].stage, Some("enumeration".to_owned()));
    assert_eq!(timeline.events[1].stage, Some("analysis".to_owned()));
}

#[tokio::test]
async fn test_cleanup_expired_workflows() {
    let config = CorrelationTrackerConfig {
        max_history_size: 10_000,
        max_active_workflows: 100,
        // Very short timeout for testing
        workflow_timeout: Duration::from_millis(50),
    };
    let tracker = CorrelationTracker::new(config);

    let meta = CorrelationMetadata::new("expiring-wf".to_owned());
    tracker
        .track_event("events.process.new", &meta)
        .await
        .expect("track");

    assert_eq!(tracker.stats().await.active_workflows, 1);

    // Wait for timeout
    tokio::time::sleep(Duration::from_millis(100)).await;

    let cleaned = tracker.cleanup_expired_workflows().await;
    assert_eq!(cleaned, 1);
    assert_eq!(tracker.stats().await.active_workflows, 0);
}

#[tokio::test]
async fn test_zero_history_size_does_not_loop() {
    let config = CorrelationTrackerConfig {
        max_history_size: 0,
        max_active_workflows: 100,
        workflow_timeout: Duration::from_secs(300),
    };
    let tracker = CorrelationTracker::new(config);

    let meta = CorrelationMetadata::new("zero-hist".to_owned());
    tracker
        .track_event("events.process.new", &meta)
        .await
        .expect("track should succeed with zero history size");

    let stats = tracker.stats().await;
    assert_eq!(stats.history_size, 0, "history should remain empty");
    assert_eq!(
        stats.total_events_tracked, 1,
        "event should still be counted"
    );
    assert_eq!(
        stats.active_workflows, 1,
        "workflow should still be tracked"
    );
}

#[tokio::test]
async fn test_input_validation_topic_too_long() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());
    let meta = CorrelationMetadata::new("wf-1".to_owned());
    let long_topic = "x".repeat(513);

    let result = tracker.track_event(&long_topic, &meta).await;
    assert!(result.is_err(), "should reject topic exceeding max length");
}

#[tokio::test]
async fn test_input_validation_topic_length_boundary() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());
    let meta = CorrelationMetadata::new("wf-1".to_owned());

    // Exactly at limit should succeed
    let ok_topic = "x".repeat(512);
    let result = tracker.track_event(&ok_topic, &meta).await;
    assert!(result.is_ok(), "512-char topic should succeed");

    // Over limit should fail
    let long_topic = "x".repeat(513);
    let result = tracker.track_event(&long_topic, &meta).await;
    assert!(result.is_err(), "513-char topic should fail");
}

#[tokio::test]
async fn test_input_validation_tag_value_too_long() {
    use std::collections::HashMap;

    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    // Construct metadata directly to bypass CorrelationMetadata::with_tag() limits
    let mut tags = HashMap::new();
    tags.insert("key".to_owned(), "v".repeat(1025));
    let meta = CorrelationMetadata {
        correlation_id: "wf-tags".to_owned(),
        root_correlation_id: "wf-tags".to_owned(),
        parent_correlation_id: None,
        workflow_stage: None,
        correlation_tags: tags,
        sequence_number: 0,
        created_at: std::time::SystemTime::now(),
    };

    let result = tracker.track_event("events.process.new", &meta).await;
    assert!(
        result.is_err(),
        "should reject tag value exceeding max length"
    );
}

#[tokio::test]
async fn test_broker_publish_with_correlation_tracks_event() {
    use daemoneye_eventbus::{CollectionEvent, DaemoneyeBroker, ProcessEvent};
    use tempfile::tempdir;

    let temp_dir = tempdir().expect("tempdir");
    let socket_path = temp_dir.path().join("tracker-test.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
        .await
        .expect("broker");

    let metadata = CorrelationMetadata::new("broker-track-wf".to_owned())
        .with_stage("collection".to_owned())
        .with_tag("source_process".to_owned(), "procmond".to_owned());

    let event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "test".to_owned(),
        command_line: None,
        executable_path: None,
        ppid: None,
        start_time: None,
        metadata: std::collections::HashMap::new(),
    });

    let payload = postcard::to_allocvec(&event).expect("serialize");

    broker
        .publish_with_correlation("events.process.new", &metadata, payload)
        .await
        .expect("publish_with_correlation should succeed");

    let tracker_stats = broker.correlation_tracker().stats().await;
    assert_eq!(tracker_stats.total_events_tracked, 1);
    assert_eq!(tracker_stats.active_workflows, 1);
}

#[tokio::test]
async fn test_eventbus_publish_with_correlation_metadata() {
    use daemoneye_eventbus::{
        CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, ProcessEvent,
    };
    use tempfile::tempdir;

    let temp_dir = tempdir().expect("tempdir");
    let socket_path = temp_dir.path().join("eventbus-corr-test.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
        .await
        .expect("broker");
    let mut event_bus = DaemoneyeEventBus::from_broker(broker)
        .await
        .expect("eventbus");

    let metadata =
        CorrelationMetadata::new("eventbus-wf".to_owned()).with_stage("detection".to_owned());

    let event = CollectionEvent::Process(ProcessEvent {
        pid: 5678,
        name: "suspicious".to_owned(),
        command_line: Some("./malware --exec".to_owned()),
        executable_path: Some("/tmp/malware".to_owned()),
        ppid: Some(1),
        start_time: None,
        metadata: std::collections::HashMap::new(),
    });

    event_bus
        .publish_with_metadata(event, metadata)
        .await
        .expect("publish_with_metadata should succeed");

    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 1);
}
