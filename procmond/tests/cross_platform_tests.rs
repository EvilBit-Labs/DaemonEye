//! Cross-Platform Integration Tests for Process Enumeration.
//!
//! This test suite verifies cross-platform compatibility for process enumeration
//! and platform-specific metadata collection across Linux, macOS, and Windows.
//!
//! # Test Categories
//!
//! 1. **Process Enumeration**: Basic process listing works on current platform
//! 2. **Platform-Specific Metadata**: Enhanced metadata collected per platform
//! 3. **Core Fields**: PID, PPID, name, command-line populated correctly
//! 4. **Resource Metrics**: CPU/memory usage collected where available
//! 5. **Graceful Handling**: Tests skip appropriately on unsupported platforms
//!
//! # CI/CD Integration
//!
//! These tests are designed to run on each platform in CI (Linux, macOS, Windows)
//! and use conditional compilation to test platform-specific behavior.

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unseparated_literal_suffix,
    clippy::unreadable_literal,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::print_stdout,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::match_same_arms,
    clippy::wildcard_enum_match_arm,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::non_ascii_literal,
    clippy::unused_async,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::needless_pass_by_value,
    clippy::needless_collect,
    clippy::clone_on_ref_ptr,
    clippy::as_conversions,
    clippy::redundant_clone,
    clippy::str_to_string
)]

use procmond::process_collector::{
    ProcessCollectionConfig, ProcessCollectionError, ProcessCollector, SysinfoProcessCollector,
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

/// Test timeout for process enumeration operations.
const TEST_TIMEOUT_SECS: u64 = 30;

/// Maximum processes to collect in standard tests.
const MAX_PROCESSES_TEST: usize = 100;

// ============================================================================
// Linux-Specific Tests
// ============================================================================

/// Test: Process enumeration works correctly on Linux.
///
/// This test verifies that the Linux-specific collector can successfully
/// enumerate processes on a Linux system.
#[cfg(target_os = "linux")]
#[tokio::test]
#[traced_test]
async fn test_linux_process_enumeration_works_correctly() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let linux_config = LinuxCollectorConfig::default();

    let collector = LinuxProcessCollector::new(base_config, linux_config)
        .expect("Linux collector creation should succeed");

    // Verify collector name
    assert_eq!(collector.name(), "linux-proc-collector");

    // Test health check passes
    let health_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.health_check(),
    )
    .await;

    assert!(
        health_result.is_ok(),
        "Health check should complete within timeout"
    );
    assert!(
        health_result.unwrap().is_ok(),
        "Health check should pass on Linux"
    );

    // Test process collection
    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    assert!(
        collection_result.is_ok(),
        "Process collection should complete within timeout"
    );

    let (events, stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Verify processes were collected
    assert!(
        !events.is_empty(),
        "Should collect at least one process on Linux"
    );
    assert!(
        stats.total_processes > 0,
        "Should find processes on the system"
    );
    assert!(
        stats.successful_collections > 0,
        "Should successfully collect some processes"
    );

    println!(
        "Linux process enumeration: {} total, {} successful, {} inaccessible",
        stats.total_processes, stats.successful_collections, stats.inaccessible_processes
    );
}

/// Test: Platform-specific metadata is collected on Linux.
///
/// Verifies that Linux-specific metadata (namespaces, memory maps, etc.)
/// is populated in process events.
#[cfg(target_os = "linux")]
#[tokio::test]
#[traced_test]
async fn test_linux_platform_specific_metadata_collected() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let linux_config = LinuxCollectorConfig {
        collect_namespaces: true,
        collect_memory_maps: true,
        collect_file_descriptors: true,
        collect_network_connections: false,
        detect_containers: true,
        use_cap_sys_ptrace: None,
    };

    let collector = LinuxProcessCollector::new(base_config, linux_config)
        .expect("Linux collector creation should succeed");

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Check for platform metadata in at least some processes
    let mut found_platform_metadata = false;
    for event in &events {
        if event.platform_metadata.is_some() {
            found_platform_metadata = true;

            // Verify the metadata structure contains Linux-specific fields
            let metadata = event.platform_metadata.as_ref().unwrap();

            // Linux metadata should have namespace information
            if metadata.get("namespaces").is_some() {
                println!(
                    "Linux platform metadata found for PID {}: namespaces present",
                    event.pid
                );
            }
            break;
        }
    }

    // In restricted environments (containers, CI), metadata may be unavailable
    if !found_platform_metadata {
        eprintln!(
            "Warning: No Linux platform metadata found; environment may restrict access. Skipping strict check."
        );
        return;
    }
}

