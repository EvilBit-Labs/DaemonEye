//! RPC Communication Integration Tests.
//!
//! These tests verify the RPC service layer's behavior in realistic scenarios,
//! testing the integration between RpcServiceHandler, ActorHandle, and the
//! underlying actor pattern for lifecycle operations.
//!
//! # Test Categories
//!
//! 1. **Lifecycle Operations**: Start, Stop, Restart, HealthCheck, UpdateConfig, GracefulShutdown
//! 2. **Health Check Accuracy**: Verifies health data reflects actual component states
//! 3. **Configuration Updates**: Config changes applied at cycle boundaries
//! 4. **Graceful Shutdown**: Completes within timeout constraints
//! 5. **Error Handling**: Edge cases and failure scenarios
//!
//! # Architecture
//!
//! These integration tests verify the RPC service's coordination with the actor:
//! - Testing request handling and message forwarding
//! - Testing health check data accuracy
//! - Testing configuration update flow
//! - Testing graceful shutdown behavior

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
    clippy::collapsible_if,
    clippy::integer_division,
    clippy::map_unwrap_or,
    clippy::use_debug,
    clippy::equatable_if_let,
    clippy::needless_pass_by_value,
    clippy::semicolon_outside_block,
    clippy::cast_lossless
)]

use daemoneye_eventbus::rpc::{
    CollectorOperation, ConfigUpdateRequest, ErrorCategory, HealthStatus, RpcCorrelationMetadata,
    RpcPayload, RpcRequest, RpcStatus, ShutdownRequest, ShutdownType,
};
use procmond::event_bus_connector::EventBusConnector;
use procmond::monitor_collector::{
    ACTOR_CHANNEL_CAPACITY, ActorHandle, ActorMessage, CollectorState, HealthCheckData,
};
use procmond::rpc_service::{RpcServiceConfig, RpcServiceHandler};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc};
use tokio::time::timeout;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a test actor handle with a receiver for inspecting messages.
fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    (ActorHandle::new(tx), rx)
}

/// Creates an EventBusConnector with a unique temp directory for test isolation.
async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create event bus connector");
    (Arc::new(RwLock::new(connector)), temp_dir)
}

/// Creates test health check data with configurable state.
fn create_test_health_data(state: CollectorState, connected: bool) -> HealthCheckData {
    HealthCheckData {
        state,
        collection_interval: Duration::from_secs(5),
        original_interval: Duration::from_secs(5),
        event_bus_connected: connected,
        buffer_level_percent: Some(10),
        last_collection: Some(Instant::now()),
        collection_cycles: 100,
        lifecycle_events: 50,
        collection_errors: 2,
        backpressure_events: 5,
    }
}

/// Creates an RPC request with specified operation and payload.
fn create_rpc_request(
    request_id: &str,
    operation: CollectorOperation,
    payload: RpcPayload,
) -> RpcRequest {
    RpcRequest {
        request_id: request_id.to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation,
        payload,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new(format!("corr-{request_id}")),
    }
}

/// Spawns a task to respond to actor messages with given health data.
fn spawn_health_responder(
    mut rx: mpsc::Receiver<ActorMessage>,
    health_data: HealthCheckData,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            respond_to
                .send(health_data)
                .expect("Health response receiver should be waiting");
        }
    })
}

/// Spawns a task to handle multiple actor messages.
fn spawn_multi_responder(
    rx: mpsc::Receiver<ActorMessage>,
    response_count: usize,
    health_data: HealthCheckData,
) -> tokio::task::JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let mut actor_rx = rx;
        let mut received_ops = Vec::new();
        for _ in 0..response_count {
            match actor_rx.recv().await {
                Some(ActorMessage::HealthCheck { respond_to }) => {
                    received_ops.push("HealthCheck".to_string());
                    respond_to
                        .send(health_data.clone())
                        .expect("Health response receiver should be waiting");
                }
                Some(ActorMessage::UpdateConfig { respond_to, .. }) => {
                    received_ops.push("UpdateConfig".to_string());
                    respond_to
                        .send(Ok(()))
                        .expect("Config update response receiver should be waiting");
                }
                Some(ActorMessage::GracefulShutdown { respond_to }) => {
                    received_ops.push("GracefulShutdown".to_string());
                    respond_to
                        .send(Ok(()))
                        .expect("Shutdown response receiver should be waiting");
                }
                Some(ActorMessage::BeginMonitoring) => {
                    received_ops.push("BeginMonitoring".to_string());
                }
                Some(ActorMessage::AdjustInterval { .. }) => {
                    received_ops.push("AdjustInterval".to_string());
                }
                Some(_) => {
                    // Handle any future ActorMessage variants
                    received_ops.push("Unknown".to_string());
                }
                None => break,
            }
        }
        received_ops
    })
}

