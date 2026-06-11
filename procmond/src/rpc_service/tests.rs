//! Unit tests for the RPC service handler.

use super::*;
use crate::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorMessage, CollectorState};
use daemoneye_eventbus::rpc::RpcCorrelationMetadata;
use tokio::sync::mpsc;
/// Creates a test actor handle with a receiver for inspecting messages.
fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    (ActorHandle::new(tx), rx)
}

/// Creates an EventBusConnector with a unique temp directory for test isolation.
async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create event bus connector");
    (Arc::new(RwLock::new(connector)), temp_dir)
}

#[tokio::test]
async fn test_rpc_service_config_default() {
    let config = RpcServiceConfig::default();
    assert_eq!(config.collector_id, "procmond");
    assert_eq!(config.control_topic, PROCMOND_CONTROL_TOPIC);
    assert_eq!(config.default_timeout, Duration::from_secs(30));
    assert_eq!(config.max_concurrent_requests, 10);
}

#[tokio::test]
async fn test_create_success_response() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-123".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-123".to_string()),
    };

    let start_time = std::time::Instant::now();
    let response = handler.create_success_response(&request, Some(RpcPayload::Empty), start_time);

    assert_eq!(response.request_id, "test-123");
    assert_eq!(response.service_id, "procmond");
    assert_eq!(response.operation, CollectorOperation::HealthCheck);
    assert_eq!(response.status, RpcStatus::Success);
    assert!(response.error_details.is_none());
}

#[tokio::test]
async fn test_create_error_response() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-456".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-456".to_string()),
    };

    let error = RpcServiceError::InvalidRequest("Missing config payload".to_string());
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    assert_eq!(response.request_id, "test-456");
    assert_eq!(response.status, RpcStatus::Error);
    assert!(response.error_details.is_some());
    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "INVALID_REQUEST");
}

#[tokio::test]
async fn test_convert_health_data() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::Running,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: true,
        buffer_level_percent: Some(25),
        last_collection: Some(std::time::Instant::now()),
        collection_cycles: 100,
        lifecycle_events: 50,
        collection_errors: 2,
        backpressure_events: 5,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);

    assert_eq!(health_data.collector_id, "procmond");
    assert_eq!(health_data.status, HealthStatus::Healthy);
    assert!(health_data.components.contains_key("event_bus"));
    assert!(health_data.components.contains_key("collector"));
    assert_eq!(
        health_data.metrics.get("collection_cycles"),
        Some(&100.0_f64)
    );
    assert_eq!(health_data.metrics.get("lifecycle_events"), Some(&50.0_f64));
}

#[tokio::test]
async fn test_convert_health_data_degraded() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::Running,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: false, // Disconnected
        buffer_level_percent: Some(80),
        last_collection: Some(std::time::Instant::now()),
        collection_cycles: 50,
        lifecycle_events: 25,
        collection_errors: 10,
        backpressure_events: 15,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);

    assert_eq!(health_data.status, HealthStatus::Degraded);
    let event_bus_health = health_data.components.get("event_bus").unwrap();
    assert_eq!(event_bus_health.status, HealthStatus::Degraded);
}

#[tokio::test]
async fn test_build_config_from_changes() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let mut changes = HashMap::new();
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(60),
    );
    changes.insert("max_processes".to_string(), serde_json::json!(500));
    changes.insert(
        "compute_executable_hashes".to_string(),
        serde_json::json!(true),
    );

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let _ = handler; // Silence unused warning
    let config = RpcServiceHandler::build_config_from_changes(&config_request)
        .expect("Config should be valid");

    assert_eq!(
        config.base_config.collection_interval,
        Duration::from_mins(1)
    );
    assert_eq!(config.process_config.max_processes, 500);
    assert!(config.process_config.compute_executable_hashes);
}

#[tokio::test]
async fn test_build_config_rejects_overflow_max_events_in_flight() {
    let mut changes = HashMap::new();
    changes.insert(
        "max_events_in_flight".to_string(),
        serde_json::json!(u64::MAX),
    );

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let result = RpcServiceHandler::build_config_from_changes(&config_request);
    assert!(result.is_err(), "Should reject overflow values");
}

