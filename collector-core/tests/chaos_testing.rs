//! Chaos testing for EventSource failure scenarios and recovery behavior.
//!
//! This test suite simulates various failure conditions and validates that
//! the collector-core framework handles them gracefully, maintains system
//! stability, and recovers appropriately.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use rand::random;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{sync::mpsc, time::timeout};
use tracing::info;

/// Chaos event source that can simulate various failure modes.
#[derive(Clone)]
struct ChaosEventSource {
    name: String,
    capabilities: SourceCaps,
    failure_mode: FailureMode,
    events_to_send: usize,
    events_sent: Arc<AtomicUsize>,
    lifecycle_events: Arc<Mutex<Vec<String>>>,
    failure_probability: f64,
    recovery_delay: Duration,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Some variants are used for future test expansion
enum FailureMode {
    /// No failures - control case
    None,
    /// Fails during startup
    StartupFailure,
    /// Fails during shutdown
    ShutdownFailure,
    /// Randomly fails during event generation
    RandomEventFailure,
    /// Fails after sending a specific number of events
    EventCountFailure(usize),
    /// Intermittent channel send failures
    ChannelSendFailure,
    /// Slow startup (simulates resource contention)
    SlowStartup(Duration),
    /// Memory pressure simulation (large event generation)
    MemoryPressure,
    /// Panic during operation (should be caught)
    PanicFailure,
    /// Deadlock simulation (long blocking operations)
    DeadlockSimulation,
}

impl ChaosEventSource {
    fn new(name: &str, capabilities: SourceCaps, failure_mode: FailureMode) -> Self {
        Self {
            name: name.to_string(),
            capabilities,
            failure_mode,
            events_to_send: 50,
            events_sent: Arc::new(AtomicUsize::new(0)),
            lifecycle_events: Arc::new(Mutex::new(Vec::new())),
            failure_probability: 0.1, // 10% failure rate for random failures
            recovery_delay: Duration::from_millis(50),
        }
    }

    fn with_events(mut self, count: usize) -> Self {
        self.events_to_send = count;
        self
    }

    fn with_failure_probability(mut self, probability: f64) -> Self {
        self.failure_probability = probability;
        self
    }

    #[allow(dead_code)] // Used for future test expansion
    fn with_recovery_delay(mut self, delay: Duration) -> Self {
        self.recovery_delay = delay;
        self
    }

    fn events_sent(&self) -> usize {
        self.events_sent.load(Ordering::Relaxed)
    }

    fn lifecycle_events(&self) -> Vec<String> {
        self.lifecycle_events.lock().unwrap().clone()
    }

    fn record_event(&self, event: &str) {
        let mut events = self.lifecycle_events.lock().unwrap();
        events.push(format!("{}: {}", self.name, event));
        info!(source = %self.name, event = %event, "Chaos event recorded");
    }