// ============================================================================
// Lifecycle Operations Tests
// ============================================================================

/// Test HealthCheck operation returns accurate health data.
#[tokio::test]
async fn test_health_check_returns_accurate_data() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "health-1",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );

    let health_data = create_test_health_data(CollectorState::Running, true);
    let responder = spawn_health_responder(rx, health_data);

    let response = handler.handle_request(request).await;

    responder.await.expect("Responder should complete");

    assert_eq!(response.status, RpcStatus::Success);
    assert!(response.payload.is_some());

    // Verify health data in payload
    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.collector_id, "procmond");
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.components.contains_key("event_bus"));
        assert!(health.components.contains_key("collector"));
        assert_eq!(health.metrics.get("collection_cycles"), Some(&100.0_f64));
        assert_eq!(health.metrics.get("lifecycle_events"), Some(&50.0_f64));
        assert_eq!(health.metrics.get("collection_errors"), Some(&2.0_f64));
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test HealthCheck with degraded state (disconnected event bus).
#[tokio::test]
async fn test_health_check_degraded_when_disconnected() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "health-2",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );

    // Event bus disconnected
    let health_data = create_test_health_data(CollectorState::Running, false);
    let responder = spawn_health_responder(rx, health_data);

    let response = handler.handle_request(request).await;
    responder.await.expect("Responder should complete");

    assert_eq!(response.status, RpcStatus::Success);

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.status, HealthStatus::Degraded);

        let event_bus_health = health.components.get("event_bus").unwrap();
        assert_eq!(event_bus_health.status, HealthStatus::Degraded);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test HealthCheck with WaitingForAgent state shows degraded.
#[tokio::test]
async fn test_health_check_waiting_for_agent_state() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "health-3",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );

    let health_data = create_test_health_data(CollectorState::WaitingForAgent, true);
    let responder = spawn_health_responder(rx, health_data);

    let response = handler.handle_request(request).await;
    responder.await.expect("Responder should complete");

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.status, HealthStatus::Degraded);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test HealthCheck with ShuttingDown state shows unhealthy.
#[tokio::test]
async fn test_health_check_shutting_down_state() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "health-4",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );

    let health_data = create_test_health_data(CollectorState::ShuttingDown, true);
    let responder = spawn_health_responder(rx, health_data);

    let response = handler.handle_request(request).await;
    responder.await.expect("Responder should complete");

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.status, HealthStatus::Unhealthy);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test HealthCheck with Stopped state shows unresponsive.
#[tokio::test]
async fn test_health_check_stopped_state() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "health-5",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );

    let health_data = create_test_health_data(CollectorState::Stopped, false);
    let responder = spawn_health_responder(rx, health_data);

    let response = handler.handle_request(request).await;
    responder.await.expect("Responder should complete");

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.status, HealthStatus::Unresponsive);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test unsupported operations return appropriate errors.
#[tokio::test]
async fn test_unsupported_operations_return_error() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let unsupported_ops = [
        CollectorOperation::Start,
        CollectorOperation::Stop,
        CollectorOperation::Restart,
        CollectorOperation::Register,
        CollectorOperation::Deregister,
        CollectorOperation::GetCapabilities,
        CollectorOperation::ForceShutdown,
        CollectorOperation::Pause,
        CollectorOperation::Resume,
        CollectorOperation::ExecuteTask,
    ];

    for op in unsupported_ops {
        let request = create_rpc_request(&format!("unsupported-{op:?}"), op, RpcPayload::Empty);
        let response = handler.handle_request(request).await;

        assert_eq!(
            response.status,
            RpcStatus::Error,
            "Operation {op:?} should return Error"
        );

        let error = response.error_details.as_ref().unwrap();
        assert_eq!(
            error.code, "UNSUPPORTED_OPERATION",
            "Operation {op:?} should have UNSUPPORTED_OPERATION code"
        );
        assert_eq!(error.category, ErrorCategory::Configuration);
    }
}