#[tokio::test]
async fn test_build_config_rejects_overflow_max_processes() {
    let mut changes = HashMap::new();
    changes.insert("max_processes".to_string(), serde_json::json!(u64::MAX));

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let result = RpcServiceHandler::build_config_from_changes(&config_request);
    assert!(result.is_err(), "Should reject overflow values");
}

#[tokio::test]
async fn test_stats_tracking() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 0);
    assert_eq!(stats.requests_succeeded, 0);
    assert_eq!(stats.requests_failed, 0);
}

#[tokio::test]
async fn test_expired_deadline_returns_timeout() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Create request with deadline in the past
    let request = RpcRequest {
        request_id: "test-expired".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now() - Duration::from_mins(1),
        deadline: SystemTime::now() - Duration::from_secs(30), // Past deadline
        correlation_metadata: RpcCorrelationMetadata::new("corr-expired".to_string()),
    };

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Timeout);
    assert!(response.error_details.is_some());
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "DEADLINE_EXCEEDED");
    assert_eq!(error.category, ErrorCategory::Timeout);
}

#[tokio::test]
async fn test_health_check_sends_message_to_actor() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-health".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-health".to_string()),
    };

    // Spawn response handler - the actor needs to respond to the health check
    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Wait for the health check message from the actor
    let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(msg.is_ok(), "Actor should receive a message");
    let actor_msg = msg.unwrap();
    assert!(actor_msg.is_some(), "Message should be present");

    // Verify it's a health check message
    match actor_msg.unwrap() {
        ActorMessage::HealthCheck { respond_to } => {
            // Respond with mock health data
            let health_data = crate::monitor_collector::HealthCheckData {
                state: CollectorState::Running,
                collection_interval: Duration::from_secs(30),
                original_interval: Duration::from_secs(30),
                event_bus_connected: true,
                buffer_level_percent: Some(10),
                last_collection: Some(std::time::Instant::now()),
                collection_cycles: 5,
                lifecycle_events: 2,
                collection_errors: 0,
                backpressure_events: 0,
                operational_sub_status: None,
            };
            // Intentionally ignore send result - receiver may have been dropped
            drop(respond_to.send(health_data));
        }
        ActorMessage::UpdateConfig { .. }
        | ActorMessage::GracefulShutdown { .. }
        | ActorMessage::BeginMonitoring
        | ActorMessage::AdjustInterval { .. } => {
            panic!("Expected HealthCheck message")
        }
    }

    // Wait for the response
    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
    assert!(response.payload.is_some());
}

#[tokio::test]
async fn test_config_update_invalid_payload() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Create config update request with wrong payload type (Empty instead of ConfigUpdate)
    let request = RpcRequest {
        request_id: "test-bad-config".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::Empty, // Wrong payload type
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-bad-config".to_string()),
    };

    let response = handler.handle_request(request).await;

    assert_eq!(response.status, RpcStatus::Error);
    assert!(response.error_details.is_some());
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "INVALID_REQUEST");
}

#[tokio::test]
async fn test_create_timeout_response() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-timeout".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() - Duration::from_secs(1), // Already expired
        correlation_metadata: RpcCorrelationMetadata::new("corr-timeout".to_string()),
    };

    let start_time = std::time::Instant::now();
    let response = handler.create_timeout_response(&request, start_time);

    assert_eq!(response.request_id, "test-timeout");
    assert_eq!(response.service_id, "procmond");
    assert_eq!(response.status, RpcStatus::Timeout);
    assert!(response.error_details.is_some());
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "DEADLINE_EXCEEDED");
    assert_eq!(error.category, ErrorCategory::Timeout);
}

// ============================================================
// Additional comprehensive tests for >80% coverage
// ============================================================

#[tokio::test]
async fn test_collector_id_returns_config_value() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let config = RpcServiceConfig {
        collector_id: "test-collector".to_owned(),
        ..RpcServiceConfig::default()
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    assert_eq!(handler.collector_id(), "test-collector");
}

