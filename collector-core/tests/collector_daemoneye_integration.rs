//! Integration tests for Collector with DaemoneyeEventBus.

use collector_core::event_bus::EventBus;
use collector_core::{Collector, CollectorConfig, DaemoneyeEventBus, event_bus::EventBusConfig};
use std::time::Duration;

#[tokio::test]
async fn test_collector_with_daemoneye_eventbus() {
    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("collector-integration.sock");
    let collector = Collector::configure_daemoneye_eventbus(config, socket_path.to_str().unwrap())
        .expect("Failed to create collector with DaemoneyeEventBus");

    // This test just verifies that the collector can be created with DaemoneyeEventBus
    // without errors. The actual event processing would require more complex setup.
    assert_eq!(collector.source_count(), 0);
}

#[tokio::test]
async fn test_daemoneye_eventbus_broker_access() {
    let config = EventBusConfig::default();
    let temp_dir2 = tempfile::tempdir().unwrap();
    let socket_path2 = temp_dir2.path().join("broker-access-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path2.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    // Test broker access
    let broker = event_bus.broker();
    let broker_stats = broker.statistics().await;

    assert_eq!(broker_stats.messages_published, 0);
    assert_eq!(broker_stats.active_subscribers, 0);

    event_bus.start().await.expect("Failed to start EventBus");
    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[tokio::test]
async fn test_eventbus_statistics_conversion() {
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("stats-conversion-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus.start().await.expect("Failed to start EventBus");

    // Get statistics through the EventBus trait
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");

    // Verify statistics structure
    assert_eq!(stats.events_published, 0);
    assert_eq!(stats.events_delivered, 0);
    assert_eq!(stats.active_subscribers, 0);
    assert!(stats.uptime >= Duration::from_secs(0));

    event_bus.shutdown().await.expect("Failed to shutdown");
}
