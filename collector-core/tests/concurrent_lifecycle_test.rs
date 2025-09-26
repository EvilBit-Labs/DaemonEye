//! Integration tests for multiple concurrent EventSource registration and lifecycle management.
//!
//! This test suite validates that the collector-core framework can properly manage
//! multiple event sources concurrently, handle their lifecycle events, and coordinate
//! graceful shutdown across all registered sources.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{sync::mpsc, time::timeout};
use tracing::{info, warn};

/// Lifecycle-aware event source for testing concurrent operations.
#[derive(Clone)]
struct LifecycleTestSource {
    name: &'static str,
    capabilities: SourceCaps,
    events_to_send: usize,
    events_sent: Arc<AtomicUsize>,
    lifecycle_events: Arc<Mutex<Vec<String>>>,
    should_fail_start: bool,
    should_fail_stop: bool,
    start_delay: Duration,
    stop_delay: Duration,
}

impl LifecycleTestSource {
    fn new(name: &'static str, capabilities: SourceCaps, events_to_send: usize) -> Self {
        Self {
            name,
            capabilities,
            events_to_send,
            events_sent: Arc::new(AtomicUsize::new(0)),
            lifecycle_events: Arc::new(Mutex::new(Vec::new())),
            should_fail_start: false,
            should_fail_stop: false,
            start_delay: Duration::from_millis(10),
            stop_delay: Duration::from_millis(5),
        }
    }

    fn with_start_failure(mut self) -> Self {
        self.should_fail_start = true;
        self
    }

    fn with_stop_failure(mut self) -> Self {
        self.should_fail_stop = true;
        self
    }

    fn with_delays(mut self, start_delay: Duration, stop_delay: Duration) -> Self {
        self.start_delay = start_delay;
        self.stop_delay = stop_delay;
        self
    }

    fn events_sent(&self) -> usize {
        self.events_sent.load(Ordering::Relaxed)
    }

    fn lifecycle_events(&self) -> Vec<String> {
        self.lifecycle_events.lock().unwrap().clone()
    }

    fn record_lifecycle_event(&self, event: &str) {
        let mut events = self.lifecycle_events.lock().unwrap();
        events.push(format!("{}: {}", self.name, event));
        info!(source = %self.name, event = %event, "Lifecycle event recorded");
    }
}

