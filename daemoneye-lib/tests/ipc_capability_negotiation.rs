//! Integration tests for IPC capability negotiation workflows.
//!
//! These tests verify that the IPC client can properly negotiate capabilities
//! with collector-core components and route tasks based on those capabilities.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::shadow_reuse,
    clippy::as_conversions
)]

use daemoneye_lib::{
    ipc::{
        IpcConfig, TransportType,
        client::{CollectorEndpoint, LoadBalancingStrategy, ResilientIpcClient},
    },
    proto::{
        AdvancedCapabilities, CollectionCapabilities, DetectionTask, MonitoringDomain, TaskType,
    },
};
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn test_capability_negotiation_basic() {
    // Create endpoint path for testing
    let endpoint_path = format!("/tmp/test-collector-{}.sock", Uuid::new_v4());

    // Create IPC client configuration
    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: endpoint_path.clone(),
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 10,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    // Create endpoint
    let endpoint = CollectorEndpoint::new("test-collector".to_owned(), endpoint_path, 1);

    // Create client with the endpoint
    let client = ResilientIpcClient::new_with_endpoints(
        &config,
        vec![endpoint],
        LoadBalancingStrategy::Priority,
    );

    // Test capability negotiation (will fail since no real server is running)
    let result = client.negotiate_capabilities("test-collector").await;

    // We expect this to fail since there's no real server
    assert!(result.is_err());

    // Verify the error is a connection-related error
    match result {
        Err(
            daemoneye_lib::ipc::codec::IpcError::ServerNotFound { .. }
            | daemoneye_lib::ipc::codec::IpcError::ConnectionRefused { .. }
            | daemoneye_lib::ipc::codec::IpcError::ConnectionTimeout { .. },
        ) => {
            // Expected error types when no server is running
        }
        Err(e) => panic!("Unexpected error type: {e:?}"),
        Ok(_) => panic!("Expected capability negotiation to fail without server"),
    }
}

#[tokio::test]
async fn test_endpoint_capability_support() {
    // Create an endpoint with process capabilities
    let mut endpoint = CollectorEndpoint::new(
        "process-collector".to_owned(),
        "/tmp/process-collector.sock".to_owned(),
        1,
    );

    // Initially, endpoint should not support any tasks (no capabilities)
    assert!(!endpoint.supports_task_type(TaskType::EnumerateProcesses));
    assert!(!endpoint.supports_task_type(TaskType::MonitorNetworkConnections));

    // Update with process capabilities
    let process_capabilities = CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Process)],
        advanced: Some(AdvancedCapabilities {
            kernel_level: false,
            realtime: true,
            system_wide: true,
        }),
    };

    endpoint.update_capabilities(process_capabilities);

    // Now it should support process tasks but not network tasks
    assert!(endpoint.supports_task_type(TaskType::EnumerateProcesses));
    assert!(endpoint.supports_task_type(TaskType::CheckProcessHash));
    assert!(endpoint.supports_task_type(TaskType::MonitorProcessTree));
    assert!(endpoint.supports_task_type(TaskType::VerifyExecutable));
    assert!(!endpoint.supports_task_type(TaskType::MonitorNetworkConnections));
    assert!(!endpoint.supports_task_type(TaskType::TrackFileOperations));
    assert!(!endpoint.supports_task_type(TaskType::CollectPerformanceMetrics));

    // Test advanced capabilities
    assert!(!endpoint.supports_advanced_capability("kernel_level"));
    assert!(endpoint.supports_advanced_capability("realtime"));
    assert!(endpoint.supports_advanced_capability("system_wide"));
    assert!(!endpoint.supports_advanced_capability("unknown_capability"));
}

