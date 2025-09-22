//! Tests for multi-domain protobuf extensions and backward compatibility.
//!
//! This module tests the extended protobuf definitions for multi-domain support,
//! ensuring proper serialization, deserialization, and backward compatibility
//! with existing message structures.
//!
//! # Test Coverage
//!
//! - Collection capabilities serialization and deserialization
//! - Extended task types for multi-domain monitoring
//! - Detection tasks with multi-domain filters
//! - Detection results with multi-domain events
//! - Backward compatibility with legacy message formats
//! - Enum value stability for protocol compatibility
//! - Empty filter handling and edge cases
//! - Message size limits and performance characteristics

#![allow(clippy::as_conversions)] // Allow enum conversions in tests

use daemoneye_lib::proto::*;
use prost::Message;

/// Tests serialization and deserialization of `CollectionCapabilities` message.
///
/// Verifies that capability negotiation messages can be properly serialized
/// and deserialized, including advanced capabilities.
#[test]
fn test_collection_capabilities_serialization() -> Result<(), Box<dyn std::error::Error>> {
    let capabilities = CollectionCapabilities {
        supported_domains: vec![
            i32::from(MonitoringDomain::Process as u8),
            i32::from(MonitoringDomain::Network as u8),
            i32::from(MonitoringDomain::Filesystem as u8),
            i32::from(MonitoringDomain::Performance as u8),
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: true,
            realtime: true,
            system_wide: true,
        }),
    };

    // Test serialization
    let serialized = capabilities.encode_to_vec();
    assert!(!serialized.is_empty());

    // Test deserialization
    let deserialized = CollectionCapabilities::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize CollectionCapabilities: {e}"))?;

    assert_eq!(deserialized.supported_domains.len(), 4);
    assert!(
        deserialized
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Process as u8))
    );
    assert!(
        deserialized
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Network as u8))
    );
    assert!(
        deserialized
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Filesystem as u8))
    );
    assert!(
        deserialized
            .supported_domains
            .contains(&i32::from(MonitoringDomain::Performance as u8))
    );

    let advanced = deserialized
        .advanced
        .ok_or("Should have advanced capabilities")?;
    assert!(advanced.kernel_level);
    assert!(advanced.realtime);
    assert!(advanced.system_wide);

    Ok(())
}

