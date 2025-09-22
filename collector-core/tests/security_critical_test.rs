//! Security-critical tests for collector-core.
//!
//! This test suite focuses on security boundaries and isolation that are
//! essential for production deployment.

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
use tokio::sync::mpsc;

/// Test source that simulates potential security issues.
struct SecurityTestSource {
    name: &'static str,
    capabilities: SourceCaps,
    events_sent: Arc<AtomicUsize>,
    should_fail: bool,
}

impl SecurityTestSource {
    fn new(name: &'static str, capabilities: SourceCaps, should_fail: bool) -> Self {
        Self {
            name,
            capabilities,
            events_sent: Arc::new(AtomicUsize::new(0)),
            should_fail,
        }
    }
}

#[async_trait]
impl EventSource for SecurityTestSource {
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
        if self.should_fail {
            return Err(anyhow::anyhow!("Simulated security failure"));
        }

        // Generate a few test events
        for i in 0..5 {
            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i,
                ppid: Some(1),
                name: format!("secure-process-{}", i),
                executable_path: Some("/usr/bin/secure".to_string()),
                command_line: vec!["secure".to_string(), "--test".to_string(), i.to_string()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(0.05),
                memory_usage: Some(512),
                executable_hash: Some("secure123".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
            });

            if tx.send(event).await.is_err() {
                break;
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_source_isolation() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(100);

    let mut collector = Collector::new(config);

    // Add a working source
    let working_source = SecurityTestSource::new("working", SourceCaps::PROCESS, false);
    collector.register(Box::new(working_source)).unwrap();

    // Add a failing source
    let failing_source = SecurityTestSource::new("failing", SourceCaps::NETWORK, true);
    collector.register(Box::new(failing_source)).unwrap();

    // Test that both sources can be registered without issues
    assert!(collector.capabilities().contains(SourceCaps::PROCESS));
    assert!(collector.capabilities().contains(SourceCaps::NETWORK));

    // Test that sources can be created independently
    let working_test = SecurityTestSource::new("working-test", SourceCaps::PROCESS, false);
    let failing_test = SecurityTestSource::new("failing-test", SourceCaps::NETWORK, true);

    // Test that the working source can be started
    let (tx, _rx) = mpsc::channel(100);
    let result = working_test
        .start(tx, Arc::new(AtomicBool::new(false)))
        .await;
    assert!(result.is_ok(), "Working source should start successfully");

    // Test that the failing source fails as expected
    let (tx, _rx) = mpsc::channel(100);
    let result = failing_test
        .start(tx, Arc::new(AtomicBool::new(false)))
        .await;
    assert!(result.is_err(), "Failing source should fail as expected");
}

#[tokio::test]
async fn test_capability_enforcement() {
    let config = CollectorConfig::default();
    let mut collector = Collector::new(config);

    // Test that sources can only access their declared capabilities
    let source = SecurityTestSource::new("capability-test", SourceCaps::PROCESS, false);
    let _events_sent = source.events_sent.clone();

    collector.register(Box::new(source)).unwrap();

    // Verify the source only has PROCESS capabilities
    let capabilities = collector.capabilities();
    assert!(
        capabilities.contains(SourceCaps::PROCESS),
        "Should have PROCESS capability"
    );
    assert!(
        !capabilities.contains(SourceCaps::NETWORK),
        "Should not have NETWORK capability"
    );
    assert!(
        !capabilities.contains(SourceCaps::FILESYSTEM),
        "Should not have FILESYSTEM capability"
    );
}

#[tokio::test]
async fn test_max_sources_security() {
    let config = CollectorConfig::default().with_max_event_sources(1);
    let mut collector = Collector::new(config);

    // Register first source (should succeed)
    let source1 = SecurityTestSource::new("source1", SourceCaps::PROCESS, false);
    let result = collector.register(Box::new(source1));
    assert!(result.is_ok(), "First source should register successfully");

    // Try to register second source (should fail)
    let source2 = SecurityTestSource::new("source2", SourceCaps::NETWORK, false);
    let result = collector.register(Box::new(source2));
    assert!(result.is_err(), "Second source should be rejected");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Cannot register more than")
    );
}