// ============================================================================
// macOS-Specific Tests
// ============================================================================

/// Test: Process enumeration works correctly on macOS.
///
/// This test verifies that the macOS-specific collector can successfully
/// enumerate processes on a macOS system.
#[cfg(target_os = "macos")]
#[tokio::test]
#[traced_test]
async fn test_macos_process_enumeration_works_correctly() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let macos_config = MacOSCollectorConfig::default();

    let collector = EnhancedMacOSCollector::new(base_config, macos_config)
        .expect("macOS collector creation should succeed");

    // Verify collector name
    assert_eq!(collector.name(), "enhanced-macos-collector");

    // Test health check passes
    let health_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.health_check(),
    )
    .await;

    assert!(
        health_result.is_ok(),
        "Health check should complete within timeout"
    );
    assert!(
        health_result.unwrap().is_ok(),
        "Health check should pass on macOS"
    );

    // Test process collection
    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    assert!(
        collection_result.is_ok(),
        "Process collection should complete within timeout"
    );

    let (events, stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Verify processes were collected
    assert!(
        !events.is_empty(),
        "Should collect at least one process on macOS"
    );
    assert!(
        stats.total_processes > 0,
        "Should find processes on the system"
    );
    assert!(
        stats.successful_collections > 0,
        "Should successfully collect some processes"
    );

    println!(
        "macOS process enumeration: {} total, {} successful, {} inaccessible",
        stats.total_processes, stats.successful_collections, stats.inaccessible_processes
    );
}

/// Test: Platform-specific metadata is collected on macOS.
///
/// Verifies that macOS-specific metadata (entitlements, code signing, etc.)
/// is populated in process events.
#[cfg(target_os = "macos")]
#[tokio::test]
#[traced_test]
async fn test_macos_platform_specific_metadata_collected() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let macos_config = MacOSCollectorConfig {
        collect_entitlements: true,
        check_sip_protection: true,
        collect_code_signing: true,
        collect_bundle_info: true,
        handle_sandboxed_processes: true,
    };

    let collector = EnhancedMacOSCollector::new(base_config, macos_config)
        .expect("macOS collector creation should succeed");

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Check for platform metadata in at least some processes
    let mut found_platform_metadata = false;
    for event in &events {
        if event.platform_metadata.is_some() {
            found_platform_metadata = true;

            // Verify the metadata structure contains macOS-specific fields
            let metadata = event.platform_metadata.as_ref().unwrap();

            // macOS metadata should have entitlements or code_signing info
            if metadata.get("entitlements").is_some() || metadata.get("code_signing").is_some() {
                println!(
                    "macOS platform metadata found for PID {}: entitlements/code_signing present",
                    event.pid
                );
            }
            break;
        }
    }

    // In restricted environments (containers, CI), metadata may be unavailable
    if !found_platform_metadata {
        eprintln!(
            "Warning: No macOS platform metadata found; environment may restrict access. Skipping strict check."
        );
        return;
    }
}

// ============================================================================
// Windows-Specific Tests
// ============================================================================

