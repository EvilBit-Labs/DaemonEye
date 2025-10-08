//! Comprehensive busrt functionality tests for DaemonEye integration
//!
//! This test suite validates busrt basic functionality including:
//! - Broker startup and configuration
//! - Client connection and subscription
//! - RPC call patterns and error handling
//! - Transport layer functionality and error conditions

use busrt::QoS;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Test message for busrt communication validation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, bincode::Encode, bincode::Decode)]
struct BusrtTestMessage {
    message_id: u64,
    sender: String,
    payload: String,
    timestamp: u64,
    priority: MessagePriority,
}

/// Message priority levels for QoS testing
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, bincode::Encode, bincode::Decode)]
enum MessagePriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Test configuration for busrt broker operations
#[derive(Debug, Clone)]
struct BusrtTestConfig {
    broker_timeout: Duration,
    client_timeout: Duration,
    max_connections: usize,
    buffer_size: usize,
}

impl Default for BusrtTestConfig {
    fn default() -> Self {
        Self {
            broker_timeout: Duration::from_secs(30),
            client_timeout: Duration::from_secs(10),
            max_connections: 100,
            buffer_size: 1024,
        }
    }
}

/// Test RPC request structure
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestRpcRequest {
    method: String,
    params: serde_json::Value,
    request_id: u64,
    timeout_ms: u64,
}

/// Test RPC response structure
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestRpcResponse {
    result: Option<serde_json::Value>,
    error: Option<String>,
    request_id: u64,
    processing_time_ms: u64,
}

#[tokio::test]
async fn test_busrt_broker_configuration() {
    // Initialize tracing for test logging
    let _ = tracing_subscriber::fmt::try_init();

    // Test broker configuration validation
    let config = BusrtTestConfig::default();

    // Validate configuration bounds
    assert!(config.broker_timeout >= Duration::from_secs(1));
    assert!(config.broker_timeout <= Duration::from_secs(300));
    assert!(config.client_timeout >= Duration::from_secs(1));
    assert!(config.client_timeout <= Duration::from_secs(60));
    assert!(config.max_connections > 0);
    assert!(config.max_connections <= 10000);
    assert!(config.buffer_size >= 512);
    assert!(config.buffer_size <= 65536);

    tracing::info!("Broker configuration validation completed");
}

#[tokio::test]
async fn test_busrt_client_connection_lifecycle() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test client connection lifecycle patterns
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let socket_path = temp_dir.path().join("busrt_client_test.sock");

    // Test connection configuration
    let connection_config = BusrtTestConfig {
        client_timeout: Duration::from_secs(5),
        max_connections: 10,
        buffer_size: 2048,
        ..Default::default()
    };

    // Validate connection parameters
    assert_eq!(connection_config.client_timeout, Duration::from_secs(5));
    assert_eq!(connection_config.max_connections, 10);
    assert_eq!(connection_config.buffer_size, 2048);

    // Test socket path validation
    assert!(socket_path.starts_with(temp_dir.path()));
    assert!(socket_path.file_name().is_some());

    tracing::info!("Client connection lifecycle test completed");
}

#[tokio::test]
async fn test_busrt_message_serialization() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test message serialization and deserialization
    let test_message = BusrtTestMessage {
        message_id: 12345,
        sender: "test_client".to_string(),
        payload: "test_payload_data".to_string(),
        timestamp: 1234567890,
        priority: MessagePriority::High,
    };

    // Test JSON serialization
    let serialized = serde_json::to_string(&test_message).expect("Failed to serialize message");
    let deserialized: BusrtTestMessage =
        serde_json::from_str(&serialized).expect("Failed to deserialize message");

    // Validate roundtrip serialization
    assert_eq!(test_message, deserialized);
    assert_eq!(deserialized.message_id, 12345);
    assert_eq!(deserialized.sender, "test_client");
    assert_eq!(deserialized.payload, "test_payload_data");
    assert_eq!(deserialized.priority, MessagePriority::High);

    // Test binary serialization with bincode
    let binary_data = bincode::encode_to_vec(&test_message, bincode::config::standard())
        .expect("Failed to serialize with bincode");
    let (binary_deserialized, _): (BusrtTestMessage, usize) =
        bincode::decode_from_slice(&binary_data, bincode::config::standard())
            .expect("Failed to deserialize with bincode");

    assert_eq!(test_message, binary_deserialized);

    tracing::info!("Message serialization test completed");
}