#[tokio::test]
async fn test_is_running_initial_state_false() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Initially should not be running
    assert!(!handler.is_running());
}

#[tokio::test]
async fn test_config_returns_configuration() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let config = RpcServiceConfig {
        collector_id: "my-collector".to_owned(),
        control_topic: "custom.topic".to_owned(),
        response_topic_prefix: "custom.response".to_owned(),
        default_timeout: Duration::from_mins(1),
        max_concurrent_requests: 20,
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config.clone());

    let retrieved_config = handler.config();
    assert_eq!(retrieved_config.collector_id, "my-collector");
    assert_eq!(retrieved_config.control_topic, "custom.topic");
    assert_eq!(retrieved_config.response_topic_prefix, "custom.response");
    assert_eq!(retrieved_config.default_timeout, Duration::from_mins(1));
    assert_eq!(retrieved_config.max_concurrent_requests, 20);
}

#[tokio::test]
async fn test_unsupported_operation_returns_error() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Test each unsupported operation
    let unsupported_ops = [
        CollectorOperation::Register,
        CollectorOperation::Deregister,
        CollectorOperation::Start,
        CollectorOperation::Stop,
        CollectorOperation::Restart,
        CollectorOperation::GetCapabilities,
        CollectorOperation::ForceShutdown,
        CollectorOperation::Pause,
        CollectorOperation::Resume,
        CollectorOperation::ExecuteTask,
    ];

    for op in unsupported_ops {
        let request = RpcRequest {
            request_id: format!("test-unsupported-{op:?}"),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: op,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
            correlation_metadata: RpcCorrelationMetadata::new(format!("corr-unsupported-{op:?}")),
        };

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

#[tokio::test]
async fn test_graceful_shutdown_sends_message_to_actor() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    use daemoneye_eventbus::rpc::{ShutdownRequest, ShutdownType};
    let shutdown_req = ShutdownRequest {
        collector_id: "procmond".to_string(),
        shutdown_type: ShutdownType::Graceful,
        graceful_timeout_ms: 5000,
        force_after_timeout: false,
        reason: Some("Test shutdown".to_string()),
    };

    let request = RpcRequest {
        request_id: "test-shutdown".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::GracefulShutdown,
        payload: RpcPayload::Shutdown(shutdown_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-shutdown".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Wait for the shutdown message
    let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(msg.is_ok(), "Actor should receive a message");
    let actor_msg = msg.unwrap();
    assert!(actor_msg.is_some(), "Message should be present");

    match actor_msg.unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            drop(respond_to.send(Ok(())));
        }
        other => panic!("Expected GracefulShutdown message, got {:?}", other),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
}

#[tokio::test]
async fn test_graceful_shutdown_without_payload() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Shutdown request with Empty payload (should still work)
    let request = RpcRequest {
        request_id: "test-shutdown-empty".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::GracefulShutdown,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-shutdown-empty".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(msg.is_ok());
    match msg.unwrap().unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            drop(respond_to.send(Ok(())));
        }
        _ => panic!("Expected GracefulShutdown message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
}

#[tokio::test]
async fn test_graceful_shutdown_marks_service_not_running() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let handler_clone = Arc::clone(&handler);
    let request = RpcRequest {
        request_id: "test-shutdown-running".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::GracefulShutdown,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-running".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler_clone.handle_request(request).await });

    let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    match msg.unwrap().unwrap() {
        ActorMessage::GracefulShutdown { respond_to } => {
            drop(respond_to.send(Ok(())));
        }
        _ => panic!("Expected GracefulShutdown message"),
    }

    handle_task.await.expect("Handle task should complete");
    // After shutdown, is_running should be false
    assert!(!handler.is_running());
}

#[tokio::test]
async fn test_config_update_validate_only() {
    let (actor_handle, _rx) = create_test_actor();
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
        validate_only: true, // Only validate, don't apply
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = RpcRequest {
        request_id: "test-validate-only".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::ConfigUpdate(config_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-validate".to_string()),
    };

    let response = handler.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);
    // No message should have been sent to the actor for validate_only
}

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

    let request = RpcRequest {
        request_id: "test-apply-config".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::ConfigUpdate(config_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-apply".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(msg.is_ok());
    match msg.unwrap().unwrap() {
        ActorMessage::UpdateConfig { config, respond_to } => {
            // Verify the config was built correctly
            assert_eq!(
                config.base_config.collection_interval,
                Duration::from_mins(1)
            );
            assert_eq!(config.process_config.max_processes, 500);
            assert!(config.process_config.collect_enhanced_metadata);
            drop(respond_to.send(Ok(())));
        }
        _ => panic!("Expected UpdateConfig message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Success);
}

#[tokio::test]
async fn test_config_update_unknown_key_ignored() {
    let mut changes = HashMap::new();
    changes.insert("unknown_field".to_string(), serde_json::json!("value"));
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(30),
    );

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    // Should not error, just log warning and ignore unknown key
    let config = RpcServiceHandler::build_config_from_changes(&config_request)
        .expect("Should succeed with unknown key");

    assert_eq!(
        config.base_config.collection_interval,
        Duration::from_secs(30)
    );
}

#[tokio::test]
async fn test_config_update_max_events_in_flight() {
    let mut changes = HashMap::new();
    changes.insert("max_events_in_flight".to_string(), serde_json::json!(5000));

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let config = RpcServiceHandler::build_config_from_changes(&config_request)
        .expect("Should succeed with valid max_events_in_flight");

    assert_eq!(config.base_config.max_events_in_flight, 5000);
}

#[tokio::test]
async fn test_config_update_max_events_in_flight_exceeds_limit() {
    let mut changes = HashMap::new();
    // Exceed the MAX_EVENTS_IN_FLIGHT_LIMIT (100_000)
    changes.insert(
        "max_events_in_flight".to_string(),
        serde_json::json!(150_000),
    );

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let result = RpcServiceHandler::build_config_from_changes(&config_request);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(
        matches!(error, RpcServiceError::InvalidRequest(_)),
        "Expected InvalidRequest error"
    );
}

#[tokio::test]
async fn test_config_update_max_processes_exceeds_limit() {
    let mut changes = HashMap::new();
    // Exceed the MAX_PROCESSES_LIMIT (1_000_000)
    changes.insert("max_processes".to_string(), serde_json::json!(2_000_000));

    let config_request = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let result = RpcServiceHandler::build_config_from_changes(&config_request);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(
        matches!(error, RpcServiceError::InvalidRequest(_)),
        "Expected InvalidRequest error"
    );
}

#[tokio::test]
async fn test_convert_health_data_waiting_for_agent() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::WaitingForAgent,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: true,
        buffer_level_percent: None,
        last_collection: None,
        collection_cycles: 0,
        lifecycle_events: 0,
        collection_errors: 0,
        backpressure_events: 0,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);
    assert_eq!(health_data.status, HealthStatus::Healthy);

    let collector_health = health_data.components.get("collector").unwrap();
    assert_eq!(collector_health.status, HealthStatus::Healthy);
    assert_eq!(
        collector_health.message.as_deref(),
        Some("idle-awaiting-begin")
    );
}

#[tokio::test]
async fn test_convert_health_data_shutting_down() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::ShuttingDown,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: false,
        buffer_level_percent: Some(50),
        last_collection: Some(std::time::Instant::now()),
        collection_cycles: 100,
        lifecycle_events: 50,
        collection_errors: 5,
        backpressure_events: 10,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);
    assert_eq!(health_data.status, HealthStatus::Unhealthy);

    let collector_health = health_data.components.get("collector").unwrap();
    assert_eq!(collector_health.status, HealthStatus::Unhealthy);
}