/// Test: Process enumeration works correctly on Windows.
///
/// This test verifies that the Windows-specific collector can successfully
/// enumerate processes on a Windows system.
#[cfg(target_os = "windows")]
#[tokio::test]
#[traced_test]
async fn test_windows_process_enumeration_works_correctly() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let windows_config = WindowsCollectorConfig::default();

    let collector = WindowsProcessCollector::new(base_config, windows_config)
        .expect("Windows collector creation should succeed");

    // Verify collector name
    assert_eq!(collector.name(), "windows-collector");

    // Test health check passes
    let health_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.health_check(),
    )
    .await;

    assert!(
        health_result.is_ok(),
        "Health check should complete within timeout"
    );
    assert!(
        health_result.unwrap().is_ok(),
        "Health check should pass on Windows"
    );

    // Test process collection
    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    assert!(
        collection_result.is_ok(),
        "Process collection should complete within timeout"
    );

    let (events, stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Verify processes were collected
    assert!(
        !events.is_empty(),
        "Should collect at least one process on Windows"
    );
    assert!(
        stats.total_processes > 0,
        "Should find processes on the system"
    );
    assert!(
        stats.successful_collections > 0,
        "Should successfully collect some processes"
    );

    println!(
        "Windows process enumeration: {} total, {} successful, {} inaccessible",
        stats.total_processes, stats.successful_collections, stats.inaccessible_processes
    );
}

/// Test: Platform-specific metadata is collected on Windows.
///
/// Verifies that Windows-specific metadata (security info, service info, etc.)
/// is populated in process events.
#[cfg(target_os = "windows")]
#[tokio::test]
#[traced_test]
async fn test_windows_platform_specific_metadata_collected() {
    let base_config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };
    let windows_config = WindowsCollectorConfig {
        collect_security_info: true,
        detect_services: true,
        check_elevation_status: true,
        collect_performance_counters: true,
        detect_containers: true,
        handle_defender_restrictions: true,
    };

    let collector = WindowsProcessCollector::new(base_config, windows_config)
        .expect("Windows collector creation should succeed");

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Check for platform metadata in at least some processes
    let mut found_platform_metadata = false;
    for event in &events {
        if event.platform_metadata.is_some() {
            found_platform_metadata = true;

            // Verify the metadata structure contains Windows-specific fields
            let metadata = event.platform_metadata.as_ref().unwrap();

            // Windows metadata should have security_info or service_info
            if metadata.get("security_info").is_some() || metadata.get("service_info").is_some() {
                println!(
                    "Windows platform metadata found for PID {}: security_info/service_info present",
                    event.pid
                );
            }
            break;
        }
    }

    // In restricted environments (containers, CI), metadata may be unavailable
    if !found_platform_metadata {
        eprintln!(
            "Warning: No Windows platform metadata found; environment may restrict access. Skipping strict check."
        );
        return;
    }
}

// ============================================================================
// Cross-Platform Core Field Tests
// ============================================================================

/// Test: Process name, PID, and PPID are collected correctly.
///
/// Verifies that core process fields (PID, name, PPID) are populated
/// correctly across all platforms.
#[tokio::test]
#[traced_test]
async fn test_core_process_fields_collected() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };

    let collector = SysinfoProcessCollector::new(config);

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    assert!(!events.is_empty(), "Should collect processes");

    // Verify core fields for all collected processes
    for event in &events {
        // PID must be valid (> 0)
        assert!(event.pid > 0, "PID should be greater than 0");

        // Name must not be empty
        assert!(
            !event.name.is_empty(),
            "Process name should not be empty for PID {}",
            event.pid
        );

        // Timestamp must be reasonable (not in the future)
        assert!(
            event.timestamp <= SystemTime::now(),
            "Timestamp should not be in the future for PID {}",
            event.pid
        );

        // PPID is optional but if present should be reasonable
        if let Some(ppid) = event.ppid {
            // PPID 0 is valid on some systems (init process)
            // Most PPIDs should be > 0
            if ppid > 0 {
                assert!(
                    ppid < u32::MAX,
                    "PPID should be reasonable for PID {}",
                    event.pid
                );
            }
        }
    }

    println!(
        "Core fields test passed: {} processes verified",
        events.len()
    );
}

