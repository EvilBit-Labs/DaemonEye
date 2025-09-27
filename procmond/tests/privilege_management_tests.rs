//! Privilege management tests for ProcessCollector implementations.
//!
//! This module tests privilege escalation and dropping behavior across all platforms,
//! ensuring that collectors handle privilege boundaries correctly and securely.

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::time::Duration;
use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test timeout for privilege-related operations.
const PRIVILEGE_TEST_TIMEOUT_SECS: u64 = 30;

/// Helper function to check if running as root/administrator.
fn is_elevated_privileges() -> bool {
    #[cfg(unix)]
    {
        // Check if running as root by examining environment variables and file permissions
        // This avoids using unsafe libc calls while still detecting elevated privileges
        if let Ok(user) = std::env::var("USER") {
            if user == "root" {
                return true;
            }
        }

        if let Ok(uid) = std::env::var("UID") {
            if uid == "0" {
                return true;
            }
        }

        // Try to access a root-only file as another indicator
        std::fs::metadata("/etc/shadow").is_ok()
    }

    #[cfg(windows)]
    {
        // On Windows, check if running as administrator
        // This is a simplified check - in practice, you'd use Windows APIs
        std::env::var("USERNAME")
            .unwrap_or_default()
            .to_lowercase()
            .contains("admin")
    }
}

/// Helper function to get current user information.
fn get_current_user_info() -> String {
    #[cfg(unix)]
    {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        let uid = std::env::var("UID").unwrap_or_else(|_| "unknown".to_string());
        format!("User: {}, UID: {}", user, uid)
    }

    #[cfg(windows)]
    {
        format!("User: {}", std::env::var("USERNAME").unwrap_or_default())
    }
}

/// Test that collectors work correctly with standard user privileges.
#[tokio::test]
#[traced_test]
async fn test_standard_user_privileges() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 50,
    };

    let collectors = create_all_available_collectors(config);
    let user_info = get_current_user_info();

    println!("Testing standard user privileges: {}", user_info);

    for (name, collector) in collectors {
        println!("Testing standard privileges for collector: {}", name);

        // Test health check with standard privileges
        let health_result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.health_check(),
        )
        .await;

        assert!(
            health_result.is_ok(),
            "Health check should complete with standard privileges for {}",
            name
        );

        let health_check = health_result.unwrap();
        assert!(
            health_check.is_ok(),
            "Health check should pass with standard privileges for {}: {:?}",
            name,
            health_check.err()
        );

        // Test process collection with standard privileges
        let collection_result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Process collection should complete with standard privileges for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Process collection should succeed with standard privileges for {}: {:?}",
            name,
            collection.as_ref().err()
        );

        let (events, stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Process collection should succeed with standard privileges for {}: {:?}",
                name, err
            )
        });

        // Should collect at least some processes even with standard privileges
        assert!(
            !events.is_empty(),
            "Should collect at least some processes with standard privileges for {}",
            name
        );

        // Log privilege-related statistics
        println!(
            "Standard privilege results for {}: {} total, {} successful, {} inaccessible",
            name, stats.total_processes, stats.successful_collections, stats.inaccessible_processes
        );

        // With standard privileges, some processes might be inaccessible
        // This is expected and acceptable
        if stats.inaccessible_processes > 0 {
            println!(
                "Note: {} inaccessible processes with standard privileges for {} (expected)",
                stats.inaccessible_processes, name
            );
        }

        println!("✓ Standard privilege test passed for {}", name);
    }
}