#[tokio::test]
async fn test_convert_health_data_stopped() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::Stopped,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: false,
        buffer_level_percent: None,
        last_collection: None,
        collection_cycles: 0,
        lifecycle_events: 0,
        collection_errors: 0,
        backpressure_events: 0,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);
    assert_eq!(health_data.status, HealthStatus::Unresponsive);

    let collector_health = health_data.components.get("collector").unwrap();
    assert_eq!(collector_health.status, HealthStatus::Unhealthy);
}

#[tokio::test]
async fn test_convert_health_data_no_buffer_level() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let actor_health = ActorHealthCheckData {
        state: CollectorState::Running,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: true,
        buffer_level_percent: None, // No buffer level
        last_collection: Some(std::time::Instant::now()),
        collection_cycles: 10,
        lifecycle_events: 5,
        collection_errors: 0,
        backpressure_events: 0,
        operational_sub_status: None,
    };

    let health_data = handler.convert_health_data(&actor_health);
    // Should not have buffer_level_percent metric when None
    assert!(!health_data.metrics.contains_key("buffer_level_percent"));
}

#[tokio::test]
async fn test_error_response_subscription_failed() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-sub-fail".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-sub-fail".to_string()),
    };

    let error = RpcServiceError::SubscriptionFailed("Topic not found".to_string());
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    assert_eq!(response.status, RpcStatus::Error);
    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "SUBSCRIPTION_FAILED");
    assert_eq!(error_details.category, ErrorCategory::Communication);
}

