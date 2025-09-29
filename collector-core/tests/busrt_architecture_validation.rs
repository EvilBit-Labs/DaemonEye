// Busrt message broker architecture validation tests
// These tests validate the design patterns and integration points for busrt

use collector_core::busrt_types::*;
use collector_core::topics;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_topic_hierarchy_structure() {
    // Validate topic naming conventions and hierarchy

    // Event topics should follow events/{domain}/{type} pattern
    assert_eq!(
        topics::EVENTS_PROCESS_ENUMERATION,
        "events/process/enumeration"
    );
    assert_eq!(topics::EVENTS_PROCESS_LIFECYCLE, "events/process/lifecycle");
    assert_eq!(topics::EVENTS_PROCESS_METADATA, "events/process/metadata");
    assert_eq!(topics::EVENTS_PROCESS_HASH, "events/process/hash");
    assert_eq!(topics::EVENTS_PROCESS_ANOMALY, "events/process/anomaly");

    // Network topics (future)
    assert_eq!(
        topics::EVENTS_NETWORK_CONNECTIONS,
        "events/network/connections"
    );
    assert_eq!(topics::EVENTS_NETWORK_TRAFFIC, "events/network/traffic");
    assert_eq!(topics::EVENTS_NETWORK_DNS, "events/network/dns");
    assert_eq!(topics::EVENTS_NETWORK_ANOMALY, "events/network/anomaly");

    // Filesystem topics (future)
    assert_eq!(
        topics::EVENTS_FILESYSTEM_OPERATIONS,
        "events/filesystem/operations"
    );
    assert_eq!(topics::EVENTS_FILESYSTEM_ACCESS, "events/filesystem/access");
    assert_eq!(
        topics::EVENTS_FILESYSTEM_METADATA,
        "events/filesystem/metadata"
    );
    assert_eq!(
        topics::EVENTS_FILESYSTEM_ANOMALY,
        "events/filesystem/anomaly"
    );

    // Performance topics (future)
    assert_eq!(
        topics::EVENTS_PERFORMANCE_METRICS,
        "events/performance/metrics"
    );
    assert_eq!(
        topics::EVENTS_PERFORMANCE_RESOURCES,
        "events/performance/resources"
    );
    assert_eq!(
        topics::EVENTS_PERFORMANCE_THRESHOLDS,
        "events/performance/thresholds"
    );
    assert_eq!(
        topics::EVENTS_PERFORMANCE_ANOMALY,
        "events/performance/anomaly"
    );

    // Control topics should follow control/{component}/{operation} pattern
    assert_eq!(
        topics::CONTROL_COLLECTOR_LIFECYCLE,
        "control/collector/lifecycle"
    );
    assert_eq!(topics::CONTROL_COLLECTOR_CONFIG, "control/collector/config");
    assert_eq!(topics::CONTROL_COLLECTOR_HEALTH, "control/collector/health");
    assert_eq!(
        topics::CONTROL_COLLECTOR_CAPABILITIES,
        "control/collector/capabilities"
    );

    // Agent coordination topics
    assert_eq!(topics::CONTROL_AGENT_TASKS, "control/agent/tasks");
    assert_eq!(
        topics::CONTROL_AGENT_COORDINATION,
        "control/agent/coordination"
    );
    assert_eq!(topics::CONTROL_AGENT_STATUS, "control/agent/status");
    assert_eq!(topics::CONTROL_AGENT_SHUTDOWN, "control/agent/shutdown");

    // Broker administrative topics
    assert_eq!(topics::CONTROL_BROKER_STATS, "control/broker/stats");
    assert_eq!(topics::CONTROL_BROKER_ADMIN, "control/broker/admin");
    assert_eq!(
        topics::CONTROL_BROKER_DIAGNOSTICS,
        "control/broker/diagnostics"
    );
}

