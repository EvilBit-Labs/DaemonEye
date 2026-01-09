#![cfg(feature = "eventbus-integration")]
//! Simplified migration tests comparing crossbeam vs daemoneye-eventbus behavior.
//!
//! This test suite validates that the migration from crossbeam-based LocalEventBus
//! to daemoneye-eventbus maintains identical behavior for core functionality.

use collector_core::{
    LocalEventBus,
    daemoneye_event_bus::DaemoneyeEventBus,
    event::{CollectionEvent, NetworkEvent, ProcessEvent},
    event_bus::{CorrelationMetadata, EventBus, EventBusConfig, EventSubscription},
    source::SourceCaps,
};
use std::time::{Duration, SystemTime};
use tokio::time::timeout;

/// Simple test configuration
struct SimpleTestConfig {
    pub event_count: usize,
    pub timeout: Duration,
}

impl Default for SimpleTestConfig {
    fn default() -> Self {
        Self {
            event_count: 3,
            timeout: Duration::from_secs(2),
        }
    }
}

/// Test helper for creating test events
struct SimpleEventGenerator;

impl SimpleEventGenerator {
    fn create_process_event(id: u32) -> CollectionEvent {
        let mut name = String::with_capacity(20);
        name.push_str("test_process_");
        name.push_str(&id.to_string());

        let mut executable_path = String::with_capacity(15);
        executable_path.push_str("/bin/test_");
        executable_path.push_str(&id.to_string());

        let mut command = String::with_capacity(10);
        command.push_str("test_");
        command.push_str(&id.to_string());

        let mut hash = String::with_capacity(10);
        hash.push_str("hash_");
        hash.push_str(&id.to_string());

        CollectionEvent::Process(ProcessEvent {
            pid: 1000 + id,
            ppid: Some(500 + id),
            name,
            executable_path: Some(executable_path),
            command_line: vec![command],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(10.0 + id as f64),
            memory_usage: Some(1024 * (id + 1) as u64),
            executable_hash: Some(hash),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        })
    }

    fn create_network_event(id: u32) -> CollectionEvent {
        let mut connection_id = String::with_capacity(10);
        connection_id.push_str("conn_");
        connection_id.push_str(&id.to_string());

        let mut source_addr = String::with_capacity(15);
        source_addr.push_str("192.168.1.");
        source_addr.push_str(&(id % 255).to_string());

        let mut dest_addr = String::with_capacity(12);
        dest_addr.push_str("10.0.0.");
        dest_addr.push_str(&(id % 255).to_string());

        CollectionEvent::Network(NetworkEvent {
            connection_id,
            source_addr,
            dest_addr,
            protocol: "TCP".to_string(),
            state: "ESTABLISHED".to_string(),
            pid: Some(1000 + id),
            bytes_sent: id as u64 * 1024,
            bytes_received: id as u64 * 512,
            timestamp: SystemTime::now(),
        })
    }
}

/// Test basic event publishing and receiving for both implementations
#[cfg(unix)]
#[tokio::test]
async fn test_basic_event_flow_comparison() {
    let config = EventBusConfig::default();
    let test_config = SimpleTestConfig::default();

    // Test LocalEventBus
    let local_received = test_basic_flow_local(config.clone(), &test_config).await;

    // Test DaemoneyeEventBus
    let daemoneye_received = test_basic_flow_daemoneye(config, &test_config).await;

    // Both should receive some events
    assert!(local_received > 0, "LocalEventBus should receive events");
    assert!(
        daemoneye_received > 0,
        "DaemoneyeEventBus should receive events"
    );

    println!(
        "LocalEventBus received: {}, DaemoneyeEventBus received: {}",
        local_received, daemoneye_received
    );
}