#[tokio::test]
async fn test_multi_domain_endpoint_selection() {
    // Create endpoints with different capabilities
    let mut process_endpoint = CollectorEndpoint::new(
        "process-collector".to_owned(),
        "/tmp/process-collector.sock".to_owned(),
        1,
    );

    let mut network_endpoint = CollectorEndpoint::new(
        "network-collector".to_owned(),
        "/tmp/network-collector.sock".to_owned(),
        2,
    );

    let mut multi_endpoint = CollectorEndpoint::new(
        "multi-collector".to_owned(),
        "/tmp/multi-collector.sock".to_owned(),
        3,
    );

    // Set up capabilities
    process_endpoint.update_capabilities(CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Process)],
        advanced: Some(AdvancedCapabilities {
            kernel_level: false,
            realtime: true,
            system_wide: true,
        }),
    });

    network_endpoint.update_capabilities(CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Network)],
        advanced: Some(AdvancedCapabilities {
            kernel_level: true,
            realtime: true,
            system_wide: true,
        }),
    });

    multi_endpoint.update_capabilities(CollectionCapabilities {
        supported_domains: vec![
            i32::from(MonitoringDomain::Process),
            i32::from(MonitoringDomain::Network),
            i32::from(MonitoringDomain::Filesystem),
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: true,
            realtime: true,
            system_wide: true,
        }),
    });

    // Create client with all endpoints
    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: "/tmp/default.sock".to_owned(),
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 10,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    let client = ResilientIpcClient::new_with_endpoints(
        &config,
        vec![process_endpoint, network_endpoint, multi_endpoint],
        LoadBalancingStrategy::Priority,
    );

    // Test endpoint selection for different task types

    // Process task should select process-collector (highest priority among compatible)
    let process_selected = client
        .select_endpoint_for_task(TaskType::EnumerateProcesses)
        .await;
    assert!(process_selected.is_some());
    let process_selected = process_selected.unwrap();
    assert_eq!(process_selected.id, "process-collector");

    // Network task should select network-collector (only compatible endpoint)
    let network_selected = client
        .select_endpoint_for_task(TaskType::MonitorNetworkConnections)
        .await;
    assert!(network_selected.is_some());
    let network_selected = network_selected.unwrap();
    assert_eq!(network_selected.id, "network-collector");

    // Filesystem task should select multi-collector (only compatible endpoint)
    let filesystem_selected = client
        .select_endpoint_for_task(TaskType::TrackFileOperations)
        .await;
    assert!(filesystem_selected.is_some());
    let filesystem_selected = filesystem_selected.unwrap();
    assert_eq!(filesystem_selected.id, "multi-collector");

    // Performance task should fall back to any healthy endpoint (no compatible endpoints)
    let performance_selected = client
        .select_endpoint_for_task(TaskType::CollectPerformanceMetrics)
        .await;
    assert!(performance_selected.is_some()); // Should get a fallback endpoint
    let performance_selected = performance_selected.unwrap();
    // Should be the highest priority healthy endpoint as fallback
    assert_eq!(performance_selected.id, "process-collector");
}

#[tokio::test]
async fn test_capability_refresh_timing() {
    let mut endpoint = CollectorEndpoint::new(
        "test-collector".to_owned(),
        "/tmp/test-collector.sock".to_owned(),
        1,
    );

    // Initially should need refresh (never checked)
    assert!(endpoint.needs_capability_refresh(Duration::from_secs(300)));

    // Update capabilities
    endpoint.update_capabilities(CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Process)],
        advanced: None,
    });

    // Should not need refresh immediately after update
    assert!(!endpoint.needs_capability_refresh(Duration::from_secs(300)));

    // Should need refresh with very short max age (wait a bit first)
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert!(endpoint.needs_capability_refresh(Duration::from_millis(1)));
}

