//! OS version and configuration compatibility tests.
//!
//! This module tests `ProcessCollector` implementations across different OS versions
//! and system configurations to ensure broad compatibility and graceful degradation.

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::print_stdout,
    clippy::str_to_string,
    clippy::map_unwrap_or,
    clippy::non_ascii_literal,
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::if_not_else,
    clippy::option_if_let_else,
    clippy::panic
)]

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test timeout for compatibility operations.
const COMPATIBILITY_TEST_TIMEOUT_SECS: u64 = 45;

/// System information structure for compatibility testing.
#[derive(Debug, Clone)]
struct SystemInfo {
    os_name: String,
    os_version: String,
    architecture: String,
    kernel_version: Option<String>,
    additional_info: HashMap<String, String>,
}

/// Helper function to gather system information for compatibility testing.
fn gather_system_info() -> SystemInfo {
    let mut additional_info = HashMap::new();

    // Basic system information
    let os_name = std::env::consts::OS.to_string();
    let architecture = std::env::consts::ARCH.to_string();

    // Try to get more detailed version information
    let os_version = get_os_version();
    let kernel_version = get_kernel_version();

    // Gather additional environment information
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        additional_info.insert("hostname".to_string(), hostname);
    }

    if let Ok(user) = std::env::var("USER") {
        additional_info.insert("user".to_string(), user);
    }

    // Platform-specific information
    #[cfg(target_os = "linux")]
    {
        if let Ok(distro) = std::fs::read_to_string("/etc/os-release")
            && let Some(name_line) = distro.lines().find(|line| line.starts_with("NAME="))
        {
            let name = name_line.trim_start_matches("NAME=").trim_matches('"');
            additional_info.insert("distribution".to_string(), name.to_string());
        }

        // Check for container environment
        if std::path::Path::new("/.dockerenv").exists() {
            additional_info.insert("container".to_string(), "docker".to_string());
        }

        if std::env::var("container").is_ok() {
            additional_info.insert(
                "container_type".to_string(),
                std::env::var("container").unwrap_or_default(),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Try to get macOS version information
        if let Ok(output) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            && let Ok(version) = String::from_utf8(output.stdout)
        {
            additional_info.insert("macos_version".to_string(), version.trim().to_string());
        }

        // Check for Rosetta (Apple Silicon compatibility layer)
        if let Ok(output) = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("sysctl.proc_translated")
            .output()
            && let Ok(translated) = String::from_utf8(output.stdout)
            && translated.trim() == "1"
        {
            additional_info.insert("rosetta".to_string(), "true".to_string());
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(version) = std::env::var("OS") {
            additional_info.insert("windows_os".to_string(), version);
        }

        if let Ok(processor) = std::env::var("PROCESSOR_ARCHITECTURE") {
            additional_info.insert("processor_arch".to_string(), processor);
        }
    }

    SystemInfo {
        os_name,
        os_version,
        architecture,
        kernel_version,
        additional_info,
    }
}

/// Helper function to get OS version information.
fn get_os_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .unwrap_or_else(|_| "unknown".to_string())
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string()
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var("OS").unwrap_or_else(|_| "unknown".to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown".to_string()
    }
}

/// Helper function to get kernel version information.
fn get_kernel_version() -> Option<String> {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
}

/// Test basic compatibility across different system configurations.
#[tokio::test]
#[traced_test]
async fn test_basic_system_compatibility() {
    let system_info = gather_system_info();

    println!("Testing basic compatibility on system:");
    println!("  OS: {} {}", system_info.os_name, system_info.os_version);
    println!("  Architecture: {}", system_info.architecture);
    if let Some(ref kernel) = system_info.kernel_version {
        println!("  Kernel: {}", kernel);
    }
    for (key, value) in &system_info.additional_info {
        println!("  {}: {}", key, value);
    }

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 50,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing basic compatibility for collector: {}", name);

        // Test health check
        let health_result = timeout(
            Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.health_check(),
        )
        .await;

        assert!(
            health_result.is_ok(),
            "Health check should complete on {} {} for {}",
            system_info.os_name,
            system_info.architecture,
            name
        );

        let health_check = health_result.unwrap();
        assert!(
            health_check.is_ok(),
            "Health check should pass on {} {} for {}: {:?}",
            system_info.os_name,
            system_info.architecture,
            name,
            health_check.err()
        );

        // Test basic process collection
        let collection_result = timeout(
            Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Process collection should complete on {} {} for {}",
            system_info.os_name,
            system_info.architecture,
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Process collection should succeed on {} {} for {}: {:?}",
            system_info.os_name,
            system_info.architecture,
            name,
            collection.as_ref().err()
        );

        let (events, _stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Process collection should succeed on {} {} for {}: {:?}",
                system_info.os_name, system_info.architecture, name, err
            )
        });
        assert!(
            !events.is_empty(),
            "Should collect processes on {} {} for {}",
            system_info.os_name,
            system_info.architecture,
            name
        );

        println!(
            "✓ Basic compatibility test passed for {} on {} {} ({} processes)",
            name,
            system_info.os_name,
            system_info.architecture,
            events.len()
        );
    }
}