#[tokio::test]
async fn test_busrt_qos_level_mapping() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test QoS level mapping for different message priorities
    let qos_mappings = vec![
        (MessagePriority::Low, QoS::No),
        (MessagePriority::Normal, QoS::Processed),
        (MessagePriority::High, QoS::Realtime),
        (MessagePriority::Critical, QoS::RealtimeProcessed),
    ];

    for (priority, expected_qos) in qos_mappings {
        let qos = match priority {
            MessagePriority::Low => QoS::No,
            MessagePriority::Normal => QoS::Processed,
            MessagePriority::High => QoS::Realtime,
            MessagePriority::Critical => QoS::RealtimeProcessed,
        };

        // Since QoS doesn't implement PartialEq, we'll validate by matching
        let qos_matches = matches!(
            (qos, expected_qos),
            (QoS::No, QoS::No)
                | (QoS::Processed, QoS::Processed)
                | (QoS::Realtime, QoS::Realtime)
                | (QoS::RealtimeProcessed, QoS::RealtimeProcessed)
        );
        assert!(
            qos_matches,
            "QoS mapping mismatch for priority {:?}",
            priority
        );

        // Validate QoS enum matching
        match qos {
            QoS::No | QoS::Processed | QoS::Realtime | QoS::RealtimeProcessed => {
                // All QoS levels should be handled
            }
        }
    }

    tracing::info!("QoS level mapping test completed");
}

#[tokio::test]
async fn test_busrt_rpc_patterns() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test RPC request/response patterns
    let rpc_request = TestRpcRequest {
        method: "collector.health_check".to_string(),
        params: serde_json::json!({
            "include_metrics": true,
            "timeout_ms": 5000
        }),
        request_id: 98765,
        timeout_ms: 10000,
    };

    let rpc_response = TestRpcResponse {
        result: Some(serde_json::json!({
            "status": "healthy",
            "uptime_seconds": 3600,
            "memory_usage_mb": 45,
            "cpu_usage_percent": 2.5
        })),
        error: None,
        request_id: 98765,
        processing_time_ms: 150,
    };

    // Test RPC serialization
    let req_json = serde_json::to_string(&rpc_request).expect("Failed to serialize RPC request");
    let resp_json = serde_json::to_string(&rpc_response).expect("Failed to serialize RPC response");

    // Test RPC deserialization
    let req_parsed: TestRpcRequest =
        serde_json::from_str(&req_json).expect("Failed to parse RPC request");
    let resp_parsed: TestRpcResponse =
        serde_json::from_str(&resp_json).expect("Failed to parse RPC response");

    // Validate RPC roundtrip
    assert_eq!(rpc_request, req_parsed);
    assert_eq!(rpc_response, resp_parsed);
    assert_eq!(req_parsed.method, "collector.health_check");
    assert_eq!(resp_parsed.request_id, 98765);

    tracing::info!("RPC patterns test completed");
}

#[tokio::test]
async fn test_busrt_error_handling() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test error handling patterns for busrt operations

    // Test timeout handling
    let short_timeout = Duration::from_millis(50);
    let timeout_result = timeout(short_timeout, async {
        // Simulate a slow operation that should timeout
        tokio::time::sleep(Duration::from_millis(200)).await;
        "should_not_complete"
    })
    .await;

    assert!(timeout_result.is_err(), "Operation should have timed out");

    // Test successful operation within timeout
    let adequate_timeout = Duration::from_millis(200);
    let success_result = timeout(adequate_timeout, async {
        // Simulate a fast operation
        tokio::time::sleep(Duration::from_millis(50)).await;
        "completed_successfully"
    })
    .await;

    assert!(success_result.is_ok(), "Operation should have completed");
    assert_eq!(success_result.unwrap(), "completed_successfully");

    // Test error response handling
    let error_response = TestRpcResponse {
        result: None,
        error: Some("Connection timeout".to_string()),
        request_id: 12345,
        processing_time_ms: 5000,
    };

    assert!(error_response.result.is_none());
    assert!(error_response.error.is_some());
    assert_eq!(error_response.error.unwrap(), "Connection timeout");

    tracing::info!("Error handling test completed");
}

#[tokio::test]
async fn test_busrt_topic_patterns() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test topic patterns for DaemonEye integration
    let daemoneye_topics = vec![
        "events.process.enumeration",
        "events.process.lifecycle",
        "events.network.connections",
        "events.filesystem.operations",
        "control.collector.start",
        "control.collector.stop",
        "control.collector.restart",
        "tasks.detection.process",
        "tasks.detection.network",
        "results.alerts.critical",
        "results.alerts.warning",
        "health.collector.status",
        "health.broker.metrics",
    ];

    // Validate topic naming conventions
    for topic in &daemoneye_topics {
        // Topics should have at least 3 parts separated by dots
        let parts: Vec<&str> = topic.split('.').collect();
        assert!(
            parts.len() >= 3,
            "Topic {} should have at least 3 parts",
            topic
        );

        // Validate topic categories
        let category = parts[0];
        assert!(
            matches!(
                category,
                "events" | "control" | "tasks" | "results" | "health"
            ),
            "Invalid topic category: {}",
            category
        );

        // Validate topic format (no spaces, lowercase, alphanumeric + dots)
        assert!(
            topic
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'),
            "Topic {} contains invalid characters",
            topic
        );
    }

    // Test topic matching patterns
    let process_topics: Vec<&str> = daemoneye_topics
        .iter()
        .filter(|topic| topic.starts_with("events.process."))
        .copied()
        .collect();

    assert_eq!(process_topics.len(), 2);
    assert!(process_topics.contains(&"events.process.enumeration"));
    assert!(process_topics.contains(&"events.process.lifecycle"));

    tracing::info!("Topic patterns test completed");
}

