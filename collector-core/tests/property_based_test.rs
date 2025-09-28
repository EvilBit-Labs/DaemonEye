//! Property-based tests for CollectionEvent serialization and capability negotiation.
//!
//! This test suite uses proptest to generate random test cases and verify
//! invariants across the collector-core framework, particularly focusing on
//! serialization correctness and capability negotiation properties.

use collector_core::{
    CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent, SourceCaps,
    TriggerRequest,
};
use proptest::prelude::*;
use proptest::strategy::Just;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Property test strategies for generating test data

/// Strategy for generating valid process IDs
fn pid_strategy() -> impl Strategy<Value = u32> {
    1u32..=65535u32
}

/// Strategy for generating optional parent process IDs
fn ppid_strategy() -> impl Strategy<Value = Option<u32>> {
    prop::option::of(1u32..=65535u32)
}

/// Strategy for generating process names
fn process_name_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex(r"[a-zA-Z0-9_-]{1,64}").unwrap()
}

/// Strategy for generating executable paths
fn executable_path_strategy() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::string::string_regex(r"/[a-zA-Z0-9/_-]{1,255}").unwrap())
}

/// Strategy for generating command line arguments
fn command_line_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop::string::string_regex(r"[a-zA-Z0-9_.-]{1,32}").unwrap(),
        0..=10,
    )
}

/// Strategy for generating system timestamps
fn timestamp_strategy() -> impl Strategy<Value = SystemTime> {
    (0u64..=2_147_483_647u64).prop_map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
}

/// Strategy for generating CPU usage percentages
fn cpu_usage_strategy() -> impl Strategy<Value = Option<f64>> {
    prop::option::of(0.0f64..=100.0f64)
}

/// Strategy for generating memory usage in bytes
fn memory_usage_strategy() -> impl Strategy<Value = Option<u64>> {
    prop::option::of(1024u64..=17_179_869_184u64) // 1KB to 16GB
}

/// Strategy for generating SHA-256 hashes
fn hash_strategy() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::string::string_regex(r"[a-f0-9]{64}").unwrap())
}

/// Strategy for generating user IDs
fn user_id_strategy() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop::string::string_regex(r"[0-9]{1,10}").unwrap())
}

/// Strategy for generating ProcessEvent instances
fn process_event_strategy() -> impl Strategy<Value = ProcessEvent> {
    // Split into smaller tuples to avoid trait bound issues
    let basic_fields = (
        pid_strategy(),
        ppid_strategy(),
        process_name_strategy(),
        executable_path_strategy(),
        command_line_strategy(),
    );

    let optional_fields = (
        prop::option::of(timestamp_strategy()),
        cpu_usage_strategy(),
        memory_usage_strategy(),
        hash_strategy(),
        user_id_strategy(),
    );

    let flags_and_timestamp = (
        any::<bool>(), // accessible
        any::<bool>(), // file_exists
        timestamp_strategy(),
    );

    (basic_fields, optional_fields, flags_and_timestamp).prop_map(
        |(
            (pid, ppid, name, executable_path, command_line),
            (start_time, cpu_usage, memory_usage, executable_hash, user_id),
            (accessible, file_exists, timestamp),
        )| ProcessEvent {
            pid,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            user_id,
            accessible,
            file_exists,
            timestamp,
            platform_metadata: None,
        },
    )
}

/// Strategy for generating NetworkEvent instances
fn network_event_strategy() -> impl Strategy<Value = NetworkEvent> {
    (
        prop::string::string_regex(r"conn_[0-9]{1,10}").unwrap(),
        prop::string::string_regex(r"[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}:[0-9]{1,5}")
            .unwrap(),
        prop::string::string_regex(r"[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}:[0-9]{1,5}")
            .unwrap(),
        prop::string::string_regex(r"(TCP|UDP|ICMP)").unwrap(),
        prop::string::string_regex(r"(ESTABLISHED|LISTEN|CLOSED)").unwrap(),
        prop::option::of(pid_strategy()),
        0u64..=1_073_741_824u64, // 0 to 1GB
        0u64..=1_073_741_824u64, // 0 to 1GB
        timestamp_strategy(),
    )
        .prop_map(
            |(
                connection_id,
                source_addr,
                dest_addr,
                protocol,
                state,
                pid,
                bytes_sent,
                bytes_received,
                timestamp,
            )| NetworkEvent {
                connection_id,
                source_addr,
                dest_addr,
                protocol,
                state,
                pid,
                bytes_sent,
                bytes_received,
                timestamp,
            },
        )
}