async fn test_basic_flow_local(config: EventBusConfig, test_config: &SimpleTestConfig) -> usize {
    let mut bus = LocalEventBus::new(config);

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "basic-test-local".to_string(),
        capabilities: SourceCaps::PROCESS | SourceCaps::NETWORK,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.#".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut receiver = bus.subscribe(subscription).await.unwrap();
    let mut received_count = 0;

    // Publish events
    for i in 0..test_config.event_count {
        let event = if i % 2 == 0 {
            SimpleEventGenerator::create_process_event(i as u32)
        } else {
            SimpleEventGenerator::create_network_event(i as u32)
        };

        let mut correlation_id = String::with_capacity(15);
        correlation_id.push_str("basic-test-");
        correlation_id.push_str(&i.to_string());
        let correlation = CorrelationMetadata::new(correlation_id);
        bus.publish(event, correlation).await.unwrap();
    }

    // Receive events with timeout
    for _ in 0..test_config.event_count {
        match timeout(test_config.timeout, receiver.recv()).await {
            Ok(Some(_)) => received_count += 1,
            Ok(None) => break,
            Err(_) => break, // Timeout
        }
    }

    bus.shutdown().await.unwrap();
    received_count
}

async fn test_basic_flow_daemoneye(
    config: EventBusConfig,
    test_config: &SimpleTestConfig,
) -> usize {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("basic-test.sock");
    let mut bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    bus.start().await.unwrap();

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "basic-test-daemoneye".to_string(),
        capabilities: SourceCaps::PROCESS | SourceCaps::NETWORK,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.#".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let mut receiver = bus.subscribe(subscription).await.unwrap();
    let mut received_count = 0;

    // Publish events
    for i in 0..test_config.event_count {
        let event = if i % 2 == 0 {
            SimpleEventGenerator::create_process_event(i as u32)
        } else {
            SimpleEventGenerator::create_network_event(i as u32)
        };

        let mut correlation_id = String::with_capacity(15);
        correlation_id.push_str("basic-test-");
        correlation_id.push_str(&i.to_string());
        let correlation = CorrelationMetadata::new(correlation_id);
        bus.publish(event, correlation).await.unwrap();
    }

    // Receive events with timeout
    for _ in 0..test_config.event_count {
        match timeout(test_config.timeout, receiver.recv()).await {
            Ok(Some(_)) => received_count += 1,
            Ok(None) => break,
            Err(_) => break, // Timeout
        }
    }

    bus.shutdown().await.unwrap();
    received_count
}

/// Test subscription and unsubscription behavior
#[cfg(unix)]
#[tokio::test]
async fn test_subscription_behavior_comparison() {
    let config = EventBusConfig::default();

    // Test LocalEventBus
    let local_result = test_subscription_local(config.clone()).await;

    // Test DaemoneyeEventBus
    let daemoneye_result = test_subscription_daemoneye(config).await;

    // Both should handle subscription/unsubscription correctly
    assert!(
        local_result,
        "LocalEventBus should handle subscription correctly"
    );
    assert!(
        daemoneye_result,
        "DaemoneyeEventBus should handle subscription correctly"
    );
}

