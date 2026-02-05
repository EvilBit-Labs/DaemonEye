//! Common test utilities for procmond integration tests.
//!
//! This module provides shared helper functions used across multiple test files
//! to reduce code duplication and ensure consistent test setup.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use collector_core::event::ProcessEvent;
use daemoneye_eventbus::rpc::{
    CollectorOperation, RpcCorrelationMetadata, RpcPayload, RpcRequest, RpcStatus,
};
use procmond::event_bus_connector::{EventBusConnector, EventBusConnectorConfig};
use procmond::monitor_collector::{ActorHandle, ActorMessage, CollectorState, HealthCheckData};
use procmond::rpc_service::{RpcServiceConfig, RpcServiceHandler};
use procmond::wal::WalConfig;
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc, oneshot};

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
        executable_hash: Some(format!("large_hash_{pid}")),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

// ============================================================================
// Actor Helpers
// ============================================================================

/// Creates a mock actor handle for testing.
pub fn create_test_actor() -> ActorHandle {
    let (tx, _rx) = mpsc::channel(100);
    ActorHandle::new(tx)
}

/// Creates a mock actor with health responder that responds to health check requests.
pub fn spawn_health_responder() -> (ActorHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<ActorMessage>(100);
    let handle = ActorHandle::new(tx);

    let task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let ActorMessage::HealthCheck { respond_to } = msg {
                let health = HealthCheckData {
                    state: CollectorState::Running,
                    connected_to_agent: true,
                    current_collection_interval: Duration::from_secs(1),
                    events_collected_total: 100,
                    events_published_total: 95,
                    events_buffered: 5,
                    last_collection_time: Some(SystemTime::now()),
                    last_publish_time: Some(SystemTime::now()),
                    error_count: 0,
                    uptime: Duration::from_secs(3600),
                    buffer_level_percent: 10.0,
                };
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
        connected_to_agent: true,
        current_collection_interval: Duration::from_secs(1),
        events_collected_total: 100,
        events_published_total: 95,
        events_buffered: 5,
        last_collection_time: Some(SystemTime::now()),
        last_publish_time: Some(SystemTime::now()),
        error_count: 0,
        uptime: Duration::from_secs(3600),
        buffer_level_percent: 10.0,
    }
}

// ============================================================================
// RPC Helpers
// ============================================================================

/// Creates a test RPC request with the given operation.
pub fn create_test_rpc_request(operation: CollectorOperation) -> RpcRequest {
    RpcRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        operation,
        payload: RpcPayload::Empty,
        deadline: Some(SystemTime::now() + Duration::from_secs(5)),
        correlation: Some(RpcCorrelationMetadata {
            trace_id: Some("test-trace-123".to_string()),
            span_id: Some("test-span-456".to_string()),
            parent_span_id: None,
            baggage: std::collections::HashMap::new(),
        }),
    }
}

/// Creates an RPC service handler for testing.
pub async fn create_test_rpc_handler(
    actor: ActorHandle,
    event_bus: Arc<RwLock<EventBusConnector>>,
) -> RpcServiceHandler {
    let config = RpcServiceConfig::default();
    RpcServiceHandler::new(config, actor, event_bus)
}

// ============================================================================
// EventBus Connector Helpers
// ============================================================================

/// Creates an isolated EventBusConnector with its own temp directory.
/// Returns the connector and temp directory (keep temp_dir alive for test duration).
pub async fn create_isolated_connector() -> (EventBusConnector, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().join("test_wal");

    let config = EventBusConnectorConfig {
        broker_socket_path: temp_dir
            .path()
            .join("nonexistent.sock")
            .to_string_lossy()
            .to_string(),
        wal_config: WalConfig {
            directory: wal_path,
            max_file_size: 1024 * 1024, // 1MB
            max_files: 3,
            sync_writes: false,
        },
        max_buffer_size: 10 * 1024 * 1024, // 10MB
        backpressure_high_water: 0.7,
        backpressure_low_water: 0.5,
        reconnect_interval: Duration::from_millis(100),
        max_reconnect_attempts: 3,
    };

    let connector = EventBusConnector::new(config)
        .await
        .expect("Failed to create connector");

    (connector, temp_dir)
}

/// Creates an EventBusConnector wrapped in Arc<RwLock> for sharing across tasks.
pub async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, TempDir) {
    let (connector, temp_dir) = create_isolated_connector().await;
    (Arc::new(RwLock::new(connector)), temp_dir)
}