/// Strategy for generating FilesystemEvent instances
fn filesystem_event_strategy() -> impl Strategy<Value = FilesystemEvent> {
    (
        prop::string::string_regex(r"/[a-zA-Z0-9/_.-]{1,255}").unwrap(),
        prop::string::string_regex(r"(create|modify|delete|access)").unwrap(),
        prop::option::of(pid_strategy()),
        prop::option::of(0u64..=1_073_741_824u64), // 0 to 1GB
        prop::option::of(prop::string::string_regex(r"[rwx-]{9}").unwrap()),
        hash_strategy(),
        timestamp_strategy(),
    )
        .prop_map(
            |(path, operation, pid, size, permissions, file_hash, timestamp)| FilesystemEvent {
                path,
                operation,
                pid,
                size,
                permissions,
                file_hash,
                timestamp,
            },
        )
}

/// Strategy for generating PerformanceEvent instances
fn performance_event_strategy() -> impl Strategy<Value = PerformanceEvent> {
    (
        prop::string::string_regex(r"[a-zA-Z0-9_]{1,32}").unwrap(),
        -1000.0f64..=1000.0f64,
        prop::string::string_regex(r"(bytes|percent|count|seconds)").unwrap(),
        prop::option::of(pid_strategy()),
        prop::string::string_regex(r"(CPU|memory|disk|network)").unwrap(),
        timestamp_strategy(),
    )
        .prop_map(
            |(metric_name, value, unit, pid, component, timestamp)| PerformanceEvent {
                metric_name,
                value,
                unit,
                pid,
                component,
                timestamp,
            },
        )
}

/// Strategy for generating metadata maps with varied content
fn metadata_strategy() -> impl Strategy<Value = std::collections::HashMap<String, String>> {
    prop::collection::hash_map(
        prop::string::string_regex(r"[a-zA-Z0-9_-]{1,16}").unwrap(), // keys
        prop::string::string_regex(r"[a-zA-Z0-9_\\s.-]{0,64}").unwrap(), // values
        0..=8, // 0 to 8 entries (including empty maps)
    )
}

/// Strategy for generating TriggerRequest instances
fn trigger_request_strategy() -> impl Strategy<Value = TriggerRequest> {
    use collector_core::{AnalysisType, TriggerPriority, TriggerRequest};

    (
        prop::string::string_regex(r"[a-zA-Z0-9_-]{1,32}").unwrap(), // trigger_id
        prop::string::string_regex(r"[a-zA-Z0-9_-]{1,32}").unwrap(), // target_collector
        prop_oneof![
            Just(AnalysisType::BinaryHash),
            Just(AnalysisType::MemoryAnalysis),
            Just(AnalysisType::YaraScan),
            Just(AnalysisType::NetworkAnalysis),
            Just(AnalysisType::BehavioralAnalysis),
            prop::string::string_regex(r"[a-zA-Z0-9_]{1,16}")
                .unwrap()
                .prop_map(AnalysisType::Custom),
        ], // analysis_type
        prop_oneof![
            Just(TriggerPriority::Low),
            Just(TriggerPriority::Normal),
            Just(TriggerPriority::High),
            Just(TriggerPriority::Critical),
        ], // priority
        prop::option::of(pid_strategy()),                            // target_pid
        prop::option::of(prop::string::string_regex(r"/[a-zA-Z0-9/_.-]{1,255}").unwrap()), // target_path
        prop::string::string_regex(r"[a-zA-Z0-9_-]{1,32}").unwrap(), // correlation_id
        metadata_strategy(),                                         // metadata
        timestamp_strategy(),                                        // timestamp
    )
        .prop_map(
            |(
                trigger_id,
                target_collector,
                analysis_type,
                priority,
                target_pid,
                target_path,
                correlation_id,
                metadata,
                timestamp,
            )| {
                TriggerRequest {
                    trigger_id,
                    target_collector,
                    analysis_type,
                    priority,
                    target_pid,
                    target_path,
                    correlation_id,
                    metadata,
                    timestamp,
                }
            },
        )
}