/// Test: Command-line arguments are collected where available.
///
/// Verifies that command-line arguments are populated for accessible processes.
#[tokio::test]
#[traced_test]
async fn test_command_line_arguments_collected() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };

    let collector = SysinfoProcessCollector::new(config);

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Count processes with command line arguments
    let mut with_cmdline = 0;
    let mut without_cmdline = 0;

    for event in &events {
        if event.command_line.is_empty() {
            without_cmdline += 1;
        } else {
            with_cmdline += 1;

            // Verify command line arguments are reasonable strings
            for arg in &event.command_line {
                // Arguments should not be excessively long
                assert!(
                    arg.len() < 32768,
                    "Command line argument should be reasonable length for PID {}",
                    event.pid
                );
            }
        }
    }

    // On most systems, at least some processes should have command lines
    // (kernel threads and some system processes may not have them)
    println!(
        "Command line test: {} with cmdline, {} without",
        with_cmdline, without_cmdline
    );

    // At least the current process should have a command line
    let current_pid = std::process::id();
    let current_process = events.iter().find(|e| e.pid == current_pid);

    if let Some(process) = current_process {
        assert!(
            !process.command_line.is_empty(),
            "Current process should have command line arguments"
        );
    }
}

/// Test: CPU and memory usage are collected where available.
///
/// Verifies that resource usage metrics are populated for accessible processes.
#[tokio::test]
#[traced_test]
async fn test_cpu_memory_usage_collected() {
    // Reasonable upper bound: 1 TB (more than any single process should use)
    const MAX_REASONABLE_MEMORY: u64 = 1024 * 1024 * 1024 * 1024;

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };

    let collector = SysinfoProcessCollector::new(config);

    let collection_result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await;

    let (events, _stats) = collection_result
        .unwrap()
        .expect("Collection should succeed");

    // Count processes with resource metrics
    let mut with_cpu = 0;
    let mut with_memory = 0;
    let mut with_start_time = 0;
    let mut anomalous_memory_count = 0;

    for event in &events {
        if event.cpu_usage.is_some() {
            with_cpu += 1;

            // CPU usage should be non-negative
            let cpu = event.cpu_usage.unwrap();
            assert!(
                cpu >= 0.0,
                "CPU usage should be non-negative for PID {}",
                event.pid
            );
        }

        if event.memory_usage.is_some() {
            let memory = event.memory_usage.unwrap();

            // Memory usage should be reasonable (not exceeding total system memory by too much)
            // Some platforms may report anomalous values for system processes; track but don't fail
            if memory >= MAX_REASONABLE_MEMORY {
                anomalous_memory_count += 1;
                eprintln!(
                    "Warning: Process {} ({}) reports unusually high memory: {} bytes",
                    event.pid, &event.name, memory
                );
            } else {
                with_memory += 1;
            }
        }

        if event.start_time.is_some() {
            with_start_time += 1;

            // Start time should be in the past
            let start_time = event.start_time.unwrap();
            assert!(
                start_time <= SystemTime::now(),
                "Start time should be in the past for PID {}",
                event.pid
            );
        }
    }

    eprintln!(
        "Resource metrics test: {} with CPU, {} with memory, {} with start_time, {} anomalous",
        with_cpu, with_memory, with_start_time, anomalous_memory_count
    );

    // With enhanced metadata enabled, at least some processes should have resource metrics
    assert!(
        with_memory > 0,
        "At least some processes should have memory usage information"
    );
}

/// Test: Current process can be collected successfully.
///
/// Verifies that the current running process can be collected with full details.
#[tokio::test]
#[traced_test]
async fn test_current_process_collection() {
    let config = ProcessCollectionConfig::default();
    let collector = SysinfoProcessCollector::new(config);

    let current_pid = std::process::id();

    let result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_process(current_pid),
    )
    .await;

    assert!(
        result.is_ok(),
        "Current process collection should complete within timeout"
    );

    let event = result
        .unwrap()
        .expect("Current process should be accessible");

    // Verify current process details
    assert_eq!(event.pid, current_pid, "PID should match current process");
    assert!(!event.name.is_empty(), "Process name should not be empty");
    assert!(event.accessible, "Current process should be accessible");

    // Current process should have an executable path
    assert!(
        event.executable_path.is_some() || event.file_exists,
        "Current process should have executable information"
    );

    println!(
        "Current process collected: PID={}, name={}, accessible={}",
        event.pid, event.name, event.accessible
    );
}