async fn test_subscription_local(config: EventBusConfig) -> bool {
    let mut bus = LocalEventBus::new(config);

    let subscription = EventSubscription {
        subscriber_id: "sub-test-local".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    // Subscribe
    let _receiver = bus.subscribe(subscription).await.unwrap();

    // Check statistics
    let stats = bus.get_statistics().await.unwrap();
    let has_subscriber = stats.active_subscribers > 0;

    // Unsubscribe
    let unsubscribe_result = bus.unsubscribe("sub-test-local").await;

    bus.shutdown().await.unwrap();

    has_subscriber && unsubscribe_result.is_ok()
}

async fn test_subscription_daemoneye(config: EventBusConfig) -> bool {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("sub-test.sock");
    let mut bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    bus.start().await.unwrap();

    let subscription = EventSubscription {
        subscriber_id: "sub-test-daemoneye".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    // Subscribe
    let _receiver = bus.subscribe(subscription).await.unwrap();

    // Check statistics
    let stats = bus.get_statistics().await.unwrap();
    let has_subscriber = stats.active_subscribers > 0;

    // Unsubscribe
    let unsubscribe_result = bus.unsubscribe("sub-test-daemoneye").await;

    bus.shutdown().await.unwrap();

    has_subscriber && unsubscribe_result.is_ok()
}

/// Test error handling behavior
#[cfg(unix)]
#[tokio::test]
async fn test_error_handling_comparison() {
    let config = EventBusConfig::default();

    // Test LocalEventBus error handling
    let local_errors = test_error_handling_local(config.clone()).await;

    // Test DaemoneyeEventBus error handling
    let daemoneye_errors = test_error_handling_daemoneye(config).await;

    // Both should handle errors consistently
    assert_eq!(
        local_errors.len(),
        daemoneye_errors.len(),
        "Both implementations should handle the same number of error scenarios"
    );

    for (local_error, daemoneye_error) in local_errors.iter().zip(daemoneye_errors.iter()) {
        assert_eq!(
            local_error, daemoneye_error,
            "Error handling should be consistent between implementations"
        );
    }
}

async fn test_error_handling_local(config: EventBusConfig) -> Vec<bool> {
    let mut bus = LocalEventBus::new(config);
    let mut results = Vec::with_capacity(2);

    // Test 1: Invalid subscription (empty ID)
    let invalid_subscription = EventSubscription {
        subscriber_id: "".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let subscribe_result = bus.subscribe(invalid_subscription).await;
    results.push(subscribe_result.is_err());

    // Test 2: Unsubscribe non-existent subscriber
    let unsubscribe_result = bus.unsubscribe("non-existent").await;
    results.push(unsubscribe_result.is_ok()); // Should handle gracefully

    bus.shutdown().await.unwrap();
    results
}

async fn test_error_handling_daemoneye(config: EventBusConfig) -> Vec<bool> {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("error-test.sock");
    let mut bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    bus.start().await.unwrap();
    let mut results = Vec::with_capacity(2);

    // Test 1: Invalid subscription (empty ID)
    let invalid_subscription = EventSubscription {
        subscriber_id: "".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let subscribe_result = bus.subscribe(invalid_subscription).await;
    results.push(subscribe_result.is_err());

    // Test 2: Unsubscribe non-existent subscriber
    let unsubscribe_result = bus.unsubscribe("non-existent").await;
    results.push(unsubscribe_result.is_ok()); // Should handle gracefully

    bus.shutdown().await.unwrap();
    results
}

/// Test statistics collection
#[cfg(unix)]
#[tokio::test]
async fn test_statistics_comparison() {
    let config = EventBusConfig::default();

    // Test LocalEventBus statistics
    let local_stats_work = test_statistics_local(config.clone()).await;

    // Test DaemoneyeEventBus statistics
    let daemoneye_stats_work = test_statistics_daemoneye(config).await;

    // Both should provide working statistics
    assert!(local_stats_work, "LocalEventBus statistics should work");
    assert!(
        daemoneye_stats_work,
        "DaemoneyeEventBus statistics should work"
    );
}

async fn test_statistics_local(config: EventBusConfig) -> bool {
    let bus = LocalEventBus::new(config);

    // Get initial statistics
    let initial_stats = bus.get_statistics().await;
    if initial_stats.is_err() {
        return false;
    }

    let stats = initial_stats.unwrap();
    let stats_valid = stats.events_published == 0 && stats.active_subscribers == 0;

    bus.shutdown().await.unwrap();
    stats_valid
}

async fn test_statistics_daemoneye(config: EventBusConfig) -> bool {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("stats-test.sock");
    let bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    bus.start().await.unwrap();

    // Get initial statistics
    let initial_stats = bus.get_statistics().await;
    if initial_stats.is_err() {
        return false;
    }

    let stats = initial_stats.unwrap();
    let stats_valid = stats.events_published == 0 && stats.active_subscribers == 0;

    bus.shutdown().await.unwrap();
    stats_valid
}