/// Test compatibility with different process limits and configurations.
#[tokio::test]
#[traced_test]
async fn test_configuration_compatibility() {
    let system_info = gather_system_info();

    println!(
        "Testing configuration compatibility on {} {}",
        system_info.os_name, system_info.architecture
    );

    // Test various configuration combinations
    let test_configs = vec![
        (
            "minimal",
            ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: true,
                skip_kernel_threads: true,
                max_processes: 10,
            },
        ),
        (
            "standard",
            ProcessCollectionConfig {
                collect_enhanced_metadata: true,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 100,
            },
        ),
        (
            "comprehensive",
            ProcessCollectionConfig {
                collect_enhanced_metadata: true,
                compute_executable_hashes: true,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 500,
            },
        ),
    ];

    for (config_name, config) in test_configs {
        println!("Testing {} configuration", config_name);

        let collectors = create_all_available_collectors(config.clone());

        for (name, collector) in collectors {
            let collection_result = timeout(
                Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
                collector.collect_processes(),
            )
            .await;

            assert!(
                collection_result.is_ok(),
                "{} configuration should work on {} {} for {}",
                config_name,
                system_info.os_name,
                system_info.architecture,
                name
            );

            let collection = collection_result.unwrap();
            assert!(
                collection.is_ok(),
                "{} configuration should succeed on {} {} for {}: {:?}",
                config_name,
                system_info.os_name,
                system_info.architecture,
                name,
                collection.as_ref().err()
            );

            let (events, _stats) = collection.unwrap_or_else(|err| {
                panic!(
                    "{} configuration should succeed on {} {} for {}: {:?}",
                    config_name, system_info.os_name, system_info.architecture, name, err
                )
            });

            // Verify configuration limits are respected
            assert!(
                events.len() <= config.max_processes,
                "Should respect max_processes limit for {} configuration on {}",
                config_name,
                name
            );

            println!(
                "✓ {} configuration test passed for {} ({} processes)",
                config_name,
                name,
                events.len()
            );
        }
    }
}

/// Test container environment compatibility.
#[tokio::test]
#[traced_test]
async fn test_container_environment_compatibility() {
    let system_info = gather_system_info();

    let is_container = system_info.additional_info.contains_key("container")
        || std::path::Path::new("/.dockerenv").exists();

    println!(
        "Testing container compatibility (container: {})",
        is_container
    );

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 100,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing container compatibility for collector: {}", name);

        let collection_result = timeout(
            Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Container compatibility test should complete for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Container compatibility test should succeed for {}: {:?}",
            name,
            collection.as_ref().err()
        );

        let (events, stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Container compatibility test should succeed for {}: {:?}",
                name, err
            )
        });

        if is_container {
            println!(
                "Container environment results for {}: {} processes, {} successful",
                name, stats.total_processes, stats.successful_collections
            );

            // In containers, we might see fewer processes
            assert!(
                !events.is_empty(),
                "Should collect some processes even in container for {}",
                name
            );
        } else {
            println!(
                "Native environment results for {}: {} processes, {} successful",
                name, stats.total_processes, stats.successful_collections
            );

            // In native environments, we should see more processes
            assert!(
                stats.total_processes > 5,
                "Should see multiple processes in native environment for {}",
                name
            );
        }

        println!("✓ Container compatibility test passed for {}", name);
    }
}

/// Test architecture-specific compatibility.
#[tokio::test]
#[traced_test]
async fn test_architecture_compatibility() {
    let system_info = gather_system_info();

    println!(
        "Testing architecture compatibility: {}",
        system_info.architecture
    );

    let config = ProcessCollectionConfig::default();
    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!(
            "Testing architecture compatibility for collector: {} on {}",
            name, system_info.architecture
        );

        // Test that collector works on current architecture
        let collection_result = timeout(
            Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Architecture compatibility test should complete for {} on {}",
            name,
            system_info.architecture
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Architecture compatibility test should succeed for {} on {}: {:?}",
            name,
            system_info.architecture,
            collection.as_ref().err()
        );

        let (events, _stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Architecture compatibility test should succeed for {} on {}: {:?}",
                name, system_info.architecture, err
            )
        });

        // Verify process data is valid for this architecture
        for event in &events {
            assert!(
                event.pid > 0,
                "PID should be valid on {}",
                system_info.architecture
            );
            assert!(
                !event.name.is_empty(),
                "Process name should be valid on {}",
                system_info.architecture
            );

            // Architecture-specific validations
            match system_info.architecture.as_str() {
                "x86_64" | "aarch64" | "arm64" => {
                    // 64-bit architectures - PID is already u32, so within valid range
                    assert!(event.pid > 0, "PID should be positive");
                }
                "x86" | "arm" => {
                    // 32-bit architectures - PID is already u32, so within valid range
                    assert!(event.pid > 0, "PID should be positive");
                }
                _ => {
                    // Unknown architecture - basic validation
                    assert!(event.pid > 0, "PID should be positive");
                }
            }
        }

        println!(
            "✓ Architecture compatibility test passed for {} on {}",
            name, system_info.architecture
        );
    }
}

