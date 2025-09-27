//! Cross-platform integration tests for all ProcessCollector implementations.
//!
//! This test suite verifies that all ProcessCollector implementations work correctly
//! across different platforms and configurations, including privilege escalation/dropping
//! tests and compatibility tests for different OS versions.

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollectionError, ProcessCollector,
    SysinfoProcessCollector,
};
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test configuration constants for cross-platform testing.
const TEST_TIMEOUT_SECS: u64 = 30;
const TEST_MAX_PROCESSES_SMALL: usize = 10;
const TEST_MAX_PROCESSES_MEDIUM: usize = 50;
const TEST_MAX_PROCESSES_LARGE: usize = 100;
const TEST_CONCURRENT_TASKS: usize = 4;

/// Helper function to create a basic test configuration.
fn create_basic_test_config() -> ProcessCollectionConfig {
    ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: TEST_MAX_PROCESSES_MEDIUM,
    }
}

/// Helper function to create an enhanced test configuration.
fn create_enhanced_test_config() -> ProcessCollectionConfig {
    ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: true,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: TEST_MAX_PROCESSES_LARGE,
    }
}

/// Helper function to create a restrictive test configuration.
fn create_restrictive_test_config() -> ProcessCollectionConfig {
    ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: true,
        skip_kernel_threads: true,
        max_processes: TEST_MAX_PROCESSES_SMALL,
    }
}

/// Test that verifies basic functionality across all available collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_basic_functionality() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing basic functionality for collector: {}", name);

        // Test collector properties
        assert_eq!(collector.name(), name);
        let capabilities = collector.capabilities();
        assert!(
            capabilities.basic_info,
            "All collectors should support basic info"
        );

        // Test health check
        let health_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.health_check(),
        )
        .await;

        assert!(
            health_result.is_ok(),
            "Health check should complete within timeout for {}",
            name
        );

        let health_check = health_result.unwrap();
        assert!(
            health_check.is_ok(),
            "Health check should pass for {}: {:?}",
            name,
            health_check.err()
        );

        // Test process collection
        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Process collection should complete within timeout for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Process collection should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, stats) = collection.unwrap();
        assert!(
            !events.is_empty(),
            "Should collect at least one process for {}",
            name
        );
        assert!(
            stats.total_processes > 0,
            "Should find processes on the system for {}",
            name
        );
        assert!(
            stats.successful_collections > 0,
            "Should successfully collect some processes for {}",
            name
        );

        // Verify event data quality
        for event in &events {
            assert!(event.pid > 0, "Process PID should be valid for {}", name);
            assert!(
                !event.name.is_empty(),
                "Process name should not be empty for {}",
                name
            );
            assert!(
                event.accessible,
                "Collected processes should be accessible for {}",
                name
            );
            assert!(
                event.timestamp <= SystemTime::now(),
                "Timestamp should be reasonable for {}",
                name
            );
        }

        println!("✓ Basic functionality test passed for {}", name);
    }
}

/// Test enhanced metadata collection across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_enhanced_metadata() {
    let config = create_enhanced_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing enhanced metadata for collector: {}", name);

        let capabilities = collector.capabilities();
        if !capabilities.enhanced_metadata {
            println!(
                "Skipping enhanced metadata test for {} (not supported)",
                name
            );
            continue;
        }

        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Enhanced collection should complete within timeout for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Enhanced collection should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, _stats) = collection.unwrap();
        assert!(
            !events.is_empty(),
            "Should collect processes with enhanced metadata for {}",
            name
        );

        // Check for enhanced metadata in at least some processes
        let mut found_enhanced = false;
        for event in &events {
            if event.cpu_usage.is_some()
                || event.memory_usage.is_some()
                || event.start_time.is_some()
            {
                found_enhanced = true;
                break;
            }
        }

        assert!(
            found_enhanced,
            "Should find at least one process with enhanced metadata for {}",
            name
        );

        println!("✓ Enhanced metadata test passed for {}", name);
    }
}

/// Test current process collection across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_current_process() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);
    let current_pid = std::process::id();

    for (name, collector) in collectors {
        println!("Testing current process collection for collector: {}", name);

        let result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_process(current_pid),
        )
        .await;

        assert!(
            result.is_ok(),
            "Current process collection should complete within timeout for {}",
            name
        );

        let process_result = result.unwrap();
        assert!(
            process_result.is_ok(),
            "Should be able to collect current process for {}: {:?}",
            name,
            process_result.err()
        );

        let event = process_result.unwrap();
        assert_eq!(event.pid, current_pid, "PID should match for {}", name);
        assert!(
            !event.name.is_empty(),
            "Process name should not be empty for {}",
            name
        );
        assert!(
            event.accessible,
            "Current process should be accessible for {}",
            name
        );

        println!("✓ Current process test passed for {}", name);
    }
}

