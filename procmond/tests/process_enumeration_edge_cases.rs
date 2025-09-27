//! Edge case tests for process enumeration.
//!
//! This module tests edge cases and boundary conditions in process enumeration
//! across different ProcessCollector implementations without using property-based testing.

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test timeout for edge case operations.
const EDGE_CASE_TEST_TIMEOUT_SECS: u64 = 30;

/// Test edge cases with very small process limits.
#[tokio::test]
#[traced_test]
async fn test_minimal_process_limits() {
    let configs = vec![
        (
            "limit_1",
            ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 1,
            },
        ),
        (
            "limit_5",
            ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 5,
            },
        ),
    ];

    for (config_name, config) in configs {
        println!("Testing minimal process limits: {}", config_name);

        let collectors = create_all_available_collectors(config.clone());

        for (name, collector) in collectors {
            let result = timeout(
                Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS),
                collector.collect_processes(),
            )
            .await;

            assert!(
                result.is_ok(),
                "Minimal limit test should complete for {} with {}",
                name,
                config_name
            );

            let collection = result.unwrap();
            assert!(
                collection.is_ok(),
                "Minimal limit test should succeed for {} with {}: {:?}",
                name,
                config_name,
                collection.err()
            );

            let (events, _stats) = collection.unwrap();

            // Should respect the limit
            assert!(
                events.len() <= config.max_processes,
                "Should respect max_processes limit for {} with {}",
                name,
                config_name
            );

            // Should still find some processes
            assert!(
                !events.is_empty(),
                "Should find at least one process for {} with {}",
                name,
                config_name
            );

            println!(
                "✓ Minimal limit test passed for {} with {} ({} processes)",
                name,
                config_name,
                events.len()
            );
        }
    }
}

/// Test edge cases with all filtering options enabled.
#[tokio::test]
#[traced_test]
async fn test_maximum_filtering() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: true,
        skip_kernel_threads: true,
        max_processes: 10,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing maximum filtering for collector: {}", name);

        let result = timeout(
            Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            result.is_ok(),
            "Maximum filtering test should complete for {}",
            name
        );

        let collection = result.unwrap();
        assert!(
            collection.is_ok(),
            "Maximum filtering test should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, stats) = collection.unwrap();

        // Should still collect some processes even with heavy filtering
        assert!(
            stats.total_processes > 0,
            "Should attempt to process some processes for {}",
            name
        );

        // Verify no obvious system processes are included
        for event in &events {
            // Basic checks - no processes with very low PIDs (likely system)
            if event.pid <= 10 {
                println!(
                    "Warning: Found low PID {} in filtered results for {}",
                    event.pid, name
                );
            }
        }

        println!(
            "✓ Maximum filtering test passed for {} ({} total, {} successful)",
            name, stats.total_processes, stats.successful_collections
        );
    }
}

/// Test edge cases with all features enabled.
#[tokio::test]
#[traced_test]
async fn test_maximum_features() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: true,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 50,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing maximum features for collector: {}", name);

        let result = timeout(
            Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            result.is_ok(),
            "Maximum features test should complete for {}",
            name
        );

        let collection = result.unwrap();
        assert!(
            collection.is_ok(),
            "Maximum features test should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, _stats) = collection.unwrap();

        // Should collect processes
        assert!(
            !events.is_empty(),
            "Should collect processes with maximum features for {}",
            name
        );

        // Check for enhanced metadata when supported
        let capabilities = collector.capabilities();
        if capabilities.enhanced_metadata {
            let has_enhanced = events.iter().any(|e| {
                e.cpu_usage.is_some() || e.memory_usage.is_some() || e.start_time.is_some()
            });

            if has_enhanced {
                println!("✓ Enhanced metadata found for {}", name);
            }
        }

        // Check for hashes when supported
        if capabilities.executable_hashing {
            let has_hashes = events.iter().any(|e| e.executable_hash.is_some());

            if has_hashes {
                println!("✓ Executable hashes found for {}", name);
            }
        }

        println!(
            "✓ Maximum features test passed for {} ({} processes)",
            name,
            events.len()
        );
    }
}

