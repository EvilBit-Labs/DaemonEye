//! Integration tests for DaemoneyeEventBus with collector-core framework.

use collector_core::{
    DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent, TriggerPriority, TriggerRequest},
    event_bus::{EventBus, EventBusConfig, EventSubscription},
    source::SourceCaps,
};
use std::time::{Duration, SystemTime};
use tokio::time::timeout;

#[tokio::test]
async fn test_daemoneye_eventbus_integration() {
    let config = EventBusConfig::default();
    let mut event_bus = DaemoneyeEventBus::new(config, "/tmp/test-integration.sock")
        .await
        .expect("Failed to create DaemoneyeEventBus");

    // Start the event bus
    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "integration-test".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.#".to_string()]),
        enable_wildcards: true,
    };

    let mut receiver = event_bus
        .subscribe(subscription)
        .await
        .expect("Failed to subscribe");

    // Create and publish a process event
    let process_event = CollectionEvent::Process(ProcessEvent {
        pid: 12345,
        ppid: Some(1),
        name: "integration_test".to_string(),
        executable_path: Some("/usr/bin/integration_test".to_string()),
        command_line: vec!["integration_test".to_string(), "--test".to_string()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(5.5),
        memory_usage: Some(1024 * 1024), // 1MB
        executable_hash: Some("abc123def456".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    event_bus
        .publish(
            process_event.clone(),
            Some("integration-test-correlation".to_string()),
        )
        .await
        .expect("Failed to publish event");

    // Receive the event
    let received_event = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    // Verify the event
    match received_event.event {
        CollectionEvent::Process(ref proc_event) => {
            assert_eq!(proc_event.pid, 12345);
            assert_eq!(proc_event.name, "integration_test");
            assert_eq!(proc_event.ppid, Some(1));
        }
        _ => panic!("Expected ProcessEvent, got {:?}", received_event.event),
    }

    // Verify correlation ID
    assert_eq!(
        received_event.correlation_id,
        Some("integration-test-correlation".to_string())
    );

    // Get statistics
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");
    assert!(stats.events_published > 0);
    assert!(stats.active_subscribers > 0);

    // Shutdown
    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[tokio::test]
async fn test_daemoneye_eventbus_trigger_requests() {
    let config = EventBusConfig::default();
    let mut event_bus = DaemoneyeEventBus::new(config, "/tmp/test-trigger-integration.sock")
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create subscription for trigger requests
    let subscription = EventSubscription {
        subscriber_id: "trigger-integration-test".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["control.#".to_string()]),
        enable_wildcards: true,
    };

    let mut receiver = event_bus
        .subscribe(subscription)
        .await
        .expect("Failed to subscribe");

    // Create and publish a trigger request
    let trigger_request = CollectionEvent::TriggerRequest(TriggerRequest {
        trigger_id: "trigger-integration-123".to_string(),
        target_collector: "yara-collector".to_string(),
        analysis_type: collector_core::event::AnalysisType::YaraScan,
        priority: TriggerPriority::High,
        target_pid: Some(12345),
        target_path: Some("/tmp/suspicious_file".to_string()),
        correlation_id: "integration-correlation".to_string(),
        metadata: std::collections::HashMap::new(),
        timestamp: SystemTime::now(),
    });

    event_bus
        .publish(
            trigger_request,
            Some("trigger-integration-correlation".to_string()),
        )
        .await
        .expect("Failed to publish trigger request");

    // Receive the event
    let received_event = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("Timeout waiting for trigger event")
        .expect("Failed to receive trigger event");

    // Verify the trigger request
    match received_event.event {
        CollectionEvent::TriggerRequest(ref trigger) => {
            assert_eq!(trigger.trigger_id, "trigger-integration-123");
            assert_eq!(trigger.target_pid, Some(12345));
        }
        _ => panic!("Expected TriggerRequest, got {:?}", received_event.event),
    }

    event_bus.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
async fn test_daemoneye_eventbus_multiple_subscribers() {
    let config = EventBusConfig::default();
    let mut event_bus = DaemoneyeEventBus::new(config, "/tmp/test-multi-sub.sock")
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create multiple subscriptions
    let subscription1 = EventSubscription {
        subscriber_id: "subscriber-1".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.#".to_string()]),
        enable_wildcards: true,
    };

    let subscription2 = EventSubscription {
        subscriber_id: "subscriber-2".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
    };

    let mut receiver1 = event_bus
        .subscribe(subscription1)
        .await
        .expect("Failed to subscribe subscriber-1");

    let mut receiver2 = event_bus
        .subscribe(subscription2)
        .await
        .expect("Failed to subscribe subscriber-2");

    // Publish an event
    let process_event = CollectionEvent::Process(ProcessEvent {
        pid: 99999,
        ppid: None,
        name: "multi_subscriber_test".to_string(),
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

    event_bus
        .publish(
            process_event,
            Some("multi-subscriber-correlation".to_string()),
        )
        .await
        .expect("Failed to publish event");

    // Both subscribers should receive the event
    let event1 = timeout(Duration::from_secs(5), receiver1.recv())
        .await
        .expect("Timeout waiting for event on subscriber-1")
        .expect("Failed to receive event on subscriber-1");

    let event2 = timeout(Duration::from_secs(5), receiver2.recv())
        .await
        .expect("Timeout waiting for event on subscriber-2")
        .expect("Failed to receive event on subscriber-2");

    // Verify both events
    match (&event1.event, &event2.event) {
        (CollectionEvent::Process(proc1), CollectionEvent::Process(proc2)) => {
            assert_eq!(proc1.pid, 99999);
            assert_eq!(proc2.pid, 99999);
            assert_eq!(proc1.name, "multi_subscriber_test");
            assert_eq!(proc2.name, "multi_subscriber_test");
        }
        _ => panic!("Expected ProcessEvents for both subscribers"),
    }

    // Unsubscribe one subscriber
    event_bus
        .unsubscribe("subscriber-1")
        .await
        .expect("Failed to unsubscribe subscriber-1");

    // Verify statistics
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");
    assert!(stats.events_published > 0);
    assert_eq!(stats.active_subscribers, 1); // Only subscriber-2 should remain

    event_bus.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
async fn test_daemoneye_eventbus_performance_comparison() {
    use collector_core::event_bus::LocalEventBus;
    use std::time::Instant;

    // Test DaemoneyeEventBus performance
    let config = EventBusConfig::default();
    let mut daemoneye_bus = DaemoneyeEventBus::new(config.clone(), "/tmp/test-perf-daemoneye.sock")
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
        topic_patterns: Some(vec!["events.#".to_string()]),
        enable_wildcards: true,
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
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
    };

    let mut local_receiver = local_bus
        .subscribe(local_subscription)
        .await
        .expect("Failed to subscribe to LocalEventBus");

    // Create test event
    let test_event = CollectionEvent::Process(ProcessEvent {
        pid: 88888,
        ppid: None,
        name: "perf_test".to_string(),
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

    // Measure DaemoneyeEventBus performance
    let daemoneye_start = Instant::now();
    daemoneye_bus
        .publish(test_event.clone(), Some("perf-test-daemoneye".to_string()))
        .await
        .expect("Failed to publish to DaemoneyeEventBus");

    let _daemoneye_event = timeout(Duration::from_secs(5), daemoneye_receiver.recv())
        .await
        .expect("Timeout on DaemoneyeEventBus")
        .expect("Failed to receive from DaemoneyeEventBus");
    let daemoneye_duration = daemoneye_start.elapsed();

    // Measure LocalEventBus performance
    let local_start = Instant::now();
    local_bus
        .publish(test_event, Some("perf-test-local".to_string()))
        .await
        .expect("Failed to publish to LocalEventBus");

    let _local_event = timeout(Duration::from_secs(5), local_receiver.recv())
        .await
        .expect("Timeout on LocalEventBus")
        .expect("Failed to receive from LocalEventBus");
    let local_duration = local_start.elapsed();

    // Log performance comparison (both should complete within reasonable time)
    println!(
        "Performance comparison - DaemoneyeEventBus: {:?}, LocalEventBus: {:?}",
        daemoneye_duration, local_duration
    );

    // Both should complete within 1 second for this simple test
    assert!(daemoneye_duration < Duration::from_secs(1));
    assert!(local_duration < Duration::from_secs(1));

    daemoneye_bus.shutdown().await.expect("Failed to shutdown");
}
