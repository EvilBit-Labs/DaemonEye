//! Linux-specific integration tests for the `LinuxProcessCollector`.
//!
//! These tests verify the Linux-specific functionality including /proc filesystem
//! access, capability detection, namespace handling, and container detection.
//!
//! # Test Coverage
//! - Basic collector creation and configuration
//! - Process enumeration and metadata collection
//! - Linux-specific features (namespaces, containers, memory maps)
//! - Error handling and permission management
//! - Performance and concurrency validation

#![cfg(target_os = "linux")]
#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::print_stdout,
    clippy::map_unwrap_or,
    clippy::non_ascii_literal,
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::panic,
    clippy::option_if_let_else,
    clippy::wildcard_enum_match_arm,
    clippy::missing_const_for_fn,
    clippy::match_wild_err_arm,
    clippy::single_match_else,
    clippy::clone_on_ref_ptr,
    clippy::let_underscore_must_use,
    clippy::ignored_unit_patterns,
    clippy::unreadable_literal,
    clippy::separated_literal_suffix,
    clippy::panic_in_result_fn,
    clippy::match_same_arms,
    clippy::unseparated_literal_suffix,
    clippy::pattern_type_mismatch
)]

use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};
use procmond::process_collector::{
    ProcessCollectionConfig, ProcessCollectionError, ProcessCollector,
};
use std::path::Path;
use std::time::SystemTime;

// Test configuration constants
const TEST_MAX_PROCESSES_SMALL: usize = 5;
const TEST_MAX_PROCESSES_MEDIUM: usize = 10;
const TEST_MAX_PROCESSES_LARGE: usize = 20;
const TEST_MAX_PROCESSES_PERFORMANCE: usize = 100;
const TEST_CONCURRENT_TASKS: usize = 3;
const TEST_PERFORMANCE_TIMEOUT_SECS: u64 = 10;
const TEST_LOW_PID_THRESHOLD: u32 = 10;

/// Helper function to check if we're running on Linux.
///
/// This is redundant with the `#![cfg(target_os = "linux")]` attribute
/// but provides runtime checking for test clarity.
fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// Helper function to check if /proc filesystem is available.
///
/// Returns `true` if the /proc directory exists and is accessible.
fn proc_available() -> bool {
    Path::new("/proc").exists()
}

/// Helper function to check if Linux-specific tests should run.
///
/// Returns `true` if we're on Linux and /proc is available, otherwise
/// prints a skip message and returns `false`.
fn should_run_linux_test() -> bool {
    if !is_linux() {
        println!("Skipping Linux-specific test on non-Linux platform");
        return false;
    }
    if !proc_available() {
        println!("Skipping Linux /proc test - not available");
        return false;
    }
    true
}

/// Helper function to create a test Linux collector with default configuration.
///
/// # Panics
/// Panics if the collector cannot be created, which indicates a fundamental
/// configuration or system issue that should fail the test.
fn create_test_collector() -> LinuxProcessCollector {
    let base_config = ProcessCollectionConfig::default();
    let linux_config = LinuxCollectorConfig::default();
    LinuxProcessCollector::new(base_config, linux_config)
        .expect("Failed to create test collector with default configuration")
}

/// Helper function to create a test Linux collector with custom configuration.
///
/// # Arguments
/// * `base_config` - Base process collection configuration
/// * `linux_config` - Linux-specific collector configuration
///
/// # Panics
/// Panics if the collector cannot be created with the provided configuration.
fn create_custom_collector(
    base_config: ProcessCollectionConfig,
    linux_config: LinuxCollectorConfig,
) -> LinuxProcessCollector {
    LinuxProcessCollector::new(base_config, linux_config)
        .expect("Failed to create custom test collector")
}

#[tokio::test]
async fn test_linux_collector_creation() {
    if !is_linux() {
        println!("Skipping Linux-specific test on non-Linux platform");
        return;
    }

    let collector = create_test_collector();
    assert_eq!(collector.name(), "linux-proc-collector");

    let capabilities = collector.capabilities();
    assert!(capabilities.basic_info);
    assert!(capabilities.enhanced_metadata);
    assert!(capabilities.realtime_collection);
}

#[tokio::test]
async fn test_linux_collector_health_check() {
    if !should_run_linux_test() {
        return;
    }

    let collector = create_test_collector();
    let result = collector.health_check().await;

    assert!(
        result.is_ok(),
        "Health check should pass on Linux with /proc: {:?}",
        result
    );
}

