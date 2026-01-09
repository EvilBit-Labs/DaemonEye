//! Integration tests for daemoneye-eventbus

use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent,
    SourceCaps,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tempfile::TempDir;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn test_broker_creation_and_startup() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-broker.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    assert!(broker.start().await.is_ok());

    // Give broker time to start
    sleep(Duration::from_millis(100)).await;

    let stats = broker.statistics().await;
    assert_eq!(stats.messages_published, 0);
    assert_eq!(stats.active_subscribers, 0);
}

#[tokio::test]
async fn test_event_bus_creation() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-eventbus.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 0);
    assert_eq!(stats.active_subscribers, 0);
}

#[tokio::test]
async fn test_process_event_publishing() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-publish.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Create a process event
    let process_event = ProcessEvent {
        pid: 1234,
        name: "test_process".to_string(),
        command_line: Some("test_process --arg".to_string()),
        executable_path: Some("/usr/bin/test_process".to_string()),
        ppid: Some(1000),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    };

    let collection_event = CollectionEvent::Process(process_event);

    // Publish the event
    let result = event_bus
        .publish(collection_event, "test-correlation-123".to_string())
        .await;
    assert!(result.is_ok());

    // Check statistics
    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 1);
}

#[tokio::test]
async fn test_event_subscription() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-subscription.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "test-subscriber".to_string(),
        capabilities: SourceCaps {
            event_types: vec!["process".to_string()],
            collectors: vec!["procmond".to_string()],
            max_priority: 5,
        },
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
    };

    // Subscribe
    let receiver = event_bus.subscribe(subscription).await.unwrap();
    // Verify receiver is open and ready to receive messages
    assert!(
        !receiver.is_closed(),
        "Receiver should not be closed immediately after subscription"
    );

    // Check statistics
    let stats = event_bus.statistics().await;
    assert_eq!(stats.active_subscribers, 1);
}

#[tokio::test]
async fn test_topic_wildcard_matching() {
    use daemoneye_eventbus::{Topic, TopicPattern};

    // Test exact match
    let pattern = TopicPattern::new("events.process.lifecycle").unwrap();
    let topic1 = Topic::new("events.process.lifecycle").unwrap();
    let topic2 = Topic::new("events.process.metadata").unwrap();
    assert!(pattern.matches(&topic1));
    assert!(!pattern.matches(&topic2));

    // Test wildcard match
    let wildcard_pattern = TopicPattern::new("events.process.+").unwrap();
    assert!(wildcard_pattern.matches(&topic1));
    assert!(wildcard_pattern.matches(&topic2));

    let network_topic = Topic::new("events.network.connections").unwrap();
    assert!(!wildcard_pattern.matches(&network_topic));

    // Test multi-level wildcard
    let multi_pattern = TopicPattern::new("events.#").unwrap();
    assert!(multi_pattern.matches(&topic1));
    assert!(multi_pattern.matches(&network_topic));

    let control_topic = Topic::new("control.collector.status").unwrap();
    assert!(!multi_pattern.matches(&control_topic));
}

#[tokio::test]
async fn test_event_bus_shutdown() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-shutdown.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Shutdown should succeed
    let result = event_bus.shutdown().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_statistics_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-stats.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Initial stats
    let initial_stats = event_bus.statistics().await;
    assert_eq!(initial_stats.messages_published, 0);
    assert_eq!(initial_stats.messages_delivered, 0);
    assert_eq!(initial_stats.active_subscribers, 0);
    assert_eq!(initial_stats.active_topics, 0);

    // Publish an event
    let process_event = ProcessEvent {
        pid: 5678,
        name: "stats_test".to_string(),
        command_line: None,
        executable_path: None,
        ppid: None,
        start_time: None,
        metadata: HashMap::new(),
    };

    let collection_event = CollectionEvent::Process(process_event);
    event_bus
        .publish(collection_event, "stats-correlation".to_string())
        .await
        .unwrap();

    // Give some time for uptime to be updated
    sleep(Duration::from_millis(100)).await;

    // Check updated stats
    let updated_stats = event_bus.statistics().await;
    assert_eq!(updated_stats.messages_published, 1);
    // Just verify uptime is a valid value (can be 0 if very fast)
    assert!(
        updated_stats.uptime_seconds < 10,
        "Uptime should be reasonable for a short test"
    );
}
