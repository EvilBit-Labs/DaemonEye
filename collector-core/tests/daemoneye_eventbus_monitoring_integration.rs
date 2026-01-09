#![cfg(feature = "eventbus-integration")]
//! Integration tests for DaemonEye EventBus monitoring and statistics collection.
//!
//! This test suite verifies that the DaemonEye EventBus monitoring features
//! integrate correctly with the broader collector-core system and maintain
//! compatibility with existing event bus interfaces.

use collector_core::{
    Collector,
    config::CollectorConfig,
    daemoneye_event_bus::DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent},
    event_bus::{CorrelationMetadata, EventBus, EventBusConfig, EventSubscription},
    source::SourceCaps,
};
use std::time::{Duration, SystemTime};
use tokio::time::timeout;

/// Test that DaemonEye EventBus monitoring integrates correctly with collector-core.
#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_monitoring_integration() {
    // Create a DaemonEye EventBus with monitoring enabled
    let eventbus_config = EventBusConfig {
        enable_statistics: true,
        ..Default::default()
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("integration-test.sock");
    let mut event_bus = DaemoneyeEventBus::new(eventbus_config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    event_bus.start().await.unwrap();

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "integration-test".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut receiver = event_bus.subscribe(subscription).await.unwrap();

    // Publish events to generate monitoring data
    for i in 0..3 {
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 5000 + i,
            ppid: None,
            name: format!("integration_test_{}", i),
            executable_path: Some(format!("/bin/test_{}", i)),
            command_line: vec![format!("test_{}", i)],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(10.0 + i as f64),
            memory_usage: Some(1024 * (i + 1) as u64),
            executable_hash: Some(format!("hash_{}", i)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_metadata =
            CorrelationMetadata::new(format!("integration-correlation-{}", i));
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();
    }

    // Verify events are received
    for i in 0..3 {
        let received_event = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("Timeout waiting for event")
            .unwrap();

        match received_event.event {
            CollectionEvent::Process(ref proc_event) => {
                assert_eq!(proc_event.pid, 5000 + i);
                assert_eq!(proc_event.name, format!("integration_test_{}", i));
            }
            _ => panic!("Expected ProcessEvent"),
        }
    }

    // Test standard statistics interface
    let stats = event_bus.get_statistics().await.unwrap();
    assert!(stats.events_published >= 3);
    assert!(stats.active_subscribers >= 1);

    // Test detailed monitoring metrics
    let detailed_metrics = event_bus.get_detailed_metrics().await.unwrap();
    assert!(detailed_metrics.broker_statistics.messages_published >= 3);
    assert!(detailed_metrics.performance_metrics.messages_per_second >= 0.0);
    assert!(detailed_metrics.broker_health.active_connections >= 1);

    // Test health check
    let is_healthy = event_bus.health_check().await.unwrap();
    assert!(is_healthy);

    // Test monitoring metrics logging (should not fail)
    event_bus.log_monitoring_metrics().await.unwrap();

    event_bus.shutdown().await.unwrap();
}

/// Test that monitoring works correctly with collector-core Collector integration.
#[tokio::test]
async fn test_monitoring_with_collector_integration() {
    // Create a basic collector to test integration
    let config = CollectorConfig::default();
    let collector = Collector::new(config);

    // Test that collector can be created without issues
    assert_eq!(collector.source_count(), 0);

    // Note: Full integration testing would require running the collector,
    // but that's beyond the scope of this unit test. The DaemonEye EventBus
    // monitoring features are tested separately in the unit tests.
}

/// Test monitoring metrics persistence across EventBus operations.
#[cfg(unix)]
#[tokio::test]
async fn test_monitoring_metrics_persistence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("persistence-test.sock");
    let mut event_bus =
        DaemoneyeEventBus::new(EventBusConfig::default(), socket_path.to_str().unwrap())
            .await
            .unwrap();

    event_bus.start().await.unwrap();

    // Initial metrics should show zero activity
    let initial_metrics = event_bus.get_detailed_metrics().await.unwrap();
    assert_eq!(initial_metrics.broker_statistics.messages_published, 0);
    assert_eq!(initial_metrics.broker_statistics.messages_delivered, 0);

    // Create subscription and publish events
    let subscription = EventSubscription {
        subscriber_id: "persistence-test".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let _receiver = event_bus.subscribe(subscription).await.unwrap();

    // Publish multiple events
    for i in 0..5 {
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 6000 + i,
            ppid: None,
            name: format!("persistence_test_{}", i),
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

        let correlation_metadata =
            CorrelationMetadata::new(format!("persistence-correlation-{}", i));
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();
    }

    // Allow time for processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Metrics should show accumulated activity
    let updated_metrics = event_bus.get_detailed_metrics().await.unwrap();
    assert!(updated_metrics.broker_statistics.messages_published >= 5);
    assert!(updated_metrics.broker_statistics.active_subscribers >= 1);

    // Performance metrics should be calculated correctly
    assert!(updated_metrics.performance_metrics.messages_per_second >= 0.0);
    assert!(updated_metrics.performance_metrics.delivery_rate >= 0.0);
    assert!(updated_metrics.performance_metrics.delivery_rate <= 1.0);

    // Health status should be healthy
    assert!(matches!(
        updated_metrics.broker_health.status,
        collector_core::daemoneye_event_bus::HealthStatus::Healthy
            | collector_core::daemoneye_event_bus::HealthStatus::Starting
    ));

    event_bus.shutdown().await.unwrap();
}