/// Test error handling for non-existent processes across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_nonexistent_process() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);
    let nonexistent_pid = 999999u32;

    for (name, collector) in collectors {
        println!(
            "Testing non-existent process handling for collector: {}",
            name
        );

        let result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_process(nonexistent_pid),
        )
        .await;

        assert!(
            result.is_ok(),
            "Non-existent process collection should complete within timeout for {}",
            name
        );

        let process_result = result.unwrap();
        assert!(
            process_result.is_err(),
            "Should fail for non-existent process for {}: {:?}",
            name,
            process_result.ok()
        );

        match process_result.unwrap_err() {
            ProcessCollectionError::ProcessNotFound { pid } => {
                assert_eq!(pid, nonexistent_pid, "Error PID should match for {}", name);
            }
            ProcessCollectionError::ProcessAccessDenied { pid, .. } => {
                assert_eq!(pid, nonexistent_pid, "Error PID should match for {}", name);
            }
            other => panic!(
                "Expected ProcessNotFound or ProcessAccessDenied for {}, got: {:?}",
                name, other
            ),
        }

        println!("✓ Non-existent process test passed for {}", name);
    }
}

/// Test concurrent access across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_concurrent_access() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing concurrent access for collector: {}", name);

        let collector = std::sync::Arc::new(collector);

        // Run multiple concurrent collections
        let mut handles = Vec::with_capacity(TEST_CONCURRENT_TASKS);
        for i in 0..TEST_CONCURRENT_TASKS {
            let collector_clone = std::sync::Arc::clone(&collector);
            let handle = tokio::spawn(async move {
                let result = collector_clone.collect_processes().await;
                (i, result)
            });
            handles.push(handle);
        }

        // Wait for all collections to complete
        let mut results = Vec::with_capacity(TEST_CONCURRENT_TASKS);
        for handle in handles {
            let task_result = timeout(Duration::from_secs(TEST_TIMEOUT_SECS), handle).await;

            assert!(
                task_result.is_ok(),
                "Concurrent task should complete within timeout for {}",
                name
            );

            let (i, result) = task_result.unwrap().unwrap();
            results.push((i, result));
        }

        // All collections should succeed
        for (i, result) in results {
            assert!(
                result.is_ok(),
                "Concurrent collection {} should succeed for {}: {:?}",
                i,
                name,
                result.err()
            );

            let (_events, stats) = result.unwrap();
            assert!(
                stats.successful_collections > 0,
                "Concurrent collection {} should find processes for {}",
                i,
                name
            );
        }

        println!("✓ Concurrent access test passed for {}", name);
    }
}

/// Test system process filtering across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_system_process_filtering() {
    let config = create_restrictive_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing system process filtering for collector: {}", name);

        let capabilities = collector.capabilities();
        if !capabilities.system_processes {
            println!(
                "Skipping system process filtering test for {} (not supported)",
                name
            );
            continue;
        }

        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Filtered collection should complete within timeout for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Filtered collection should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (_events, stats) = collection.unwrap();

        // Should still collect some processes (non-system ones)
        assert!(
            stats.successful_collections > 0 || stats.inaccessible_processes > 0,
            "Should find some processes even with filtering for {}",
            name
        );

        // Log filtering effectiveness
        println!(
            "Filtering results for {}: {} total, {} successful, {} inaccessible",
            name, stats.total_processes, stats.successful_collections, stats.inaccessible_processes
        );

        println!("✓ System process filtering test passed for {}", name);
    }
}

/// Test performance characteristics across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_performance() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false, // Disable for faster testing
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: TEST_MAX_PROCESSES_LARGE,
    };
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing performance for collector: {}", name);

        let start_time = std::time::Instant::now();
        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;
        let total_duration = start_time.elapsed();

        assert!(
            collection_result.is_ok(),
            "Performance test should complete within timeout for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Performance test should succeed for {}: {:?}",
            name,
            collection.err()
        );

        let (events, stats) = collection.unwrap();

        // Performance assertions - collect baseline metrics without strict requirements
        println!(
            "Performance metrics for {}: {} processes in {}ms (wall: {}ms), rate: {:.1} proc/sec",
            name,
            events.len(),
            stats.collection_duration_ms,
            total_duration.as_millis(),
            events.len() as f64 / total_duration.as_secs_f64()
        );

        // Basic sanity checks
        assert!(
            stats.collection_duration_ms > 0,
            "Should record collection duration for {}",
            name
        );
        assert!(
            total_duration.as_secs() < TEST_TIMEOUT_SECS,
            "Collection should complete within reasonable time for {}",
            name
        );

        println!("✓ Performance test passed for {}", name);
    }
}

/// Test error resilience across all collectors.
#[tokio::test]
#[traced_test]
async fn test_all_collectors_error_resilience() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing error resilience for collector: {}", name);

        // Test that the collector handles various error conditions gracefully
        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Resilience test should complete within timeout for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Basic collection should be resilient to errors for {}: {:?}",
            name,
            collection.err()
        );

        let (_events, stats) = collection.unwrap();

        // Even if some processes are inaccessible, we should get some results
        assert!(
            stats.total_processes > 0,
            "Should find processes on the system for {}",
            name
        );

        // The collector should handle individual process failures gracefully
        // This is reflected in the stats
        let success_rate = if stats.total_processes > 0 {
            stats.successful_collections as f64 / stats.total_processes as f64
        } else {
            0.0
        };

        println!(
            "Resilience metrics for {}: {} total, {} success, {} inaccessible, {} invalid, success rate: {:.1}%",
            name,
            stats.total_processes,
            stats.successful_collections,
            stats.inaccessible_processes,
            stats.invalid_processes,
            success_rate * 100.0
        );

        println!("✓ Error resilience test passed for {}", name);
    }
}