#[tokio::test]
async fn test_error_response_publish_failed() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-pub-fail".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-pub-fail".to_string()),
    };

    let error = RpcServiceError::PublishFailed("Broker unavailable".to_string());
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "PUBLISH_FAILED");
    assert_eq!(error_details.category, ErrorCategory::Communication);
}

#[tokio::test]
async fn test_error_response_actor_error() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-actor-err".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-actor-err".to_string()),
    };

    let error = RpcServiceError::ActorError("Channel closed".to_string());
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "ACTOR_ERROR");
    assert_eq!(error_details.category, ErrorCategory::Internal);
}

#[tokio::test]
async fn test_error_response_timeout() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-timeout-err".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-timeout-err".to_string()),
    };

    let error = RpcServiceError::Timeout { timeout_ms: 5000 };
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "TIMEOUT");
    assert_eq!(error_details.category, ErrorCategory::Timeout);
}

#[tokio::test]
async fn test_error_response_shutting_down() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-shutting-down".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-shutting-down".to_string()),
    };

    let error = RpcServiceError::ShuttingDown;
    let start_time = std::time::Instant::now();
    let response = handler.create_error_response(&request, &error, start_time);

    let error_details = response.error_details.unwrap();
    assert_eq!(error_details.code, "SHUTTING_DOWN");
    assert_eq!(error_details.category, ErrorCategory::Internal);
}

#[tokio::test]
async fn test_publish_response_fails_when_disconnected() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let response = RpcResponse {
        request_id: "test-publish".to_string(),
        service_id: "procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        status: RpcStatus::Success,
        payload: Some(RpcPayload::Empty),
        timestamp: SystemTime::now(),
        execution_time_ms: 10,
        queue_time_ms: None,
        total_time_ms: 10,
        error_details: None,
        correlation_metadata: RpcCorrelationMetadata::new("corr-publish".to_string()),
    };

    // publish_response now actually publishes via event bus;
    // without a connected broker, it should return PublishFailed
    let result = handler.publish_response(response).await;
    assert!(result.is_err());
    assert!(matches!(result, Err(RpcServiceError::PublishFailed(_))));

    // Verify responses_failed counter was incremented
    let stats = handler.stats().await;
    assert_eq!(stats.responses_failed, 1);
}

#[tokio::test]
async fn test_response_contains_correlation_metadata() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let correlation = RpcCorrelationMetadata::new("unique-correlation-id".to_string());

    let request = RpcRequest {
        request_id: "test-correlation".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: correlation.clone(),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Respond to actor
    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::HealthCheck { respond_to } => {
            let health_data = ActorHealthCheckData {
                state: CollectorState::Running,
                collection_interval: Duration::from_secs(30),
                original_interval: Duration::from_secs(30),
                event_bus_connected: true,
                buffer_level_percent: Some(10),
                last_collection: Some(std::time::Instant::now()),
                collection_cycles: 5,
                lifecycle_events: 2,
                collection_errors: 0,
                backpressure_events: 0,
                operational_sub_status: None,
            };
            drop(respond_to.send(health_data));
        }
        _ => panic!("Expected HealthCheck message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(
        response.correlation_metadata.correlation_id,
        "unique-correlation-id"
    );
}

