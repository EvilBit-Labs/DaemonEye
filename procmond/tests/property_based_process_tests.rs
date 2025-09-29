//! Property-based tests for process enumeration edge cases.
//!
//! This module uses proptest to generate test cases that explore edge cases
//! and boundary conditions in process enumeration across different ProcessCollector
//! implementations, ensuring robust behavior under various scenarios.

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use proptest::prelude::*;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test timeout for property-based tests.
const PROPERTY_TEST_TIMEOUT_SECS: u64 = 60;

/// Property-based test for process collection configuration invariants.
#[tokio::test]
#[traced_test]
async fn test_process_collection_config_properties() {
    let rt = tokio::runtime::Runtime::new().unwrap();

    proptest!(|(
        collect_enhanced_metadata in any::<bool>(),
        compute_executable_hashes in any::<bool>(),
        skip_system_processes in any::<bool>(),
        skip_kernel_threads in any::<bool>(),
        max_processes in 1usize..=1000usize,
    )| {
        rt.block_on(async {
            let config = ProcessCollectionConfig {
                collect_enhanced_metadata,
                compute_executable_hashes,
                skip_system_processes,
                skip_kernel_threads,
                max_processes,
            };

            let collectors = create_all_available_collectors(config.clone());

            for (name, collector) in collectors {
                // Test that configuration is respected
                let result = timeout(
                    Duration::from_secs(PROPERTY_TEST_TIMEOUT_SECS),
                    collector.collect_processes(),
                )
                .await;

                if let Ok(Ok((events, stats))) = result {
                    // Property: max_processes limit is respected
                    assert!(
                        events.len() <= max_processes,
                        "Collector {} should respect max_processes limit: {} <= {}",
                        name,
                        events.len(),
                        max_processes
                    );

                    // Property: statistics are consistent
                    assert_eq!(
                        stats.total_processes,
                        stats.successful_collections + stats.inaccessible_processes + stats.invalid_processes,
                        "Statistics should be consistent for {}",
                        name
                    );

                    // Property: successful collections should match event count
                    assert_eq!(
                        stats.successful_collections,
                        events.len(),
                        "Successful collections should match event count for {}",
                        name
                    );

                    // Property: collection duration should be recorded
                    assert!(
                        stats.collection_duration_ms > 0,
                        "Collection duration should be recorded for {}",
                        name
                    );
                }
            }
        });
    });
}

/// Property-based test for process data validity invariants.
#[tokio::test]
#[traced_test]
async fn test_process_data_validity_properties() {
    let rt = tokio::runtime::Runtime::new().unwrap();

    proptest!(|(
        max_processes in 10usize..=100usize,
        collect_enhanced_metadata in any::<bool>(),
    )| {
        rt.block_on(async {
            let config = ProcessCollectionConfig {
                collect_enhanced_metadata,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes,
            };

            let collectors = create_all_available_collectors(config);

            for (name, collector) in collectors {
                let result = timeout(
                    Duration::from_secs(PROPERTY_TEST_TIMEOUT_SECS),
                    collector.collect_processes(),
                )
                .await;

                if let Ok(Ok((events, _stats))) = result {
                    for event in &events {
                        // Property: PID must be positive
                        assert!(
                            event.pid > 0,
                            "PID must be positive for {}: {}",
                            name,
                            event.pid
                        );

                        // Property: Process name must not be empty
                        assert!(
                            !event.name.is_empty(),
                            "Process name must not be empty for {}",
                            name
                        );

                        // Property: Process name must be reasonable length
                        assert!(
                            event.name.len() <= 1024,
                            "Process name must be reasonable length for {}: {}",
                            name,
                            event.name.len()
                        );

                        // Property: Timestamp must not be in the future
                        assert!(
                            event.timestamp <= SystemTime::now(),
                            "Event timestamp must not be in the future for {}",
                            name
                        );

                        // Property: PPID must be positive if present
                        if let Some(ppid) = event.ppid {
                            assert!(
                                ppid > 0,
                                "PPID must be positive if present for {}: {}",
                                name,
                                ppid
                            );
                        }

                        // Property: CPU usage must be non-negative if present
                        if let Some(cpu_usage) = event.cpu_usage {
                            assert!(
                                cpu_usage >= 0.0,
                                "CPU usage must be non-negative for {}: {}",
                                name,
                                cpu_usage
                            );
                        }

                        // Property: Memory usage must be positive if present
                        if let Some(memory_usage) = event.memory_usage {
                            assert!(
                                memory_usage > 0,
                                "Memory usage must be positive if present for {}: {}",
                                name,
                                memory_usage
                            );
                        }

                        // Property: Start time must not be in the future if present
                        if let Some(start_time) = event.start_time {
                            assert!(
                                start_time <= SystemTime::now(),
                                "Process start time must not be in the future for {}",
                                name
                            );

                            assert!(
                                start_time <= event.timestamp,
                                "Process start time must not be after collection time for {}",
                                name
                            );
                        }

                        // Property: Command line arguments should be reasonable
                        assert!(
                            event.command_line.len() <= 1000,
                            "Command line should have reasonable number of arguments for {}: {}",
                            name,
                            event.command_line.len()
                        );

                        for arg in &event.command_line {
                            assert!(
                                arg.len() <= 4096,
                                "Command line argument should be reasonable length for {}: {}",
                                name,
                                arg.len()
                            );
                        }
                    }
                }
            }
        });
    });
}

/// Property-based test for PID uniqueness invariants.
#[tokio::test]
#[traced_test]
async fn test_pid_uniqueness_properties() {
    let rt = tokio::runtime::Runtime::new().unwrap();

    proptest!(|(
        max_processes in 50usize..=200usize,
    )| {
        rt.block_on(async {
            let config = ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes,
            };

            let collectors = create_all_available_collectors(config);

            for (name, collector) in collectors {
                let result = timeout(
                    Duration::from_secs(PROPERTY_TEST_TIMEOUT_SECS),
                    collector.collect_processes(),
                )
                .await;

                if let Ok(Ok((events, _stats))) = result {
                    // Property: All PIDs should be unique within a single collection
                    let mut seen_pids = std::collections::HashSet::new();
                    for event in &events {
                        assert!(
                            seen_pids.insert(event.pid),
                            "PID {} should be unique within collection for {}",
                            event.pid,
                            name
                        );
                    }

                    // Property: PID distribution should be reasonable
                    if events.len() > 10 {
                        let min_pid = events.iter().map(|e| e.pid).min().unwrap();
                        let max_pid = events.iter().map(|e| e.pid).max().unwrap();

                        // PIDs should span a reasonable range (not all clustered)
                        assert!(
                            max_pid > min_pid,
                            "PIDs should span a range for {}: {} to {}",
                            name,
                            min_pid,
                            max_pid
                        );
                    }
                }
            }
        });
    });
}

/// Helper function to create all available collectors for property testing.
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