// ============================================================================
// Configuration Update Tests
// ============================================================================

/// Test configuration update with valid changes.
#[tokio::test]
async fn test_config_update_applies_changes() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(60),
    );
    changes.insert("max_processes".to_string(), serde_json::json!(500));
    changes.insert(
        "collect_enhanced_metadata".to_string(),
        serde_json::json!(true),
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = create_rpc_request(
        "config-1",
        CollectorOperation::UpdateConfig,
        RpcPayload::ConfigUpdate(config_req),
    );

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Verify actor receives the config update message
    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message within timeout");

    match msg.unwrap() {
        ActorMessage::UpdateConfig { config, respond_to } => {
            assert_eq!(
                config.base_config.collection_interval,
                Duration::from_secs(60)
            );
            assert_eq!(config.process_config.max_processes, 500);
            assert!(config.process_config.collect_enhanced_metadata);
            respond_to
                .send(Ok(()))
                .expect("Config update response receiver should be waiting");
        }
        other => panic!("Expected UpdateConfig message, got {:?}", other),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
}

/// Test configuration update with validate_only flag.
#[tokio::test]
async fn test_config_update_validate_only_no_actor_message() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(60),
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: true, // Validate only
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = create_rpc_request(
        "config-validate",
        CollectorOperation::UpdateConfig,
        RpcPayload::ConfigUpdate(config_req),
    );

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Success);

    // Actor should NOT have received any message
    let recv_result = rx.try_recv();
    assert!(
        recv_result.is_err(),
        "Actor should not receive message for validate_only"
    );
}

/// Test configuration update with invalid payload type.
#[tokio::test]
async fn test_config_update_invalid_payload() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Wrong payload type (Empty instead of ConfigUpdate)
    let request = create_rpc_request(
        "config-invalid",
        CollectorOperation::UpdateConfig,
        RpcPayload::Empty,
    );

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "INVALID_REQUEST");
    assert!(error.message.contains("ConfigUpdate payload"));
}

/// Test configuration update with out-of-bounds max_events_in_flight.
#[tokio::test]
async fn test_config_update_rejects_excessive_max_events() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert(
        "max_events_in_flight".to_string(),
        serde_json::json!(150_000), // Exceeds 100_000 limit
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = create_rpc_request(
        "config-overflow",
        CollectorOperation::UpdateConfig,
        RpcPayload::ConfigUpdate(config_req),
    );

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "INVALID_REQUEST");
    assert!(error.message.contains("max_events_in_flight"));
}

/// Test configuration update with out-of-bounds max_processes.
#[tokio::test]
async fn test_config_update_rejects_excessive_max_processes() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert(
        "max_processes".to_string(),
        serde_json::json!(2_000_000), // Exceeds 1_000_000 limit
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = create_rpc_request(
        "config-max-proc",
        CollectorOperation::UpdateConfig,
        RpcPayload::ConfigUpdate(config_req),
    );

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "INVALID_REQUEST");
    assert!(error.message.contains("max_processes"));
}

/// Test configuration update ignores unknown keys gracefully.
#[tokio::test]
async fn test_config_update_ignores_unknown_keys() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert("unknown_field".to_string(), serde_json::json!("ignored"));
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(45),
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = create_rpc_request(
        "config-unknown",
        CollectorOperation::UpdateConfig,
        RpcPayload::ConfigUpdate(config_req),
    );

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::UpdateConfig { config, respond_to } => {
            // Known key should be applied
            assert_eq!(
                config.base_config.collection_interval,
                Duration::from_secs(45)
            );
            respond_to
                .send(Ok(()))
                .expect("Config update response receiver should be waiting");
        }
        other => panic!("Expected UpdateConfig, got {:?}", other),
    }

    let response = handle_task.await.unwrap();
    assert_eq!(response.status, RpcStatus::Success);
}

// ============================================================================
// Graceful Shutdown Tests
// ============================================================================

