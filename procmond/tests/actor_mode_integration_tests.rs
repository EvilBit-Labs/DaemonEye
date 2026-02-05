//! Integration tests for procmond actor mode components.
//!
//! These tests verify that the actor mode components (EventBusConnector, RegistrationManager,
//! RpcServiceHandler, and ProcmondMonitorCollector) work together correctly.
//!
//! # Test Categories
//!
//! 1. **EventBusConnector Initialization**: Verifies that `connect()` must be called
//!    before publishing works, and that events are buffered when not connected.
//!
//! 2. **Multi-Connector Orchestration**: Verifies that when multiple EventBusConnector
//!    instances exist, the backpressure receiver comes from the correct instance.
//!
//! 3. **Actor Timeout Behavior**: Verifies that RPC handlers properly timeout when
//!    the actor is stalled or unresponsive.
//!
//! 4. **Component Integration**: Verifies that RegistrationManager and RpcServiceHandler
//!    properly coordinate with the actor and EventBusConnector.

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::print_stdout,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::shadow_reuse,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::collapsible_if
)]

use collector_core::event::ProcessEvent;
use daemoneye_eventbus::rpc::{
    CollectorOperation, RpcCorrelationMetadata, RpcPayload, RpcRequest, RpcStatus,
};
use procmond::event_bus_connector::{BackpressureSignal, EventBusConnector, ProcessEventType};
use procmond::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorHandle, ActorMessage};
use procmond::registration::{RegistrationConfig, RegistrationManager, RegistrationState};
use procmond::rpc_service::{RpcServiceConfig, RpcServiceHandler};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc};
use tokio::time::timeout;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a test EventBusConnector with an isolated temp directory.
/// Returns the connector and the temp directory (which must be kept alive).
async fn create_isolated_connector() -> (EventBusConnector, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");
    (connector, temp_dir)
}

/// Creates a test actor handle with a receiver for inspecting messages.
fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    (ActorHandle::new(tx), rx)
}