#[test]
fn test_busrt_event_serialization() {
    // Test BusrtEvent serialization and deserialization
    let process_record = ProcessRecord {
        pid: 1234,
        name: "test_process".to_string(),
        executable_path: Some("/usr/bin/test".to_string()),
    };

    let process_event_data = ProcessEventData {
        process: process_record,
        collection_cycle_id: "cycle_001".to_string(),
        event_type: ProcessEventType::Discovery,
    };

    let correlation = EventCorrelation {
        workflow_id: "workflow_123".to_string(),
        related_events: vec!["event_001".to_string(), "event_002".to_string()],
        context: {
            let mut ctx = HashMap::new();
            ctx.insert("source".to_string(), "procmond".to_string());
            ctx.insert("priority".to_string(), "high".to_string());
            ctx
        },
    };

    let busrt_event = BusrtEvent {
        event_id: "event_123".to_string(),
        correlation_id: Some("corr_456".to_string()),
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
        source_collector: "procmond_001".to_string(),
        topic: topics::EVENTS_PROCESS_ENUMERATION.to_string(),
        payload: EventPayload::Process(process_event_data),
        correlation: Some(correlation),
    };

    // Test JSON serialization
    let serialized = serde_json::to_string(&busrt_event).expect("Failed to serialize BusrtEvent");
    let deserialized: BusrtEvent =
        serde_json::from_str(&serialized).expect("Failed to deserialize BusrtEvent");

    // Validate deserialized event
    assert_eq!(deserialized.event_id, "event_123");
    assert_eq!(deserialized.correlation_id, Some("corr_456".to_string()));
    assert_eq!(deserialized.source_collector, "procmond_001");
    assert_eq!(deserialized.topic, topics::EVENTS_PROCESS_ENUMERATION);

    // Validate payload
    match deserialized.payload {
        EventPayload::Process(data) => {
            assert_eq!(data.process.pid, 1234);
            assert_eq!(data.process.name, "test_process");
            assert_eq!(data.collection_cycle_id, "cycle_001");
            match data.event_type {
                ProcessEventType::Discovery => {} // Expected
                _ => panic!("Unexpected event type"),
            }
        }
        _ => panic!("Expected Process payload"),
    }

    // Validate correlation
    let corr = deserialized
        .correlation
        .expect("Expected correlation metadata");
    assert_eq!(corr.workflow_id, "workflow_123");
    assert_eq!(corr.related_events.len(), 2);
    assert_eq!(corr.context.get("source"), Some(&"procmond".to_string()));
}