/// Strategy for generating CollectionEvent instances
fn collection_event_strategy() -> impl Strategy<Value = CollectionEvent> {
    prop_oneof![
        process_event_strategy().prop_map(CollectionEvent::Process),
        network_event_strategy().prop_map(CollectionEvent::Network),
        filesystem_event_strategy().prop_map(CollectionEvent::Filesystem),
        performance_event_strategy().prop_map(CollectionEvent::Performance),
        trigger_request_strategy().prop_map(CollectionEvent::TriggerRequest),
    ]
}

/// Strategy for generating SourceCaps combinations
fn source_caps_strategy() -> impl Strategy<Value = SourceCaps> {
    any::<u32>().prop_map(|bits| SourceCaps::from_bits_truncate(bits & 0x7F)) // Mask to valid bits
}

// Property-based tests

proptest! {
    /// Test that CollectionEvent serialization is bidirectional and preserves data.
    #[test]
    fn test_collection_event_serialization_roundtrip(event in collection_event_strategy()) {
        // Serialize to JSON
        let json = serde_json::to_string(&event)
            .expect("Serialization should succeed");

        // Deserialize back
        let deserialized: CollectionEvent = serde_json::from_str(&json)
            .expect("Deserialization should succeed");

        // Verify roundtrip preservation
        prop_assert_eq!(event.event_type(), deserialized.event_type());
        prop_assert_eq!(event.pid(), deserialized.pid());

        // Timestamps should be preserved (within reasonable precision)
        let original_timestamp = event.timestamp();
        let deserialized_timestamp = deserialized.timestamp();
        let time_diff = if original_timestamp > deserialized_timestamp {
            original_timestamp.duration_since(deserialized_timestamp).unwrap()
        } else {
            deserialized_timestamp.duration_since(original_timestamp).unwrap()
        };
        prop_assert!(time_diff < Duration::from_millis(1), "Timestamp should be preserved");
    }

    /// Test that ProcessEvent fields are preserved through serialization.
    #[test]
    fn test_process_event_field_preservation(event in process_event_strategy()) {
        let collection_event = CollectionEvent::Process(event.clone());

        let json = serde_json::to_string(&collection_event)
            .expect("Serialization should succeed");

        let deserialized: CollectionEvent = serde_json::from_str(&json)
            .expect("Deserialization should succeed");

        if let CollectionEvent::Process(deserialized_event) = deserialized {
            prop_assert_eq!(event.pid, deserialized_event.pid);
            prop_assert_eq!(event.ppid, deserialized_event.ppid);
            prop_assert_eq!(event.name, deserialized_event.name);
            prop_assert_eq!(event.executable_path, deserialized_event.executable_path);
            prop_assert_eq!(event.command_line, deserialized_event.command_line);
            prop_assert_eq!(event.accessible, deserialized_event.accessible);
            prop_assert_eq!(event.file_exists, deserialized_event.file_exists);

            // Optional fields should be preserved (with floating point tolerance for CPU usage)
            match (event.cpu_usage, deserialized_event.cpu_usage) {
                (Some(orig), Some(deser)) => {
                    let diff = (orig - deser).abs();
                    prop_assert!(diff < 1e-10, "CPU usage values differ by more than tolerance: {} vs {}", orig, deser);
                }
                (None, None) => {},
                _ => prop_assert_eq!(event.cpu_usage, deserialized_event.cpu_usage),
            }
            prop_assert_eq!(event.memory_usage, deserialized_event.memory_usage);
            prop_assert_eq!(event.executable_hash, deserialized_event.executable_hash);
            prop_assert_eq!(event.user_id, deserialized_event.user_id);
        } else {
            prop_assert!(false, "Deserialized event should be ProcessEvent");
        }
    }

    /// Test SourceCaps bitflag operations preserve mathematical properties.
    #[test]
    fn test_source_caps_bitflag_properties(caps1 in source_caps_strategy(), caps2 in source_caps_strategy()) {
        // Test commutativity: A | B = B | A
        prop_assert_eq!(caps1 | caps2, caps2 | caps1);

        // Test associativity: (A | B) | C = A | (B | C)
        let caps3 = SourceCaps::PROCESS;
        prop_assert_eq!((caps1 | caps2) | caps3, caps1 | (caps2 | caps3));

        // Test identity: A | empty = A
        prop_assert_eq!(caps1 | SourceCaps::empty(), caps1);

        // Test idempotence: A | A = A
        prop_assert_eq!(caps1 | caps1, caps1);

        // Test intersection commutativity: A & B = B & A
        prop_assert_eq!(caps1 & caps2, caps2 & caps1);

        // Test intersection with self: A & A = A
        prop_assert_eq!(caps1 & caps1, caps1);

        // Test De Morgan's law: !(A | B) = !A & !B (within valid bits)
        let valid_mask = SourceCaps::all();
        let not_union = !(caps1 | caps2) & valid_mask;
        let intersection_not = (!caps1 & valid_mask) & (!caps2 & valid_mask);
        prop_assert_eq!(not_union, intersection_not);
    }

    /// Test that capability containment relationships are consistent.
    #[test]
    fn test_source_caps_containment_consistency(caps in source_caps_strategy()) {
        // If caps contains a specific capability, the union should still contain it
        if caps.contains(SourceCaps::PROCESS) {
            prop_assert!((caps | SourceCaps::NETWORK).contains(SourceCaps::PROCESS));
        }

        // Intersection should be subset of both operands
        let intersection = caps & SourceCaps::PROCESS;
        prop_assert!(caps.contains(intersection));
        prop_assert!(SourceCaps::PROCESS.contains(intersection));

        // Union should contain both operands
        let union = caps | SourceCaps::NETWORK;
        prop_assert!(union.contains(caps));
        prop_assert!(union.contains(SourceCaps::NETWORK));
    }

    /// Test that event timestamps are reasonable and within expected bounds.
    #[test]
    fn test_event_timestamp_bounds(event in collection_event_strategy()) {
        let timestamp = event.timestamp();

        // Timestamp should be after Unix epoch
        prop_assert!(timestamp >= UNIX_EPOCH);

        // Timestamp should be before some reasonable future date (year 2038)
        let max_timestamp = UNIX_EPOCH + Duration::from_secs(2_147_483_647);
        prop_assert!(timestamp <= max_timestamp);
    }

    /// Test that process events have consistent PID relationships.
    #[test]
    fn test_process_event_pid_consistency(event in process_event_strategy()) {
        // PID should be positive
        prop_assert!(event.pid > 0);

        // If PPID exists, it should be positive and different from PID
        if let Some(ppid) = event.ppid {
            prop_assert!(ppid > 0);
            // Allow PID == PPID for init processes or special cases
        }

        // Process name should not be empty
        prop_assert!(!event.name.is_empty());

        // If CPU usage is specified, it should be reasonable
        if let Some(cpu) = event.cpu_usage {
            prop_assert!(cpu >= 0.0);
            prop_assert!(cpu <= 100.0);
        }

        // If memory usage is specified, it should be positive
        if let Some(memory) = event.memory_usage {
            prop_assert!(memory > 0);
        }
    }

    /// Test that network events have valid address formats.
    #[test]
    fn test_network_event_address_consistency(event in network_event_strategy()) {
        // Connection ID should not be empty
        prop_assert!(!event.connection_id.is_empty());

        // Addresses should contain port numbers
        prop_assert!(event.source_addr.contains(':'));
        prop_assert!(event.dest_addr.contains(':'));

        // Protocol should be recognized
        prop_assert!(["TCP", "UDP", "ICMP"].contains(&event.protocol.as_str()));

        // State should be recognized
        prop_assert!(["ESTABLISHED", "LISTEN", "CLOSED"].contains(&event.state.as_str()));

        // Byte counts should be reasonable
        prop_assert!(event.bytes_sent <= 1_073_741_824); // <= 1GB
        prop_assert!(event.bytes_received <= 1_073_741_824); // <= 1GB
    }

    /// Test that filesystem events have valid paths and operations.
    #[test]
    fn test_filesystem_event_path_consistency(event in filesystem_event_strategy()) {
        // Path should start with /
        prop_assert!(event.path.starts_with('/'));

        // Path should not be empty
        prop_assert!(!event.path.is_empty());

        // Operation should be recognized
        prop_assert!(["create", "modify", "delete", "access"].contains(&event.operation.as_str()));

        // If size is specified, it should be reasonable
        if let Some(size) = event.size {
            prop_assert!(size <= 1_073_741_824); // <= 1GB
        }

        // If permissions are specified, they should be valid format
        if let Some(ref perms) = event.permissions {
            prop_assert_eq!(perms.len(), 9);
            for c in perms.chars() {
                prop_assert!(['r', 'w', 'x', '-'].contains(&c));
            }
        }
    }

    /// Test that performance events have valid metrics.
    #[test]
    fn test_performance_event_metric_consistency(event in performance_event_strategy()) {
        // Metric name should not be empty
        prop_assert!(!event.metric_name.is_empty());

        // Value should be finite
        prop_assert!(event.value.is_finite());

        // Unit should be recognized
        prop_assert!(["bytes", "percent", "count", "seconds"].contains(&event.unit.as_str()));

        // Component should be recognized
        prop_assert!(["CPU", "memory", "disk", "network"].contains(&event.component.as_str()));
    }

    /// Test JSON serialization produces valid JSON for all event types.
    #[test]
    fn test_json_serialization_validity(event in collection_event_strategy()) {
        let json = serde_json::to_string(&event)
            .expect("Serialization should succeed");

        // JSON should be valid
        let _: serde_json::Value = serde_json::from_str(&json)
            .expect("Generated JSON should be valid");

        // JSON should contain expected fields
        prop_assert!(json.contains("\"timestamp\""));

        match event {
            CollectionEvent::Process(_) => {
                prop_assert!(json.contains("\"Process\""));
                prop_assert!(json.contains("\"pid\""));
            }
            CollectionEvent::Network(_) => {
                prop_assert!(json.contains("\"Network\""));
                prop_assert!(json.contains("\"connection_id\""));
            }
            CollectionEvent::Filesystem(_) => {
                prop_assert!(json.contains("\"Filesystem\""));
                prop_assert!(json.contains("\"path\""));
            }
            CollectionEvent::Performance(_) => {
                prop_assert!(json.contains("\"Performance\""));
                prop_assert!(json.contains("\"metric_name\""));
            }
            CollectionEvent::TriggerRequest(_) => {
                prop_assert!(json.contains("\"TriggerRequest\""));
                prop_assert!(json.contains("\"trigger_id\""));
            }
        }
    }
}

#[cfg(test)]
mod deterministic_tests {
    use super::*;

    #[test]
    fn test_source_caps_all_individual_flags() {
        // Test that all individual flags are distinct
        let flags = [
            SourceCaps::PROCESS,
            SourceCaps::NETWORK,
            SourceCaps::FILESYSTEM,
            SourceCaps::PERFORMANCE,
            SourceCaps::REALTIME,
            SourceCaps::KERNEL_LEVEL,
            SourceCaps::SYSTEM_WIDE,
        ];

        for (i, &flag1) in flags.iter().enumerate() {
            for (j, &flag2) in flags.iter().enumerate() {
                if i != j {
                    assert_ne!(flag1, flag2, "Flags should be distinct");
                    assert_eq!(
                        flag1 & flag2,
                        SourceCaps::empty(),
                        "Flags should not overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn test_collection_event_type_consistency() {
        let timestamp = SystemTime::now();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test".to_string(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        });

        assert_eq!(process_event.event_type(), "process");
        assert_eq!(process_event.pid(), Some(1234));
        assert_eq!(process_event.timestamp(), timestamp);
    }
}