#[tokio::test]
async fn test_stats_increment_on_health_check_success() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let handler_clone = Arc::clone(&handler);
    let request = RpcRequest {
        request_id: "test-stats-health".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-stats-health".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler_clone.handle_request(request).await });

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::HealthCheck { respond_to } => {
            let health_data = ActorHealthCheckData {
                state: CollectorState::Running,
                collection_interval: Duration::from_secs(30),
                original_interval: Duration::from_secs(30),
                event_bus_connected: true,
                buffer_level_percent: Some(10),
                last_collection: Some(std::time::Instant::now()),
                collection_cycles: 5,
                lifecycle_events: 2,
                collection_errors: 0,
                backpressure_events: 0,
                operational_sub_status: None,
            };
            drop(respond_to.send(health_data));
        }
        _ => panic!("Expected HealthCheck message"),
    }

    handle_task.await.unwrap();

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 1);
    assert_eq!(stats.requests_succeeded, 1);
    assert_eq!(stats.health_checks, 1);
}

#[tokio::test]
async fn test_stats_increment_on_config_update_success() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let handler_clone = Arc::clone(&handler);
    let mut changes = HashMap::new();
    changes.insert(
        "collection_interval_secs".to_string(),
        serde_json::json!(60),
    );

    let config_req = ConfigUpdateRequest {
        collector_id: "procmond".to_string(),
        config_changes: changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = RpcRequest {
        request_id: "test-stats-config".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::ConfigUpdate(config_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-stats-config".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler_clone.handle_request(request).await });

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::UpdateConfig { respond_to, .. } => {
            drop(respond_to.send(Ok(())));
        }
        _ => panic!("Expected UpdateConfig message"),
    }

    handle_task.await.unwrap();

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 1);
    assert_eq!(stats.requests_succeeded, 1);
    assert_eq!(stats.config_updates, 1);
}

#[tokio::test]
async fn test_stats_increment_on_shutdown_success() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    let handler_clone = Arc::clone(&handler);
    let request = RpcRequest {
        request_id: "test-stats-shutdown".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::GracefulShutdown,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-stats-shutdown".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler_clone.handle_request(request).await });

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::GracefulShutdown { respond_to } => {
            drop(respond_to.send(Ok(())));
        }
        _ => panic!("Expected GracefulShutdown message"),
    }

    handle_task.await.unwrap();

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 1);
    assert_eq!(stats.requests_succeeded, 1);
    assert_eq!(stats.shutdown_requests, 1);
}

#[tokio::test]
async fn test_stats_increment_on_failure() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    // Send a request that will fail (UpdateConfig with Empty payload)
    let request = RpcRequest {
        request_id: "test-stats-failure".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::Empty, // Invalid payload for UpdateConfig
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-stats-failure".to_string()),
    };

    handler.handle_request(request).await;

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 1);
    assert_eq!(stats.requests_failed, 1);
}

#[tokio::test]
async fn test_stats_increment_on_timeout() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    // Send a request with expired deadline
    let request = RpcRequest {
        request_id: "test-stats-timeout".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now() - Duration::from_mins(1),
        deadline: SystemTime::now() - Duration::from_secs(30), // Already expired
        correlation_metadata: RpcCorrelationMetadata::new("corr-stats-timeout".to_string()),
    };

    handler.handle_request(request).await;

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 1);
    assert_eq!(stats.requests_timed_out, 1);
}

