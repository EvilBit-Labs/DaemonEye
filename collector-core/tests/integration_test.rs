//! Integration tests for collector-core framework.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

/// Mock event source for testing
struct MockProcessSource {
    name: &'static str,
    event_count: Arc<AtomicUsize>,
    should_fail: bool,
}

impl MockProcessSource {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            event_count: Arc::new(AtomicUsize::new(0)),
            should_fail: false,
        }
    }

    #[allow(dead_code)]
    fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }

    #[allow(dead_code)]
    fn events_sent(&self) -> usize {
        self.event_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EventSource for MockProcessSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        if self.should_fail {
            anyhow::bail!("Mock source configured to fail");
        }

        // Send a few mock events
        for i in 0..3 {
            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i,
                ppid: Some(1),
                name: format!("mock_process_{}", i),
                executable_path: Some(format!("/usr/bin/mock_{}", i)),
                command_line: vec![format!("mock_{}", i), "--test".to_string()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(1.5),
                memory_usage: Some(1024 * 1024),
                executable_hash: Some("mock_hash".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
            });

            if tx.send(event).await.is_err() {
                break; // Channel closed
            }

            self.event_count.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        if self.should_fail {
            anyhow::bail!("Mock source is unhealthy");
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_collector_with_single_source() {
    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(100)
        .with_shutdown_timeout(Duration::from_secs(1));

    let mut collector = Collector::new(config);
    let source = MockProcessSource::new("test-source");

    // Verify initial state
    assert_eq!(collector.source_count(), 0);
    assert_eq!(collector.capabilities(), SourceCaps::empty());

    // Register source
    let _ = collector.register(Box::new(source));

    // Verify registration
    assert_eq!(collector.source_count(), 1);
    assert!(collector.capabilities().contains(SourceCaps::PROCESS));
    assert!(collector.capabilities().contains(SourceCaps::REALTIME));
    assert!(collector.capabilities().contains(SourceCaps::SYSTEM_WIDE));
}

#[tokio::test]
async fn test_collector_with_multiple_sources() {
    let config = CollectorConfig::default()
        .with_max_event_sources(3)
        .with_event_buffer_size(100);

    let mut collector = Collector::new(config);

    // Register multiple sources
    let _ = collector.register(Box::new(MockProcessSource::new("source-1")));
    let _ = collector.register(Box::new(MockProcessSource::new("source-2")));
    let _ = collector.register(Box::new(MockProcessSource::new("source-3")));

    // Verify registration
    assert_eq!(collector.source_count(), 3);

    // All sources have the same capabilities, so they should be combined
    let expected_caps = SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE;
    assert_eq!(collector.capabilities(), expected_caps);
}

#[tokio::test]
async fn test_collector_max_sources_exceeded() {
    let config = CollectorConfig::default().with_max_event_sources(1);
    let mut collector = Collector::new(config);

    // Register first source (should succeed)
    let result = collector.register(Box::new(MockProcessSource::new("source-1")));
    assert!(result.is_ok(), "First source registration should succeed");

    // Register second source (should fail)
    let result = collector.register(Box::new(MockProcessSource::new("source-2")));
    assert!(result.is_err(), "Second source registration should fail");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Cannot register more than")
    );
}

#[tokio::test]
async fn test_config_validation() {
    // Valid config should pass
    let valid_config = CollectorConfig::default();
    assert!(valid_config.validate().is_ok());

    // Invalid configs should fail
    let invalid_configs = vec![
        CollectorConfig::default().with_max_event_sources(0),
        CollectorConfig::default().with_event_buffer_size(0),
        CollectorConfig::default().with_shutdown_timeout(Duration::ZERO),
    ];

    for config in invalid_configs {
        assert!(config.validate().is_err());
    }
}

#[tokio::test]
async fn test_source_capabilities() {
    // Test individual capabilities
    assert_eq!(SourceCaps::PROCESS.bits(), 1);
    assert_eq!(SourceCaps::NETWORK.bits(), 2);
    assert_eq!(SourceCaps::FILESYSTEM.bits(), 4);
    assert_eq!(SourceCaps::PERFORMANCE.bits(), 8);

    // Test capability combinations
    let combined = SourceCaps::PROCESS | SourceCaps::NETWORK | SourceCaps::REALTIME;
    assert!(combined.contains(SourceCaps::PROCESS));
    assert!(combined.contains(SourceCaps::NETWORK));
    assert!(combined.contains(SourceCaps::REALTIME));
    assert!(!combined.contains(SourceCaps::FILESYSTEM));

    // Test capability intersection
    let caps1 = SourceCaps::PROCESS | SourceCaps::REALTIME;
    let caps2 = SourceCaps::PROCESS | SourceCaps::NETWORK;
    let intersection = caps1 & caps2;
    assert_eq!(intersection, SourceCaps::PROCESS);
}

#[tokio::test]
async fn test_event_types() {
    let timestamp = SystemTime::now();

    // Test process event
    let process_event = ProcessEvent {
        pid: 1234,
        ppid: Some(1),
        name: "test_process".to_string(),
        executable_path: Some("/usr/bin/test".to_string()),
        command_line: vec!["test".to_string(), "--flag".to_string()],
        start_time: Some(timestamp),
        cpu_usage: Some(5.5),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("abc123".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp,
    };

    let collection_event = CollectionEvent::Process(process_event);

    // Test event methods
    assert_eq!(collection_event.event_type(), "process");
    assert_eq!(collection_event.pid(), Some(1234));
    assert_eq!(collection_event.timestamp(), timestamp);
}

#[tokio::test]
async fn test_config_builder_pattern() {
    let config = CollectorConfig::new()
        .with_max_event_sources(32)
        .with_event_buffer_size(2000)
        .with_shutdown_timeout(Duration::from_secs(60))
        .with_health_check_interval(Duration::from_secs(120))
        .with_debug_logging(true);

    assert_eq!(config.max_event_sources, 32);
    assert_eq!(config.event_buffer_size, 2000);
    assert_eq!(config.shutdown_timeout, Duration::from_secs(60));
    assert_eq!(config.health_check_interval, Duration::from_secs(120));
    assert!(config.enable_debug_logging);

    // Validate the built config
    assert!(config.validate().is_ok());
}