#[test]
fn test_rpc_message_schemas() {
    // Test RPC request/response message serialization

    // Test StartCollectorRequest
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "procmond_001".to_string(),
        config: CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 30000,
            batch_size: 1000,
            timeout_seconds: 60,
            capabilities: vec![
                "process_enumeration".to_string(),
                "hash_computation".to_string(),
            ],
            settings: {
                let mut settings = HashMap::new();
                settings.insert("enable_hashing".to_string(), serde_json::Value::Bool(true));
                settings.insert(
                    "max_processes".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(10000)),
                );
                settings
            },
        },
        environment: {
            let mut env = HashMap::new();
            env.insert("LOG_LEVEL".to_string(), "info".to_string());
            env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
            env
        },
    };

    let serialized =
        serde_json::to_string(&start_request).expect("Failed to serialize StartCollectorRequest");
    let deserialized: StartCollectorRequest =
        serde_json::from_str(&serialized).expect("Failed to deserialize StartCollectorRequest");

    assert_eq!(deserialized.collector_type, "process");
    assert_eq!(deserialized.collector_id, "procmond_001");
    assert_eq!(deserialized.config.scan_interval_ms, 30000);
    assert_eq!(deserialized.config.capabilities.len(), 2);
    assert_eq!(deserialized.environment.len(), 2);

    // Test StartCollectorResponse
    let start_response = StartCollectorResponse {
        success: true,
        collector_id: "procmond_001".to_string(),
        error_message: None,
        status: CollectorStatus::Running,
    };

    let serialized =
        serde_json::to_string(&start_response).expect("Failed to serialize StartCollectorResponse");
    let deserialized: StartCollectorResponse =
        serde_json::from_str(&serialized).expect("Failed to deserialize StartCollectorResponse");

    assert!(deserialized.success);
    assert_eq!(deserialized.collector_id, "procmond_001");
    assert!(deserialized.error_message.is_none());
    match deserialized.status {
        CollectorStatus::Running => {} // Expected
        _ => panic!("Expected Running status"),
    }

    // Test HealthCheckRequest/Response
    let health_request = HealthCheckRequest {
        collector_id: "procmond_001".to_string(),
        include_metrics: true,
    };

    let health_response = HealthCheckResponse {
        status: HealthStatus::Healthy,
        message: "All systems operational".to_string(),
        metrics: {
            let mut metrics = HashMap::new();
            metrics.insert("cpu_usage_percent".to_string(), 2.5);
            metrics.insert("memory_usage_mb".to_string(), 45.2);
            metrics.insert("events_per_second".to_string(), 150.0);
            metrics
        },
        uptime_seconds: 3600,
    };

    let req_serialized =
        serde_json::to_string(&health_request).expect("Failed to serialize HealthCheckRequest");
    let resp_serialized =
        serde_json::to_string(&health_response).expect("Failed to serialize HealthCheckResponse");

    let req_deserialized: HealthCheckRequest =
        serde_json::from_str(&req_serialized).expect("Failed to deserialize HealthCheckRequest");
    let resp_deserialized: HealthCheckResponse =
        serde_json::from_str(&resp_serialized).expect("Failed to deserialize HealthCheckResponse");

    assert_eq!(req_deserialized.collector_id, "procmond_001");
    assert!(req_deserialized.include_metrics);

    match resp_deserialized.status {
        HealthStatus::Healthy => {} // Expected
        _ => panic!("Expected Healthy status"),
    }
    assert_eq!(resp_deserialized.message, "All systems operational");
    assert_eq!(resp_deserialized.metrics.len(), 3);
    assert_eq!(resp_deserialized.uptime_seconds, 3600);
}

#[test]
fn test_broker_configuration_schemas() {
    // Test broker configuration serialization

    // Test embedded broker configuration
    let embedded_config = EmbeddedBrokerConfig {
        max_connections: 64,
        message_buffer_size: 10000,
        transport: TransportConfig {
            transport_type: TransportType::UnixSocket,
            path: Some("/tmp/daemoneye-broker.sock".to_string()),
            address: None,
            port: None,
        },
        security: SecurityConfig {
            authentication_enabled: false,
            tls_enabled: false,
            cert_file: None,
            key_file: None,
            ca_file: None,
        },
    };

    let serialized =
        serde_json::to_string(&embedded_config).expect("Failed to serialize EmbeddedBrokerConfig");
    let deserialized: EmbeddedBrokerConfig =
        serde_json::from_str(&serialized).expect("Failed to deserialize EmbeddedBrokerConfig");

    assert_eq!(deserialized.max_connections, 64);
    assert_eq!(deserialized.message_buffer_size, 10000);
    match deserialized.transport.transport_type {
        TransportType::UnixSocket => {} // Expected
        _ => panic!("Expected UnixSocket transport"),
    }
    assert_eq!(
        deserialized.transport.path,
        Some("/tmp/daemoneye-broker.sock".to_string())
    );
    assert!(!deserialized.security.authentication_enabled);

    // Test standalone broker configuration
    let standalone_config = StandaloneBrokerConfig {
        bind_address: "127.0.0.1:9090".to_string(),
        max_connections: 1000,
        persistence: PersistenceConfig {
            enabled: true,
            storage_path: "/var/lib/daemoneye/broker".to_string(),
            max_message_size: 1048576, // 1MB
            retention_hours: 24,
        },
        clustering: ClusterConfig {
            enabled: false,
            node_id: "broker_001".to_string(),
            cluster_nodes: vec![],
            election_timeout_ms: 5000,
        },
        monitoring: MonitoringConfig {
            metrics_enabled: true,
            metrics_port: 9091,
            health_check_port: 9092,
            log_level: "info".to_string(),
        },
    };

    let serialized = serde_json::to_string(&standalone_config)
        .expect("Failed to serialize StandaloneBrokerConfig");
    let deserialized: StandaloneBrokerConfig =
        serde_json::from_str(&serialized).expect("Failed to deserialize StandaloneBrokerConfig");

    assert_eq!(deserialized.bind_address, "127.0.0.1:9090");
    assert_eq!(deserialized.max_connections, 1000);
    assert!(deserialized.persistence.enabled);
    assert_eq!(deserialized.persistence.retention_hours, 24);
    assert!(!deserialized.clustering.enabled);
    assert!(deserialized.monitoring.metrics_enabled);
    assert_eq!(deserialized.monitoring.log_level, "info");

    // Test complete broker configuration
    let broker_config = BrokerConfig {
        mode: BrokerMode::Embedded,
        embedded: Some(embedded_config),
        standalone: Some(standalone_config),
    };

    let serialized =
        serde_json::to_string(&broker_config).expect("Failed to serialize BrokerConfig");
    let deserialized: BrokerConfig =
        serde_json::from_str(&serialized).expect("Failed to deserialize BrokerConfig");

    match deserialized.mode {
        BrokerMode::Embedded => {} // Expected
        _ => panic!("Expected Embedded mode"),
    }
    assert!(deserialized.embedded.is_some());
    assert!(deserialized.standalone.is_some());
}