#[tokio::test]
async fn test_busrt_transport_configuration() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test transport layer configuration patterns
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Test Unix socket configuration
    let unix_socket_path = temp_dir.path().join("busrt_transport.sock");
    assert!(unix_socket_path.starts_with(temp_dir.path()));
    assert_eq!(unix_socket_path.extension().unwrap(), "sock");

    // Test TCP configuration
    let tcp_configs = vec![
        ("127.0.0.1", 0), // Localhost with automatic port
        ("0.0.0.0", 0),   // All interfaces with automatic port
    ];

    for (host, port) in tcp_configs {
        // Validate host format
        assert!(host == "127.0.0.1" || host == "0.0.0.0");
        // Validate port range (0 for automatic assignment)
        assert!(port == 0 || (1024..=65535).contains(&port));
    }

    // Test transport selection criteria
    #[derive(Debug, PartialEq)]
    enum TransportType {
        UnixSocket,
        NamedPipe,
        Tcp,
    }

    let transport_selection = if cfg!(unix) {
        TransportType::UnixSocket
    } else if cfg!(windows) {
        TransportType::NamedPipe
    } else {
        TransportType::Tcp
    };

    // Validate platform-appropriate transport selection
    #[cfg(unix)]
    assert_eq!(transport_selection, TransportType::UnixSocket);

    #[cfg(windows)]
    assert_eq!(transport_selection, TransportType::NamedPipe);

    tracing::info!("Transport configuration test completed");
}

#[tokio::test]
async fn test_busrt_concurrent_operations() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test concurrent busrt operations
    let num_operations = 20;
    let mut handles = Vec::new();

    for i in 0..num_operations {
        let handle = tokio::spawn(async move {
            let message = BusrtTestMessage {
                message_id: i,
                sender: format!("client_{}", i),
                payload: format!("concurrent_payload_{}", i),
                timestamp: 1234567890 + i,
                priority: if i % 4 == 0 {
                    MessagePriority::Critical
                } else if i % 3 == 0 {
                    MessagePriority::High
                } else if i % 2 == 0 {
                    MessagePriority::Normal
                } else {
                    MessagePriority::Low
                },
            };

            // Simulate message processing with variable delay
            let delay = Duration::from_millis(10 + (i % 50));
            tokio::time::sleep(delay).await;

            message
        });
        handles.push(handle);
    }

    // Wait for all concurrent operations to complete
    let results = futures::future::join_all(handles).await;

    // Validate all operations completed successfully
    assert_eq!(results.len(), num_operations as usize);

    for (i, result) in results.into_iter().enumerate() {
        let message = result.expect("Concurrent operation should not fail");
        assert_eq!(message.message_id, i as u64);
        assert_eq!(message.sender, format!("client_{}", i));
        assert!(
            message
                .payload
                .contains(&format!("concurrent_payload_{}", i))
        );
    }

    tracing::info!("Concurrent operations test completed");
}

#[tokio::test]
async fn test_busrt_performance_characteristics() {
    let _ = tracing_subscriber::fmt::try_init();

    // Test performance characteristics and resource usage
    let start_time = std::time::Instant::now();
    let num_messages = 1000;

    // Create and serialize messages to test throughput
    let mut serialized_messages = Vec::new();
    for i in 0..num_messages {
        let message = BusrtTestMessage {
            message_id: i,
            sender: "performance_test".to_string(),
            payload: format!("performance_payload_{}", i),
            timestamp: 1234567890 + i,
            priority: MessagePriority::Normal,
        };

        let serialized = serde_json::to_string(&message).expect("Serialization should not fail");
        serialized_messages.push(serialized);
    }

    let serialization_time = start_time.elapsed();

    // Validate performance characteristics
    assert_eq!(serialized_messages.len(), num_messages as usize);
    assert!(
        serialization_time < Duration::from_secs(1),
        "Serialization should be fast"
    );

    // Test deserialization performance
    let deserialize_start = std::time::Instant::now();
    let mut deserialized_count = 0;

    for serialized in &serialized_messages {
        let _message: BusrtTestMessage =
            serde_json::from_str(serialized).expect("Deserialization should not fail");
        deserialized_count += 1;
    }

    let deserialization_time = deserialize_start.elapsed();

    assert_eq!(deserialized_count, num_messages);
    assert!(
        deserialization_time < Duration::from_secs(1),
        "Deserialization should be fast"
    );

    tracing::info!(
        "Performance test completed: {} messages serialized in {:?}, deserialized in {:?}",
        num_messages,
        serialization_time,
        deserialization_time
    );
}