/// Test that collectors handle elevated privileges appropriately.
#[tokio::test]
#[traced_test]
async fn test_elevated_privileges_handling() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 100,
    };

    let collectors = create_all_available_collectors(config);
    let user_info = get_current_user_info();
    let is_elevated = is_elevated_privileges();

    println!(
        "Testing elevated privileges handling: {} (elevated: {})",
        user_info, is_elevated
    );

    for (name, collector) in collectors {
        println!(
            "Testing elevated privileges handling for collector: {}",
            name
        );

        let collection_result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Process collection should complete regardless of privilege level for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Process collection should succeed regardless of privilege level for {}: {:?}",
            name,
            collection.as_ref().err()
        );

        let (events, stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Process collection should succeed regardless of privilege level for {}: {:?}",
                name, err
            )
        });

        // Should collect processes regardless of privilege level
        assert!(
            !events.is_empty(),
            "Should collect processes regardless of privilege level for {}",
            name
        );

        // Log privilege-related statistics
        println!(
            "Privilege handling results for {}: {} total, {} successful, {} inaccessible (elevated: {})",
            name,
            stats.total_processes,
            stats.successful_collections,
            stats.inaccessible_processes,
            is_elevated
        );

        if is_elevated {
            // With elevated privileges, we might have access to more processes
            println!(
                "Running with elevated privileges - may have enhanced access for {}",
                name
            );
        } else {
            // With standard privileges, some processes might be inaccessible
            println!(
                "Running with standard privileges - some processes may be inaccessible for {}",
                name
            );
        }

        println!("✓ Elevated privilege handling test passed for {}", name);
    }
}

/// Test privilege boundary enforcement for system processes.
#[tokio::test]
#[traced_test]
async fn test_system_process_privilege_boundaries() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false, // Don't skip - test access
        skip_kernel_threads: false,   // Don't skip - test access
        max_processes: 200,
    };

    let collectors = create_all_available_collectors(config);
    let user_info = get_current_user_info();
    let is_elevated = is_elevated_privileges();

    println!(
        "Testing system process privilege boundaries: {} (elevated: {})",
        user_info, is_elevated
    );

    for (name, collector) in collectors {
        println!("Testing system process boundaries for collector: {}", name);

        let collection_result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "System process collection should complete for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "System process collection should succeed for {}: {:?}",
            name,
            collection.as_ref().err()
        );

        let (events, stats) = collection.unwrap_or_else(|err| {
            panic!(
                "System process collection should succeed for {}: {:?}",
                name, err
            )
        });

        // Analyze system process access patterns
        let mut system_processes = 0;
        let mut low_pid_processes = 0;

        for event in &events {
            // Count likely system processes (low PIDs)
            if event.pid <= 100 {
                low_pid_processes += 1;
            }

            // Count processes that might be system processes based on name
            if event.name.contains("kernel")
                || event.name.contains("system")
                || event.name.starts_with("k")
                || event.name.starts_with('[')
            {
                system_processes += 1;
            }
        }

        println!(
            "System process boundary results for {}: {} total, {} low-PID, {} system-like (elevated: {})",
            name,
            events.len(),
            low_pid_processes,
            system_processes,
            is_elevated
        );

        // The collector should handle system process access gracefully
        // Whether it can access them depends on privileges, but it shouldn't crash
        assert!(
            stats.total_processes > 0,
            "Should attempt to access some processes for {}",
            name
        );

        println!("✓ System process boundary test passed for {}", name);
    }
}

/// Test current process access (should always work).
#[tokio::test]
#[traced_test]
async fn test_current_process_access() {
    let config = ProcessCollectionConfig::default();
    let collectors = create_all_available_collectors(config);
    let current_pid = std::process::id();
    let user_info = get_current_user_info();

    println!(
        "Testing current process access: PID {} ({})",
        current_pid, user_info
    );

    for (name, collector) in collectors {
        println!("Testing current process access for collector: {}", name);

        let result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.collect_process(current_pid),
        )
        .await;

        assert!(
            result.is_ok(),
            "Current process collection should complete for {}",
            name
        );

        let process_result = result.unwrap();
        assert!(
            process_result.is_ok(),
            "Should always be able to collect current process for {}: {:?}",
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

        println!("✓ Current process access test passed for {}", name);
    }
}