#[test]
fn test_crossbeam_compatibility_layer() {
    // Test compatibility layer for crossbeam migration

    // Create mock busrt client for testing
    struct MockBusrtClient;

    #[async_trait::async_trait]
    impl BusrtClient for MockBusrtClient {
        async fn publish(&self, _topic: &str, _message: BusrtEvent) -> Result<(), BusrtError> {
            Ok(())
        }

        async fn subscribe(
            &self,
            _pattern: &str,
        ) -> Result<tokio::sync::mpsc::Receiver<BusrtEvent>, BusrtError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(100);
            Ok(rx)
        }

        async fn unsubscribe(&self, _pattern: &str) -> Result<(), BusrtError> {
            Ok(())
        }

        async fn rpc_call_json(
            &self,
            _service: &str,
            _method: &str,
            _request: serde_json::Value,
        ) -> Result<serde_json::Value, BusrtError> {
            Err(BusrtError::RpcTimeout("Mock implementation".to_string()))
        }

        async fn get_broker_stats(&self) -> Result<BrokerStats, BusrtError> {
            Ok(BrokerStats {
                uptime_seconds: 3600,
                total_connections: 10,
                active_connections: 5,
                messages_published: 1000,
                messages_delivered: 950,
                topics_count: 20,
                memory_usage_bytes: 52428800, // 50MB
            })
        }
    }

    let mock_client = Box::new(MockBusrtClient);
    let compatibility_layer = CrossbeamCompatibilityLayer::new(mock_client);

    // Test topic mapping
    assert_eq!(
        compatibility_layer.map_event_to_topic("process"),
        topics::EVENTS_PROCESS_ENUMERATION
    );
    assert_eq!(
        compatibility_layer.map_event_to_topic("network"),
        topics::EVENTS_NETWORK_CONNECTIONS
    );
    assert_eq!(
        compatibility_layer.map_event_to_topic("filesystem"),
        topics::EVENTS_FILESYSTEM_OPERATIONS
    );
    assert_eq!(
        compatibility_layer.map_event_to_topic("performance"),
        topics::EVENTS_PERFORMANCE_METRICS
    );

    // Test unknown event type mapping
    assert_eq!(
        compatibility_layer.map_event_to_topic("unknown"),
        "events/unknown/default"
    );
}

