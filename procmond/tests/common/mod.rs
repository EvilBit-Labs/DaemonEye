//! Common test utilities for procmond integration tests.
//!
//! This module provides shared helper functions used across multiple test files
//! to reduce code duplication and ensure consistent test setup.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use collector_core::event::ProcessEvent;
use daemoneye_eventbus::rpc::{CollectorOperation, RpcCorrelationMetadata, RpcPayload, RpcRequest};
use procmond::event_bus_connector::EventBusConnector;
use procmond::monitor_collector::{
    ACTOR_CHANNEL_CAPACITY, ActorHandle, ActorMessage, CollectorState, HealthCheckData,
};
use procmond::rpc_service::{RpcServiceConfig, RpcServiceHandler};
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc};

// ============================================================================
// Process Event Helpers
// ============================================================================

/// Creates a test process event with specified PID.
pub fn create_test_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("test-process-{pid}"),
        executable_path: Some(format!("/usr/bin/test_{pid}")),
        command_line: vec![
            "test".to_string(),
            "--flag".to_string(),
            format!("--pid={pid}"),
        ],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(5.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some(format!("hash_{pid}")),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Creates a large test event to fill buffers quickly.
pub fn create_large_event(pid: u32, arg_count: usize) -> ProcessEvent {
    let command_line: Vec<String> = (0..arg_count)
        .map(|i| format!("--arg{i}=value{}", "x".repeat(100)))
        .collect();

    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("large-process-{pid}"),
        executable_path: Some(format!("/usr/bin/large_{pid}")),
        command_line,
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(50.0),
        memory_usage: Some(100 * 1024 * 1024),
        executable_hash: Some("a".repeat(64)),
        user_id: Some("root".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

// ============================================================================
// Actor Helpers
// ============================================================================

/// Creates a mock actor handle for testing (without receiver).
pub fn create_test_actor() -> ActorHandle {
    let (tx, _rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    ActorHandle::new(tx)
}

/// Creates a mock actor handle with receiver for message inspection.
pub fn create_test_actor_with_receiver() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    (ActorHandle::new(tx), rx)
}

/// Creates a mock actor with health responder that responds to health check requests.
pub fn spawn_health_responder() -> (ActorHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<ActorMessage>(ACTOR_CHANNEL_CAPACITY);
    let handle = ActorHandle::new(tx);

    let task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let ActorMessage::HealthCheck { respond_to } = msg {
                let health = create_mock_health_data();
                respond_to
                    .send(health)
                    .expect("Health response receiver should be waiting");
            }
        }
    });

    (handle, task)
}

/// Creates mock health check data for testing.
pub fn create_mock_health_data() -> HealthCheckData {
    HealthCheckData {
        state: CollectorState::Running,
        collection_interval: Duration::from_secs(30),
        original_interval: Duration::from_secs(30),
        event_bus_connected: true,
        buffer_level_percent: Some(10),
        last_collection: Some(Instant::now()),
        collection_cycles: 5,
        lifecycle_events: 2,
        collection_errors: 0,
        backpressure_events: 0,
    }
}

// ============================================================================
// RPC Helpers
// ============================================================================

/// Creates a test RPC request with the given operation.
pub fn create_test_rpc_request(operation: CollectorOperation) -> RpcRequest {
    RpcRequest {
        request_id: format!(
            "test-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(5),
        correlation_metadata: RpcCorrelationMetadata::new("test-correlation".to_string()),
    }
}

/// Creates a test RPC request for health check with configurable deadline.
pub fn create_health_check_request(deadline_secs: u64) -> RpcRequest {
    RpcRequest {
        request_id: format!(
            "health-test-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        client_id: "test-client".to_string(),
        target: "control.collector.procmond".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(deadline_secs),
        correlation_metadata: RpcCorrelationMetadata::new("health-test".to_string()),
    }
}

/// Creates an RPC service handler for testing.
pub fn create_test_rpc_handler(
    actor: ActorHandle,
    event_bus: Arc<RwLock<EventBusConnector>>,
) -> RpcServiceHandler {
    let config = RpcServiceConfig::default();
    RpcServiceHandler::new(actor, event_bus, config)
}

// ============================================================================
// EventBus Connector Helpers
// ============================================================================

/// Creates an isolated EventBusConnector with its own temp directory.
/// Returns the connector and temp directory (keep temp_dir alive for test duration).
pub async fn create_isolated_connector() -> (EventBusConnector, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");
    (connector, temp_dir)
}

/// Creates an EventBusConnector wrapped in Arc<RwLock> for sharing across tasks.
pub async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, TempDir) {
    let (connector, temp_dir) = create_isolated_connector().await;
    (Arc::new(RwLock::new(connector)), temp_dir)
}
