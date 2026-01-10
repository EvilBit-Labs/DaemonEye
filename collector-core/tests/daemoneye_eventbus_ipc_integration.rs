#![cfg(feature = "eventbus-integration")]
//! Integration tests for DaemoneyeEventBus with IPC server infrastructure.
//!
//! This test suite verifies the seamless integration between the daemoneye-eventbus
//! broker and the existing IPC server infrastructure, ensuring backward compatibility
//! and performance characteristics.

use collector_core::{
    DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent},
    event_bus::{EventBus, EventBusConfig, EventSubscription, LocalEventBus},
    ipc::CollectorIpcServer,
    source::SourceCaps,
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::RwLock, time::timeout};

fn temp_socket_path(name: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("failed to create tempdir for socket");
    let socket_path = dir.path().join(name);
    let socket_path_str = socket_path
        .to_str()
        .expect("temp socket path is not valid UTF-8")
        .to_owned();
    (dir, socket_path_str)
}

#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_vs_local_eventbus_performance() {
    // Test DaemoneyeEventBus performance
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-perf-comparison.sock");
    let mut daemoneye_bus = DaemoneyeEventBus::new(config.clone(), socket_path.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    daemoneye_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    let subscription = EventSubscription {
        subscriber_id: "perf-test-daemoneye".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut daemoneye_receiver = daemoneye_bus
        .subscribe(subscription)
        .await
        .expect("Failed to subscribe to DaemoneyeEventBus");

    // Test LocalEventBus performance
    let mut local_bus = LocalEventBus::new(config);
    let local_subscription = EventSubscription {
        subscriber_id: "perf-test-local".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut local_receiver = local_bus
        .subscribe(local_subscription)
        .await
        .expect("Failed to subscribe to LocalEventBus");

    // Create test events
    let num_events = 100;
    let mut test_events = Vec::new();
    for i in 0..num_events {
        test_events.push(CollectionEvent::Process(ProcessEvent {
            pid: 10000 + i,
            ppid: Some(1),
            name: format!("perf_test_{}", i),
            executable_path: Some(format!("/bin/test_{}", i)),
            command_line: vec![format!("test_{}", i)],
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }));
    }

    // Measure DaemoneyeEventBus performance
    let daemoneye_start = Instant::now();
    for (i, event) in test_events.iter().enumerate() {
        daemoneye_bus
            .publish(
                event.clone(),
                collector_core::event_bus::CorrelationMetadata::new(format!(
                    "perf-test-daemoneye-{}",
                    i
                )),
            )
            .await
            .expect("Failed to publish to DaemoneyeEventBus");
    }

    // Receive all events
    for _ in 0..num_events {
        let _event = timeout(Duration::from_secs(10), daemoneye_receiver.recv())
            .await
            .expect("Timeout on DaemoneyeEventBus")
            .expect("Failed to receive from DaemoneyeEventBus");
    }
    let daemoneye_duration = daemoneye_start.elapsed();

    // Measure LocalEventBus performance
    let local_start = Instant::now();
    for (i, event) in test_events.iter().enumerate() {
        local_bus
            .publish(
                event.clone(),
                collector_core::event_bus::CorrelationMetadata::new(format!(
                    "perf-test-local-{}",
                    i
                )),
            )
            .await
            .expect("Failed to publish to LocalEventBus");
    }

    // Receive all events
    for _ in 0..num_events {
        let _event = timeout(Duration::from_secs(10), local_receiver.recv())
            .await
            .expect("Timeout on LocalEventBus")
            .expect("Failed to receive from LocalEventBus");
    }
    let local_duration = local_start.elapsed();

    // Log performance comparison
    println!("Performance comparison for {} events:", num_events);
    println!(
        "  DaemoneyeEventBus: {:?} ({:.2} events/sec)",
        daemoneye_duration,
        num_events as f64 / daemoneye_duration.as_secs_f64()
    );
    println!(
        "  LocalEventBus: {:?} ({:.2} events/sec)",
        local_duration,
        num_events as f64 / local_duration.as_secs_f64()
    );

    // Both should complete within reasonable time
    assert!(daemoneye_duration < Duration::from_secs(30));
    assert!(local_duration < Duration::from_secs(30));

    // Get statistics
    let daemoneye_stats = daemoneye_bus
        .get_statistics()
        .await
        .expect("Failed to get DaemoneyeEventBus stats");
    let local_stats = local_bus
        .get_statistics()
        .await
        .expect("Failed to get LocalEventBus stats");

    println!("DaemoneyeEventBus stats: {:?}", daemoneye_stats);
    println!("LocalEventBus stats: {:?}", local_stats);

    // Verify statistics
    assert!(daemoneye_stats.events_published >= num_events as u64);
    assert!(local_stats.events_published >= num_events as u64);

    daemoneye_bus
        .shutdown()
        .await
        .expect("Failed to shutdown DaemoneyeEventBus");
}

#[cfg(unix)]
#[tokio::test]
async fn test_ipc_server_capability_integration() {
    // Create DaemoneyeEventBus
    let config = EventBusConfig::default();
    let (_socket_dir, socket_path) = temp_socket_path("test-ipc-capability.sock");
    let event_bus = DaemoneyeEventBus::new(config, &socket_path)
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create IPC server with specific capabilities
    let capabilities = Arc::new(RwLock::new(
        SourceCaps::PROCESS | SourceCaps::NETWORK | SourceCaps::REALTIME,
    ));
    let collector_config = collector_core::CollectorConfig::default();
    let ipc_server = CollectorIpcServer::new(collector_config, Arc::clone(&capabilities))
        .expect("Failed to create IPC server");

    // Get capabilities from IPC server
    let proto_capabilities = ipc_server.get_capabilities().await;

    // Verify capabilities include expected domains
    use daemoneye_lib::proto::MonitoringDomain;
    assert!(
        proto_capabilities
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32))
    );
    assert!(
        proto_capabilities
            .supported_domains
            .contains(&(MonitoringDomain::Network as i32))
    );

    // Verify advanced capabilities
    let advanced = proto_capabilities
        .advanced
        .as_ref()
        .expect("Advanced capabilities should be present");
    assert!(advanced.realtime);
    assert!(!advanced.kernel_level); // Not set in our test capabilities

    // Update capabilities and verify
    ipc_server
        .update_capabilities(SourceCaps::PROCESS | SourceCaps::KERNEL_LEVEL)
        .await;

    let updated_capabilities = ipc_server.get_capabilities().await;
    assert!(
        updated_capabilities
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32))
    );
    assert!(
        !updated_capabilities
            .supported_domains
            .contains(&(MonitoringDomain::Network as i32))
    ); // Should be removed

    let updated_advanced = updated_capabilities
        .advanced
        .as_ref()
        .expect("Updated advanced capabilities should be present");
    assert!(updated_advanced.kernel_level); // Should be added
    assert!(!updated_advanced.realtime); // Should be removed

    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[cfg(unix)]