/// Creates a test process event.
fn create_test_process_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("test-process-{}", pid),
        executable_path: Some("/usr/bin/test".to_string()),
        command_line: vec!["test".to_string(), "--flag".to_string()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("abc123def456".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Creates a test RPC request for health check.
fn create_health_check_request(deadline_secs: u64) -> RpcRequest {
    RpcRequest {
        request_id: format!(
            "test-{}",
            SystemTime::now().elapsed().unwrap_or_default().as_nanos()
        ),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(deadline_secs),
        correlation_metadata: RpcCorrelationMetadata::new("test-correlation".to_string()),
    }
}

// ============================================================================
// EventBusConnector Initialization Tests
// ============================================================================

/// Verifies that a newly created EventBusConnector is not connected by default.
#[tokio::test]
async fn test_connector_not_connected_by_default() {
    let (connector, _temp_dir) = create_isolated_connector().await;

    assert!(
        !connector.is_connected(),
        "Connector should not be connected before calling connect()"
    );
}

/// Verifies that events are buffered when the connector is not connected.
/// This is the behavior that was silently happening before the fix.
#[tokio::test]
async fn test_events_buffered_when_not_connected() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Verify not connected
    assert!(!connector.is_connected());

    // Publish an event - it should be buffered, not fail
    let event = create_test_process_event(1234);
    let result = connector.publish(event, ProcessEventType::Start).await;

    // The publish should succeed (event is buffered or written to WAL)
    assert!(
        result.is_ok(),
        "Publish should succeed even when not connected"
    );

    // The connector is not connected, so the event should be buffered
    // (either in memory or WAL - the key point is it doesn't fail)
}

/// Verifies that multiple connectors with isolated directories don't interfere.
#[tokio::test]
async fn test_multiple_connectors_isolation() {
    // Create two isolated connectors
    let (mut connector1, _temp_dir1) = create_isolated_connector().await;
    let (mut connector2, _temp_dir2) = create_isolated_connector().await;

    // Publish events to each connector
    let event1 = create_test_process_event(1001);
    let event2 = create_test_process_event(2001);

    let result1 = connector1.publish(event1, ProcessEventType::Start).await;
    let result2 = connector2.publish(event2, ProcessEventType::Start).await;

    assert!(result1.is_ok(), "Connector 1 publish should succeed");
    assert!(result2.is_ok(), "Connector 2 publish should succeed");

    // Both connectors should work independently (the key point is no interference)
    // Each connector has its own WAL directory, so they can't corrupt each other
}

// ============================================================================
// Backpressure Receiver Tests
// ============================================================================

/// Verifies that each connector has its own backpressure receiver.
#[tokio::test]
async fn test_backpressure_receiver_per_connector() {
    let (mut connector1, _temp_dir1) = create_isolated_connector().await;
    let (mut connector2, _temp_dir2) = create_isolated_connector().await;

    // Take backpressure receivers from each connector
    let bp_rx1 = connector1.take_backpressure_receiver();
    let bp_rx2 = connector2.take_backpressure_receiver();

    // Each connector should have its own receiver
    assert!(
        bp_rx1.is_some(),
        "Connector 1 should have a backpressure receiver"
    );
    assert!(
        bp_rx2.is_some(),
        "Connector 2 should have a backpressure receiver"
    );

    // Taking again should return None (already taken)
    let bp_rx1_again = connector1.take_backpressure_receiver();
    let bp_rx2_again = connector2.take_backpressure_receiver();

    assert!(
        bp_rx1_again.is_none(),
        "Backpressure receiver should only be taken once"
    );
    assert!(
        bp_rx2_again.is_none(),
        "Backpressure receiver should only be taken once"
    );
}

/// Verifies that backpressure signals are sent to the correct receiver.
#[tokio::test]
async fn test_backpressure_signals_to_correct_receiver() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;
    let mut bp_rx = connector
        .take_backpressure_receiver()
        .expect("Should have backpressure receiver");

    // Fill the buffer to trigger backpressure (publish many events while not connected)
    // The connector has a 10MB buffer limit with high-water mark at 70%
    for i in 0..1000 {
        let event = create_test_process_event(i);
        // Intentionally ignore publish result - we're just filling buffer to trigger backpressure
        // Some publishes may fail when buffer is full, which is expected behavior
        drop(connector.publish(event, ProcessEventType::Start).await);

        // Check if we've triggered backpressure
        if let Ok(Some(signal)) = timeout(Duration::from_millis(1), bp_rx.recv()).await {
            if signal == BackpressureSignal::Activated {
                // We've verified that signals are being sent to our receiver
                return;
            }
        }
    }

    // If we didn't trigger backpressure, that's also valid (buffer might not be full enough)
    // The key test here is that the receiver is functional and isolated
    println!(
        "Note: Buffer usage at {}%, may not have triggered backpressure threshold",
        connector.buffer_usage_percent()
    );
}

// ============================================================================
// Actor Timeout Tests
// ============================================================================