#[tokio::test]
async fn test_linux_collector_basic_process_collection() {
    if !should_run_linux_test() {
        return;
    }

    let collector = create_test_collector();
    let result = collector.collect_processes().await;

    assert!(
        result.is_ok(),
        "Process collection should succeed on Linux: {:?}",
        result
    );

    let (events, stats) = result.unwrap();
    assert!(!events.is_empty(), "Should collect at least one process");
    assert!(
        stats.total_processes > 0,
        "Should find processes on the system"
    );
    assert!(
        stats.successful_collections > 0,
        "Should successfully collect some processes"
    );

    // Verify Linux-specific event data quality
    let mut accessible_count = 0;
    for event in &events {
        assert!(event.pid > 0, "Process PID should be valid");
        assert!(!event.name.is_empty(), "Process name should not be empty");
        if event.accessible {
            accessible_count += 1;
        }
        assert!(
            event.timestamp <= SystemTime::now(),
            "Timestamp should be reasonable"
        );

        // Linux-specific validations
        if let Some(ppid) = event.ppid {
            assert!(ppid > 0, "Parent PID should be valid if present");
        }
    }

    // Assert that at least one collected event was accessible
    assert!(
        accessible_count > 0,
        "At least one collected process should be accessible"
    );
}

#[tokio::test]
async fn test_linux_collector_enhanced_metadata() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        max_processes: TEST_MAX_PROCESSES_MEDIUM,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig {
        collect_namespaces: true,
        collect_memory_maps: true,
        collect_file_descriptors: true,
        detect_containers: true,
        ..Default::default()
    };

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(result.is_ok(), "Enhanced collection should succeed");

    let (events, _stats) = result.unwrap();
    assert!(
        !events.is_empty(),
        "Should collect processes with enhanced metadata"
    );

    // Check that enhanced metadata is present for at least some processes
    let mut found_enhanced = false;
    for event in &events {
        if event.memory_usage.is_some() || event.start_time.is_some() {
            found_enhanced = true;
            break;
        }
    }

    assert!(
        found_enhanced,
        "Should find at least one process with enhanced metadata"
    );
}

#[tokio::test]
async fn test_linux_collector_current_process() {
    if !should_run_linux_test() {
        return;
    }

    let collector = create_test_collector();
    let current_pid = std::process::id();
    let result = collector.collect_process(current_pid).await;

    assert!(
        result.is_ok(),
        "Should be able to collect current process: {:?}",
        result
    );

    let event = result.unwrap();
    assert_eq!(event.pid, current_pid);
    assert!(!event.name.is_empty());
    assert!(event.accessible);

    // Linux-specific checks for current process
    assert!(event.ppid.is_some(), "Current process should have a parent");
    if let Some(exe_path) = &event.executable_path {
        assert!(
            Path::new(exe_path).exists(),
            "Executable path should exist: {}",
            exe_path
        );
    }
}

#[tokio::test]
async fn test_linux_collector_nonexistent_process() {
    if !should_run_linux_test() {
        return;
    }

    let collector = create_test_collector();
    let nonexistent_pid = 999999u32;
    let result = collector.collect_process(nonexistent_pid).await;

    assert!(
        result.is_err(),
        "Should fail for non-existent process: {:?}",
        result
    );

    match result.unwrap_err() {
        ProcessCollectionError::ProcessNotFound { pid } => {
            assert_eq!(pid, nonexistent_pid);
        }
        ProcessCollectionError::ProcessAccessDenied { pid, .. } => {
            assert_eq!(pid, nonexistent_pid);
        }
        other => panic!(
            "Expected ProcessNotFound or ProcessAccessDenied, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_linux_collector_capability_detection() {
    if !should_run_linux_test() {
        return;
    }

    // Test with explicit capability settings
    let base_config = ProcessCollectionConfig::default();
    let linux_config_no_cap = LinuxCollectorConfig {
        use_cap_sys_ptrace: Some(false),
        ..Default::default()
    };

    let collector_no_cap = create_custom_collector(base_config.clone(), linux_config_no_cap);
    assert_eq!(collector_no_cap.name(), "linux-proc-collector");

    // Test auto-detection (None)
    let linux_config_auto = LinuxCollectorConfig {
        use_cap_sys_ptrace: None,
        ..Default::default()
    };

    let collector_auto = create_custom_collector(base_config, linux_config_auto);
    assert_eq!(collector_auto.name(), "linux-proc-collector");

    // Both should be able to perform basic collection
    let result1 = collector_no_cap.collect_processes().await;
    let result2 = collector_auto.collect_processes().await;

    assert!(
        result1.is_ok(),
        "Collection without CAP_SYS_PTRACE should work"
    );
    assert!(
        result2.is_ok(),
        "Collection with auto-detection should work"
    );
}

#[tokio::test]
async fn test_linux_collector_namespace_collection() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        max_processes: TEST_MAX_PROCESSES_SMALL,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig {
        collect_namespaces: true,
        detect_containers: true,
        ..Default::default()
    };

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(result.is_ok(), "Namespace collection should succeed");

    let (events, _stats) = result.unwrap();
    assert!(!events.is_empty(), "Should collect processes");

    // The namespace information is stored in enhanced metadata
    // which is not directly exposed in ProcessEvent, but the collection should succeed
}

#[tokio::test]
async fn test_linux_collector_container_detection() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        max_processes: TEST_MAX_PROCESSES_MEDIUM,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig {
        detect_containers: true,
        collect_namespaces: true,
        ..Default::default()
    };

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(
        result.is_ok(),
        "Container detection should not fail collection"
    );

    let (events, _stats) = result.unwrap();
    assert!(!events.is_empty(), "Should collect processes");

    // Container detection runs internally but doesn't directly affect ProcessEvent
    // The test verifies that enabling container detection doesn't break collection
}