#[tokio::test]
async fn test_task_routing_with_capabilities() {
    // Create a client with no endpoints
    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: "/tmp/default.sock".to_owned(),
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 10,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    let client = ResilientIpcClient::new(&config);

    // Create a task
    let task = DetectionTask {
        task_id: "test-task".to_owned(),
        task_type: i32::from(TaskType::EnumerateProcesses),
        process_filter: None,
        hash_check: None,
        metadata: Some("test".to_owned()),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    // Should fail with no endpoints available
    let result = client.send_task_with_routing(task).await;
    assert!(result.is_err());

    // Verify it's the expected error type
    match result {
        Err(daemoneye_lib::ipc::codec::IpcError::Io(io_err)) => {
            assert_eq!(io_err.kind(), std::io::ErrorKind::NotFound);
            assert_eq!(
                io_err.to_string(),
                "No endpoints available for task type: EnumerateProcesses"
            );
        }
        Err(e) => panic!("Unexpected error type: {e:?}"),
        Ok(_) => panic!("Expected task routing to fail with no endpoints"),
    }
}

#[tokio::test]
async fn test_client_statistics_with_capabilities() {
    // Create endpoints with different capabilities
    let mut endpoint1 =
        CollectorEndpoint::new("endpoint1".to_owned(), "/tmp/endpoint1.sock".to_owned(), 1);

    let mut endpoint2 =
        CollectorEndpoint::new("endpoint2".to_owned(), "/tmp/endpoint2.sock".to_owned(), 2);

    // Set up capabilities
    endpoint1.update_capabilities(CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Process)],
        advanced: Some(AdvancedCapabilities {
            kernel_level: false,
            realtime: true,
            system_wide: true,
        }),
    });

    endpoint2.update_capabilities(CollectionCapabilities {
        supported_domains: vec![
            i32::from(MonitoringDomain::Process),
            i32::from(MonitoringDomain::Network),
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: true,
            realtime: true,
            system_wide: true,
        }),
    });

    // Create client
    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: "/tmp/default.sock".to_owned(),
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 10,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    let client = ResilientIpcClient::new_with_endpoints(
        &config,
        vec![endpoint1, endpoint2],
        LoadBalancingStrategy::Weighted,
    );

    // Get statistics
    let stats = client.get_stats().await;

    // Verify statistics structure
    assert_eq!(stats.endpoint_stats.len(), 2);
    assert!(matches!(
        stats.load_balancing_strategy,
        LoadBalancingStrategy::Weighted
    ));

    // Verify endpoint statistics include capabilities
    let endpoint1_stats = stats
        .endpoint_stats
        .iter()
        .find(|s| s.endpoint_id == "endpoint1")
        .expect("endpoint1 stats should exist");

    assert!(endpoint1_stats.capabilities.is_some());
    let caps1 = endpoint1_stats.capabilities.as_ref().unwrap();
    assert!(
        caps1
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Process))
    );
    assert!(
        !caps1
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Network))
    );

    let endpoint2_stats = stats
        .endpoint_stats
        .iter()
        .find(|s| s.endpoint_id == "endpoint2")
        .expect("endpoint2 stats should exist");

    assert!(endpoint2_stats.capabilities.is_some());
    let caps2 = endpoint2_stats.capabilities.as_ref().unwrap();
    assert!(
        caps2
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Process))
    );
    assert!(
        caps2
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Network))
    );
}

#[tokio::test]
async fn test_capability_negotiation_error_handling() {
    // Test various error scenarios in capability negotiation

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: "/tmp/nonexistent.sock".to_owned(),
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 100, // Very short timeout
        read_timeout_ms: 100,
        write_timeout_ms: 100,
        max_connections: 10,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    let endpoint = CollectorEndpoint::new(
        "nonexistent-collector".to_owned(),
        "/tmp/nonexistent.sock".to_owned(),
        1,
    );

    let client = ResilientIpcClient::new_with_endpoints(
        &config,
        vec![endpoint],
        LoadBalancingStrategy::Priority,
    );

    // Test capability negotiation with nonexistent endpoint
    let result1 = client.negotiate_capabilities("nonexistent-collector").await;
    assert!(result1.is_err());

    // Test capability negotiation with invalid endpoint ID
    let result2 = client.negotiate_capabilities("invalid-endpoint").await;
    assert!(result2.is_err());

    match result2 {
        Err(daemoneye_lib::ipc::codec::IpcError::Io(io_err)) => {
            assert_eq!(io_err.kind(), std::io::ErrorKind::NotFound);
            assert!(io_err.to_string().contains("Endpoint not found"));
        }
        Err(e) => panic!("Unexpected error type: {e:?}"),
        Ok(_) => panic!("Expected capability negotiation to fail"),
    }
}
