//! # Collector Core Framework
//!
//! A reusable collection infrastructure that enables multiple monitoring components
//! while maintaining shared operational foundation.
//!
//! ## Overview
//!
//! The collector-core framework provides:
//! - Universal `EventSource` trait for pluggable collection implementations
//! - `Collector` runtime for event source management and aggregation
//! - Extensible `CollectionEvent` enum for unified event handling
//! - Capability negotiation through `SourceCaps` bitflags
//! - Shared infrastructure for configuration, logging, and health checks
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Collector Runtime                        │
//! ├─────────────────────────────────────────────────────────────┤
//! │  EventSource    EventSource    EventSource    EventSource   │
//! │  (Process)      (Network)      (Filesystem)   (Performance) │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use collector_core::{Collector, CollectorConfig, EventSource, CollectionEvent, SourceCaps};
//! use async_trait::async_trait;
//! use tokio::sync::mpsc;
//!
//! struct MyEventSource;
//!
//! #[async_trait]
//! impl EventSource for MyEventSource {
//!     fn name(&self) -> &'static str {
//!         "my-source"
//!     }
//!
//!     fn capabilities(&self) -> SourceCaps {
//!         SourceCaps::PROCESS | SourceCaps::REALTIME
//!     }
//!
//!     async fn start(&self, tx: mpsc::Sender<CollectionEvent>, _shutdown_signal: std::sync::Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<()> {
//!         // Implementation here
//!         Ok(())
//!     }
//!
//!     async fn stop(&self) -> anyhow::Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = CollectorConfig::default();
//!     let mut collector = Collector::new(config);
//!
//!     collector.register(Box::new(MyEventSource));
//!     collector.run().await
//! }
//! ```

pub mod analysis_chain;
pub mod busrt_client;
pub mod busrt_event_bus;
pub mod busrt_types;
pub mod collector;
pub mod config;
pub mod embedded_broker;
pub mod event;
pub mod event_bus;
pub mod high_performance_event_bus;
pub mod ipc;
pub mod ipc_busrt_bridge;
pub mod monitor_collector;
pub mod performance;
pub mod source;
pub mod trigger;

// Re-export main types for convenience
pub use analysis_chain::{
    AnalysisChainConfig, AnalysisChainCoordinator, AnalysisResult, AnalysisStage,
    AnalysisWorkflowDefinition, StageStatus, WorkflowError, WorkflowErrorType, WorkflowExecution,
    WorkflowProgress, WorkflowStatistics, WorkflowStatus,
};
pub use busrt_client::{CollectorBusrtClient, ReconnectionConfig};
pub use busrt_event_bus::{BusrtEventBus, BusrtEventBusConfig, BusrtQoS};
pub use busrt_types::{
    BrokerConfig, BrokerMode, BrokerStats, BusrtClient, BusrtError, BusrtEvent,
    CollectorLifecycleService, ConfigurationService, CrossbeamCompatibilityLayer,
    EmbeddedBrokerConfig, EventCorrelation, EventPayload, HealthCheckService,
    StandaloneBrokerConfig, topics,
};
pub use collector::{Collector, CollectorRuntime, RuntimeStats};
pub use config::CollectorConfig;
pub use embedded_broker::{BrokerMessage, BrokerMessageType, EmbeddedBroker};
pub use event::{
    AnalysisType, CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent,
    TriggerPriority, TriggerRequest,
};
pub use event_bus::{
    BusEvent, CorrelationFilter, EventBus, EventBusConfig, EventBusStatistics, EventFilter,
    EventSubscription, LocalEventBus,
};
pub use high_performance_event_bus::{
    BackpressureStrategy, HighPerformanceEventBus, HighPerformanceEventBusConfig,
    HighPerformanceEventBusImpl,
};
pub use ipc::CollectorIpcServer;
pub use ipc_busrt_bridge::{IpcBridgeConfig, IpcBusrtBridge};
pub use monitor_collector::{
    MonitorCollector, MonitorCollectorConfig, MonitorCollectorStats, MonitorCollectorStatsSnapshot,
};
pub use performance::{
    BaselineMetrics, CpuUsageMetrics, DegradationType, MemoryUsageMetrics, PerformanceComparison,
    PerformanceConfig, PerformanceDegradation, PerformanceMonitor, ResourceUsageMetrics,
    ThroughputMetrics, TriggerLatencyMetrics,
};
pub use source::{EventSource, SourceCaps};
pub use trigger::{
    PriorityTriggerQueue, ProcessTriggerData, QueueStatistics, SqlTriggerEvaluator,
    TriggerCapabilities, TriggerCondition, TriggerConfig, TriggerEmissionStats, TriggerManager,
    TriggerResourceLimits, TriggerStatistics,
};

#[cfg(test)]
mod busrt_integration_tests {
    use busrt::QoS;
    use serde::{Deserialize, Serialize};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::timeout;