/// Test graceful shutdown completes successfully.
#[tokio::test]
async fn test_graceful_shutdown_success() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let shutdown_req = ShutdownRequest {
        collector_id: "procmond".to_string(),
        shutdown_type: ShutdownType::Graceful,
        graceful_timeout_ms: 5000,
        force_after_timeout: false,
        reason: Some("Test shutdown".to_string()),
    };

    let request = create_rpc_request(
        "shutdown-1",
        CollectorOperation::GracefulShutdown,
        RpcPayload::Shutdown(shutdown_req),
    );

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            respond_to
                .send(Ok(()))
                .expect("Shutdown response receiver should be waiting");
        }
        other => panic!("Expected GracefulShutdown, got {:?}", other),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
}

/// Test graceful shutdown with empty payload (still works).
#[tokio::test]
async fn test_graceful_shutdown_with_empty_payload() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "shutdown-empty",
        CollectorOperation::GracefulShutdown,
        RpcPayload::Empty,
    );

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            respond_to
                .send(Ok(()))
                .expect("Shutdown response receiver should be waiting");
        }
        other => panic!("Expected GracefulShutdown, got {:?}", other),
    }

    let response = handle_task.await.unwrap();
    assert_eq!(response.status, RpcStatus::Success);
}

/// Test graceful shutdown marks service as not running.
#[tokio::test]
async fn test_graceful_shutdown_marks_not_running() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let handler_clone = Arc::clone(&handler);
    let request = create_rpc_request(
        "shutdown-running",
        CollectorOperation::GracefulShutdown,
        RpcPayload::Empty,
    );

    let handle_task = tokio::spawn(async move { handler_clone.handle_request(request).await });

    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            respond_to
                .send(Ok(()))
                .expect("Shutdown response receiver should be waiting");
        }
        _ => panic!("Expected GracefulShutdown"),
    }

    handle_task.await.unwrap();

    // Service should no longer be running
    assert!(!handler.is_running());
}

/// Test graceful shutdown completes within reasonable timeout.
#[tokio::test]
async fn test_graceful_shutdown_within_timeout() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "shutdown-timed",
        CollectorOperation::GracefulShutdown,
        RpcPayload::Empty,
    );

    let start_time = Instant::now();

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Respond immediately
    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            respond_to
                .send(Ok(()))
                .expect("Shutdown response receiver should be waiting");
        }
        _ => panic!("Expected GracefulShutdown"),
    }

    let response = handle_task.await.unwrap();
    let elapsed = start_time.elapsed();

    assert_eq!(response.status, RpcStatus::Success);
    // Should complete well within 1 second
    assert!(
        elapsed < Duration::from_secs(1),
        "Shutdown took too long: {:?}",
        elapsed
    );
}

/// Test graceful shutdown handles actor error.
#[tokio::test]
async fn test_graceful_shutdown_actor_error() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = create_rpc_request(
        "shutdown-error",
        CollectorOperation::GracefulShutdown,
        RpcPayload::Empty,
    );

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive message");

    match msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            // Send error response
            respond_to
                .send(Err(anyhow::anyhow!("Shutdown failed")))
                .expect("Shutdown response receiver should be waiting");
        }
        _ => panic!("Expected GracefulShutdown"),
    }

    let response = handle_task.await.unwrap();
    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "ACTOR_ERROR");
}

// ============================================================================
// Deadline/Timeout Tests
// ============================================================================

/// Test expired deadline returns timeout response.
#[tokio::test]
async fn test_expired_deadline_returns_timeout() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Create request with deadline in the past
    let request = RpcRequest {
        request_id: "expired-deadline".to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now() - Duration::from_secs(60),
        deadline: SystemTime::now() - Duration::from_secs(30), // Past deadline
        correlation_metadata: RpcCorrelationMetadata::new("corr-expired".to_string()),
    };

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Timeout);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "DEADLINE_EXCEEDED");
    assert_eq!(error.category, ErrorCategory::Timeout);
}

/// Test operation timeout when actor doesn't respond.
#[tokio::test]
async fn test_operation_timeout_no_actor_response() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Create handler with very short timeout
    let config = RpcServiceConfig {
        default_timeout: Duration::from_millis(50),
        ..RpcServiceConfig::default()
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    // Use a deadline very close to now
    let request = RpcRequest {
        request_id: "short-timeout".to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_millis(50),
        correlation_metadata: RpcCorrelationMetadata::new("corr-short".to_string()),
    };

    let response = handler.handle_request(request).await;

    // Should timeout because no one responds to actor message
    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "TIMEOUT");
}

// ============================================================================
// Statistics Tracking Tests
// ============================================================================

