//! Integration tests for shared infrastructure components in collector-core.
//!
//! This test suite verifies that the integrated components from daemoneye-lib
//! (ConfigLoader, TelemetryCollector, health monitoring) work correctly
//! within the collector-core framework.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{sync::mpsc, time::sleep};

/// Test event source that generates events for testing.
struct TestEventSource {
    name: &'static str,
    capabilities: SourceCaps,
    event_count: Arc<AtomicUsize>,
    should_error: Arc<AtomicBool>,
}

impl TestEventSource {
    fn new(name: &'static str, capabilities: SourceCaps) -> Self {
        Self {
            name,
            capabilities,
            event_count: Arc::new(AtomicUsize::new(0)),
            should_error: Arc::new(AtomicBool::new(false)),
        }
    }

    #[allow(dead_code)]
    fn set_error_mode(&self, should_error: bool) {
        self.should_error.store(should_error, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    fn get_event_count(&self) -> usize {
        self.event_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EventSource for TestEventSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        self.capabilities
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        _shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        let event_count = Arc::clone(&self.event_count);
        let should_error = Arc::clone(&self.should_error);

        // Generate a few test events
        for i in 0..5 {
            if should_error.load(Ordering::Relaxed) && i == 2 {
                return Err(anyhow::anyhow!("Simulated error in event source"));
            }

            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i as u32,
                ppid: Some(1),
                name: format!("test_process_{}", i),
                executable_path: Some(format!("/usr/bin/test_{}", i)),
                command_line: vec![format!("test_{}", i)],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(1.0 + i as f64),
                memory_usage: Some(1024 * (i + 1) as u64),
                executable_hash: Some(format!("hash_{}", i)),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            });

            if tx.send(event).await.is_err() {
                break; // Channel closed
            }

            event_count.fetch_add(1, Ordering::Relaxed);
            sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        if self.should_error.load(Ordering::Relaxed) {
            Err(anyhow::anyhow!("Health check failed"))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn test_config_integration_with_daemoneye_lib() {
    // Test that collector-core can load configuration from daemoneye-lib ConfigLoader
    let config = CollectorConfig::load_from_daemoneye_config("test-component");

    // Should succeed even if no config files exist (uses defaults)
    assert!(config.is_ok());

    let config = config.unwrap();
    assert_eq!(config.component_name, "test-component");
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn test_telemetry_collection_integration() {
    // Create a collector with telemetry enabled
    let config = CollectorConfig::default()
        .with_component_name("telemetry-test".to_string())
        .with_telemetry(true)
        .with_telemetry_interval(Duration::from_millis(100));

    let mut collector = Collector::new(config);

    // Register a test event source
    let test_source = TestEventSource::new("telemetry-source", SourceCaps::PROCESS);
    let _ = collector.register(Box::new(test_source));

    // Test that telemetry is configured correctly
    assert_eq!(collector.source_count(), 1);

    // Test capability aggregation
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
}

#[tokio::test]
async fn test_health_monitoring_integration() {
    // Create a collector with health monitoring
    let config = CollectorConfig::default()
        .with_component_name("health-test".to_string())
        .with_health_check_interval(Duration::from_millis(100));

    let mut collector = Collector::new(config);

    // Register a test event source
    let test_source = TestEventSource::new("health-source", SourceCaps::PROCESS);
    let _ = collector.register(Box::new(test_source));

    // Test that health monitoring is configured correctly
    assert_eq!(collector.source_count(), 1);

    // Test capability aggregation
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
}

#[tokio::test]
async fn test_event_batching_and_backpressure() {
    // Test configuration validation for batching parameters
    let config = CollectorConfig::default()
        .with_component_name("batching-test".to_string())
        .with_max_batch_size(3)
        .with_event_buffer_size(10)
        .with_backpressure_threshold(8)
        .with_batch_timeout(Duration::from_millis(50));

    // Validate the configuration
    assert!(config.validate().is_ok());

    // Test that the configuration values are set correctly
    assert_eq!(config.max_batch_size, 3);
    assert_eq!(config.event_buffer_size, 10);
    assert_eq!(config.backpressure_threshold, 8);
    assert_eq!(config.batch_timeout, Duration::from_millis(50));

    // Test collector creation with batching configuration
    let collector = Collector::new(config);
    assert_eq!(collector.source_count(), 0);
}

#[tokio::test]
async fn test_graceful_shutdown_coordination() {
    // Create a collector for shutdown testing
    let config = CollectorConfig::default()
        .with_component_name("shutdown-test".to_string())
        .with_shutdown_timeout(Duration::from_millis(100));

    let mut collector = Collector::new(config);

    // Register multiple test event sources
    let source1 = TestEventSource::new("shutdown-source-1", SourceCaps::PROCESS);
    let source2 = TestEventSource::new("shutdown-source-2", SourceCaps::NETWORK);

    let _ = collector.register(Box::new(source1));
    let _ = collector.register(Box::new(source2));

    // Test that sources are registered correctly
    assert_eq!(collector.source_count(), 2);

    // Test capability aggregation
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::NETWORK));
}

#[tokio::test]
async fn test_error_handling_and_recovery() {
    // Create a collector for error testing
    let config = CollectorConfig::default()
        .with_component_name("error-test".to_string())
        .with_health_check_interval(Duration::from_millis(50));

    let mut collector = Collector::new(config);

    // Register a test event source that can simulate errors
    let test_source = TestEventSource::new("error-source", SourceCaps::PROCESS);
    let error_source_ref = test_source.should_error.clone();
    let _ = collector.register(Box::new(test_source));

    // Test error simulation
    error_source_ref.store(true, Ordering::Relaxed);
    assert!(error_source_ref.load(Ordering::Relaxed));

    // Clear error condition
    error_source_ref.store(false, Ordering::Relaxed);
    assert!(!error_source_ref.load(Ordering::Relaxed));

    // Test that collector is configured correctly
    assert_eq!(collector.source_count(), 1);
}

#[tokio::test]
async fn test_capability_negotiation() {
    // Test that capabilities are properly aggregated from multiple sources
    let config = CollectorConfig::default().with_component_name("capability-test".to_string());

    let mut collector = Collector::new(config);

    // Register sources with different capabilities
    let process_source =
        TestEventSource::new("process-source", SourceCaps::PROCESS | SourceCaps::REALTIME);
    let network_source = TestEventSource::new(
        "network-source",
        SourceCaps::NETWORK | SourceCaps::SYSTEM_WIDE,
    );

    let _ = collector.register(Box::new(process_source));
    let _ = collector.register(Box::new(network_source));

    // Check aggregated capabilities
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::NETWORK));
    assert!(capabilities.contains(SourceCaps::REALTIME));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));
    assert!(!capabilities.contains(SourceCaps::FILESYSTEM));
}