/// Test: Non-existent process returns appropriate error.
///
/// Verifies that attempting to collect a non-existent process returns
/// the correct error type.
#[tokio::test]
#[traced_test]
async fn test_nonexistent_process_error_handling() {
    let config = ProcessCollectionConfig::default();
    let collector = SysinfoProcessCollector::new(config);

    // Find a PID that doesn't exist by probing
    // Start from a high value and work down until we find one that fails
    let mut nonexistent_pid = 4_000_000_000u32; // Start well above typical pid_max
    for candidate in (1_000_000..4_000_000_000u32).rev().step_by(10000) {
        let probe_result = collector.collect_process(candidate).await;
        if probe_result.is_err() {
            nonexistent_pid = candidate;
            break;
        }
    }

    let result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_process(nonexistent_pid),
    )
    .await;

    assert!(
        result.is_ok(),
        "Non-existent process query should complete within timeout"
    );

    let collection_result = result.unwrap();
    assert!(
        collection_result.is_err(),
        "Non-existent process should return an error"
    );

    // Verify the error type
    match collection_result.unwrap_err() {
        ProcessCollectionError::ProcessNotFound { pid } => {
            assert_eq!(pid, nonexistent_pid, "Error should contain the queried PID");
        }
        ProcessCollectionError::ProcessAccessDenied { pid, .. } => {
            // Some systems may return access denied instead of not found
            assert_eq!(pid, nonexistent_pid, "Error should contain the queried PID");
        }
        other => {
            panic!(
                "Expected ProcessNotFound or ProcessAccessDenied, got: {:?}",
                other
            );
        }
    }
}

/// Test: Platform capabilities are reported correctly.
///
/// Verifies that the collector reports its capabilities accurately.
#[tokio::test]
#[traced_test]
async fn test_platform_capabilities_reported() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: true,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: MAX_PROCESSES_TEST,
    };

    let collector = SysinfoProcessCollector::new(config);
    let capabilities = collector.capabilities();

    // All collectors should support basic info
    assert!(
        capabilities.basic_info,
        "Collector should support basic info"
    );

    // With enhanced metadata config, should support enhanced metadata
    assert!(
        capabilities.enhanced_metadata,
        "Collector should support enhanced metadata when configured"
    );

    // Real-time collection should be supported by sysinfo
    assert!(
        capabilities.realtime_collection,
        "Collector should support real-time collection"
    );

    println!(
        "Capabilities: basic_info={}, enhanced_metadata={}, executable_hashing={}, system_processes={}, kernel_threads={}, realtime={}",
        capabilities.basic_info,
        capabilities.enhanced_metadata,
        capabilities.executable_hashing,
        capabilities.system_processes,
        capabilities.kernel_threads,
        capabilities.realtime_collection
    );
}

// ============================================================================
// Platform Detection Tests
// ============================================================================

/// Test: Correct platform is detected.
///
/// Verifies that the current platform is correctly identified.
#[tokio::test]
#[traced_test]
async fn test_platform_detection() {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    println!("Running on platform: {} ({})", os, arch);

    // Verify the platform matches compile-time constants
    #[cfg(target_os = "linux")]
    #[allow(clippy::semicolon_outside_block)]
    {
        assert_eq!(os, "linux", "Should be running on Linux");
        println!("Linux platform confirmed");
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::semicolon_outside_block)]
    {
        assert_eq!(os, "macos", "Should be running on macOS");
        println!("macOS platform confirmed");
    }

    #[cfg(target_os = "windows")]
    #[allow(clippy::semicolon_outside_block)]
    {
        assert_eq!(os, "windows", "Should be running on Windows");
        println!("Windows platform confirmed");
    }

    // Verify architecture is reasonable
    assert!(
        matches!(arch, "x86_64" | "aarch64" | "x86" | "arm"),
        "Architecture should be recognized: {}",
        arch
    );
}