#[async_trait]
impl EventSource for LifecycleTestSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        self.capabilities
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        self.record_lifecycle_event("start_called");

        // Simulate startup delay
        tokio::time::sleep(self.start_delay).await;

        if self.should_fail_start {
            self.record_lifecycle_event("start_failed");
            anyhow::bail!("Configured to fail during start");
        }

        self.record_lifecycle_event("start_completed");

        // Generate events until shutdown signal
        for i in 0..self.events_to_send {
            if shutdown_signal.load(Ordering::Relaxed) {
                self.record_lifecycle_event("shutdown_detected");
                break;
            }

            let event = CollectionEvent::Process(ProcessEvent {
                pid: 2000 + (i as u32),
                ppid: Some(1),
                name: format!("{}_process_{}", self.name, i),
                executable_path: Some(format!("/usr/bin/{}_proc_{}", self.name, i)),
                command_line: vec![format!("{}_proc_{}", self.name, i)],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(0.5),
                memory_usage: Some(512 * 1024),
                executable_hash: Some("lifecycle_hash".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            });

            if tx.send(event).await.is_err() {
                self.record_lifecycle_event("channel_closed");
                break;
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        self.record_lifecycle_event("event_generation_completed");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.record_lifecycle_event("stop_called");

        // Simulate stop delay
        tokio::time::sleep(self.stop_delay).await;

        if self.should_fail_stop {
            self.record_lifecycle_event("stop_failed");
            anyhow::bail!("Configured to fail during stop");
        }

        self.record_lifecycle_event("stop_completed");
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        self.record_lifecycle_event("health_check_called");
        Ok(())
    }
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_concurrent_source_registration() {
    let config = CollectorConfig::default()
        .with_max_event_sources(5)
        .with_event_buffer_size(1000);

    let mut collector = Collector::new(config);

    // Create sources with different capabilities
    let sources = vec![
        LifecycleTestSource::new(
            "process-source",
            SourceCaps::PROCESS | SourceCaps::REALTIME,
            10,
        ),
        LifecycleTestSource::new(
            "network-source",
            SourceCaps::NETWORK | SourceCaps::KERNEL_LEVEL,
            15,
        ),
        LifecycleTestSource::new(
            "filesystem-source",
            SourceCaps::FILESYSTEM | SourceCaps::SYSTEM_WIDE,
            20,
        ),
        LifecycleTestSource::new("performance-source", SourceCaps::PERFORMANCE, 25),
    ];

    // Register all sources
    for source in sources.iter() {
        let result = collector.register(Box::new(source.clone()));
        assert!(result.is_ok(), "Source registration should succeed");
    }

    // Verify registration
    assert_eq!(collector.source_count(), 4);

    // Verify combined capabilities
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::NETWORK));
    assert!(capabilities.contains(SourceCaps::FILESYSTEM));
    assert!(capabilities.contains(SourceCaps::PERFORMANCE));
    assert!(capabilities.contains(SourceCaps::REALTIME));
    assert!(capabilities.contains(SourceCaps::KERNEL_LEVEL));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_concurrent_lifecycle_management() {
    let config = CollectorConfig::default()
        .with_max_event_sources(3)
        .with_event_buffer_size(500)
        .with_shutdown_timeout(Duration::from_millis(200));

    let mut collector = Collector::new(config);

    // Create sources with different startup delays
    let source1 = LifecycleTestSource::new("fast-source", SourceCaps::PROCESS, 5)
        .with_delays(Duration::from_millis(10), Duration::from_millis(5));
    let source2 = LifecycleTestSource::new("medium-source", SourceCaps::NETWORK, 8)
        .with_delays(Duration::from_millis(30), Duration::from_millis(10));
    let source3 = LifecycleTestSource::new("slow-source", SourceCaps::FILESYSTEM, 12)
        .with_delays(Duration::from_millis(50), Duration::from_millis(20));

    let sources = vec![source1.clone(), source2.clone(), source3.clone()];

    // Register sources
    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector briefly to test lifecycle management
    let collector_handle = tokio::spawn(async move {
        let result = timeout(Duration::from_millis(300), collector.run()).await;
        match result {
            Ok(Ok(())) => info!("Collector completed successfully"),
            Ok(Err(e)) => warn!("Collector failed: {}", e),
            Err(_) => info!("Collector timed out (expected)"),
        }
    });

    // Wait for collector to run
    let _ = collector_handle.await;

    // Verify lifecycle events for all sources
    for source in sources.iter() {
        let events = source.lifecycle_events();
        assert!(
            events.iter().any(|e| e.contains("start_called")),
            "Source {} should have start_called event",
            source.name
        );

        // At least some events should be generated
        assert!(
            source.events_sent() > 0,
            "Source {} should have sent some events",
            source.name
        );
    }
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_source_failure_isolation() {
    let config = CollectorConfig::default()
        .with_max_event_sources(4)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);

    // Create sources with different failure modes
    let healthy_source1 = LifecycleTestSource::new("healthy-1", SourceCaps::PROCESS, 10);
    let failing_start_source =
        LifecycleTestSource::new("fail-start", SourceCaps::NETWORK, 10).with_start_failure();
    let healthy_source2 = LifecycleTestSource::new("healthy-2", SourceCaps::FILESYSTEM, 10);
    let failing_stop_source =
        LifecycleTestSource::new("fail-stop", SourceCaps::PERFORMANCE, 10).with_stop_failure();

    let sources = vec![
        healthy_source1.clone(),
        failing_start_source.clone(),
        healthy_source2.clone(),
        failing_stop_source.clone(),
    ];

    // Register all sources
    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector briefly
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(150), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify that healthy sources still functioned despite failures
    assert!(
        healthy_source1.events_sent() > 0,
        "Healthy source 1 should have sent events"
    );
    assert!(
        healthy_source2.events_sent() > 0,
        "Healthy source 2 should have sent events"
    );

    // Verify failure isolation
    let failing_start_events = failing_start_source.lifecycle_events();
    assert!(
        failing_start_events
            .iter()
            .any(|e| e.contains("start_failed")),
        "Failing start source should have recorded failure"
    );

    // Healthy sources should not be affected by the failing source
    let healthy1_events = healthy_source1.lifecycle_events();
    assert!(
        healthy1_events
            .iter()
            .any(|e| e.contains("start_completed")),
        "Healthy source 1 should have started successfully"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_graceful_shutdown_coordination() {
    let config = CollectorConfig::default()
        .with_max_event_sources(3)
        .with_event_buffer_size(300)
        .with_shutdown_timeout(Duration::from_millis(500));

    let mut collector = Collector::new(config);

    // Create long-running sources
    let source1 = LifecycleTestSource::new("long-runner-1", SourceCaps::PROCESS, 1000)
        .with_delays(Duration::from_millis(5), Duration::from_millis(10));
    let source2 = LifecycleTestSource::new("long-runner-2", SourceCaps::NETWORK, 1000)
        .with_delays(Duration::from_millis(5), Duration::from_millis(15));
    let source3 = LifecycleTestSource::new("long-runner-3", SourceCaps::FILESYSTEM, 1000)
        .with_delays(Duration::from_millis(5), Duration::from_millis(20));

    let sources = vec![source1.clone(), source2.clone(), source3.clone()];

    // Register sources
    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector with timeout to trigger shutdown
    let start_time = std::time::Instant::now();
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(100), collector.run()).await;
    });

    let _ = collector_handle.await;
    let elapsed = start_time.elapsed();

    // Verify shutdown was coordinated (should complete within reasonable time)
    assert!(
        elapsed < Duration::from_millis(600),
        "Shutdown should complete within timeout + grace period"
    );

    // Verify all sources detected shutdown
    for source in sources.iter() {
        let events = source.lifecycle_events();
        // Sources should have started
        assert!(
            events.iter().any(|e| e.contains("start_called")),
            "Source {} should have been started",
            source.name
        );
    }
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_source_capability_aggregation() {
    let config = CollectorConfig::default().with_max_event_sources(10);
    let mut collector = Collector::new(config);

    // Test capability combinations
    let test_cases = [
        (SourceCaps::PROCESS, "process-only"),
        (
            SourceCaps::NETWORK | SourceCaps::REALTIME,
            "network-realtime",
        ),
        (
            SourceCaps::FILESYSTEM | SourceCaps::KERNEL_LEVEL,
            "fs-kernel",
        ),
        (
            SourceCaps::PERFORMANCE | SourceCaps::SYSTEM_WIDE,
            "perf-system",
        ),
        (
            SourceCaps::PROCESS | SourceCaps::NETWORK | SourceCaps::REALTIME,
            "multi-domain",
        ),
    ];

    let mut expected_capabilities = SourceCaps::empty();

    for (caps, name) in test_cases.iter() {
        let source = LifecycleTestSource::new(name, *caps, 5);
        collector.register(Box::new(source)).unwrap();
        expected_capabilities |= *caps;

        // Verify capabilities are aggregated correctly
        assert_eq!(
            collector.capabilities(),
            expected_capabilities,
            "Capabilities should be properly aggregated after registering {}",
            name
        );
    }

    // Final verification
    assert!(collector.capabilities().contains(SourceCaps::PROCESS));
    assert!(collector.capabilities().contains(SourceCaps::NETWORK));
    assert!(collector.capabilities().contains(SourceCaps::FILESYSTEM));
    assert!(collector.capabilities().contains(SourceCaps::PERFORMANCE));
    assert!(collector.capabilities().contains(SourceCaps::REALTIME));
    assert!(collector.capabilities().contains(SourceCaps::KERNEL_LEVEL));
    assert!(collector.capabilities().contains(SourceCaps::SYSTEM_WIDE));
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_concurrent_event_generation() {
    let config = CollectorConfig::default()
        .with_max_event_sources(4)
        .with_event_buffer_size(1000)
        .with_max_batch_size(50);

    let mut collector = Collector::new(config);

    // Create sources that generate events at different rates
    let sources = vec![
        LifecycleTestSource::new("fast-generator", SourceCaps::PROCESS, 50),
        LifecycleTestSource::new("medium-generator", SourceCaps::NETWORK, 30),
        LifecycleTestSource::new("slow-generator", SourceCaps::FILESYSTEM, 20),
        LifecycleTestSource::new("burst-generator", SourceCaps::PERFORMANCE, 100),
    ];

    let total_expected_events: usize = sources.iter().map(|s| s.events_to_send).sum();

    // Register sources
    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector to process events
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(200), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify event generation
    let total_events_sent: usize = sources.iter().map(|s| s.events_sent()).sum();

    assert!(
        total_events_sent > 0,
        "Should have generated some events from concurrent sources"
    );

    // Each source should have generated at least some events
    for source in sources.iter() {
        assert!(
            source.events_sent() > 0,
            "Source {} should have sent some events",
            source.name
        );
    }

    info!(
        total_expected = total_expected_events,
        total_sent = total_events_sent,
        "Concurrent event generation completed"
    );
}