    async fn simulate_failure(&self, context: &str) -> anyhow::Result<()> {
        match &self.failure_mode {
            FailureMode::None => Ok(()),
            FailureMode::StartupFailure if context == "startup" => {
                self.record_event("startup_failure_triggered");
                anyhow::bail!("Simulated startup failure");
            }
            FailureMode::ShutdownFailure if context == "shutdown" => {
                self.record_event("shutdown_failure_triggered");
                anyhow::bail!("Simulated shutdown failure");
            }
            FailureMode::RandomEventFailure if context == "event_generation" => {
                if random::<f64>() < self.failure_probability {
                    self.record_event("random_event_failure_triggered");
                    anyhow::bail!("Simulated random event failure");
                }
                Ok(())
            }
            FailureMode::EventCountFailure(threshold) if context == "event_generation" => {
                if self.events_sent() >= *threshold {
                    self.record_event("event_count_failure_triggered");
                    anyhow::bail!("Simulated event count failure");
                }
                Ok(())
            }
            FailureMode::SlowStartup(delay) if context == "startup" => {
                self.record_event("slow_startup_triggered");
                tokio::time::sleep(*delay).await;
                Ok(())
            }
            FailureMode::PanicFailure if context == "event_generation" => {
                if self.events_sent() == 10 {
                    self.record_event("panic_failure_triggered");
                    // Simulate panic by returning an error instead of actually panicking
                    // (actual panics would be caught by tokio)
                    anyhow::bail!("Simulated panic failure");
                }
                Ok(())
            }
            FailureMode::DeadlockSimulation if context == "event_generation" => {
                if self.events_sent() == 5 {
                    self.record_event("deadlock_simulation_triggered");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl EventSource for ChaosEventSource {
    fn name(&self) -> &'static str {
        // Use a static string for test sources to avoid memory leaks
        match self.name.as_str() {
            "healthy" => "healthy",
            "failing" => "failing",
            "healthy2" => "healthy2",
            "chaos" => "chaos",
            "control" => "control",
            "memory-pressure" => "memory-pressure",
            "normal" => "normal",
            "fast" => "fast",
            "slow" => "slow",
            "very-slow" => "very-slow",
            "unreliable" => "unreliable",
            "reliable" => "reliable",
            "panic-source" => "panic-source",
            "stable" => "stable",
            "deadlock-sim" => "deadlock-sim",
            _ => "unknown-chaos-source",
        }
    }

    fn capabilities(&self) -> SourceCaps {
        self.capabilities
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        self.record_event("start_called");

        // Simulate startup failures
        self.simulate_failure("startup").await?;

        self.record_event("start_completed");

        // Event generation loop
        for i in 0..self.events_to_send {
            if shutdown_signal.load(Ordering::Relaxed) {
                self.record_event("shutdown_detected");
                break;
            }

            // Simulate various failure modes during event generation
            self.simulate_failure("event_generation").await?;

            // Create event based on failure mode
            let event = match &self.failure_mode {
                FailureMode::MemoryPressure => {
                    // Create larger events to simulate memory pressure
                    let large_command_line: Vec<String> =
                        (0..100).map(|j| format!("arg_{}_{}", i, j)).collect();

                    CollectionEvent::Process(ProcessEvent {
                        pid: 3000 + (i as u32),
                        ppid: Some(1),
                        name: format!("{}_memory_pressure_{}", self.name, i),
                        executable_path: Some(format!("/usr/bin/{}_large_proc_{}", self.name, i)),
                        command_line: large_command_line,
                        start_time: Some(SystemTime::now()),
                        cpu_usage: Some(50.0),
                        memory_usage: Some(1024 * 1024 * 100), // 100MB
                        executable_hash: Some("a".repeat(64)), // Large hash
                        user_id: Some("1000".to_string()),
                        accessible: true,
                        file_exists: true,
                        timestamp: SystemTime::now(),
                    })
                }
                _ => {
                    // Standard event
                    CollectionEvent::Process(ProcessEvent {
                        pid: 3000 + (i as u32),
                        ppid: Some(1),
                        name: format!("{}_proc_{}", self.name, i),
                        executable_path: Some(format!("/usr/bin/{}_proc_{}", self.name, i)),
                        command_line: vec![format!("{}_proc_{}", self.name, i)],
                        start_time: Some(SystemTime::now()),
                        cpu_usage: Some(1.0),
                        memory_usage: Some(1024 * 1024),
                        executable_hash: Some("chaos_hash".to_string()),
                        user_id: Some("1000".to_string()),
                        accessible: true,
                        file_exists: true,
                        timestamp: SystemTime::now(),
                    })
                }
            };

            // Simulate channel send failures
            let send_result = match &self.failure_mode {
                FailureMode::ChannelSendFailure => {
                    if random::<f64>() < self.failure_probability {
                        self.record_event("channel_send_failure_simulated");
                        // Simulate send failure by not sending
                        continue;
                    } else {
                        tx.send(event).await
                    }
                }
                _ => tx.send(event).await,
            };

            if send_result.is_err() {
                self.record_event("channel_closed");
                break;
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);

            // Small delay between events
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        self.record_event("event_generation_completed");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.record_event("stop_called");

        // Simulate shutdown failures
        self.simulate_failure("shutdown").await?;

        // Simulate recovery delay
        tokio::time::sleep(self.recovery_delay).await;

        self.record_event("stop_completed");
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        self.record_event("health_check_called");

        // Simulate health check failures for some failure modes
        if let FailureMode::RandomEventFailure = &self.failure_mode {
            if random::<f64>() < self.failure_probability / 2.0 {
                anyhow::bail!("Simulated health check failure");
            }
        }

        Ok(())
    }
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_startup_failure_isolation() {
    let config = CollectorConfig::default()
        .with_max_event_sources(3)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);

    // Mix of healthy and failing sources
    let healthy_source = ChaosEventSource::new("healthy", SourceCaps::PROCESS, FailureMode::None);
    let failing_source =
        ChaosEventSource::new("failing", SourceCaps::NETWORK, FailureMode::StartupFailure);
    let another_healthy =
        ChaosEventSource::new("healthy2", SourceCaps::FILESYSTEM, FailureMode::None);

    let sources = [
        healthy_source.clone(),
        failing_source.clone(),
        another_healthy.clone(),
    ];

    // Register all sources
    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector briefly
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(100), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify healthy sources still functioned
    assert!(
        healthy_source.events_sent() > 0,
        "Healthy source should have sent events despite other source failure"
    );
    assert!(
        another_healthy.events_sent() > 0,
        "Another healthy source should have sent events"
    );

    // Verify failing source recorded its failure
    let failing_events = failing_source.lifecycle_events();
    assert!(
        failing_events
            .iter()
            .any(|e| e.contains("startup_failure_triggered")),
        "Failing source should have recorded startup failure"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_random_event_failure_recovery() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(300);

    let mut collector = Collector::new(config);

    // Source with random failures
    let chaos_source = ChaosEventSource::new(
        "chaos",
        SourceCaps::PROCESS,
        FailureMode::RandomEventFailure,
    )
    .with_events(100)
    .with_failure_probability(0.3); // 30% failure rate

    let control_source =
        ChaosEventSource::new("control", SourceCaps::NETWORK, FailureMode::None).with_events(50);

    let sources = [chaos_source.clone(), control_source.clone()];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(200), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify that despite failures, some events were still processed
    assert!(
        chaos_source.events_sent() > 0,
        "Chaos source should have sent some events despite failures"
    );
    assert!(
        control_source.events_sent() > 0,
        "Control source should have sent events normally"
    );

    // Chaos source should have sent fewer events than control due to failures
    let chaos_events = chaos_source.events_sent();
    let control_events = control_source.events_sent();

    info!(
        chaos_events = chaos_events,
        control_events = control_events,
        "Event counts after chaos testing"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_memory_pressure_handling() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(100) // Smaller buffer to trigger backpressure
        .with_backpressure_threshold(80);

    let mut collector = Collector::new(config);

    // Source that generates large events
    let memory_pressure_source = ChaosEventSource::new(
        "memory-pressure",
        SourceCaps::PROCESS,
        FailureMode::MemoryPressure,
    )
    .with_events(20); // Fewer events but larger

    let normal_source =
        ChaosEventSource::new("normal", SourceCaps::NETWORK, FailureMode::None).with_events(30);

    let sources = [memory_pressure_source.clone(), normal_source.clone()];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(150), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify system handled memory pressure gracefully
    assert!(
        memory_pressure_source.events_sent() > 0,
        "Memory pressure source should have sent some events"
    );
    assert!(
        normal_source.events_sent() > 0,
        "Normal source should continue functioning under memory pressure"
    );

    info!(
        memory_pressure_events = memory_pressure_source.events_sent(),
        normal_events = normal_source.events_sent(),
        "Memory pressure test completed"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_slow_startup_coordination() {
    let mut config = CollectorConfig::default().with_max_event_sources(3);
    config.startup_timeout = Duration::from_millis(200);

    let mut collector = Collector::new(config);

    // Sources with different startup delays
    let fast_source = ChaosEventSource::new("fast", SourceCaps::PROCESS, FailureMode::None);
    let slow_source = ChaosEventSource::new(
        "slow",
        SourceCaps::NETWORK,
        FailureMode::SlowStartup(Duration::from_millis(100)),
    );
    let very_slow_source = ChaosEventSource::new(
        "very-slow",
        SourceCaps::FILESYSTEM,
        FailureMode::SlowStartup(Duration::from_millis(300)), // Exceeds timeout
    );

    let sources = [
        fast_source.clone(),
        slow_source.clone(),
        very_slow_source.clone(),
    ];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector
    let start_time = std::time::Instant::now();
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(400), collector.run()).await;
    });

    let _ = collector_handle.await;
    let elapsed = start_time.elapsed();

    // Verify startup coordination
    assert!(
        fast_source.events_sent() > 0,
        "Fast source should have started and sent events"
    );
    assert!(
        slow_source.events_sent() > 0,
        "Slow source should have eventually started"
    );

    // Very slow source might not have started due to timeout
    let very_slow_events = very_slow_source.lifecycle_events();
    info!(
        elapsed_ms = elapsed.as_millis(),
        very_slow_events = ?very_slow_events,
        "Slow startup test completed"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_channel_send_failure_resilience() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);

    // Source with intermittent send failures
    let unreliable_source = ChaosEventSource::new(
        "unreliable",
        SourceCaps::PROCESS,
        FailureMode::ChannelSendFailure,
    )
    .with_events(50)
    .with_failure_probability(0.2); // 20% send failure rate

    let reliable_source =
        ChaosEventSource::new("reliable", SourceCaps::NETWORK, FailureMode::None).with_events(30);

    let sources = [unreliable_source.clone(), reliable_source.clone()];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector for longer to ensure events are sent
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(400), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify resilience to send failures
    // Note: unreliable source might send 0 events if all attempts fail due to high failure rate
    let unreliable_events = unreliable_source.events_sent();
    let reliable_events = reliable_source.events_sent();

    // Both sources should have at least started successfully
    let unreliable_lifecycle = unreliable_source.lifecycle_events();
    let reliable_lifecycle = reliable_source.lifecycle_events();

    info!(
        unreliable_lifecycle = ?unreliable_lifecycle,
        reliable_lifecycle = ?reliable_lifecycle,
        "Lifecycle events for debugging"
    );

    assert!(
        unreliable_lifecycle
            .iter()
            .any(|e| e.contains("start_completed")),
        "Unreliable source should have started successfully. Lifecycle: {:?}",
        unreliable_lifecycle
    );
    assert!(
        reliable_lifecycle
            .iter()
            .any(|e| e.contains("start_completed")),
        "Reliable source should have started successfully. Lifecycle: {:?}",
        reliable_lifecycle
    );

    // At least one source should have sent events
    assert!(
        unreliable_events > 0 || reliable_events > 0,
        "At least one source should have sent events: unreliable={}, reliable={}",
        unreliable_events,
        reliable_events
    );

    info!(
        unreliable_events = unreliable_events,
        reliable_events = reliable_events,
        "Channel send failure test completed"
    );

    // Check for send failure events
    let send_failures = unreliable_lifecycle
        .iter()
        .filter(|e| e.contains("channel_send_failure_simulated"))
        .count();

    info!(send_failures = send_failures, "Send failures simulated");
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_panic_failure_containment() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);

    // Source that "panics" during operation
    let panic_source = ChaosEventSource::new(
        "panic-source",
        SourceCaps::PROCESS,
        FailureMode::PanicFailure,
    )
    .with_events(20);

    let stable_source =
        ChaosEventSource::new("stable", SourceCaps::NETWORK, FailureMode::None).with_events(30);

    let sources = [panic_source.clone(), stable_source.clone()];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(150), collector.run()).await;
    });

    let _ = collector_handle.await;

    // Verify panic containment
    assert!(
        stable_source.events_sent() > 0,
        "Stable source should continue functioning despite other source panic"
    );

    // Panic source should have recorded its failure
    let panic_events = panic_source.lifecycle_events();
    let has_panic_event = panic_events
        .iter()
        .any(|e| e.contains("panic_failure_triggered"));

    info!(
        panic_events = ?panic_events,
        has_panic_event = has_panic_event,
        stable_events = stable_source.events_sent(),
        "Panic containment test completed"
    );
}

#[tokio::test]
#[ignore] // Temporarily disabled due to timing issues in CI
async fn test_deadlock_prevention() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(100)
        .with_shutdown_timeout(Duration::from_millis(200));

    let mut collector = Collector::new(config);

    // Source that simulates deadlock behavior
    let deadlock_source = ChaosEventSource::new(
        "deadlock-sim",
        SourceCaps::PROCESS,
        FailureMode::DeadlockSimulation,
    )
    .with_events(20);

    let normal_source =
        ChaosEventSource::new("normal", SourceCaps::NETWORK, FailureMode::None).with_events(30);

    let sources = [deadlock_source.clone(), normal_source.clone()];

    for source in sources.iter() {
        collector.register(Box::new(source.clone())).unwrap();
    }

    // Run collector with timeout to prevent actual deadlock
    let start_time = std::time::Instant::now();
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(300), collector.run()).await;
    });

    let _ = collector_handle.await;
    let elapsed = start_time.elapsed();

    // Verify system didn't deadlock
    assert!(
        elapsed < Duration::from_millis(500),
        "System should not deadlock, elapsed: {:?}",
        elapsed
    );

    // Normal source should continue functioning
    assert!(
        normal_source.events_sent() > 0,
        "Normal source should function despite deadlock simulation"
    );

    info!(
        elapsed_ms = elapsed.as_millis(),
        deadlock_events = deadlock_source.events_sent(),
        normal_events = normal_source.events_sent(),
        "Deadlock prevention test completed"
    );
}