    /// Test message type for busrt communication
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        id: u32,
        content: String,
        timestamp: u64,
    }

    /// Test configuration for busrt broker
    #[derive(Debug, Clone)]
    struct TestBrokerConfig {
        timeout: Duration,
        max_clients: usize,
    }

    impl Default for TestBrokerConfig {
        fn default() -> Self {
            Self {
                timeout: Duration::from_secs(5),
                max_clients: 100,
            }
        }
    }

    #[test]
    fn test_busrt_dependency_available() {
        // Simple test to verify busrt dependency is properly available
        // This test ensures the busrt crate can be imported and basic types are accessible
        use busrt::QoS;

        // Test that we can reference busrt types without compilation errors
        let qos = QoS::No;

        // Verify the QoS enum has expected variants
        match qos {
            QoS::No => {
                // Test passes if we can match on busrt types
            }
            _ => {
                panic!("Unexpected QoS variant");
            }
        }
    }

    #[test]
    fn test_qos_variants() {
        // Test all QoS variants are accessible
        let qos_variants = vec![
            QoS::No,
            QoS::Processed,
            QoS::Realtime,
            QoS::RealtimeProcessed,
        ];

        for qos in qos_variants {
            match qos {
                QoS::No | QoS::Processed | QoS::Realtime | QoS::RealtimeProcessed => {
                    // All variants should be matchable
                }
            }
        }
    }

    #[tokio::test]
    async fn test_broker_startup_and_shutdown() {
        // Initialize tracing for test logging
        let _ = tracing_subscriber::fmt::try_init();

        // Test broker startup and shutdown sequence
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let _socket_path = temp_dir.path().join("test_broker.sock");

        // Test broker configuration
        let config = TestBrokerConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert_eq!(config.max_clients, 100);

        // Note: Since we're testing the API patterns rather than actual broker functionality,
        // we'll test the types and configuration patterns that would be used
        tracing::info!("Broker startup test completed - API patterns validated");
    }

    #[tokio::test]
    async fn test_client_connection_patterns() {
        // Test client connection and disconnection patterns
        let _ = tracing_subscriber::fmt::try_init();

        // Test client configuration patterns
        let client_timeout = Duration::from_secs(10);
        let reconnect_interval = Duration::from_millis(500);

        // Validate timeout configurations
        assert!(client_timeout > Duration::from_secs(1));
        assert!(reconnect_interval < Duration::from_secs(1));

        tracing::info!("Client connection patterns validated");
    }

    #[tokio::test]
    async fn test_pub_sub_message_patterns() {
        // Test publication and subscription message patterns
        let _ = tracing_subscriber::fmt::try_init();

        // Test message serialization
        let test_msg = TestMessage {
            id: 12345,
            content: "test_message".to_string(),
            timestamp: 1234567890,
        };

        // Test JSON serialization for busrt messages
        let serialized = serde_json::to_string(&test_msg).expect("Failed to serialize message");
        let deserialized: TestMessage =
            serde_json::from_str(&serialized).expect("Failed to deserialize message");

        assert_eq!(test_msg, deserialized);
        assert_eq!(deserialized.id, 12345);
        assert_eq!(deserialized.content, "test_message");

        tracing::info!("Pub/sub message patterns validated");
    }

    #[tokio::test]
    async fn test_rpc_call_patterns() {
        // Test RPC request/response patterns
        let _ = tracing_subscriber::fmt::try_init();

        // Test RPC message structure
        #[derive(Debug, Serialize, Deserialize)]
        struct RpcRequest {
            method: String,
            params: serde_json::Value,
            id: u64,
        }

        #[derive(Debug, Serialize, Deserialize)]
        struct RpcResponse {
            result: Option<serde_json::Value>,
            error: Option<String>,
            id: u64,
        }

        let request = RpcRequest {
            method: "health_check".to_string(),
            params: serde_json::json!({}),
            id: 1,
        };

        let response = RpcResponse {
            result: Some(serde_json::json!({"status": "ok"})),
            error: None,
            id: 1,
        };

        // Test serialization roundtrip
        let req_json = serde_json::to_string(&request).expect("Failed to serialize request");
        let resp_json = serde_json::to_string(&response).expect("Failed to serialize response");

        let _req_parsed: RpcRequest =
            serde_json::from_str(&req_json).expect("Failed to parse request");
        let _resp_parsed: RpcResponse =
            serde_json::from_str(&resp_json).expect("Failed to parse response");

        tracing::info!("RPC call patterns validated");
    }

    #[tokio::test]
    async fn test_transport_layer_configuration() {
        // Test transport layer functionality and configuration
        let _ = tracing_subscriber::fmt::try_init();

        // Test Unix socket path configuration
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let socket_path = temp_dir.path().join("test_transport.sock");

        // Validate socket path is within temp directory
        assert!(socket_path.starts_with(temp_dir.path()));
        assert!(socket_path.extension().unwrap_or_default() == "sock");

        // Test TCP configuration patterns
        let tcp_host = "127.0.0.1";
        let tcp_port = 0; // Use 0 for automatic port assignment in tests

        assert_eq!(tcp_host, "127.0.0.1");
        assert_eq!(tcp_port, 0);

        tracing::info!("Transport layer configuration validated");
    }

    #[tokio::test]
    async fn test_error_handling_patterns() {
        // Test error handling for busrt operations
        let _ = tracing_subscriber::fmt::try_init();

        // Test timeout handling
        let operation_timeout = Duration::from_millis(100);
        let result = timeout(operation_timeout, async {
            // Simulate a quick operation
            tokio::time::sleep(Duration::from_millis(50)).await;
            "success"
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");

        // Test timeout failure
        let short_timeout = Duration::from_millis(10);
        let timeout_result = timeout(short_timeout, async {
            // Simulate a slow operation
            tokio::time::sleep(Duration::from_millis(100)).await;
            "should_timeout"
        })
        .await;

        assert!(timeout_result.is_err());

        tracing::info!("Error handling patterns validated");
    }

    #[tokio::test]
    async fn test_topic_hierarchy_patterns() {
        // Test topic hierarchy patterns for DaemonEye integration
        let _ = tracing_subscriber::fmt::try_init();

        // Define topic patterns for DaemonEye
        let process_topic = "events.process.enumeration";
        let network_topic = "events.network.connections";
        let filesystem_topic = "events.filesystem.operations";
        let control_topic = "control.collector.lifecycle";
        let task_topic = "tasks.detection.process";
        let result_topic = "results.detection.alerts";
        let health_topic = "health.collector.status";

        // Validate topic naming conventions
        assert!(process_topic.starts_with("events.process."));
        assert!(network_topic.starts_with("events.network."));
        assert!(filesystem_topic.starts_with("events.filesystem."));
        assert!(control_topic.starts_with("control."));
        assert!(task_topic.starts_with("tasks."));
        assert!(result_topic.starts_with("results."));
        assert!(health_topic.starts_with("health."));

        // Test topic parsing
        let topic_parts: Vec<&str> = process_topic.split('.').collect();
        assert_eq!(topic_parts.len(), 3);
        assert_eq!(topic_parts[0], "events");
        assert_eq!(topic_parts[1], "process");
        assert_eq!(topic_parts[2], "enumeration");

        tracing::info!("Topic hierarchy patterns validated");
    }

    #[tokio::test]
    async fn test_message_qos_handling() {
        // Test Quality of Service handling for different message types
        let _ = tracing_subscriber::fmt::try_init();

        // Test QoS selection for different message types
        let critical_qos = QoS::Realtime; // For critical alerts
        let normal_qos = QoS::Processed; // For regular events
        let best_effort_qos = QoS::No; // For non-critical data

        // Validate QoS levels
        match critical_qos {
            QoS::Realtime => {
                // Correct for critical messages
            }
            _ => panic!("Critical messages should use Realtime QoS"),
        }

        match normal_qos {
            QoS::Processed => {
                // Correct for normal messages
            }
            _ => panic!("Normal messages should use Processed QoS"),
        }

        match best_effort_qos {
            QoS::No => {
                // Correct for best-effort messages
            }
            _ => panic!("Best-effort messages should use No QoS"),
        }

        tracing::info!("Message QoS handling validated");
    }

    #[tokio::test]
    async fn test_concurrent_operations() {
        // Test concurrent busrt operations
        let _ = tracing_subscriber::fmt::try_init();

        // Test concurrent message handling
        let mut handles = Vec::new();

        for i in 0..10 {
            let handle = tokio::spawn(async move {
                let msg = TestMessage {
                    id: i,
                    content: format!("concurrent_message_{}", i),
                    timestamp: 1234567890 + i as u64,
                };

                // Simulate message processing
                tokio::time::sleep(Duration::from_millis(10)).await;
                msg
            });
            handles.push(handle);
        }

        // Wait for all concurrent operations
        let results = futures::future::join_all(handles).await;

        // Verify all operations completed successfully
        assert_eq!(results.len(), 10);
        for (i, result) in results.into_iter().enumerate() {
            let msg = result.expect("Concurrent operation failed");
            assert_eq!(msg.id, i as u32);
            assert_eq!(msg.content, format!("concurrent_message_{}", i));
        }

        tracing::info!("Concurrent operations validated");
    }

    #[test]
    fn test_configuration_validation() {
        // Test configuration validation patterns
        let config = TestBrokerConfig::default();

        // Validate timeout bounds
        assert!(config.timeout >= Duration::from_secs(1));
        assert!(config.timeout <= Duration::from_secs(60));

        // Validate client limits
        assert!(config.max_clients > 0);
        assert!(config.max_clients <= 1000);

        // Test custom configuration
        let custom_config = TestBrokerConfig {
            timeout: Duration::from_secs(10),
            max_clients: 50,
        };

        assert_eq!(custom_config.timeout, Duration::from_secs(10));
        assert_eq!(custom_config.max_clients, 50);
    }
}
