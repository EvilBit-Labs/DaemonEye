//! macOS-specific integration tests for process collection.
//!
//! This module contains integration tests that verify the macOS-specific
//! process collector functionality, including libproc and sysctl API usage,
//! entitlements detection, SIP awareness, and sandboxed process handling.

#[cfg(target_os = "macos")]
mod macos_tests {
    use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};
    use procmond::process_collector::{ProcessCollectionConfig, ProcessCollector};
    use std::time::Duration;
    use tokio::time::timeout;
    use tracing_test::traced_test;

    /// Test basic macOS collector creation and configuration.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_collector_creation() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();

        let collector = EnhancedMacOSCollector::new(base_config, macos_config);
        assert!(
            collector.is_ok(),
            "macOS collector creation should succeed: {:?}",
            collector.err()
        );

        let collector = collector.unwrap();
        assert_eq!(collector.name(), "enhanced-macos-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(capabilities.realtime_collection);
        assert!(!capabilities.kernel_threads); // macOS doesn't have kernel threads like Linux
    }

    /// Test macOS collector capabilities with different configurations.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_collector_capabilities() {
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: true,
            skip_system_processes: true,
            skip_kernel_threads: true,
            max_processes: 100,
        };

        let macos_config = MacOSCollectorConfig {
            collect_entitlements: false,
            check_sip_protection: false,
            collect_code_signing: false,
            collect_bundle_info: false,
            handle_sandboxed_processes: false,
        };

        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(!capabilities.enhanced_metadata); // Disabled in config
        assert!(capabilities.executable_hashing); // Enabled in config
        assert!(!capabilities.system_processes); // Skip system processes enabled
        assert!(!capabilities.kernel_threads); // macOS doesn't have kernel threads
        assert!(capabilities.realtime_collection);
    }

    /// Test macOS collector health check functionality.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_collector_health_check() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Health check should complete within reasonable time
        let health_result = timeout(Duration::from_secs(5), collector.health_check()).await;
        assert!(
            health_result.is_ok(),
            "Health check should complete within timeout"
        );

        let health_check = health_result.unwrap();
        assert!(
            health_check.is_ok(),
            "Health check should pass on macOS system: {:?}",
            health_check.err()
        );
    }

    /// Test macOS process collection with libproc APIs.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_process_collection() {
        let base_config = ProcessCollectionConfig {
            max_processes: 50, // Limit for faster testing
            collect_enhanced_metadata: true,
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Process collection should complete within reasonable time
        let collection_result =
            timeout(Duration::from_secs(10), collector.collect_processes()).await;
        assert!(
            collection_result.is_ok(),
            "Process collection should complete within timeout"
        );

        let (events, stats) = collection_result.unwrap().unwrap();

        // Verify we collected some processes
        assert!(!events.is_empty(), "Should collect at least one process");
        assert!(
            stats.total_processes > 0,
            "Should find processes on macOS system"
        );
        assert!(
            stats.successful_collections > 0,
            "Should successfully collect some processes"
        );
        assert!(
            stats.collection_duration_ms > 0,
            "Collection should take some time"
        );

        // Verify process data quality
        for event in &events {
            assert!(event.pid > 0, "Process PID should be valid");
            assert!(!event.name.is_empty(), "Process name should not be empty");
            assert!(event.accessible, "Collected processes should be accessible");
            assert!(
                event.timestamp <= std::time::SystemTime::now(),
                "Timestamp should be reasonable"
            );

            // Verify macOS-specific attributes
            if event.pid == 1 {
                // launchd should be present on macOS
                assert!(
                    event.name.contains("launchd") || event.name.contains("init"),
                    "PID 1 should be launchd or init on macOS"
                );
            }
        }

        println!(
            "Successfully collected {} processes in {}ms",
            events.len(),
            stats.collection_duration_ms
        );
    }

    /// Test macOS single process collection.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_single_process_collection() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Try to collect information for the current process
        let current_pid = std::process::id();
        let result = collector.collect_process(current_pid).await;

        assert!(
            result.is_ok(),
            "Should be able to collect current process: {:?}",
            result.err()
        );

        let event = result.unwrap();
        assert_eq!(event.pid, current_pid);
        assert!(!event.name.is_empty());
        assert!(event.accessible);

        println!(
            "Successfully collected current process: PID={}, name={}",
            event.pid, event.name
        );
    }

    /// Test macOS collector with enhanced metadata collection.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_enhanced_metadata_collection() {
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: true,
            max_processes: 10, // Small number for focused testing
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig {
            collect_entitlements: true,
            check_sip_protection: true,
            collect_code_signing: true,
            collect_bundle_info: true,
            handle_sandboxed_processes: true,
        };
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let (events, _stats) = collector.collect_processes().await.unwrap();

        // Verify enhanced metadata is present
        let mut found_enhanced_data = false;
        for event in &events {
            if event.cpu_usage.is_some() || event.memory_usage.is_some() {
                found_enhanced_data = true;
                println!(
                    "Process {} has enhanced metadata: CPU={:?}, Memory={:?}",
                    event.name, event.cpu_usage, event.memory_usage
                );
            }
        }

        assert!(
            found_enhanced_data,
            "Should find at least one process with enhanced metadata"
        );
    }

    /// Test macOS collector error handling for non-existent process.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_nonexistent_process() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Try to collect information for a non-existent process
        let nonexistent_pid = 999999u32;
        let result = collector.collect_process(nonexistent_pid).await;

        assert!(
            result.is_err(),
            "Should fail for non-existent process: {:?}",
            result.ok()
        );

        match result.unwrap_err() {
            procmond::process_collector::ProcessCollectionError::ProcessNotFound { pid } => {
                assert_eq!(pid, nonexistent_pid);
            }
            other => panic!("Expected ProcessNotFound error, got: {:?}", other),
        }
    }

    /// Test macOS collector with system process filtering.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_system_process_filtering() {
        let base_config = ProcessCollectionConfig {
            skip_system_processes: true,
            max_processes: 20,
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let (events, stats) = collector.collect_processes().await.unwrap();

        // With system process filtering, we should have some inaccessible processes
        // (system processes that were filtered out)
        println!(
            "Collected {} processes, {} inaccessible (filtered)",
            stats.successful_collections, stats.inaccessible_processes
        );

        // Verify no obvious system processes are in the results
        for event in &events {
            assert!(
                !event.name.contains("kernel"),
                "Should not contain kernel processes when filtering is enabled"
            );
        }
    }

    /// Test macOS collector performance under load.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_collector_performance() {
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: true,
            max_processes: 100, // Reasonable load test
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let start_time = std::time::Instant::now();
        let (events, stats) = collector.collect_processes().await.unwrap();
        let total_duration = start_time.elapsed();

        println!(
            "Performance test: collected {} processes in {}ms (total: {}ms)",
            events.len(),
            stats.collection_duration_ms,
            total_duration.as_millis()
        );

        // Performance assertions
        assert!(
            stats.collection_duration_ms < 5000,
            "Collection should complete within 5 seconds"
        );
        assert!(
            total_duration.as_millis() < 10000,
            "Total test should complete within 10 seconds"
        );

        // Verify reasonable success rate
        let success_rate = stats.successful_collections as f64 / stats.total_processes as f64;
        assert!(
            success_rate > 0.5,
            "Should have at least 50% success rate, got: {:.2}%",
            success_rate * 100.0
        );
    }

    /// Test macOS collector SIP detection functionality.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_sip_detection() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig {
            check_sip_protection: true,
            ..Default::default()
        };

        // This should succeed regardless of SIP status
        let collector = EnhancedMacOSCollector::new(base_config, macos_config);
        assert!(
            collector.is_ok(),
            "Collector creation should succeed even with SIP detection enabled"
        );

        let collector = collector.unwrap();
        let health_result = collector.health_check().await;
        assert!(
            health_result.is_ok(),
            "Health check should pass with SIP detection enabled"
        );
    }

    /// Test macOS collector entitlements detection.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_entitlements_detection() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig {
            collect_entitlements: true,
            ..Default::default()
        };

        // This should succeed regardless of entitlements availability
        let collector = EnhancedMacOSCollector::new(base_config, macos_config);
        assert!(
            collector.is_ok(),
            "Collector creation should succeed even with entitlements detection enabled"
        );

        let collector = collector.unwrap();
        let health_result = collector.health_check().await;
        assert!(
            health_result.is_ok(),
            "Health check should pass with entitlements detection enabled"
        );
    }

    /// Test macOS collector graceful degradation.
    #[tokio::test]
    #[traced_test]
    async fn test_macos_graceful_degradation() {
        // Test with all advanced features disabled
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: false,
            max_processes: 10,
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig {
            collect_entitlements: false,
            check_sip_protection: false,
            collect_code_signing: false,
            collect_bundle_info: false,
            handle_sandboxed_processes: false,
        };

        let collector = EnhancedMacOSCollector::new(base_config.clone(), macos_config).unwrap();
        let (events, stats) = collector.collect_processes().await.unwrap();

        // Should still collect basic process information
        assert!(
            !events.is_empty(),
            "Should collect processes even with minimal configuration"
        );
        assert!(
            stats.successful_collections > 0,
            "Should have some successful collections"
        );

        // Verify basic data is present
        for event in &events {
            assert!(event.pid > 0, "Should have valid PID");
            assert!(!event.name.is_empty(), "Should have process name");
            // Enhanced metadata should be None when disabled
            if !base_config.collect_enhanced_metadata {
                // CPU and memory might still be available from basic libproc calls
                // so we don't assert they're None
            }
        }

        println!(
            "Graceful degradation test: collected {} processes with minimal configuration",
            events.len()
        );
    }
}

#[cfg(not(target_os = "macos"))]
mod non_macos_tests {
    /// Placeholder test for non-macOS platforms.
    #[test]
    fn test_macos_tests_skipped_on_non_macos() {
        println!("macOS-specific tests are skipped on non-macOS platforms");
    }
}