/// Test privilege escalation detection (platform-specific).
#[tokio::test]
#[traced_test]
async fn test_privilege_escalation_detection() {
    let user_info = get_current_user_info();
    let is_elevated = is_elevated_privileges();

    println!(
        "Testing privilege escalation detection: {} (elevated: {})",
        user_info, is_elevated
    );

    // Test Linux-specific privilege detection
    #[cfg(target_os = "linux")]
    {
        println!("Testing Linux privilege detection");

        let config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig {
            use_cap_sys_ptrace: None, // Auto-detect
            ..Default::default()
        };

        if let Ok(collector) = LinuxProcessCollector::new(config, linux_config) {
            let capabilities = collector.capabilities();

            // Log capability detection results
            println!(
                "Linux capabilities: enhanced_metadata={}, system_processes={}, kernel_threads={}",
                capabilities.enhanced_metadata,
                capabilities.system_processes,
                capabilities.kernel_threads
            );

            // Test that capability detection works
            let health_result = collector.health_check().await;
            assert!(
                health_result.is_ok(),
                "Linux privilege detection should work: {:?}",
                health_result.err()
            );

            println!("✓ Linux privilege detection test passed");
        }
    }

    // Test macOS-specific privilege detection
    #[cfg(target_os = "macos")]
    {
        println!("Testing macOS privilege detection");

        let config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig {
            check_sip_protection: true,
            collect_entitlements: true,
            ..Default::default()
        };

        if let Ok(collector) = EnhancedMacOSCollector::new(config, macos_config) {
            let capabilities = collector.capabilities();

            // Log capability detection results
            println!(
                "macOS capabilities: enhanced_metadata={}, system_processes={}",
                capabilities.enhanced_metadata, capabilities.system_processes
            );

            // Test that SIP and entitlement detection works
            let health_result = collector.health_check().await;
            assert!(
                health_result.is_ok(),
                "macOS privilege detection should work: {:?}",
                health_result.err()
            );

            println!("✓ macOS privilege detection test passed");
        }
    }

    // Test Windows-specific privilege detection
    #[cfg(target_os = "windows")]
    {
        println!("Testing Windows privilege detection");

        let config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();

        if let Ok(collector) = WindowsProcessCollector::new(config, windows_config) {
            let capabilities = collector.capabilities();

            // Log capability detection results
            println!(
                "Windows capabilities: enhanced_metadata={}, system_processes={}",
                capabilities.enhanced_metadata, capabilities.system_processes
            );

            // Test that privilege detection works
            let health_result = collector.health_check().await;
            assert!(
                health_result.is_ok(),
                "Windows privilege detection should work: {:?}",
                health_result.err()
            );

            println!("✓ Windows privilege detection test passed");
        }
    }
}

/// Test graceful degradation when privileges are insufficient.
#[tokio::test]
#[traced_test]
async fn test_graceful_privilege_degradation() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: true,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 100,
    };

    let collectors = create_all_available_collectors(config);
    let user_info = get_current_user_info();

    println!("Testing graceful privilege degradation: {}", user_info);

    for (name, collector) in collectors {
        println!("Testing graceful degradation for collector: {}", name);

        // Even if some features require elevated privileges, basic collection should work
        let collection_result = timeout(
            Duration::from_secs(PRIVILEGE_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            collection_result.is_ok(),
            "Collection should complete even with privilege limitations for {}",
            name
        );

        let collection = collection_result.unwrap();
        assert!(
            collection.is_ok(),
            "Collection should succeed with graceful degradation for {}: {:?}",
            name,
            collection.as_ref().err()
        );

        let (events, stats) = collection.unwrap_or_else(|err| {
            panic!(
                "Collection should succeed with graceful degradation for {}: {:?}",
                name, err
            )
        });

        // Should collect at least some processes
        assert!(
            !events.is_empty(),
            "Should collect some processes even with privilege limitations for {}",
            name
        );

        // Calculate success rate
        let success_rate = if stats.total_processes > 0 {
            stats.successful_collections as f64 / stats.total_processes as f64
        } else {
            0.0
        };

        println!(
            "Graceful degradation results for {}: {:.1}% success rate ({}/{} processes)",
            name,
            success_rate * 100.0,
            stats.successful_collections,
            stats.total_processes
        );

        // Should have some success even with limited privileges
        assert!(
            success_rate > 0.0,
            "Should have some success even with privilege limitations for {}",
            name
        );

        println!("✓ Graceful degradation test passed for {}", name);
    }
}

/// Helper function to create all available collectors for privilege testing.
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