#[tokio::test]
async fn test_linux_collector_memory_maps_collection() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        max_processes: TEST_MAX_PROCESSES_SMALL,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig {
        collect_memory_maps: true,
        ..Default::default()
    };

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(result.is_ok(), "Memory maps collection should succeed");

    let (events, _stats) = result.unwrap();
    assert!(!events.is_empty(), "Should collect processes");

    // Memory maps information is collected internally but not exposed in ProcessEvent
    // The test verifies that enabling memory maps collection doesn't break the process
}

#[tokio::test]
async fn test_linux_collector_file_descriptors_collection() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        max_processes: TEST_MAX_PROCESSES_SMALL,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig {
        collect_file_descriptors: true,
        ..Default::default()
    };

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(result.is_ok(), "File descriptors collection should succeed");

    let (events, _stats) = result.unwrap();
    assert!(!events.is_empty(), "Should collect processes");

    // File descriptor information is collected internally but not exposed in ProcessEvent
    // The test verifies that enabling FD collection doesn't break the process
}

#[tokio::test]
async fn test_linux_collector_proc_filesystem_access() {
    if !should_run_linux_test() {
        return;
    }

    // Verify that we can access basic /proc entries
    assert!(Path::new("/proc/self").exists(), "/proc/self should exist");
    assert!(
        Path::new("/proc/self/stat").exists(),
        "/proc/self/stat should exist"
    );
    assert!(
        Path::new("/proc/self/status").exists(),
        "/proc/self/status should exist"
    );
    assert!(
        Path::new("/proc/self/cmdline").exists(),
        "/proc/self/cmdline should exist"
    );

    // Test that the collector can handle /proc access
    let collector = create_test_collector();
    let current_pid = std::process::id();
    let result = collector.collect_process(current_pid).await;

    assert!(
        result.is_ok(),
        "Should be able to collect process via /proc access: {:?}",
        result
    );
}

#[tokio::test]
async fn test_linux_collector_permission_handling() {
    if !should_run_linux_test() {
        return;
    }

    let collector = create_test_collector();
    let result = collector.collect_processes().await;

    assert!(
        result.is_ok(),
        "Collection should handle permissions gracefully"
    );

    let (_events, stats) = result.unwrap();

    // We should have some successful collections and possibly some inaccessible ones
    assert!(
        stats.successful_collections > 0,
        "Should have some successful collections"
    );

    // The total should be the sum of all categories
    assert_eq!(
        stats.total_processes,
        stats.successful_collections + stats.inaccessible_processes + stats.invalid_processes,
        "Stats should be consistent"
    );
}