#[test]
fn test_event_payload_variants() {
    // Test all event payload variants

    // Process event
    let process_payload = EventPayload::Process(ProcessEventData {
        process: ProcessRecord {
            pid: 1234,
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
        },
        collection_cycle_id: "cycle_001".to_string(),
        event_type: ProcessEventType::Discovery,
    });

    match process_payload {
        EventPayload::Process(data) => {
            assert_eq!(data.process.pid, 1234);
            match data.event_type {
                ProcessEventType::Discovery => {} // Expected
                _ => panic!("Expected Discovery event type"),
            }
        }
        _ => panic!("Expected Process payload"),
    }

    // Network event (future)
    let network_payload = EventPayload::Network(NetworkEventData {
        connection_id: "conn_001".to_string(),
        event_type: NetworkEventType::ConnectionEstablished,
    });

    match network_payload {
        EventPayload::Network(data) => {
            assert_eq!(data.connection_id, "conn_001");
            match data.event_type {
                NetworkEventType::ConnectionEstablished => {} // Expected
                _ => panic!("Expected ConnectionEstablished event type"),
            }
        }
        _ => panic!("Expected Network payload"),
    }

    // Filesystem event (future)
    let filesystem_payload = EventPayload::Filesystem(FilesystemEventData {
        file_path: "/etc/passwd".to_string(),
        event_type: FilesystemEventType::FileModified,
    });

    match filesystem_payload {
        EventPayload::Filesystem(data) => {
            assert_eq!(data.file_path, "/etc/passwd");
            match data.event_type {
                FilesystemEventType::FileModified => {} // Expected
                _ => panic!("Expected FileModified event type"),
            }
        }
        _ => panic!("Expected Filesystem payload"),
    }

    // Performance event (future)
    let performance_payload = EventPayload::Performance(PerformanceEventData {
        metric_name: "cpu_usage".to_string(),
        event_type: PerformanceEventType::ThresholdViolation,
    });

    match performance_payload {
        EventPayload::Performance(data) => {
            assert_eq!(data.metric_name, "cpu_usage");
            match data.event_type {
                PerformanceEventType::ThresholdViolation => {} // Expected
                _ => panic!("Expected ThresholdViolation event type"),
            }
        }
        _ => panic!("Expected Performance payload"),
    }
}

#[test]
fn test_error_handling_patterns() {
    // Test BusrtError variants

    let connection_error = BusrtError::ConnectionFailed("Failed to connect to broker".to_string());
    assert_eq!(
        connection_error.to_string(),
        "Connection failed: Failed to connect to broker"
    );

    let serialization_error = BusrtError::SerializationFailed("Invalid JSON".to_string());
    assert_eq!(
        serialization_error.to_string(),
        "Message serialization failed: Invalid JSON"
    );

    let timeout_error = BusrtError::RpcTimeout("Health check timed out".to_string());
    assert_eq!(
        timeout_error.to_string(),
        "RPC timeout: Health check timed out"
    );

    let topic_error = BusrtError::TopicNotFound("events/invalid/topic".to_string());
    assert_eq!(
        topic_error.to_string(),
        "Topic not found: events/invalid/topic"
    );

    let permission_error =
        BusrtError::PermissionDenied("Access denied to control topics".to_string());
    assert_eq!(
        permission_error.to_string(),
        "Permission denied: Access denied to control topics"
    );

    let broker_error = BusrtError::BrokerUnavailable("Broker is shutting down".to_string());
    assert_eq!(
        broker_error.to_string(),
        "Broker unavailable: Broker is shutting down"
    );
}