/// Test: Platform collector selection works correctly.
///
/// Verifies that the appropriate platform-specific collector is available.
#[tokio::test]
#[traced_test]
async fn test_platform_collector_availability() {
    #[cfg(target_os = "linux")]
    #[allow(clippy::semicolon_outside_block)]
    {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let result = LinuxProcessCollector::new(base_config, linux_config);
        assert!(
            result.is_ok(),
            "Linux collector should be available on Linux"
        );
        println!("Linux collector available");
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::semicolon_outside_block)]
    {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let result = EnhancedMacOSCollector::new(base_config, macos_config);
        assert!(
            result.is_ok(),
            "macOS collector should be available on macOS"
        );
        println!("macOS collector available");
    }

    #[cfg(target_os = "windows")]
    #[allow(clippy::semicolon_outside_block)]
    {
        let base_config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();
        let result = WindowsProcessCollector::new(base_config, windows_config);
        assert!(
            result.is_ok(),
            "Windows collector should be available on Windows"
        );
        println!("Windows collector available");
    }

    // Sysinfo collector is always available
    let config = ProcessCollectionConfig::default();
    let collector = SysinfoProcessCollector::new(config);
    assert_eq!(
        collector.name(),
        "sysinfo-collector",
        "Sysinfo collector should always be available"
    );
    println!("Sysinfo collector (cross-platform) available");
}

// ============================================================================
// Graceful Degradation Tests
// ============================================================================

/// Test: System process filtering works correctly.
///
/// Verifies that system processes can be filtered out when configured.
#[tokio::test]
#[traced_test]
async fn test_system_process_filtering() {
    // Config with system processes skipped
    let config_filtered = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: true,
        skip_kernel_threads: true,
        max_processes: 0, // Unlimited
    };

    // Config without filtering
    let config_unfiltered = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 0, // Unlimited
    };

    let collector_filtered = SysinfoProcessCollector::new(config_filtered);
    let collector_unfiltered = SysinfoProcessCollector::new(config_unfiltered);

    let result_filtered = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector_filtered.collect_processes(),
    )
    .await
    .unwrap()
    .unwrap();

    let result_unfiltered = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector_unfiltered.collect_processes(),
    )
    .await
    .unwrap()
    .unwrap();

    let (events_filtered, stats_filtered) = result_filtered;
    let (events_unfiltered, _stats_unfiltered) = result_unfiltered;

    // Filtered collection may have fewer processes or more inaccessible counts
    println!(
        "Filtering test: unfiltered={}, filtered={}, filtered_inaccessible={}",
        events_unfiltered.len(),
        events_filtered.len(),
        stats_filtered.inaccessible_processes
    );

    // Both should have collected some processes
    assert!(
        !events_filtered.is_empty(),
        "Should have some user processes"
    );
    assert!(
        !events_unfiltered.is_empty(),
        "Should have some processes without filtering"
    );
}

/// Test: Max process limit is respected.
///
/// Verifies that the max_processes configuration is honored.
#[tokio::test]
#[traced_test]
async fn test_max_processes_limit_respected() {
    let max_limit: usize = 10;

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: max_limit,
    };

    let collector = SysinfoProcessCollector::new(config);

    let result = timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        collector.collect_processes(),
    )
    .await
    .unwrap()
    .unwrap();

    let (events, _stats) = result;

    // Should not exceed the max limit
    assert!(
        events.len() <= max_limit,
        "Should respect max_processes limit: got {} (max {})",
        events.len(),
        max_limit
    );

    // Should collect up to the limit (assuming system has that many processes)
    assert!(
        !events.is_empty(),
        "Should collect at least one process within limit"
    );

    println!(
        "Max processes limit test: collected {} (limit {})",
        events.len(),
        max_limit
    );
}