#[tokio::test]
async fn test_eventbus_broker_access_integration() {
    // Create DaemoneyeEventBus
    let config = EventBusConfig::default();
    let (_socket_dir, socket_path) = temp_socket_path("test-broker-access.sock");
    let event_bus = DaemoneyeEventBus::new(config, &socket_path)
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Get broker reference
    let broker = event_bus.broker();

    // Verify broker statistics
    let broker_stats = broker.statistics().await;
    assert_eq!(broker_stats.messages_published, 0);
    assert_eq!(broker_stats.active_subscribers, 0);

    // Test direct broker publish using a valid topic
    let test_event = CollectionEvent::Process(ProcessEvent {
        pid: 12345,
        ppid: Some(1),
        name: "broker_test".to_string(),
        executable_path: Some("/bin/broker_test".to_string()),
        command_line: vec!["broker_test".to_string()],
        start_time: Some(SystemTime::now()),
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    let test_payload = postcard::to_allocvec(&test_event).expect("Failed to serialize test event");

    broker
        .publish("events.process.new", "test-correlation", test_payload)
        .await
        .expect("Failed to publish to broker");

    // Verify statistics updated
    let updated_stats = broker.statistics().await;
    assert!(updated_stats.messages_published > 0);

    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[cfg(unix)]
#[tokio::test]
async fn test_eventbus_seamless_migration_compatibility() {
    // This test verifies that DaemoneyeEventBus can be used as a drop-in replacement
    // for LocalEventBus with identical API behavior

    let config = EventBusConfig::default();

    // Test with LocalEventBus
    let mut local_bus = LocalEventBus::new(config.clone());
    let local_subscription = EventSubscription {
        subscriber_id: "migration-test-local".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut local_receiver = local_bus
        .subscribe(local_subscription)
        .await
        .expect("Failed to subscribe to LocalEventBus");

    // Test with DaemoneyeEventBus
    let (_socket_dir, socket_path) = temp_socket_path("test-migration-compat.sock");
    let mut daemoneye_bus = DaemoneyeEventBus::new(config, &socket_path)
        .await
        .expect("Failed to create DaemoneyeEventBus");

    daemoneye_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    let daemoneye_subscription = EventSubscription {
        subscriber_id: "migration-test-daemoneye".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut daemoneye_receiver = daemoneye_bus
        .subscribe(daemoneye_subscription)
        .await
        .expect("Failed to subscribe to DaemoneyeEventBus");

    // Create identical test event
    let test_event = CollectionEvent::Process(ProcessEvent {
        pid: 99999,
        ppid: Some(1),
        name: "migration_test".to_string(),
        executable_path: Some("/bin/migration_test".to_string()),
        command_line: vec!["migration_test".to_string()],
        start_time: Some(SystemTime::now()),
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    // Publish to both buses
    let local_correlation =
        collector_core::event_bus::CorrelationMetadata::new("migration-test-local".to_string());
    local_bus
        .publish(test_event.clone(), local_correlation)
        .await
        .expect("Failed to publish to LocalEventBus");

    daemoneye_bus
        .publish(
            test_event.clone(),
            collector_core::event_bus::CorrelationMetadata::new(
                "migration-test-daemoneye".to_string(),
            ),
        )
        .await
        .expect("Failed to publish to DaemoneyeEventBus");

    // Receive from both buses
    let local_event = timeout(Duration::from_secs(5), local_receiver.recv())
        .await
        .expect("Timeout on LocalEventBus")
        .expect("Failed to receive from LocalEventBus");

    let daemoneye_event = timeout(Duration::from_secs(5), daemoneye_receiver.recv())
        .await
        .expect("Timeout on DaemoneyeEventBus")
        .expect("Failed to receive from DaemoneyeEventBus");

    // Verify both events have the same content
    match (&local_event.event, &daemoneye_event.event) {
        (CollectionEvent::Process(local_proc), CollectionEvent::Process(daemoneye_proc)) => {
            assert_eq!(local_proc.pid, daemoneye_proc.pid);
            assert_eq!(local_proc.name, daemoneye_proc.name);
            assert_eq!(local_proc.executable_path, daemoneye_proc.executable_path);
        }
        _ => {
            panic!("Expected ProcessEvent from both buses");
        }
    }

    // Verify correlation IDs (LocalEventBus may not preserve correlation IDs the same way)
    // DaemoneyeEventBus should preserve correlation IDs
    assert_eq!(
        daemoneye_event.correlation_metadata.correlation_id,
        "migration-test-daemoneye".to_string()
    );

    // LocalEventBus behavior may differ, so we just verify the event was received
    // (correlation ID preservation is not guaranteed in LocalEventBus)
    println!(
        "LocalEventBus correlation_id: {:?}",
        local_event.correlation_metadata.correlation_id
    );

    // Get statistics from both buses
    let local_stats = local_bus
        .get_statistics()
        .await
        .expect("Failed to get LocalEventBus stats");
    let daemoneye_stats = daemoneye_bus
        .get_statistics()
        .await
        .expect("Failed to get DaemoneyeEventBus stats");

    // Both should have processed events
    assert!(local_stats.events_published > 0);
    assert!(daemoneye_stats.events_published > 0);

    daemoneye_bus
        .shutdown()
        .await
        .expect("Failed to shutdown DaemoneyeEventBus");
}