/// Tests serialization of all extended task types for multi-domain monitoring.
///
/// Ensures that new task types (network, filesystem, performance) can be
/// properly serialized alongside existing process monitoring tasks.
#[test]
fn test_extended_task_types() -> Result<(), Box<dyn std::error::Error>> {
    // Test all task types can be serialized
    let task_types = vec![
        TaskType::EnumerateProcesses,
        TaskType::CheckProcessHash,
        TaskType::MonitorProcessTree,
        TaskType::VerifyExecutable,
        TaskType::MonitorNetworkConnections,
        TaskType::TrackFileOperations,
        TaskType::CollectPerformanceMetrics,
    ];

    for task_type in task_types {
        let task = DetectionTask {
            task_id: format!("test-{}", i32::from(task_type as u8)),
            task_type: i32::from(task_type as u8),
            process_filter: None,
            hash_check: None,
            metadata: Some("test metadata".to_owned()),
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        let serialized = task.encode_to_vec();
        assert!(!serialized.is_empty());

        let deserialized = DetectionTask::decode(&serialized[..])
            .map_err(|e| format!("Failed to deserialize DetectionTask: {e}"))?;

        assert_eq!(deserialized.task_type, i32::from(task_type as u8));
        assert_eq!(deserialized.metadata.as_deref(), Some("test metadata"));
    }

    Ok(())
}

/// Tests `DetectionTask` with filters for all monitoring domains.
///
/// Verifies that a single detection task can contain filters for process,
/// network, filesystem, and performance monitoring simultaneously.
#[test]
fn test_detection_task_with_multi_domain_filters() -> Result<(), Box<dyn std::error::Error>> {
    let task = DetectionTask {
        task_id: "multi-domain-task".to_owned(),
        task_type: i32::from(TaskType::EnumerateProcesses as u8),
        process_filter: Some(ProcessFilter {
            process_names: vec!["firefox".to_owned(), "chrome".to_owned()],
            pids: vec![1234, 5678],
            executable_pattern: Some("/usr/bin/*".to_owned()),
        }),
        hash_check: None,
        metadata: Some("multi-domain test".to_owned()),
        network_filter: Some(NetworkFilter {
            protocols: vec!["TCP".to_owned(), "UDP".to_owned()],
            source_addresses: vec!["192.168.1.0/24".to_owned()],
            destination_addresses: vec!["10.0.0.0/8".to_owned()],
            connection_states: vec!["ESTABLISHED".to_owned(), "LISTEN".to_owned()],
        }),
        filesystem_filter: Some(FilesystemFilter {
            path_patterns: vec!["/etc/*".to_owned(), "/var/log/*".to_owned()],
            operation_types: vec![
                "CREATE".to_owned(),
                "MODIFY".to_owned(),
                "DELETE".to_owned(),
            ],
            file_extensions: vec![".log".to_owned(), ".conf".to_owned()],
        }),
        performance_filter: Some(PerformanceFilter {
            metric_names: vec!["cpu_usage".to_owned(), "memory_usage".to_owned()],
            components: vec!["CPU".to_owned(), "MEMORY".to_owned(), "DISK".to_owned()],
            min_thresholds: [
                ("cpu_usage".to_owned(), 0.1),
                ("memory_usage".to_owned(), 1024.0),
            ]
            .into_iter()
            .collect(),
            max_thresholds: [
                ("cpu_usage".to_owned(), 90.0),
                ("memory_usage".to_owned(), 8192.0),
            ]
            .into_iter()
            .collect(),
        }),
    };

    // Test serialization
    let serialized = task.encode_to_vec();
    assert!(!serialized.is_empty());

    // Test deserialization
    let deserialized = DetectionTask::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize DetectionTask with all filters: {e}"))?;

    assert_eq!(deserialized.task_id, "multi-domain-task");
    assert_eq!(
        deserialized.task_type,
        i32::from(TaskType::EnumerateProcesses as u8)
    );

    // Verify process filter
    let process_filter = deserialized
        .process_filter
        .ok_or("Should have process filter")?;
    assert_eq!(process_filter.process_names.len(), 2);
    assert_eq!(process_filter.pids.len(), 2);
    assert_eq!(
        process_filter.executable_pattern.as_deref(),
        Some("/usr/bin/*")
    );

    // Verify network filter
    let network_filter = deserialized
        .network_filter
        .ok_or("Should have network filter")?;
    assert_eq!(network_filter.protocols.len(), 2);
    assert_eq!(network_filter.source_addresses.len(), 1);
    assert_eq!(network_filter.destination_addresses.len(), 1);
    assert_eq!(network_filter.connection_states.len(), 2);

    // Verify filesystem filter
    let filesystem_filter = deserialized
        .filesystem_filter
        .ok_or("Should have filesystem filter")?;
    assert_eq!(filesystem_filter.path_patterns.len(), 2);
    assert_eq!(filesystem_filter.operation_types.len(), 3);
    assert_eq!(filesystem_filter.file_extensions.len(), 2);

    // Verify performance filter
    let performance_filter = deserialized
        .performance_filter
        .ok_or("Should have performance filter")?;
    assert_eq!(performance_filter.metric_names.len(), 2);
    assert_eq!(performance_filter.components.len(), 3);
    assert_eq!(performance_filter.min_thresholds.len(), 2);
    assert_eq!(performance_filter.max_thresholds.len(), 2);

    Ok(())
}

/// Tests `DetectionResult` containing events from all monitoring domains.
///
/// Verifies that detection results can contain process, network, filesystem,
/// and performance events in a single response message.
#[test]
fn test_detection_result_with_multi_domain_events() -> Result<(), Box<dyn std::error::Error>> {
    let result = DetectionResult {
        task_id: "multi-domain-result".to_owned(),
        success: true,
        error_message: None,
        processes: vec![ProcessRecord {
            pid: 1234,
            ppid: Some(1),
            name: "test-process".to_owned(),
            executable_path: Some("/usr/bin/test".to_owned()),
            command_line: vec!["test".to_owned(), "--arg".to_owned(), "value".to_owned()],
            start_time: Some(1_640_995_200), // 2022-01-01 00:00:00 UTC
            cpu_usage: Some(5.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123def456".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            collection_time: 1_640_995_200_000, // 2022-01-01 00:00:00 UTC in milliseconds
        }],
        hash_result: None,
        network_events: vec![NetworkRecord {
            connection_id: "conn-1".to_owned(),
            source_address: "192.168.1.100:8080".to_owned(),
            destination_address: "10.0.0.1:443".to_owned(),
            protocol: "TCP".to_owned(),
            state: "ESTABLISHED".to_owned(),
            pid: Some(1234),
            bytes_sent: 1024,
            bytes_received: 2048,
            collection_time: 1_640_995_200_000,
        }],
        filesystem_events: vec![FilesystemRecord {
            operation_id: "fs-op-1".to_owned(),
            path: "/var/log/test.log".to_owned(),
            operation_type: "CREATE".to_owned(),
            pid: Some(1234),
            file_size: Some(512),
            permissions: Some("644".to_owned()),
            file_hash: Some("def456abc123".to_owned()),
            collection_time: 1_640_995_200_000,
        }],
        performance_events: vec![PerformanceRecord {
            metric_id: "perf-1".to_owned(),
            metric_name: "cpu_usage".to_owned(),
            value: 15.5,
            unit: "percent".to_owned(),
            pid: Some(1234),
            component: "CPU".to_owned(),
            collection_time: 1_640_995_200_000,
        }],
    };

    // Test serialization
    let serialized = result.encode_to_vec();
    assert!(!serialized.is_empty());

    // Test deserialization
    let deserialized = DetectionResult::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize DetectionResult: {e}"))?;

    assert_eq!(deserialized.task_id, "multi-domain-result");
    assert!(deserialized.success);
    assert!(deserialized.error_message.is_none());

    // Verify process records
    assert_eq!(deserialized.processes.len(), 1);
    let process = deserialized
        .processes
        .first()
        .ok_or("No process records found")?;
    assert_eq!(process.pid, 1234);
    assert_eq!(process.name, "test-process");

    // Verify network events
    assert_eq!(deserialized.network_events.len(), 1);
    let network_event = deserialized
        .network_events
        .first()
        .ok_or("No network events found")?;
    assert_eq!(network_event.connection_id, "conn-1");
    assert_eq!(network_event.protocol, "TCP");
    assert_eq!(network_event.pid, Some(1234));

    // Verify filesystem events
    assert_eq!(deserialized.filesystem_events.len(), 1);
    let fs_event = deserialized
        .filesystem_events
        .first()
        .ok_or("No filesystem events found")?;
    assert_eq!(fs_event.operation_id, "fs-op-1");
    assert_eq!(fs_event.operation_type, "CREATE");
    assert_eq!(fs_event.pid, Some(1234));

    // Verify performance events
    assert_eq!(deserialized.performance_events.len(), 1);
    let perf_event = deserialized
        .performance_events
        .first()
        .ok_or("No performance events found")?;
    assert_eq!(perf_event.metric_id, "perf-1");
    assert_eq!(perf_event.metric_name, "cpu_usage");
    assert!((perf_event.value - 15.5).abs() < f64::EPSILON);
    assert_eq!(perf_event.pid, Some(1234));

    Ok(())
}

/// Tests backward compatibility with legacy `DetectionTask` messages.
///
/// Ensures that detection tasks created with only the original fields
/// can still be serialized and deserialized correctly.
#[test]
fn test_backward_compatibility_detection_task() -> Result<(), Box<dyn std::error::Error>> {
    // Create a "legacy" detection task with only the original fields
    let legacy_task = DetectionTask {
        task_id: "legacy-task".to_owned(),
        task_type: i32::from(TaskType::EnumerateProcesses as u8),
        process_filter: Some(ProcessFilter {
            process_names: vec!["firefox".to_owned()],
            pids: vec![1234],
            executable_pattern: None,
        }),
        hash_check: None,
        metadata: Some("legacy metadata".to_owned()),
        // New fields should be None/empty for backward compatibility
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    // Test that legacy task can be serialized and deserialized
    let serialized = legacy_task.encode_to_vec();
    let deserialized = DetectionTask::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize legacy DetectionTask: {e}"))?;

    assert_eq!(deserialized.task_id, "legacy-task");
    assert_eq!(
        deserialized.task_type,
        i32::from(TaskType::EnumerateProcesses as u8)
    );
    assert!(deserialized.process_filter.is_some());
    assert!(deserialized.network_filter.is_none());
    assert!(deserialized.filesystem_filter.is_none());
    assert!(deserialized.performance_filter.is_none());

    Ok(())
}

/// Tests backward compatibility with legacy `DetectionResult` messages.
///
/// Ensures that detection results created with only the original fields
/// can still be serialized and deserialized correctly.
#[test]
fn test_backward_compatibility_detection_result() -> Result<(), Box<dyn std::error::Error>> {
    // Create a "legacy" detection result with only the original fields
    let legacy_result = DetectionResult {
        task_id: "legacy-result".to_owned(),
        success: true,
        error_message: None,
        processes: vec![ProcessRecord {
            pid: 1234,
            ppid: Some(1),
            name: "legacy-process".to_owned(),
            executable_path: Some("/usr/bin/legacy".to_owned()),
            command_line: vec!["legacy".to_owned()],
            start_time: Some(1_640_995_200),
            cpu_usage: Some(2.5),
            memory_usage: Some(512 * 1024),
            executable_hash: Some("legacy123".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            collection_time: 1_640_995_200_000,
        }],
        hash_result: None,
        // New fields should be empty for backward compatibility
        network_events: vec![],
        filesystem_events: vec![],
        performance_events: vec![],
    };

    // Test that legacy result can be serialized and deserialized
    let serialized = legacy_result.encode_to_vec();
    let deserialized = DetectionResult::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize legacy DetectionResult: {e}"))?;

    assert_eq!(deserialized.task_id, "legacy-result");
    assert!(deserialized.success);
    assert_eq!(deserialized.processes.len(), 1);
    assert!(deserialized.network_events.is_empty());
    assert!(deserialized.filesystem_events.is_empty());
    assert!(deserialized.performance_events.is_empty());

    Ok(())
}

/// Tests stability of `MonitoringDomain` enum values for protocol compatibility.
///
/// Ensures that enum values remain stable across versions to maintain
/// backward compatibility in the protobuf protocol.
#[test]
fn test_monitoring_domain_enum_values() {
    // Ensure enum values are stable for backward compatibility
    assert_eq!(i32::from(MonitoringDomain::Process as u8), 0);
    assert_eq!(i32::from(MonitoringDomain::Network as u8), 1);
    assert_eq!(i32::from(MonitoringDomain::Filesystem as u8), 2);
    assert_eq!(i32::from(MonitoringDomain::Performance as u8), 3);
}

/// Tests stability of `TaskType` enum values for protocol compatibility.
///
/// Ensures that both legacy and new task type enum values remain stable
/// across versions to maintain backward compatibility.
#[test]
fn test_task_type_enum_values() {
    // Ensure enum values are stable for backward compatibility
    assert_eq!(i32::from(TaskType::EnumerateProcesses as u8), 0);
    assert_eq!(i32::from(TaskType::CheckProcessHash as u8), 1);
    assert_eq!(i32::from(TaskType::MonitorProcessTree as u8), 2);
    assert_eq!(i32::from(TaskType::VerifyExecutable as u8), 3);
    // New task types
    assert_eq!(i32::from(TaskType::MonitorNetworkConnections as u8), 4);
    assert_eq!(i32::from(TaskType::TrackFileOperations as u8), 5);
    assert_eq!(i32::from(TaskType::CollectPerformanceMetrics as u8), 6);
}

/// Tests serialization and deserialization of empty filter structures.
///
/// Verifies that filters with no criteria can be properly handled,
/// which is important for optional filtering scenarios.
#[test]
fn test_empty_filters_serialization() -> Result<(), Box<dyn std::error::Error>> {
    // Test that empty filters serialize and deserialize correctly
    let empty_network_filter = NetworkFilter {
        protocols: vec![],
        source_addresses: vec![],
        destination_addresses: vec![],
        connection_states: vec![],
    };

    let empty_filesystem_filter = FilesystemFilter {
        path_patterns: vec![],
        operation_types: vec![],
        file_extensions: vec![],
    };

    let empty_performance_filter = PerformanceFilter {
        metric_names: vec![],
        components: vec![],
        min_thresholds: std::collections::HashMap::new(),
        max_thresholds: std::collections::HashMap::new(),
    };

    // Test serialization of empty filters
    let network_serialized = empty_network_filter.encode_to_vec();
    let filesystem_serialized = empty_filesystem_filter.encode_to_vec();
    let performance_serialized = empty_performance_filter.encode_to_vec();

    // Test deserialization of empty filters
    let network_deserialized = NetworkFilter::decode(&network_serialized[..])
        .map_err(|e| format!("Failed to deserialize empty NetworkFilter: {e}"))?;
    let filesystem_deserialized = FilesystemFilter::decode(&filesystem_serialized[..])
        .map_err(|e| format!("Failed to deserialize empty FilesystemFilter: {e}"))?;
    let performance_deserialized = PerformanceFilter::decode(&performance_serialized[..])
        .map_err(|e| format!("Failed to deserialize empty PerformanceFilter: {e}"))?;

    assert!(network_deserialized.protocols.is_empty());
    assert!(filesystem_deserialized.path_patterns.is_empty());
    assert!(performance_deserialized.metric_names.is_empty());

    Ok(())
}

/// Tests optional advanced capabilities in `CollectionCapabilities`.
///
/// Verifies that advanced capabilities can be omitted (None) or included
/// with specific settings, supporting different deployment scenarios.
#[test]
fn test_advanced_capabilities_optional() -> Result<(), Box<dyn std::error::Error>> {
    // Test CollectionCapabilities with no advanced capabilities
    let basic_capabilities = CollectionCapabilities {
        supported_domains: vec![i32::from(MonitoringDomain::Process as u8)],
        advanced: None,
    };

    let basic_serialized = basic_capabilities.encode_to_vec();
    let basic_deserialized = CollectionCapabilities::decode(&basic_serialized[..])
        .map_err(|e| format!("Failed to deserialize basic CollectionCapabilities: {e}"))?;

    assert_eq!(basic_deserialized.supported_domains.len(), 1);
    assert!(basic_deserialized.advanced.is_none());

    // Test CollectionCapabilities with advanced capabilities
    let advanced_capabilities = CollectionCapabilities {
        supported_domains: vec![
            i32::from(MonitoringDomain::Process as u8),
            i32::from(MonitoringDomain::Network as u8),
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: false,
            realtime: true,
            system_wide: false,
        }),
    };

    let advanced_serialized = advanced_capabilities.encode_to_vec();
    let advanced_deserialized = CollectionCapabilities::decode(&advanced_serialized[..])
        .map_err(|e| format!("Failed to deserialize advanced CollectionCapabilities: {e}"))?;

    assert_eq!(advanced_deserialized.supported_domains.len(), 2);
    let advanced = advanced_deserialized
        .advanced
        .ok_or("Should have advanced capabilities")?;
    assert!(!advanced.kernel_level);
    assert!(advanced.realtime);
    assert!(!advanced.system_wide);

    Ok(())
}

/// Tests serialization of large messages with realistic data volumes.
///
/// Verifies that messages with substantial amounts of data (large filter lists,
/// metadata) can be properly serialized and deserialized without issues.
#[test]
fn test_message_size_limits() -> Result<(), Box<dyn std::error::Error>> {
    // Test that messages with reasonable data sizes can be serialized
    let large_task = DetectionTask {
        task_id: "large-task".to_owned(),
        task_type: i32::from(TaskType::EnumerateProcesses as u8),
        process_filter: Some(ProcessFilter {
            process_names: (0..100).map(|i| format!("process-{i}")).collect(),
            pids: (1000..2000).collect(),
            executable_pattern: Some("/usr/bin/*".to_owned()),
        }),
        hash_check: None,
        metadata: Some("large metadata".repeat(100)),
        network_filter: Some(NetworkFilter {
            protocols: vec!["TCP".to_owned(), "UDP".to_owned(), "ICMP".to_owned()],
            source_addresses: (0..50).map(|i| format!("192.168.1.{i}")).collect(),
            destination_addresses: (0..50).map(|i| format!("10.0.0.{i}")).collect(),
            connection_states: vec![
                "ESTABLISHED".to_owned(),
                "LISTEN".to_owned(),
                "TIME_WAIT".to_owned(),
            ],
        }),
        filesystem_filter: Some(FilesystemFilter {
            path_patterns: (0..20).map(|i| format!("/path/to/file-{i}")).collect(),
            operation_types: vec![
                "CREATE".to_owned(),
                "MODIFY".to_owned(),
                "DELETE".to_owned(),
                "ACCESS".to_owned(),
            ],
            file_extensions: vec![
                ".log".to_owned(),
                ".conf".to_owned(),
                ".tmp".to_owned(),
                ".dat".to_owned(),
            ],
        }),
        performance_filter: Some(PerformanceFilter {
            metric_names: (0..10).map(|i| format!("metric-{i}")).collect(),
            components: vec![
                "CPU".to_owned(),
                "MEMORY".to_owned(),
                "DISK".to_owned(),
                "NETWORK".to_owned(),
            ],
            min_thresholds: (0..10)
                .map(|i| (format!("metric-{i}"), f64::from(i)))
                .collect(),
            max_thresholds: (0..10)
                .map(|i| (format!("metric-{i}"), f64::from(i + 100)))
                .collect(),
        }),
    };

    // Should be able to serialize and deserialize large messages
    let serialized = large_task.encode_to_vec();
    assert!(!serialized.is_empty());

    let deserialized = DetectionTask::decode(&serialized[..])
        .map_err(|e| format!("Failed to deserialize large DetectionTask: {e}"))?;

    assert_eq!(deserialized.task_id, "large-task");
    assert_eq!(
        deserialized
            .process_filter
            .ok_or("Missing process filter")?
            .process_names
            .len(),
        100
    );
    assert_eq!(
        deserialized
            .network_filter
            .ok_or("Missing network filter")?
            .source_addresses
            .len(),
        50
    );
    assert_eq!(
        deserialized
            .filesystem_filter
            .ok_or("Missing filesystem filter")?
            .path_patterns
            .len(),
        20
    );
    assert_eq!(
        deserialized
            .performance_filter
            .ok_or("Missing performance filter")?
            .metric_names
            .len(),
        10
    );

    Ok(())
}
