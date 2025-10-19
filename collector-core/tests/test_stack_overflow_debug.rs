use collector_core::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_simple_collector_debug() {
    // Create a very simple collector configuration
    let collector_config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(100)
        .with_backpressure_threshold(50) // Must be less than event_buffer_size
        .with_telemetry(false);

    let mut collector = Collector::new(collector_config);

    // Create a simple mock source that doesn't generate many events
    let simple_source = SimpleTestSource::new();
    collector.register(Box::new(simple_source)).unwrap();

    // Run with a very short timeout to see if we can isolate the issue
    let collector_handle = tokio::spawn(async move {
        let timeout_res = timeout(Duration::from_millis(100), collector.run()).await;

        // Assert that the collector timed out, meaning it was still running
        assert!(
            timeout_res.is_err(),
            "Collector unexpectedly completed instead of timing out: {:?}",
            timeout_res
        );

        timeout_res
    });

    let result = collector_handle.await;
    println!("Test result: {:?}", result);

    // Verify the test properly detected timeout
    match result {
        Ok(Err(_)) => {
            // Successfully timed out as expected
            println!("Collector properly timed out as expected");
        }
        Ok(Ok(_)) => {
            panic!("Collector completed unexpectedly when it should have timed out");
        }
        Err(e) => {
            panic!("Collector task panicked: {:?}", e);
        }
    }
}

// Simple test source that generates minimal events
struct SimpleTestSource;

impl SimpleTestSource {
    fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl EventSource for SimpleTestSource {
    fn name(&self) -> &'static str {
        "simple-test"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        println!("SimpleTestSource starting");

        // Generate just a few events
        for i in 0..5 {
            if shutdown_signal.load(Ordering::Relaxed) {
                println!("Shutdown detected at event {}", i);
                break;
            }

            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i as u32,
                ppid: Some(1),
                name: format!("test_proc_{}", i),
                executable_path: Some(format!("/usr/bin/test_{}", i)),
                command_line: vec![format!("test_{}", i)],
                start_time: Some(std::time::SystemTime::now()),
                cpu_usage: Some(1.0),
                memory_usage: Some(1024 * 1024),
                executable_hash: Some(format!("hash_{}", i)),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: std::time::SystemTime::now(),
                platform_metadata: None,
            });

            if tx.send(event).await.is_err() {
                println!("Channel closed at event {}", i);
                break;
            }

            println!("Sent event {}", i);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        println!("SimpleTestSource completed");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        println!("SimpleTestSource stopping");
        Ok(())
    }
}