#[tokio::test]
async fn test_concurrent_requests_handled() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(actor_handle, event_bus));

    // Spawn multiple concurrent requests
    let mut handles = Vec::new();
    for i in 0..3 {
        let handler_clone = Arc::clone(&handler);
        let handle = tokio::spawn(async move {
            let request = RpcRequest {
                request_id: format!("concurrent-{i}"),
                client_id: "client-1".to_string(),
                target: "control.collector.procmond".to_string(),
                operation: CollectorOperation::HealthCheck,
                payload: RpcPayload::Empty,
                timestamp: SystemTime::now(),
                deadline: SystemTime::now() + Duration::from_secs(30),
                correlation_metadata: RpcCorrelationMetadata::new(format!("corr-concurrent-{i}")),
            };
            handler_clone.handle_request(request).await
        });
        handles.push(handle);
    }

    // Respond to all actor messages
    for _ in 0..3 {
        let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("Should receive message")
            .unwrap();

        match msg {
            ActorMessage::HealthCheck { respond_to } => {
                let health_data = ActorHealthCheckData {
                    state: CollectorState::Running,
                    collection_interval: Duration::from_secs(30),
                    original_interval: Duration::from_secs(30),
                    event_bus_connected: true,
                    buffer_level_percent: Some(10),
                    last_collection: Some(std::time::Instant::now()),
                    collection_cycles: 5,
                    lifecycle_events: 2,
                    collection_errors: 0,
                    backpressure_events: 0,
                    operational_sub_status: None,
                };
                drop(respond_to.send(health_data));
            }
            _ => panic!("Expected HealthCheck message"),
        }
    }

    // Wait for all responses
    for handle in handles {
        let response = handle.await.expect("Task should complete");
        assert_eq!(response.status, RpcStatus::Success);
    }

    let stats = handler.stats().await;
    assert_eq!(stats.requests_received, 3);
    assert_eq!(stats.requests_succeeded, 3);
}

#[tokio::test]
async fn test_health_check_timeout_from_actor() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Use a very short timeout
    let config = RpcServiceConfig {
        default_timeout: Duration::from_millis(50),
        ..RpcServiceConfig::default()
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    let request = RpcRequest {
        request_id: "test-actor-timeout".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-actor-timeout".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Receive the message but don't respond - let it timeout
    let _msg = rx.recv().await;
    // Don't respond, causing timeout

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "TIMEOUT");
}

#[tokio::test]
async fn test_config_update_actor_error() {
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
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = RpcRequest {
        request_id: "test-config-actor-error".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::UpdateConfig,
        payload: RpcPayload::ConfigUpdate(config_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-config-actor-error".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::UpdateConfig { respond_to, .. } => {
            // Send an error response
            drop(respond_to.send(Err(anyhow::anyhow!("Config validation failed"))));
        }
        _ => panic!("Expected UpdateConfig message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "ACTOR_ERROR");
}

#[tokio::test]
async fn test_graceful_shutdown_actor_error() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-shutdown-actor-error".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::GracefulShutdown,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-shutdown-actor-error".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::GracefulShutdown { respond_to } => {
            drop(respond_to.send(Err(anyhow::anyhow!("Shutdown failed"))));
        }
        _ => panic!("Expected GracefulShutdown message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    assert_eq!(response.status, RpcStatus::Error);
    let error = response.error_details.unwrap();
    assert_eq!(error.code, "ACTOR_ERROR");
}

#[tokio::test]
async fn test_calculate_timeout_uses_shorter_of_deadline_or_default() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Long default timeout
    let config = RpcServiceConfig {
        default_timeout: Duration::from_mins(1),
        ..RpcServiceConfig::default()
    };
    let handler = RpcServiceHandler::new(actor_handle, event_bus, config);

    // Request with short deadline
    let request = RpcRequest {
        request_id: "test-timeout-calc".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(5), // Short deadline
        correlation_metadata: RpcCorrelationMetadata::new("corr-timeout-calc".to_string()),
    };

    let timeout = handler.calculate_timeout(&request);
    // Should use the shorter of deadline (5s) or default (60s)
    assert!(timeout <= Duration::from_secs(6)); // Allow some tolerance
}

#[tokio::test]
async fn test_rpc_service_error_display() {
    // Test all error variants for Display implementation
    let errors = [
        RpcServiceError::SubscriptionFailed("test".to_string()),
        RpcServiceError::PublishFailed("test".to_string()),
        RpcServiceError::ActorError("test".to_string()),
        RpcServiceError::InvalidRequest("test".to_string()),
        RpcServiceError::UnsupportedOperation {
            operation: CollectorOperation::Register,
        },
        RpcServiceError::Timeout { timeout_ms: 1000 },
        RpcServiceError::ShuttingDown,
    ];

    for error in errors {
        let display = format!("{error}");
        assert!(!display.is_empty(), "Error {error:?} should have display");
    }
}

#[tokio::test]
async fn test_response_execution_time_tracked() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    let request = RpcRequest {
        request_id: "test-exec-time".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-exec-time".to_string()),
    };

    let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

    // Add a small delay before responding
    tokio::time::sleep(Duration::from_millis(10)).await;

    let msg = rx.recv().await.unwrap();
    match msg {
        ActorMessage::HealthCheck { respond_to } => {
            let health_data = ActorHealthCheckData {
                state: CollectorState::Running,
                collection_interval: Duration::from_secs(30),
                original_interval: Duration::from_secs(30),
                event_bus_connected: true,
                buffer_level_percent: Some(10),
                last_collection: Some(std::time::Instant::now()),
                collection_cycles: 5,
                lifecycle_events: 2,
                collection_errors: 0,
                backpressure_events: 0,
                operational_sub_status: None,
            };
            drop(respond_to.send(health_data));
        }
        _ => panic!("Expected HealthCheck message"),
    }

    let response = handle_task.await.expect("Handle task should complete");
    // Execution time should be at least 10ms
    assert!(response.execution_time_ms >= 10);
    assert_eq!(response.execution_time_ms, response.total_time_ms);
}