/// Test platform-specific feature compatibility.
#[tokio::test]
#[traced_test]
async fn test_platform_feature_compatibility() {
    let system_info = gather_system_info();

    println!(
        "Testing platform feature compatibility on {}",
        system_info.os_name
    );

    // Test platform-specific collectors with their native features
    #[cfg(target_os = "linux")]
    {
        println!("Testing Linux-specific features");

        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: true,
            compute_executable_hashes: false,
            skip_system_processes: false,
            skip_kernel_threads: false,
            max_processes: 100,
        };

        let linux_config = LinuxCollectorConfig {
            collect_namespaces: true,
            collect_memory_maps: true,
            collect_file_descriptors: true,
            collect_network_connections: false, // Can be expensive
            detect_containers: true,
            use_cap_sys_ptrace: None, // Auto-detect
        };

        if let Ok(collector) = LinuxProcessCollector::new(base_config, linux_config) {
            let result = collector.collect_processes().await;
            assert!(
                result.is_ok(),
                "Linux-specific features should work: {:?}",
                result.err()
            );

            let (events, _stats) = result.unwrap();
            assert!(!events.is_empty(), "Linux collector should find processes");

            println!("✓ Linux-specific features test passed");
        }
    }

    #[cfg(target_os = "macos")]
    {
        println!("Testing macOS-specific features");

        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: true,
            compute_executable_hashes: false,
            skip_system_processes: false,
            skip_kernel_threads: false,
            max_processes: 100,
        };

        let macos_config = MacOSCollectorConfig {
            collect_entitlements: true,
            check_sip_protection: true,
            collect_code_signing: true,
            collect_bundle_info: true,
            handle_sandboxed_processes: true,
        };

        if let Ok(collector) = EnhancedMacOSCollector::new(base_config, macos_config) {
            let result = collector.collect_processes().await;
            assert!(
                result.is_ok(),
                "macOS-specific features should work: {:?}",
                result.err()
            );

            let (events, _stats) = result.unwrap();
            assert!(!events.is_empty(), "macOS collector should find processes");

            println!("✓ macOS-specific features test passed");
        }
    }

    #[cfg(target_os = "windows")]
    {
        println!("Testing Windows-specific features");

        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: true,
            compute_executable_hashes: false,
            skip_system_processes: false,
            skip_kernel_threads: false,
            max_processes: 100,
        };

        let windows_config = WindowsCollectorConfig::default();

        if let Ok(collector) = WindowsProcessCollector::new(base_config, windows_config) {
            let result = collector.collect_processes().await;
            assert!(
                result.is_ok(),
                "Windows-specific features should work: {:?}",
                result.err()
            );

            let (events, _stats) = result.unwrap();
            assert!(
                !events.is_empty(),
                "Windows collector should find processes"
            );

            println!("✓ Windows-specific features test passed");
        }
    }
}

/// Test graceful degradation on older or limited systems.
#[tokio::test]
#[traced_test]
async fn test_graceful_degradation_compatibility() {
    let system_info = gather_system_info();

    println!(
        "Testing graceful degradation on {} {}",
        system_info.os_name, system_info.architecture
    );

    // Test with very restrictive configuration to simulate limited systems
    let restrictive_config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: true,
        skip_kernel_threads: true,
        max_processes: 5, // Very small limit
    };

    let collectors = create_all_available_collectors(restrictive_config);

    for (name, collector) in collectors {
        println!("Testing graceful degradation for collector: {}", name);

        let collection_result = timeout(
            Duration::from_secs(COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Graceful degradation should work for {} on {} {}",
            name,
            system_info.os_name,
            system_info.architecture
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Graceful degradation should succeed for {} on {} {}: {:?}",
            name,
            system_info.os_name,
            system_info.architecture,
            collection.as_ref().err()
        );

        let (events, _stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Graceful degradation should succeed for {} on {} {}: {:?}",
                name, system_info.os_name, system_info.architecture, err
            )
        });

        // Should still collect some processes even with restrictions
        assert!(
            !events.is_empty(),
            "Should collect some processes with degraded config for {}",
            name
        );

        // Should respect the restrictive limits
        assert!(
            events.len() <= 5,
            "Should respect restrictive max_processes limit for {}",
            name
        );

        println!(
            "✓ Graceful degradation test passed for {} ({} processes with limit 5)",
            name,
            events.len()
        );
    }
}

/// Helper function to create all available collectors for compatibility testing.
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