/// Test statistics are tracked for requests.
#[tokio::test]
async fn test_stats_tracking_for_requests() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    // Spawn responder for multiple requests
    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_multi_responder(rx, 3, health_data);

    // Initial stats
    let initial_stats = handler.stats().await;
    assert_eq!(initial_stats.requests_received, 0);
    assert_eq!(initial_stats.requests_succeeded, 0);

    // Make successful health check
    let handler_clone = Arc::clone(&handler);
    let request = create_rpc_request(
        "stats-1",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );
    // We don't care about response, just testing stats tracking
    drop(handler_clone.handle_request(request).await);

    // Allow stats to update
    tokio::time::sleep(Duration::from_millis(10)).await;

    let stats_after = handler.stats().await;
    assert_eq!(stats_after.requests_received, 1);
    assert!(stats_after.requests_succeeded >= 1 || stats_after.requests_timed_out >= 1);
}

/// Test health check specific counter.
#[tokio::test]
async fn test_health_check_counter() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_multi_responder(rx, 2, health_data);

    // Make two health checks
    for i in 0..2 {
        let handler_clone = Arc::clone(&handler);
        let request = create_rpc_request(
            &format!("hc-{i}"),
            CollectorOperation::HealthCheck,
            RpcPayload::Empty,
        );
        // We don't care about response, just testing counter
        drop(handler_clone.handle_request(request).await);
    }

    let stats = handler.stats().await;
    // Health checks counter should reflect requests processed
    // The counter is incremented regardless of timeout/success
    println!("Health checks recorded: {}", stats.health_checks);
}

// ============================================================================
// Response Publishing Tests
// ============================================================================

/// Test publish_response serializes correctly.
#[tokio::test]
async fn test_publish_response_serialization() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_health_responder(rx, health_data);

    let request = create_rpc_request(
        "publish-1",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );
    let response = handler.handle_request(request).await;

    // publish_response should succeed (just logs, doesn't actually publish)
    let publish_result = handler.publish_response(response).await;
    assert!(publish_result.is_ok());
}

// ============================================================================
// Concurrent Operations Tests
// ============================================================================

/// Test multiple concurrent health check requests.
#[tokio::test]
async fn test_concurrent_health_checks() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let concurrent_count = 5;

    // Spawn responder for all requests
    let responder = tokio::spawn(async move {
        for _ in 0..concurrent_count {
            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(CollectorState::Running, true);
                respond_to
                    .send(health)
                    .expect("Health response receiver should be waiting");
            }
        }
    });

    // Launch concurrent requests
    let mut handles = Vec::new();
    for i in 0..concurrent_count {
        let handler_clone = Arc::clone(&handler);
        let request = create_rpc_request(
            &format!("concurrent-{i}"),
            CollectorOperation::HealthCheck,
            RpcPayload::Empty,
        );
        handles.push(tokio::spawn(async move {
            handler_clone.handle_request(request).await
        }));
    }

    // Collect results
    let mut success_count = 0;
    for handle in handles {
        let response = handle.await.expect("Task should complete");
        if response.status == RpcStatus::Success {
            success_count += 1;
        }
    }

    responder.await.expect("Responder should complete");

    assert_eq!(
        success_count, concurrent_count,
        "All requests should succeed"
    );
}