#[tokio::test]
async fn test_linux_collector_system_process_filtering() {
    if !should_run_linux_test() {
        return;
    }

    // Test with system process filtering enabled
    let base_config = ProcessCollectionConfig {
        skip_system_processes: true,
        max_processes: TEST_MAX_PROCESSES_LARGE,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig::default();

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(
        result.is_ok(),
        "Collection with system process filtering should work"
    );

    let (events, stats) = result.unwrap();

    // Should still collect some processes (non-system ones)
    assert!(
        stats.successful_collections > 0 || stats.inaccessible_processes > 0,
        "Should find some processes even with system filtering"
    );

    // Verify that very low PID processes (likely system processes) are filtered out
    let low_pid_count = events
        .iter()
        .filter(|e| e.pid < TEST_LOW_PID_THRESHOLD)
        .count();
    assert!(
        low_pid_count == 0,
        "Should filter out very low PID system processes"
    );
}

#[tokio::test]
async fn test_linux_collector_kernel_thread_filtering() {
    if !should_run_linux_test() {
        return;
    }

    // Test with kernel thread filtering enabled
    let base_config = ProcessCollectionConfig {
        skip_kernel_threads: true,
        max_processes: TEST_MAX_PROCESSES_LARGE,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig::default();

    let collector = create_custom_collector(base_config, linux_config);
    let result = collector.collect_processes().await;

    assert!(
        result.is_ok(),
        "Collection with kernel thread filtering should work"
    );

    let (events, stats) = result.unwrap();

    // Should still collect some processes (non-kernel threads)
    assert!(
        stats.successful_collections > 0 || stats.inaccessible_processes > 0,
        "Should find some processes even with kernel thread filtering"
    );

    // Verify that processes with bracket names (typical kernel threads) are filtered
    let bracket_name_count = events
        .iter()
        .filter(|e| e.name.starts_with('[') && e.name.ends_with(']'))
        .count();
    assert!(
        bracket_name_count == 0,
        "Should filter out kernel threads with bracket names"
    );
}

#[tokio::test]
async fn test_linux_collector_performance() {
    if !should_run_linux_test() {
        return;
    }

    let base_config = ProcessCollectionConfig {
        max_processes: TEST_MAX_PROCESSES_PERFORMANCE,
        ..Default::default()
    };
    let linux_config = LinuxCollectorConfig::default();

    let collector = create_custom_collector(base_config, linux_config);

    let start_time = std::time::Instant::now();
    let result = collector.collect_processes().await;
    let duration = start_time.elapsed();

    assert!(result.is_ok(), "Performance test collection should succeed");

    let (_events, stats) = result.unwrap();

    // Performance assertions
    assert!(
        duration.as_secs() < TEST_PERFORMANCE_TIMEOUT_SECS,
        "Collection should complete within {} seconds, took: {:?}",
        TEST_PERFORMANCE_TIMEOUT_SECS,
        duration
    );

    assert!(
        stats.collection_duration_ms > 0,
        "Should record collection duration"
    );

    // Log performance metrics for analysis
    eprintln!(
        "Performance metrics - Processes: {}, Wall time: {}ms, Reported: {}ms, Rate: {:.1} proc/sec",
        stats.total_processes,
        duration.as_millis(),
        stats.collection_duration_ms,
        stats.total_processes as f64 / duration.as_secs_f64()
    );
}

#[tokio::test]
async fn test_linux_collector_concurrent_access() {
    if !should_run_linux_test() {
        return;
    }

    let collector = std::sync::Arc::new(create_test_collector());

    // Run multiple concurrent collections
    let mut handles = Vec::with_capacity(TEST_CONCURRENT_TASKS);
    for i in 0..TEST_CONCURRENT_TASKS {
        let collector_clone = collector.clone();
        let handle = tokio::spawn(async move {
            let result = collector_clone.collect_processes().await;
            (i, result)
        });
        handles.push(handle);
    }

    // Wait for all collections to complete
    let mut results = Vec::with_capacity(TEST_CONCURRENT_TASKS);
    for handle in handles {
        let (i, result) = handle.await.expect("Task should complete");
        results.push((i, result));
    }

    // All collections should succeed
    for (i, result) in results {
        assert!(
            result.is_ok(),
            "Concurrent collection {} should succeed: {:?}",
            i,
            result
        );

        let (_events, stats) = result.unwrap();
        assert!(
            stats.successful_collections > 0,
            "Concurrent collection {} should find processes",
            i
        );
    }
}

#[tokio::test]
async fn test_linux_collector_error_resilience() {
    if !should_run_linux_test() {
        return;
    }

    // Test that the collector handles various error conditions gracefully
    let collector = create_test_collector();

    // Test collection with a reasonable limit
    let result = collector.collect_processes().await;
    assert!(
        result.is_ok(),
        "Basic collection should be resilient to errors"
    );

    let (_events, stats) = result.unwrap();

    // Even if some processes are inaccessible, we should get some results
    assert!(
        stats.total_processes > 0,
        "Should find processes on the system"
    );

    // The collector should handle individual process failures gracefully
    // This is reflected in the stats
    eprintln!(
        "Resilience metrics - Total: {}, Success: {}, Inaccessible: {}, Invalid: {}, Success rate: {:.1}%",
        stats.total_processes,
        stats.successful_collections,
        stats.inaccessible_processes,
        stats.invalid_processes,
        (stats.successful_collections as f64 / stats.total_processes as f64) * 100.0
    );
}
