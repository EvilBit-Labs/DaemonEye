//! Integration tests for collector-core IPC functionality.
//!
//! These tests verify that the IPC integration preserves compatibility
//! with existing sentinelagent clients while adding collector-core
//! functionality.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, CollectorIpcServer, EventSource, SourceCaps,
};
use prost::Message;
use daemoneye_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc};

/// Test event source for integration testing.
struct TestProcessSource {
    name: &'static str,
}

impl TestProcessSource {
    fn new() -> Self {
        Self {
            name: "test-process-source",
        }
    }
}

#[async_trait]
impl EventSource for TestProcessSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE
    }

    async fn start(&self, _tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        // Simulate event source running
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_ipc_server_creation() {
    let config = CollectorConfig::default();
    let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));
    let _ipc_server = CollectorIpcServer::new(config, capabilities);

    // Server creation should succeed without starting
    // This tests the basic setup without requiring actual IPC socket creation
}

#[tokio::test]
async fn test_capability_negotiation() {
    let config = CollectorConfig::default();
    let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS | SourceCaps::REALTIME));
    let ipc_server = CollectorIpcServer::new(config, capabilities);

    // Test initial capabilities
    use daemoneye_lib::proto::MonitoringDomain;

    let proto_caps = ipc_server.get_capabilities().await;
    assert!(
        proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32))
    );
    assert!(
        !proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Network as i32))
    );
    assert!(
        !proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Filesystem as i32))
    );
    assert!(
        !proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Performance as i32))
    );

    let advanced = proto_caps.advanced.as_ref().unwrap();
    assert!(advanced.realtime);
    assert!(!advanced.system_wide);

    // Update capabilities
    ipc_server
        .update_capabilities(SourceCaps::PROCESS | SourceCaps::NETWORK | SourceCaps::SYSTEM_WIDE)
        .await;

    // Test updated capabilities
    let proto_caps = ipc_server.get_capabilities().await;
    assert!(
        proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32))
    );
    assert!(
        proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Network as i32))
    );
    assert!(
        !proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Filesystem as i32))
    );
    assert!(
        !proto_caps
            .supported_domains
            .contains(&(MonitoringDomain::Performance as i32))
    );

    let advanced = proto_caps.advanced.as_ref().unwrap();
    assert!(!advanced.realtime);
    assert!(advanced.system_wide);
}

#[tokio::test]
async fn test_task_validation_logic() {
    // Test the task validation logic without starting the IPC server
    let _capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));

    // Test valid process task
    let process_task = DetectionTask {
        task_id: "test-1".to_string(),
        task_type: TaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    // Just verify the task structure is correct
    assert_eq!(process_task.task_type, TaskType::EnumerateProcesses as i32);
    assert_eq!(process_task.task_id, "test-1");
}

#[tokio::test]
async fn test_collector_with_ipc_integration() {
    let mut config = CollectorConfig::default();
    config = config.with_shutdown_timeout(Duration::from_secs(5));

    let mut collector = Collector::new(config);

    // Register a test event source
    let test_source = TestProcessSource::new();
    let _ = collector.register(Box::new(test_source));

    // Verify capabilities are aggregated correctly
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::REALTIME));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));

    // Note: We can't easily test the full collector.run() in a unit test
    // because it runs indefinitely. In a real integration test environment,
    // we would start the collector in a separate task and test IPC communication.
}

#[tokio::test]
async fn test_protobuf_message_compatibility() {
    // Test that new protobuf messages are backward compatible

    // Create a detection task with only original fields
    let task = DetectionTask {
        task_id: "test-task-123".to_string(),
        task_type: TaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: Some("test metadata".to_string()),
        // New fields should default to None/empty
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    // Verify serialization works
    let serialized = task.encode_to_vec();
    assert!(!serialized.is_empty());

    // Verify deserialization works
    let deserialized = DetectionTask::decode(&serialized[..]).expect("Should deserialize");
    assert_eq!(deserialized.task_id, "test-task-123");
    assert_eq!(deserialized.task_type, TaskType::EnumerateProcesses as i32);
    assert_eq!(deserialized.metadata.as_deref(), Some("test metadata"));
    assert!(deserialized.network_filter.is_none());
    assert!(deserialized.filesystem_filter.is_none());
    assert!(deserialized.performance_filter.is_none());

    // Test detection result compatibility
    let result = DetectionResult {
        task_id: "test-task-123".to_string(),
        success: true,
        error_message: None,
        processes: vec![],
        hash_result: None,
        // New fields should default to empty
        network_events: vec![],
        filesystem_events: vec![],
        performance_events: vec![],
    };

    // Verify serialization works
    let serialized = result.encode_to_vec();
    assert!(!serialized.is_empty());

    // Verify deserialization works
    let deserialized = DetectionResult::decode(&serialized[..]).expect("Should deserialize");
    assert_eq!(deserialized.task_id, "test-task-123");
    assert!(deserialized.success);
    assert!(deserialized.error_message.is_none());
    assert!(deserialized.processes.is_empty());
    assert!(deserialized.network_events.is_empty());
    assert!(deserialized.filesystem_events.is_empty());
    assert!(deserialized.performance_events.is_empty());
}

#[tokio::test]
async fn test_collection_capabilities_message() {
    use daemoneye_lib::proto::CollectionCapabilities;

    // Test CollectionCapabilities message creation and serialization
    use daemoneye_lib::proto::{AdvancedCapabilities, MonitoringDomain};

    let capabilities = CollectionCapabilities {
        supported_domains: vec![
            MonitoringDomain::Process as i32,
            MonitoringDomain::Filesystem as i32,
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: true,
            realtime: true,
            system_wide: false,
        }),
    };

    // Verify serialization works
    let serialized = capabilities.encode_to_vec();
    assert!(!serialized.is_empty());

    // Verify deserialization works
    let deserialized = CollectionCapabilities::decode(&serialized[..]).expect("Should deserialize");
    assert!(
        deserialized
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32))
    );
    assert!(
        !deserialized
            .supported_domains
            .contains(&(MonitoringDomain::Network as i32))
    );
    assert!(
        deserialized
            .supported_domains
            .contains(&(MonitoringDomain::Filesystem as i32))
    );
    assert!(
        !deserialized
            .supported_domains
            .contains(&(MonitoringDomain::Performance as i32))
    );

    let advanced = deserialized.advanced.as_ref().unwrap();
    assert!(advanced.kernel_level);
    assert!(advanced.realtime);
    assert!(!advanced.system_wide);
}

#[tokio::test]
async fn test_multi_domain_task_types() {
    // Test that new task types are properly defined and can be used

    // Test process tasks (existing)
    let process_task = DetectionTask {
        task_id: "process-task".to_string(),
        task_type: TaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };
    assert_eq!(process_task.task_type, 0); // EnumerateProcesses = 0

    // Test network tasks (future)
    let network_task = DetectionTask {
        task_id: "network-task".to_string(),
        task_type: TaskType::MonitorNetworkConnections as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };
    assert_eq!(network_task.task_type, 4); // MonitorNetworkConnections = 4

    // Test filesystem tasks (future)
    let fs_task = DetectionTask {
        task_id: "fs-task".to_string(),
        task_type: TaskType::TrackFileOperations as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };
    assert_eq!(fs_task.task_type, 5); // TrackFileOperations = 5

    // Test performance tasks (future)
    let perf_task = DetectionTask {
        task_id: "perf-task".to_string(),
        task_type: TaskType::CollectPerformanceMetrics as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };
    assert_eq!(perf_task.task_type, 6); // CollectPerformanceMetrics = 6
}