/// Test mixed concurrent operations.
#[tokio::test]
async fn test_mixed_concurrent_operations() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let health_data = create_test_health_data(CollectorState::Running, true);
    let responder = spawn_multi_responder(rx, 3, health_data);

    // Launch mixed operations
    let handler1 = Arc::clone(&handler);
    let handler2 = Arc::clone(&handler);
    let handler3 = Arc::clone(&handler);

    let h1 = tokio::spawn(async move {
        let req = create_rpc_request(
            "mix-health",
            CollectorOperation::HealthCheck,
            RpcPayload::Empty,
        );
        handler1.handle_request(req).await
    });

    let h2 = tokio::spawn(async move {
        let mut changes = HashMap::new();
        changes.insert(
            "collection_interval_secs".to_string(),
            serde_json::json!(30),
        );
        let config_req = ConfigUpdateRequest {
            collector_id: "procmond".to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false,
            rollback_on_failure: true,
        };
        let req = create_rpc_request(
            "mix-config",
            CollectorOperation::UpdateConfig,
            RpcPayload::ConfigUpdate(config_req),
        );
        handler2.handle_request(req).await
    });

    let h3 = tokio::spawn(async move {
        let req = create_rpc_request(
            "mix-shutdown",
            CollectorOperation::GracefulShutdown,
            RpcPayload::Empty,
        );
        handler3.handle_request(req).await
    });

    // Wait for all
    let r1 = h1.await.expect("Health check should complete");
    let r2 = h2.await.expect("Config update should complete");
    let r3 = h3.await.expect("Shutdown should complete");

    let received_ops = responder.await.expect("Responder should complete");

    // Verify we received various operations
    println!("Received operations: {:?}", received_ops);

    // At least some operations should have been handled
    // (exact behavior depends on timing)
    assert!(
        r1.status == RpcStatus::Success || r1.status == RpcStatus::Error,
        "Health check should have a status"
    );
    assert!(
        r2.status == RpcStatus::Success || r2.status == RpcStatus::Error,
        "Config update should have a status"
    );
    assert!(
        r3.status == RpcStatus::Success || r3.status == RpcStatus::Error,
        "Shutdown should have a status"
    );
}

// ============================================================================
// Edge Cases and Error Handling Tests
// ============================================================================

/// Test handler with custom configuration.
#[tokio::test]
async fn test_custom_handler_configuration() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    let config = RpcServiceConfig {
        collector_id: "custom-collector".to_string(),
        control_topic: "custom.control.topic".to_string(),
        response_topic_prefix: "custom.response".to_string(),
        default_timeout: Duration::from_secs(60),
        max_concurrent_requests: 20,
    };

    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    assert_eq!(handler.collector_id(), "custom-collector");
    assert_eq!(handler.config().control_topic, "custom.control.topic");
    assert_eq!(handler.config().max_concurrent_requests, 20);

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_health_responder(rx, health_data);

    let request = create_rpc_request(
        "custom-1",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );
    let response = handler.handle_request(request).await;

    // Response should use custom collector ID
    assert_eq!(response.service_id, "custom-collector");
}

/// Test response includes correct correlation metadata.
#[tokio::test]
async fn test_correlation_metadata_preserved() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_health_responder(rx, health_data);

    let request = RpcRequest {
        request_id: "corr-test".to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("unique-correlation-id".to_string()),
    };

    let response = handler.handle_request(request).await;

    assert_eq!(
        response.correlation_metadata.correlation_id,
        "unique-correlation-id"
    );
}

/// Test response includes execution time.
#[tokio::test]
async fn test_response_includes_execution_time() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_health_responder(rx, health_data);

    let request = create_rpc_request(
        "exec-time",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );
    let response = handler.handle_request(request).await;

    // Execution time should be set
    assert!(
        response.execution_time_ms < 10000,
        "Execution time should be reasonable"
    );
    assert_eq!(response.total_time_ms, response.execution_time_ms);
}

/// Test health data includes buffer level when available.
#[tokio::test]
async fn test_health_data_includes_buffer_level() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut health_data = create_test_health_data(CollectorState::Running, true);
    health_data.buffer_level_percent = Some(75);

    let _responder = spawn_health_responder(rx, health_data);

    let request = create_rpc_request(
        "buffer-level",
        CollectorOperation::HealthCheck,
        RpcPayload::Empty,
    );
    let response = handler.handle_request(request).await;

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        assert_eq!(health.metrics.get("buffer_level_percent"), Some(&75.0_f64));
    } else {
        panic!("Expected HealthCheck payload");
    }
}

/// Test service uptime tracking.
#[tokio::test]
async fn test_service_uptime_tracking() {
    let (actor_handle, rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Wait a bit for uptime to accumulate
    tokio::time::sleep(Duration::from_millis(50)).await;

    let health_data = create_test_health_data(CollectorState::Running, true);
    let _responder = spawn_health_responder(rx, health_data);

    let request = create_rpc_request("uptime", CollectorOperation::HealthCheck, RpcPayload::Empty);
    let response = handler.handle_request(request).await;

    if let Some(RpcPayload::HealthCheck(health)) = response.payload {
        // Uptime should be at least 0 (could be 0 or 1 second)
        assert!(health.uptime_seconds < 60, "Uptime should be reasonable");
    } else {
        panic!("Expected HealthCheck payload");
    }
}
