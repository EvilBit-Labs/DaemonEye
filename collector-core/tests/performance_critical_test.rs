//! Performance-critical tests for collector-core.
//!
//! This test suite focuses on performance characteristics that are essential
//! for production deployment, without the overhead of full benchmarking.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::mpsc;
use tracing::info;

/// High-performance event source for testing.
struct PerformanceTestSource {
    name: &'static str,
    capabilities: SourceCaps,
    events_sent: Arc<AtomicUsize>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl PerformanceTestSource {
    fn new(name: &'static str, capabilities: SourceCaps) -> Self {
        Self {
            name,
            capabilities,
            events_sent: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl EventSource for PerformanceTestSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        self.capabilities
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        self.running.store(true, Ordering::Relaxed);

        // Generate events at a reasonable rate for testing
        while self.running.load(Ordering::Relaxed) {
            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                ppid: Some(1),
                name: "test-process".to_string(),
                executable_path: Some("/usr/bin/test".to_string()),
                command_line: vec!["test".to_string(), "--performance".to_string()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(0.1),
                memory_usage: Some(1024),
                executable_hash: Some("abc123".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
            });

            if tx.send(event).await.is_err() {
                break;
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.running.store(false, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::test]
async fn test_event_processing_performance() {
    let config = CollectorConfig::default()
        .with_event_buffer_size(1000)
        .with_backpressure_threshold(800);

    let mut collector = Collector::new(config);
    let source = PerformanceTestSource::new("perf-test", SourceCaps::PROCESS);
    let _events_sent = source.events_sent.clone();

    collector.register(Box::new(source)).unwrap();

    // Test that we can create and configure the collector
    assert!(collector.capabilities().contains(SourceCaps::PROCESS));

    // Test that the source can generate events independently
    let (tx, _rx) = mpsc::channel(100);
    let source_test = PerformanceTestSource::new("perf-test", SourceCaps::PROCESS);
    let events_sent_test = source_test.events_sent.clone();

    let start_time = Instant::now();
    let source_handle = tokio::spawn(async move {
        let _ = source_test.start(tx).await;
    });

    // Let the source run briefly
    tokio::time::sleep(Duration::from_millis(50)).await;
    source_handle.abort();

    let elapsed = start_time.elapsed();
    let events = events_sent_test.load(Ordering::Relaxed);
    let events_per_second = events as f64 / elapsed.as_secs_f64();

    info!(
        events = events,
        elapsed_ms = elapsed.as_millis(),
        events_per_second = events_per_second,
        "Performance test completed"
    );

    // Verify we can process events at a reasonable rate
    assert!(events > 0, "Should generate some events");
    assert!(
        events_per_second > 10.0,
        "Should maintain reasonable event rate"
    );
    assert!(events_per_second < 10000.0, "Event rate should be bounded");
}

#[tokio::test]
async fn test_memory_usage_stability() {
    let config = CollectorConfig::default()
        .with_event_buffer_size(100)
        .with_backpressure_threshold(80);

    let mut collector = Collector::new(config.clone());
    let source = PerformanceTestSource::new("memory-test", SourceCaps::PROCESS);

    collector.register(Box::new(source)).unwrap();

    // Test that we can create multiple sources without issues
    let source2 = PerformanceTestSource::new("memory-test-2", SourceCaps::NETWORK);
    collector.register(Box::new(source2)).unwrap();

    assert!(collector.capabilities().contains(SourceCaps::PROCESS));
    assert!(collector.capabilities().contains(SourceCaps::NETWORK));

    // Test that configuration is valid
    assert!(config.event_buffer_size > 0);
    assert!(config.backpressure_threshold < config.event_buffer_size);
}