/// Helper function to create all available collectors for the current platform.
fn create_all_available_collectors(
    config: ProcessCollectionConfig,
) -> Vec<(&'static str, Box<dyn ProcessCollector>)> {
    let mut collectors: Vec<(&'static str, Box<dyn ProcessCollector>)> = Vec::new();

    // Always available: SysinfoProcessCollector
    collectors.push((
        "sysinfo-collector",
        Box::new(SysinfoProcessCollector::new(config.clone())),
    ));

    // Always available: FallbackProcessCollector
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

/// Test platform-specific capabilities and features.
#[tokio::test]
#[traced_test]
async fn test_platform_specific_capabilities() {
    let config = create_enhanced_test_config();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!(
            "Testing platform-specific capabilities for collector: {}",
            name
        );

        let capabilities = collector.capabilities();

        // Log capabilities for analysis
        println!(
            "Capabilities for {}: basic_info={}, enhanced_metadata={}, executable_hashing={}, system_processes={}, kernel_threads={}, realtime_collection={}",
            name,
            capabilities.basic_info,
            capabilities.enhanced_metadata,
            capabilities.executable_hashing,
            capabilities.system_processes,
            capabilities.kernel_threads,
            capabilities.realtime_collection
        );

        // All collectors should support basic info
        assert!(
            capabilities.basic_info,
            "All collectors should support basic info: {}",
            name
        );

        // Platform-specific capability validation
        match name {
            "sysinfo-collector" => {
                // Sysinfo should support most features across platforms
                assert!(
                    capabilities.enhanced_metadata,
                    "Sysinfo should support enhanced metadata"
                );
                assert!(
                    capabilities.realtime_collection,
                    "Sysinfo should support realtime collection"
                );
            }
            "fallback-collector" => {
                // Fallback should have basic capabilities
                assert!(
                    capabilities.basic_info,
                    "Fallback should support basic info"
                );
            }
            "linux-proc-collector" => {
                // Linux collector should have comprehensive capabilities
                assert!(
                    capabilities.enhanced_metadata,
                    "Linux collector should support enhanced metadata"
                );
                assert!(
                    capabilities.system_processes,
                    "Linux collector should support system processes"
                );
                assert!(
                    capabilities.kernel_threads,
                    "Linux collector should support kernel threads"
                );
            }
            "enhanced-macos-collector" => {
                // macOS collector should have enhanced capabilities
                assert!(
                    capabilities.enhanced_metadata,
                    "macOS collector should support enhanced metadata"
                );
                assert!(
                    capabilities.realtime_collection,
                    "macOS collector should support realtime collection"
                );
            }
            "windows-collector" => {
                // Windows collector should have comprehensive capabilities
                assert!(
                    capabilities.enhanced_metadata,
                    "Windows collector should support enhanced metadata"
                );
                assert!(
                    capabilities.system_processes,
                    "Windows collector should support system processes"
                );
            }
            _ => {
                // Unknown collector - just verify basic functionality
                assert!(
                    capabilities.basic_info,
                    "Unknown collector should support basic info"
                );
            }
        }

        println!("✓ Platform-specific capabilities test passed for {}", name);
    }
}

/// Test OS version compatibility (basic compatibility check).
#[tokio::test]
#[traced_test]
async fn test_os_version_compatibility() {
    let config = create_basic_test_config();
    let collectors = create_all_available_collectors(config);

    // Get OS information for compatibility testing
    let os_info = std::env::consts::OS;
    let arch_info = std::env::consts::ARCH;

    println!("Testing OS compatibility: {} on {}", os_info, arch_info);

    for (name, collector) in collectors {
        println!(
            "Testing OS compatibility for collector: {} on {} {}",
            name, os_info, arch_info
        );

        // Test that collector works on current OS/architecture combination
        let health_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.health_check(),
        )
        .await;

        assert!(
            health_result.is_ok(),
            "OS compatibility health check should complete for {} on {} {}",
            name,
            os_info,
            arch_info
        );

        let health_check = health_result.unwrap();
        assert!(
            health_check.is_ok(),
            "OS compatibility health check should pass for {} on {} {}: {:?}",
            name,
            os_info,
            arch_info,
            health_check.err()
        );

        // Test basic collection works
        let collection_result = timeout(
            Duration::from_secs(TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "OS compatibility collection should complete for {} on {} {}",
            name,
            os_info,
            arch_info
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "OS compatibility collection should succeed for {} on {} {}: {:?}",
            name,
            os_info,
            arch_info,
            collection.err()
        );

        println!(
            "✓ OS compatibility test passed for {} on {} {}",
            name, os_info, arch_info
        );
    }
}