/// Test data consistency invariants.
#[tokio::test]
#[traced_test]
async fn test_data_consistency_invariants() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 100,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing data consistency for collector: {}", name);

        let result = timeout(
            Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            result.is_ok(),
            "Data consistency test should complete for {}",
            name
        );

        let collection = result.unwrap();
        assert!(
            collection.is_ok(),
            "Data consistency test should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, stats) = collection.unwrap();

        // Verify statistics consistency
        assert_eq!(
            stats.total_processes,
            stats.successful_collections + stats.inaccessible_processes + stats.invalid_processes,
            "Statistics should be consistent for {}",
            name
        );

        // Verify PID uniqueness
        let mut seen_pids = HashSet::new();
        for event in &events {
            assert!(
                seen_pids.insert(event.pid),
                "PIDs should be unique within collection for {}",
                name
            );
        }

        // Verify data validity
        for event in &events {
            // PID validity
            assert!(event.pid > 0, "PID should be positive for {}", name);

            // Name validity
            assert!(
                !event.name.is_empty(),
                "Process name should not be empty for {}",
                name
            );
            assert!(
                event.name.len() <= 1024,
                "Process name should be reasonable length for {}",
                name
            );

            // Timestamp validity
            assert!(
                event.timestamp <= SystemTime::now(),
                "Event timestamp should not be in the future for {}",
                name
            );

            // PPID validity
            if let Some(ppid) = event.ppid {
                assert!(ppid > 0, "PPID should be positive if present for {}", name);
            }

            // CPU usage validity
            if let Some(cpu_usage) = event.cpu_usage {
                assert!(
                    cpu_usage >= 0.0,
                    "CPU usage should not be negative for {}",
                    name
                );
            }

            // Memory usage validity
            if let Some(memory_usage) = event.memory_usage {
                assert!(
                    memory_usage > 0,
                    "Memory usage should be positive if present for {}",
                    name
                );
            }

            // Start time validity
            if let Some(start_time) = event.start_time {
                assert!(
                    start_time <= SystemTime::now(),
                    "Process start time should not be in the future for {}",
                    name
                );
                assert!(
                    start_time <= event.timestamp,
                    "Process start time should not be after collection time for {}",
                    name
                );
            }

            // Accessibility consistency
            assert!(
                event.accessible,
                "All collected events should be marked as accessible for {}",
                name
            );
        }

        println!(
            "✓ Data consistency test passed for {} ({} processes)",
            name,
            events.len()
        );
    }
}

/// Test concurrent collection behavior.
#[tokio::test]
#[traced_test]
async fn test_concurrent_collection_edge_cases() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 50,
    };

    let collectors = create_all_available_collectors(config.clone());

    for (name, collector) in collectors {
        println!("Testing concurrent collection for collector: {}", name);

        let collector = std::sync::Arc::new(collector);

        // Run multiple concurrent collections
        let concurrent_tasks = 3;
        let mut handles = Vec::with_capacity(concurrent_tasks);

        for i in 0..concurrent_tasks {
            let collector_clone = std::sync::Arc::clone(&collector);
            let handle = tokio::spawn(async move {
                let result = collector_clone.collect_processes().await;
                (i, result)
            });
            handles.push(handle);
        }

        // Wait for all collections to complete
        let mut results = Vec::with_capacity(concurrent_tasks);
        for handle in handles {
            let task_result =
                timeout(Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS), handle).await;

            assert!(
                task_result.is_ok(),
                "Concurrent task should complete within timeout for {}",
                name
            );

            let (i, result) = task_result.unwrap().unwrap();
            results.push((i, result));
        }

        // Analyze results
        let mut success_count = 0;
        let mut error_count = 0;

        for (i, result) in results {
            match result {
                Ok((events, stats)) => {
                    success_count += 1;

                    // Verify concurrent collection results are reasonable
                    assert!(
                        events.len() <= config.max_processes,
                        "Concurrent collection {} should respect max_processes for {}",
                        i,
                        name
                    );

                    assert_eq!(
                        stats.total_processes,
                        stats.successful_collections
                            + stats.inaccessible_processes
                            + stats.invalid_processes,
                        "Concurrent collection {} statistics should be consistent for {}",
                        i,
                        name
                    );
                }
                Err(_) => {
                    error_count += 1;
                }
            }
        }

        // At least some collections should succeed
        assert!(
            success_count > 0,
            "At least some concurrent collections should succeed for {}",
            name
        );

        println!(
            "✓ Concurrent collection test passed for {} ({} success, {} errors)",
            name, success_count, error_count
        );
    }
}