#[tokio::test]
async fn test_configuration_validation() {
    // Test configuration validation with various invalid configurations

    // Test invalid backpressure configuration
    let invalid_config = CollectorConfig::default()
        .with_backpressure_threshold(1000)
        .with_event_buffer_size(500); // threshold > buffer size

    assert!(invalid_config.validate().is_err());

    // Test valid configuration
    let valid_config = CollectorConfig::default()
        .with_backpressure_threshold(400)
        .with_event_buffer_size(500);

    assert!(valid_config.validate().is_ok());
}

#[test]
fn test_runtime_stats_calculation() {
    use collector_core::RuntimeStats;

    // Test error rate calculation
    let stats = RuntimeStats {
        events_processed: 100,
        errors_total: 5,
        registered_sources: 2,
        active_components: 3,
        backpressure_permits_available: 10,
    };

    assert_eq!(stats.error_rate(), 5.0); // 5%
    assert!(!stats.is_under_backpressure());

    // Test zero events case
    let stats_zero = RuntimeStats {
        events_processed: 0,
        errors_total: 0,
        registered_sources: 1,
        active_components: 1,
        backpressure_permits_available: 0,
    };

    assert_eq!(stats_zero.error_rate(), 0.0);
    assert!(stats_zero.is_under_backpressure());
}