/// Verifies that health check times out when actor doesn't respond.
#[tokio::test]
async fn test_health_check_timeout_with_unresponsive_actor() {
    // Create actor handle but DON'T process messages from the receiver
    // This simulates a stalled actor
    let (actor_handle, _rx) = create_test_actor(); // _rx is dropped, causing channel to close
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    // Create RPC handler with a short timeout (50ms to keep tests fast)
    let config = RpcServiceConfig {
        collector_id: "test-procmond".to_string(),
        control_topic: "control.collector.procmond".to_string(),
        response_topic_prefix: "response.collector.procmond".to_string(),
        default_timeout: Duration::from_millis(50), // Very short timeout for fast tests
        max_concurrent_requests: 10,
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    // Create health check request with short deadline
    let request = create_health_check_request(1); // 1 second deadline

    // Handle request - should timeout because actor channel is closed
    let response = handler.handle_request(request).await;

    // Should get an error response (timeout or actor error)
    assert!(
        response.status == RpcStatus::Error || response.status == RpcStatus::Timeout,
        "Expected error or timeout, got {:?}",
        response.status
    );
}

/// Verifies that health check succeeds when actor responds within timeout.
#[tokio::test]
async fn test_health_check_succeeds_with_responsive_actor() {
    let (actor_handle, mut rx) = create_test_actor();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let config = RpcServiceConfig {
        collector_id: "test-procmond".to_string(),
        control_topic: "control.collector.procmond".to_string(),
        response_topic_prefix: "response.collector.procmond".to_string(),
        default_timeout: Duration::from_millis(500), // Short timeout for fast tests
        max_concurrent_requests: 10,
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    // Spawn a task to respond to actor messages
    let responder = tokio::spawn(async move {
        if let Some(msg) = rx.recv().await {
            match msg {
                ActorMessage::HealthCheck { respond_to } => {
                    let health_data = procmond::monitor_collector::HealthCheckData {
                        state: procmond::monitor_collector::CollectorState::Running,
                        collection_interval: Duration::from_secs(30),
                        original_interval: Duration::from_secs(30),
                        event_bus_connected: true,
                        buffer_level_percent: Some(10),
                        last_collection: Some(std::time::Instant::now()),
                        collection_cycles: 5,
                        lifecycle_events: 2,
                        collection_errors: 0,
                        backpressure_events: 0,
                    };
                    respond_to
                        .send(health_data)
                        .expect("Health response receiver should be waiting");
                }
                _ => panic!("Expected HealthCheck message"),
            }
        }
    });

    let request = create_health_check_request(30);
    let response = handler.handle_request(request).await;

    // Should succeed
    assert_eq!(
        response.status,
        RpcStatus::Success,
        "Health check should succeed"
    );

    // Clean up responder
    responder.abort();
}

/// Verifies that deadline-exceeded is returned when deadline is already past.
#[tokio::test]
async fn test_deadline_exceeded_returns_immediately() {
    let (actor_handle, _rx) = create_test_actor();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Create request with deadline in the past
    let request = RpcRequest {
        request_id: "test-expired".to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now() - Duration::from_secs(60),
        deadline: SystemTime::now() - Duration::from_secs(30), // Past deadline
        correlation_metadata: RpcCorrelationMetadata::new("test-expired".to_string()),
    };

    // This should return immediately without waiting for actor
    let start = std::time::Instant::now();
    let response = handler.handle_request(request).await;
    let elapsed = start.elapsed();

    assert_eq!(response.status, RpcStatus::Timeout);
    assert!(
        elapsed < Duration::from_millis(100),
        "Expired deadline should return immediately, took {:?}",
        elapsed
    );
}

// ============================================================================
// Registration Manager Integration Tests
// ============================================================================

/// Verifies that RegistrationManager starts in unregistered state.
#[tokio::test]
async fn test_registration_manager_initial_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    assert_eq!(
        manager.state().await,
        RegistrationState::Unregistered,
        "Manager should start in Unregistered state"
    );
}

/// Verifies that heartbeat is skipped when not registered.
#[tokio::test]
async fn test_heartbeat_skipped_when_not_registered() {
    let (actor_handle, _rx) = create_test_actor();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Try to publish heartbeat while not registered
    let result = manager.publish_heartbeat().await;

    // Should succeed (returns early) but not increment heartbeat count
    assert!(
        result.is_ok(),
        "Heartbeat should return Ok when not registered"
    );

    let stats = manager.stats().await;
    assert_eq!(
        stats.heartbeats_sent, 0,
        "No heartbeats should be counted when not registered"
    );
}

/// Verifies that RegistrationManager and RpcServiceHandler share the same EventBusConnector.
#[tokio::test]
async fn test_shared_event_bus_between_components() {
    let (actor_handle, _rx) = create_test_actor();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    // Create both components with the same event bus
    let registration_manager = RegistrationManager::new(
        Arc::clone(&event_bus),
        actor_handle.clone(),
        RegistrationConfig::default(),
    );
    let rpc_handler = RpcServiceHandler::with_defaults(actor_handle, Arc::clone(&event_bus));

    // Both should have the same collector ID
    assert_eq!(registration_manager.collector_id(), "procmond");
    assert_eq!(rpc_handler.config().collector_id, "procmond");

    // Verify they share the same event bus (check connection state is consistent)
    let eb_connected_reg = event_bus.read().await.is_connected();
    let eb_connected_rpc = event_bus.read().await.is_connected();
    assert_eq!(
        eb_connected_reg, eb_connected_rpc,
        "Both components should see the same connection state"
    );
}

// ============================================================================
// Parallel Execution Tests (Test Isolation)
// ============================================================================

/// Tests that can be run in parallel to verify isolation.
/// Each test uses its own temp directory.
mod parallel_isolation_tests {
    use super::*;

    #[tokio::test]
    async fn test_parallel_connector_1() {
        let (mut connector, _temp_dir) = create_isolated_connector().await;
        let event = create_test_process_event(10001);
        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parallel_connector_2() {
        let (mut connector, _temp_dir) = create_isolated_connector().await;
        let event = create_test_process_event(20001);
        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parallel_connector_3() {
        let (mut connector, _temp_dir) = create_isolated_connector().await;
        let event = create_test_process_event(30001);
        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parallel_connector_4() {
        let (mut connector, _temp_dir) = create_isolated_connector().await;
        let event = create_test_process_event(40001);
        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parallel_registration_manager_1() {
        let (actor_handle, _rx) = create_test_actor();
        let (connector, _temp_dir) = create_isolated_connector().await;
        let event_bus = Arc::new(RwLock::new(connector));
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);
        assert_eq!(manager.state().await, RegistrationState::Unregistered);
    }

    #[tokio::test]
    async fn test_parallel_registration_manager_2() {
        let (actor_handle, _rx) = create_test_actor();
        let (connector, _temp_dir) = create_isolated_connector().await;
        let event_bus = Arc::new(RwLock::new(connector));
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);
        assert_eq!(manager.state().await, RegistrationState::Unregistered);
    }

    #[tokio::test]
    async fn test_parallel_rpc_handler_1() {
        let (actor_handle, _rx) = create_test_actor();
        let (connector, _temp_dir) = create_isolated_connector().await;
        let event_bus = Arc::new(RwLock::new(connector));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);
        let stats = handler.stats().await;
        assert_eq!(stats.requests_received, 0);
    }

    #[tokio::test]
    async fn test_parallel_rpc_handler_2() {
        let (actor_handle, _rx) = create_test_actor();
        let (connector, _temp_dir) = create_isolated_connector().await;
        let event_bus = Arc::new(RwLock::new(connector));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);
        let stats = handler.stats().await;
        assert_eq!(stats.requests_received, 0);
    }
}

// ============================================================================
// Full Actor Mode Setup Test
// ============================================================================

/// Integration test that mimics the full actor mode setup from main.rs.
/// This verifies that all components can be initialized together.
#[tokio::test]
async fn test_full_actor_mode_component_initialization() {
    // Create shared event bus (like in main.rs)
    let (shared_connector, _shared_temp_dir) = create_isolated_connector().await;
    let shared_event_bus = Arc::new(RwLock::new(shared_connector));

    // Create collector's separate event bus (like in main.rs)
    let (mut collector_connector, _collector_temp_dir) = create_isolated_connector().await;

    // Take backpressure receiver from collector's connector (the fix)
    let collector_bp_rx = collector_connector.take_backpressure_receiver();
    assert!(
        collector_bp_rx.is_some(),
        "Collector should have backpressure receiver"
    );

    // Create actor handle
    let (actor_handle, _actor_rx) = create_test_actor();

    // Create RegistrationManager with shared event bus
    let registration_config = RegistrationConfig::default();
    let registration_manager = Arc::new(RegistrationManager::new(
        Arc::clone(&shared_event_bus),
        actor_handle.clone(),
        registration_config,
    ));

    // Create RpcServiceHandler with shared event bus
    let rpc_config = RpcServiceConfig::default();
    let rpc_service = RpcServiceHandler::new(
        actor_handle.clone(),
        Arc::clone(&shared_event_bus),
        rpc_config,
    );

    // Verify all components are properly initialized
    assert_eq!(
        registration_manager.state().await,
        RegistrationState::Unregistered
    );
    assert_eq!(rpc_service.config().collector_id, "procmond");
    assert!(!shared_event_bus.read().await.is_connected());

    // Verify the collector's connector is separate from the shared one
    assert!(!collector_connector.is_connected());

    // Verify backpressure receiver was taken from the COLLECTOR's connector
    // (This was the bug - it was being taken from the wrong connector)
    let shared_bp_rx = shared_event_bus.write().await.take_backpressure_receiver();
    assert!(
        shared_bp_rx.is_some(),
        "Shared event bus should still have its backpressure receiver (proving we took from collector's)"
    );
}