/// Test single process collection edge cases.
#[tokio::test]
#[traced_test]
async fn test_single_process_edge_cases() {
    let config = ProcessCollectionConfig::default();
    let collectors = create_all_available_collectors(config);

    // Test PIDs to try
    let test_pids = vec![
        1u32,               // init process
        std::process::id(), // current process
        999999u32,          // likely non-existent PID
    ];

    for (name, collector) in collectors {
        println!("Testing single process edge cases for collector: {}", name);

        for &pid in &test_pids {
            let result = timeout(
                Duration::from_secs(EDGE_CASE_TEST_TIMEOUT_SECS),
                collector.collect_process(pid),
            )
            .await;

            assert!(
                result.is_ok(),
                "Single process collection should complete for {} with PID {}",
                name,
                pid
            );

            let process_result = result.unwrap();

            match process_result {
                Ok(event) => {
                    // If successful, validate the event
                    assert_eq!(event.pid, pid, "Returned PID should match for {}", name);
                    assert!(
                        !event.name.is_empty(),
                        "Process name should not be empty for {}",
                        name
                    );
                    assert!(
                        event.accessible,
                        "Successfully collected process should be accessible for {}",
                        name
                    );

                    println!("✓ Successfully collected PID {} for {}", pid, name);
                }
                Err(err) => {
                    // If failed, error should be appropriate
                    match err {
                        procmond::process_collector::ProcessCollectionError::ProcessNotFound { pid: error_pid } => {
                            assert_eq!(error_pid, pid, "Error PID should match for {}", name);
                            println!("✓ Correctly failed to find PID {} for {}", pid, name);
                        }
                        procmond::process_collector::ProcessCollectionError::ProcessAccessDenied { pid: error_pid, .. } => {
                            assert_eq!(error_pid, pid, "Error PID should match for {}", name);
                            println!("✓ Correctly denied access to PID {} for {}", pid, name);
                        }
                        _ => {
                            println!("Unexpected error for {} with PID {}: {:?}", name, pid, err);
                        }
                    }
                }
            }
        }

        println!("✓ Single process edge cases test passed for {}", name);
    }
}

/// Helper function to create all available collectors for edge case testing.
fn create_all_available_collectors(
    config: ProcessCollectionConfig,
) -> Vec<(&'static str, Box<dyn ProcessCollector>)> {
    let mut collectors: Vec<(&'static str, Box<dyn ProcessCollector>)> = Vec::new();

    // Always available collectors
    collectors.push((
        "sysinfo-collector",
        Box::new(SysinfoProcessCollector::new(config.clone())),
    ));

    collectors.push((
        "fallback-collector",
        Box::new(FallbackProcessCollector::new(config.clone())),
    ));

    // Platform-specific collectors
    #[cfg(target_os = "linux")]
    {
        let linux_config = LinuxCollectorConfig::default();
        if let Ok(linux_collector) = LinuxProcessCollector::new(config.clone(), linux_config) {
            collectors.push(("linux-proc-collector", Box::new(linux_collector)));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let macos_config = MacOSCollectorConfig::default();
        if let Ok(macos_collector) = EnhancedMacOSCollector::new(config.clone(), macos_config) {
            collectors.push(("enhanced-macos-collector", Box::new(macos_collector)));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let windows_config = WindowsCollectorConfig::default();
        if let Ok(windows_collector) = WindowsProcessCollector::new(config.clone(), windows_config)
        {
            collectors.push(("windows-collector", Box::new(windows_collector)));
        }
    }

    collectors
}