#[test]
fn test_configuration_validation_errors() {
    // Test configuration validation error handling

    let validation_error = ConfigValidationError {
        field: "scan_interval_ms".to_string(),
        message: "Scan interval must be between 1000 and 300000 milliseconds".to_string(),
        error_code: "INVALID_RANGE".to_string(),
    };

    let serialized = serde_json::to_string(&validation_error)
        .expect("Failed to serialize ConfigValidationError");
    let deserialized: ConfigValidationError =
        serde_json::from_str(&serialized).expect("Failed to deserialize ConfigValidationError");

    assert_eq!(deserialized.field, "scan_interval_ms");
    assert!(deserialized.message.contains("between 1000 and 300000"));
    assert_eq!(deserialized.error_code, "INVALID_RANGE");
}

#[tokio::test]
async fn test_broker_stats_monitoring() {
    // Test broker statistics for monitoring

    let broker_stats = BrokerStats {
        uptime_seconds: 86400, // 24 hours
        total_connections: 100,
        active_connections: 25,
        messages_published: 50000,
        messages_delivered: 49500,
        topics_count: 50,
        memory_usage_bytes: 104857600, // 100MB
    };

    // Test serialization
    let serialized = serde_json::to_string(&broker_stats).expect("Failed to serialize BrokerStats");
    let deserialized: BrokerStats =
        serde_json::from_str(&serialized).expect("Failed to deserialize BrokerStats");

    assert_eq!(deserialized.uptime_seconds, 86400);
    assert_eq!(deserialized.total_connections, 100);
    assert_eq!(deserialized.active_connections, 25);
    assert_eq!(deserialized.messages_published, 50000);
    assert_eq!(deserialized.messages_delivered, 49500);
    assert_eq!(deserialized.topics_count, 50);
    assert_eq!(deserialized.memory_usage_bytes, 104857600);

    // Test delivery rate calculation
    let delivery_rate =
        deserialized.messages_delivered as f64 / deserialized.messages_published as f64;
    assert!(delivery_rate > 0.98); // 98% delivery rate (49500/50000 = 0.99)

    // Test memory usage in MB
    let memory_mb = deserialized.memory_usage_bytes / 1024 / 1024;
    assert_eq!(memory_mb, 100);
}

#[test]
fn test_transport_configuration_variants() {
    // Test different transport configurations

    // Unix socket transport
    let unix_transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/daemoneye.sock".to_string()),
        address: None,
        port: None,
    };

    match unix_transport.transport_type {
        TransportType::UnixSocket => {
            assert!(unix_transport.path.is_some());
            assert!(unix_transport.address.is_none());
            assert!(unix_transport.port.is_none());
        }
        _ => panic!("Expected UnixSocket transport"),
    }

    // Named pipe transport (Windows)
    let named_pipe_transport = TransportConfig {
        transport_type: TransportType::NamedPipe,
        path: Some(r"\\.\pipe\daemoneye".to_string()),
        address: None,
        port: None,
    };

    match named_pipe_transport.transport_type {
        TransportType::NamedPipe => {
            assert!(named_pipe_transport.path.is_some());
            assert!(named_pipe_transport.path.as_ref().unwrap().contains("pipe"));
        }
        _ => panic!("Expected NamedPipe transport"),
    }

    // TCP transport
    let tcp_transport = TransportConfig {
        transport_type: TransportType::Tcp,
        path: None,
        address: Some("127.0.0.1".to_string()),
        port: Some(9090),
    };

    match tcp_transport.transport_type {
        TransportType::Tcp => {
            assert!(tcp_transport.path.is_none());
            assert_eq!(tcp_transport.address, Some("127.0.0.1".to_string()));
            assert_eq!(tcp_transport.port, Some(9090));
        }
        _ => panic!("Expected Tcp transport"),
    }

    // In-process transport
    let inprocess_transport = TransportConfig {
        transport_type: TransportType::InProcess,
        path: None,
        address: None,
        port: None,
    };

    match inprocess_transport.transport_type {
        TransportType::InProcess => {
            assert!(inprocess_transport.path.is_none());
            assert!(inprocess_transport.address.is_none());
            assert!(inprocess_transport.port.is_none());
        }
        _ => panic!("Expected InProcess transport"),
    }
}