/// Creates a test actor handle with a specified channel capacity.
fn create_test_actor_with_capacity(capacity: usize) -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(capacity);
    (ActorHandle::new(tx), rx)
}

#[tokio::test]
async fn test_actor_channel_full_handled_gracefully() {
    // Create an actor handle with minimal capacity (1)
    let (actor_handle, _rx) = create_test_actor_with_capacity(1);
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let handler = Arc::new(RpcServiceHandler::with_defaults(
        actor_handle.clone(),
        event_bus,
    ));

    // Fill the channel by sending a message without consuming it
    // First request will succeed (fills the channel)
    let first_request = RpcRequest {
        request_id: "first-request".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-first".to_string()),
    };

    // Spawn first request but don't await it - this will fill the channel
    let handler_clone = Arc::clone(&handler);
    let _first_handle =
        tokio::spawn(async move { handler_clone.handle_request(first_request).await });

    // Small delay to ensure first request has sent to the channel
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Second request should fail because the channel is full
    let second_request = RpcRequest {
        request_id: "second-request".to_string(),
        client_id: "client-1".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
        correlation_metadata: RpcCorrelationMetadata::new("corr-second".to_string()),
    };

    let response = handler.handle_request(second_request).await;

    // The response should indicate an error due to channel full
    assert_eq!(
        response.status,
        RpcStatus::Error,
        "Expected Error status when channel is full"
    );
    let error = response
        .error_details
        .as_ref()
        .expect("Should have error details");
    assert_eq!(
        error.code, "ACTOR_ERROR",
        "Expected ACTOR_ERROR code for channel full"
    );
    assert!(
        error.message.contains("full") || error.message.contains("capacity"),
        "Error message should mention channel full: {}",
        error.message
    );
    assert_eq!(error.category, ErrorCategory::Internal);
}

#[tokio::test]
async fn test_actor_handle_channel_full_error() {
    // Test the ActorHandle directly to verify ChannelFull error
    use crate::monitor_collector::ActorError;

    // Create a channel with capacity 1
    let (tx, _rx) = mpsc::channel::<ActorMessage>(1);
    let actor_handle = ActorHandle::new(tx);

    // Fill the channel with a message (we won't consume it)
    actor_handle
        .begin_monitoring()
        .expect("First message should succeed");

    // Now try to send another message - should fail with ChannelFull
    let result = actor_handle.begin_monitoring();

    match result {
        Err(ActorError::ChannelFull { .. }) => {
            // Successfully detected channel full condition
        }
        other => {
            panic!("Expected ChannelFull error, got: {other:?}");
        }
    }
}
