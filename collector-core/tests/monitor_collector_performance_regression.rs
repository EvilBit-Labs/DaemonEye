//! Performance regression tests with baseline validation for Monitor Collectors.
//!
//! This test suite implements comprehensive performance monitoring and regression
//! detection for Monitor Collector components, establishing baselines and detecting
//! performance degradation over time.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, MonitorCollector,
    MonitorCollectorConfig, MonitorCollectorStats, PerformanceConfig, PerformanceMonitor,
    ProcessEvent, SourceCaps,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::mpsc, time::timeout};
use tracing::info;

/// Performance test monitor collector with configurable load characteristics.
#[derive(Clone)]
struct PerformanceTestMonitorCollector {
    name: &'static str,
    capabilities: SourceCaps,
    #[allow(dead_code)]
    config: MonitorCollectorConfig,
    stats: Arc<MonitorCollectorStats>,
    events_sent: Arc<AtomicUsize>,
    performance_metrics: Arc<Mutex<PerformanceTestMetrics>>,
    load_profile: LoadProfile,
}

#[derive(Debug, Clone)]
enum LoadProfile {
    /// Baseline load for establishing performance baselines
    Baseline { events_per_second: u64 },
    /// High throughput load testing
    HighThroughput { events_per_second: u64 },
    /// Memory intensive operations
    MemoryIntensive { event_size_multiplier: usize },
    /// CPU intensive operations
    CpuIntensive { computation_cycles: u64 },
    /// Burst load with periods of high activity
    BurstLoad {
        burst_size: usize,
        burst_interval: Duration,
    },
    /// Sustained load over long periods
    SustainedLoad {
        duration: Duration,
        events_per_second: u64,
    },
    /// Variable load with changing patterns
    VariableLoad {
        pattern_changes: Vec<(Duration, u64)>,
    },
}

#[derive(Debug, Clone)]
struct PerformanceTestMetrics {
    start_time: Option<Instant>,
    first_event_time: Option<Instant>,
    last_event_time: Option<Instant>,
    total_events: usize,
    total_bytes_processed: u64,
    peak_memory_usage: u64,
    cpu_time_used: Duration,
    event_processing_times: Vec<Duration>,
    throughput_samples: Vec<f64>,
    latency_samples: Vec<Duration>,
}

impl Default for PerformanceTestMetrics {
    fn default() -> Self {
        Self {
            start_time: None,
            first_event_time: None,
            last_event_time: None,
            total_events: 0,
            total_bytes_processed: 0,
            peak_memory_usage: 0,
            cpu_time_used: Duration::ZERO,
            event_processing_times: Vec::new(),
            throughput_samples: Vec::new(),
            latency_samples: Vec::new(),
        }
    }
}

impl PerformanceTestMetrics {
    fn record_event(&mut self, event_size: u64, processing_time: Duration) {
        let now = Instant::now();

        if self.start_time.is_none() {
            self.start_time = Some(now);
        }

        if self.total_events == 0 {
            self.first_event_time = Some(now);
        }

        self.last_event_time = Some(now);
        self.total_events += 1;
        self.total_bytes_processed += event_size;
        self.event_processing_times.push(processing_time);
        self.latency_samples.push(processing_time);

        // Calculate throughput sample
        if let Some(start) = self.start_time {
            let elapsed = now.duration_since(start);
            if elapsed.as_secs_f64() > 0.0 {
                let throughput = self.total_events as f64 / elapsed.as_secs_f64();
                self.throughput_samples.push(throughput);
            }
        }
    }

    fn calculate_summary(&self) -> PerformanceSummary {
        let total_duration =
            if let (Some(start), Some(end)) = (self.first_event_time, self.last_event_time) {
                end.duration_since(start)
            } else {
                Duration::ZERO
            };

        let avg_throughput = if total_duration.as_secs_f64() > 0.0 {
            self.total_events as f64 / total_duration.as_secs_f64()
        } else {
            0.0
        };

        let avg_latency = if !self.latency_samples.is_empty() {
            let total_latency: Duration = self.latency_samples.iter().sum();
            total_latency / self.latency_samples.len() as u32
        } else {
            Duration::ZERO
        };

        let p95_latency = if !self.latency_samples.is_empty() {
            let mut sorted_latencies = self.latency_samples.clone();
            sorted_latencies.sort();
            let p95_index = (sorted_latencies.len() as f64 * 0.95) as usize;
            sorted_latencies
                .get(p95_index)
                .copied()
                .unwrap_or(Duration::ZERO)
        } else {
            Duration::ZERO
        };

        let p99_latency = if !self.latency_samples.is_empty() {
            let mut sorted_latencies = self.latency_samples.clone();
            sorted_latencies.sort();
            let p99_index = (sorted_latencies.len() as f64 * 0.99) as usize;
            sorted_latencies
                .get(p99_index)
                .copied()
                .unwrap_or(Duration::ZERO)
        } else {
            Duration::ZERO
        };

        PerformanceSummary {
            total_events: self.total_events,
            total_duration,
            avg_throughput,
            peak_throughput: self.throughput_samples.iter().fold(0.0, |a, &b| a.max(b)),
            avg_latency,
            p95_latency,
            p99_latency,
            total_bytes_processed: self.total_bytes_processed,
            peak_memory_usage: self.peak_memory_usage,
            cpu_time_used: self.cpu_time_used,
        }
    }
}

#[derive(Debug, Clone)]
struct PerformanceSummary {
    total_events: usize,
    total_duration: Duration,
    avg_throughput: f64,
    peak_throughput: f64,
    avg_latency: Duration,
    p95_latency: Duration,
    p99_latency: Duration,
    total_bytes_processed: u64,
    #[allow(dead_code)]
    peak_memory_usage: u64,
    #[allow(dead_code)]
    cpu_time_used: Duration,
}

impl PerformanceTestMonitorCollector {
    fn new(name: &'static str, capabilities: SourceCaps, load_profile: LoadProfile) -> Self {
        let config = MonitorCollectorConfig {
            collection_interval: Duration::from_millis(10), // Fast for performance testing
            max_events_in_flight: match load_profile {
                LoadProfile::HighThroughput { .. } => 5000,
                LoadProfile::MemoryIntensive { .. } => 2000,
                _ => 1000,
            },
            enable_event_driven: true,
            enable_debug_logging: false, // Disable for performance testing
            ..Default::default()
        };

        Self {
            name,
            capabilities,
            config,
            stats: Arc::new(MonitorCollectorStats::default()),
            events_sent: Arc::new(AtomicUsize::new(0)),
            performance_metrics: Arc::new(Mutex::new(PerformanceTestMetrics::default())),
            load_profile,
        }
    }

    fn get_performance_summary(&self) -> PerformanceSummary {
        self.performance_metrics.lock().unwrap().calculate_summary()
    }

    async fn generate_performance_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        match &self.load_profile {
            LoadProfile::Baseline { events_per_second } => {
                self.generate_baseline_events(tx, shutdown_signal, *events_per_second)
                    .await
            }
            LoadProfile::HighThroughput { events_per_second } => {
                self.generate_high_throughput_events(tx, shutdown_signal, *events_per_second)
                    .await
            }
            LoadProfile::MemoryIntensive {
                event_size_multiplier,
            } => {
                self.generate_memory_intensive_events(tx, shutdown_signal, *event_size_multiplier)
                    .await
            }
            LoadProfile::CpuIntensive { computation_cycles } => {
                self.generate_cpu_intensive_events(tx, shutdown_signal, *computation_cycles)
                    .await
            }
            LoadProfile::BurstLoad {
                burst_size,
                burst_interval,
            } => {
                self.generate_burst_load_events(tx, shutdown_signal, *burst_size, *burst_interval)
                    .await
            }
            LoadProfile::SustainedLoad {
                duration,
                events_per_second,
            } => {
                self.generate_sustained_load_events(
                    tx,
                    shutdown_signal,
                    *duration,
                    *events_per_second,
                )
                .await
            }
            LoadProfile::VariableLoad { pattern_changes } => {
                self.generate_variable_load_events(tx, shutdown_signal, pattern_changes)
                    .await
            }
        }
    }

    async fn generate_baseline_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        events_per_second: u64,
    ) -> anyhow::Result<()> {
        let interval = Duration::from_nanos(1_000_000_000 / events_per_second);
        let total_events = events_per_second * 2; // Run for 2 seconds

        for i in 0..total_events {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let event_start = Instant::now();
            let event = self.create_baseline_event(i as usize).await?;
            let event_size = self.estimate_event_size(&event);

            if tx.send(event).await.is_err() {
                break;
            }

            let processing_time = event_start.elapsed();
            self.record_performance_metrics(event_size, processing_time);

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

            tokio::time::sleep(interval).await;
        }

        Ok(())
    }

    async fn generate_high_throughput_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        events_per_second: u64,
    ) -> anyhow::Result<()> {
        let interval = Duration::from_nanos(1_000_000_000 / events_per_second);
        let total_events = events_per_second; // Run for 1 second at high throughput

        for i in 0..total_events {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let event_start = Instant::now();
            let event = self.create_high_throughput_event(i as usize).await?;
            let event_size = self.estimate_event_size(&event);

            if tx.send(event).await.is_err() {
                break;
            }

            let processing_time = event_start.elapsed();
            self.record_performance_metrics(event_size, processing_time);

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

            if interval.as_nanos() > 0 {
                tokio::time::sleep(interval).await;
            }
        }

        Ok(())
    }

    async fn generate_memory_intensive_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        size_multiplier: usize,
    ) -> anyhow::Result<()> {
        let total_events = 100; // Fewer events due to size

        for i in 0..total_events {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let event_start = Instant::now();
            let event = self
                .create_memory_intensive_event(i, size_multiplier)
                .await?;
            let event_size = self.estimate_event_size(&event);

            if tx.send(event).await.is_err() {
                break;
            }

            let processing_time = event_start.elapsed();
            self.record_performance_metrics(event_size, processing_time);

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    async fn generate_cpu_intensive_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        computation_cycles: u64,
    ) -> anyhow::Result<()> {
        let total_events = 50; // Fewer events due to CPU intensity

        for i in 0..total_events {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let event_start = Instant::now();

            // Simulate CPU-intensive computation
            let mut result = 0u64;
            for j in 0..computation_cycles {
                result = result.wrapping_add(j * i as u64);
                result = result.wrapping_mul(1103515245).wrapping_add(12345); // Linear congruential generator
            }

            let event = self.create_cpu_intensive_event(i, result).await?;
            let event_size = self.estimate_event_size(&event);

            if tx.send(event).await.is_err() {
                break;
            }

            let processing_time = event_start.elapsed();
            self.record_performance_metrics(event_size, processing_time);

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        Ok(())
    }

    async fn generate_burst_load_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        burst_size: usize,
        burst_interval: Duration,
    ) -> anyhow::Result<()> {
        let total_bursts = 5;

        for burst in 0..total_bursts {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            // Generate burst of events
            for i in 0..burst_size {
                if shutdown_signal.load(Ordering::Relaxed) {
                    break;
                }

                let event_start = Instant::now();
                let event = self.create_burst_event(burst * burst_size + i).await?;
                let event_size = self.estimate_event_size(&event);

                if tx.send(event).await.is_err() {
                    break;
                }

                let processing_time = event_start.elapsed();
                self.record_performance_metrics(event_size, processing_time);

                self.events_sent.fetch_add(1, Ordering::Relaxed);
                self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);
            }

            // Wait between bursts
            tokio::time::sleep(burst_interval).await;
        }

        Ok(())
    }

    async fn generate_sustained_load_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        duration: Duration,
        events_per_second: u64,
    ) -> anyhow::Result<()> {
        let interval = Duration::from_nanos(1_000_000_000 / events_per_second);
        let start_time = Instant::now();
        let mut event_count = 0;

        while start_time.elapsed() < duration && !shutdown_signal.load(Ordering::Relaxed) {
            let event_start = Instant::now();
            let event = self.create_sustained_event(event_count).await?;
            let event_size = self.estimate_event_size(&event);

            if tx.send(event).await.is_err() {
                break;
            }

            let processing_time = event_start.elapsed();
            self.record_performance_metrics(event_size, processing_time);

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

            event_count += 1;
            tokio::time::sleep(interval).await;
        }

        Ok(())
    }

    async fn generate_variable_load_events(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
        pattern_changes: &[(Duration, u64)],
    ) -> anyhow::Result<()> {
        let mut event_count = 0;

        for (phase_duration, events_per_second) in pattern_changes {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let interval = Duration::from_nanos(1_000_000_000 / events_per_second);
            let phase_start = Instant::now();

            while phase_start.elapsed() < *phase_duration
                && !shutdown_signal.load(Ordering::Relaxed)
            {
                let event_start = Instant::now();
                let event = self.create_variable_event(event_count).await?;
                let event_size = self.estimate_event_size(&event);

                if tx.send(event).await.is_err() {
                    break;
                }

                let processing_time = event_start.elapsed();
                self.record_performance_metrics(event_size, processing_time);

                self.events_sent.fetch_add(1, Ordering::Relaxed);
                self.stats.lifecycle_events.fetch_add(1, Ordering::Relaxed);

                event_count += 1;
                tokio::time::sleep(interval).await;
            }
        }

        Ok(())
    }

    async fn create_baseline_event(&self, index: usize) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 20000 + (index as u32),
            ppid: Some(1),
            name: format!("baseline_proc_{}", index),
            executable_path: Some(format!("/usr/bin/baseline_proc_{}", index)),
            command_line: vec![format!("baseline_proc_{}", index), "--test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(2 * 1024 * 1024),
            executable_hash: Some(format!("baseline_hash_{}", index)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_high_throughput_event(&self, index: usize) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 21000 + (index as u32),
            ppid: Some(1),
            name: format!("throughput_proc_{}", index),
            executable_path: Some(format!("/usr/bin/throughput_proc_{}", index)),
            command_line: vec![format!("throughput_proc_{}", index)],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(2.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some(format!("throughput_hash_{}", index)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_memory_intensive_event(
        &self,
        index: usize,
        size_multiplier: usize,
    ) -> anyhow::Result<CollectionEvent> {
        let large_command_line: Vec<String> = (0..(10 * size_multiplier))
            .map(|j| format!("memory_intensive_arg_{}_{}", index, j))
            .collect();

        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 22000 + (index as u32),
            ppid: Some(1),
            name: format!("memory_intensive_proc_{}", index),
            executable_path: Some(format!("/usr/bin/memory_intensive_proc_{}", index)),
            command_line: large_command_line,
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(5.0),
            memory_usage: Some(100 * 1024 * 1024 * size_multiplier as u64),
            executable_hash: Some("a".repeat(64 * size_multiplier)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_cpu_intensive_event(
        &self,
        index: usize,
        computation_result: u64,
    ) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 23000 + (index as u32),
            ppid: Some(1),
            name: format!("cpu_intensive_proc_{}", index),
            executable_path: Some(format!("/usr/bin/cpu_intensive_proc_{}", index)),
            command_line: vec![
                format!("cpu_intensive_proc_{}", index),
                format!("--result={}", computation_result),
            ],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(80.0 + (index as f64 % 20.0)),
            memory_usage: Some(10 * 1024 * 1024),
            executable_hash: Some(format!(
                "cpu_intensive_hash_{}_{}",
                index, computation_result
            )),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_burst_event(&self, index: usize) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 24000 + (index as u32),
            ppid: Some(1),
            name: format!("burst_proc_{}", index),
            executable_path: Some(format!("/usr/bin/burst_proc_{}", index)),
            command_line: vec![format!("burst_proc_{}", index), "--burst".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(10.0),
            memory_usage: Some(5 * 1024 * 1024),
            executable_hash: Some(format!("burst_hash_{}", index)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_sustained_event(&self, index: usize) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 25000 + (index as u32),
            ppid: Some(1),
            name: format!("sustained_proc_{}", index),
            executable_path: Some(format!("/usr/bin/sustained_proc_{}", index)),
            command_line: vec![
                format!("sustained_proc_{}", index),
                "--sustained".to_string(),
            ],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(3.0),
            memory_usage: Some(3 * 1024 * 1024),
            executable_hash: Some(format!("sustained_hash_{}", index)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    async fn create_variable_event(&self, index: usize) -> anyhow::Result<CollectionEvent> {
        Ok(CollectionEvent::Process(ProcessEvent {
            pid: 26000 + (index as u32),
            ppid: Some(1),
            name: format!("variable_proc_{}", index),
            executable_path: Some(format!("/usr/bin/variable_proc_{}", index)),
            command_line: vec![format!("variable_proc_{}", index), "--variable".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0 + (index as f64 % 5.0)),
            memory_usage: Some(1024 * 1024 + (index as u64 * 1024)),
            executable_hash: Some(format!("variable_hash_{}", index)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }))
    }

    fn estimate_event_size(&self, event: &CollectionEvent) -> u64 {
        // Rough estimation of event size in bytes
        match event {
            CollectionEvent::Process(proc_event) => {
                let base_size = 200; // Base struct size
                let name_size = proc_event.name.len();
                let path_size = proc_event.executable_path.as_ref().map_or(0, |p| p.len());
                let cmd_size: usize = proc_event.command_line.iter().map(|arg| arg.len()).sum();
                let hash_size = proc_event.executable_hash.as_ref().map_or(0, |h| h.len());
                let user_size = proc_event.user_id.as_ref().map_or(0, |u| u.len());

                (base_size + name_size + path_size + cmd_size + hash_size + user_size) as u64
            }
            _ => 100, // Default size for other event types
        }
    }

    fn record_performance_metrics(&self, event_size: u64, processing_time: Duration) {
        let mut metrics = self.performance_metrics.lock().unwrap();
        metrics.record_event(event_size, processing_time);
    }
}

#[async_trait]
impl EventSource for PerformanceTestMonitorCollector {
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
        self.generate_performance_events(&tx, &shutdown_signal)
            .await?;
        self.stats.collection_cycles.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl MonitorCollector for PerformanceTestMonitorCollector {
    fn stats(&self) -> collector_core::MonitorCollectorStatsSnapshot {
        self.stats.snapshot()
    }
}

// Performance Regression Tests

#[tokio::test]
async fn test_baseline_performance_establishment() {
    let performance_config = PerformanceConfig {
        enabled: true,
        collection_interval: Duration::from_millis(10),
        max_throughput_samples: 1000,
        max_latency_samples: 5000,
        ..Default::default()
    };

    let performance_monitor = PerformanceMonitor::new(performance_config);

    // Create baseline monitor
    let baseline_monitor = PerformanceTestMonitorCollector::new(
        "baseline-perf",
        SourceCaps::PROCESS | SourceCaps::REALTIME,
        LoadProfile::Baseline {
            events_per_second: 100,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(1000);

    let mut collector = Collector::new(config);
    collector
        .register(Box::new(baseline_monitor.clone()))
        .unwrap();

    // Run baseline test
    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(3000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = baseline_monitor.get_performance_summary();

    // Record baseline metrics in performance monitor
    for _ in 0..summary.total_events {
        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1000,
            ppid: Some(1),
            name: "baseline_test".to_string(),
            executable_path: Some("/usr/bin/baseline_test".to_string()),
            command_line: vec!["baseline_test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("baseline_hash".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        performance_monitor.record_event(&event);
    }

    // Establish baseline
    let baseline = performance_monitor.establish_baseline(20).await.unwrap();

    // Validate baseline establishment
    assert!(
        summary.total_events > 100,
        "Baseline should generate substantial events: {}",
        summary.total_events
    );

    assert!(
        summary.avg_throughput > 50.0,
        "Baseline throughput should be reasonable: {:.2} events/sec",
        summary.avg_throughput
    );

    assert!(
        summary.avg_latency < Duration::from_millis(10),
        "Baseline latency should be low: {:?}",
        summary.avg_latency
    );

    assert!(
        baseline.baseline_throughput > 0.0,
        "Performance baseline should be established"
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        avg_throughput = summary.avg_throughput,
        peak_throughput = summary.peak_throughput,
        avg_latency_us = summary.avg_latency.as_micros(),
        p95_latency_us = summary.p95_latency.as_micros(),
        p99_latency_us = summary.p99_latency.as_micros(),
        baseline_throughput = baseline.baseline_throughput,
        "Baseline performance established"
    );
}

#[tokio::test]
async fn test_high_throughput_performance() {
    let monitor = PerformanceTestMonitorCollector::new(
        "high-throughput",
        SourceCaps::PROCESS | SourceCaps::REALTIME,
        LoadProfile::HighThroughput {
            events_per_second: 1000,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(2000);

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(2000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate high throughput performance
    assert!(
        summary.total_events > 500,
        "High throughput should generate many events: {}",
        summary.total_events
    );

    assert!(
        summary.avg_throughput > 200.0,
        "High throughput should achieve target rate: {:.2} events/sec",
        summary.avg_throughput
    );

    assert!(
        summary.peak_throughput > summary.avg_throughput,
        "Peak throughput should exceed average: peak={:.2}, avg={:.2}",
        summary.peak_throughput,
        summary.avg_throughput
    );

    assert!(
        summary.avg_latency < Duration::from_millis(5),
        "High throughput latency should remain low: {:?}",
        summary.avg_latency
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        avg_throughput = summary.avg_throughput,
        peak_throughput = summary.peak_throughput,
        avg_latency_us = summary.avg_latency.as_micros(),
        p99_latency_us = summary.p99_latency.as_micros(),
        "High throughput performance test completed"
    );
}

#[tokio::test]
async fn test_memory_intensive_performance() {
    let monitor = PerformanceTestMonitorCollector::new(
        "memory-intensive",
        SourceCaps::PROCESS,
        LoadProfile::MemoryIntensive {
            event_size_multiplier: 10,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(500); // Smaller buffer for large events

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(2000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate memory intensive performance
    assert!(
        summary.total_events > 0,
        "Memory intensive should generate events"
    );

    assert!(
        summary.total_bytes_processed > 100_000,
        "Should process substantial data: {} bytes",
        summary.total_bytes_processed
    );

    // Memory intensive operations may have higher latency
    assert!(
        summary.avg_latency < Duration::from_millis(50),
        "Memory intensive latency should be reasonable: {:?}",
        summary.avg_latency
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        total_bytes_processed = summary.total_bytes_processed,
        avg_throughput = summary.avg_throughput,
        avg_latency_ms = summary.avg_latency.as_millis(),
        "Memory intensive performance test completed"
    );
}

#[tokio::test]
async fn test_cpu_intensive_performance() {
    let monitor = PerformanceTestMonitorCollector::new(
        "cpu-intensive",
        SourceCaps::PROCESS,
        LoadProfile::CpuIntensive {
            computation_cycles: 10000,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(2000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate CPU intensive performance
    assert!(
        summary.total_events > 0,
        "CPU intensive should generate events"
    );

    // CPU intensive operations will have higher latency
    assert!(
        summary.avg_latency < Duration::from_millis(100),
        "CPU intensive latency should be bounded: {:?}",
        summary.avg_latency
    );

    assert!(
        summary.avg_throughput > 5.0,
        "CPU intensive should maintain minimum throughput: {:.2} events/sec",
        summary.avg_throughput
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        avg_throughput = summary.avg_throughput,
        avg_latency_ms = summary.avg_latency.as_millis(),
        p95_latency_ms = summary.p95_latency.as_millis(),
        "CPU intensive performance test completed"
    );
}

#[tokio::test]
async fn test_burst_load_performance() {
    let monitor = PerformanceTestMonitorCollector::new(
        "burst-load",
        SourceCaps::PROCESS,
        LoadProfile::BurstLoad {
            burst_size: 50,
            burst_interval: Duration::from_millis(100),
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(1000);

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(1000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate burst load performance
    assert!(
        summary.total_events > 100,
        "Burst load should generate events: {}",
        summary.total_events
    );

    // Burst load should show variation in throughput
    assert!(
        summary.peak_throughput > summary.avg_throughput * 1.5,
        "Burst load should show throughput variation: peak={:.2}, avg={:.2}",
        summary.peak_throughput,
        summary.avg_throughput
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        avg_throughput = summary.avg_throughput,
        peak_throughput = summary.peak_throughput,
        avg_latency_us = summary.avg_latency.as_micros(),
        "Burst load performance test completed"
    );
}

#[tokio::test]
async fn test_sustained_load_performance() {
    let monitor = PerformanceTestMonitorCollector::new(
        "sustained-load",
        SourceCaps::PROCESS,
        LoadProfile::SustainedLoad {
            duration: Duration::from_millis(1000),
            events_per_second: 50,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(500);

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(1500), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate sustained load performance
    assert!(
        summary.total_events > 30,
        "Sustained load should generate consistent events: {}",
        summary.total_events
    );

    assert!(
        summary.total_duration >= Duration::from_millis(800),
        "Sustained load should run for expected duration: {:?}",
        summary.total_duration
    );

    // Sustained load should have consistent performance
    assert!(
        summary.avg_throughput > 20.0,
        "Sustained load should maintain throughput: {:.2} events/sec",
        summary.avg_throughput
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        total_duration_ms = summary.total_duration.as_millis(),
        avg_throughput = summary.avg_throughput,
        avg_latency_us = summary.avg_latency.as_micros(),
        "Sustained load performance test completed"
    );
}

#[tokio::test]
async fn test_variable_load_performance() {
    let pattern_changes = vec![
        (Duration::from_millis(200), 20),  // Low load
        (Duration::from_millis(200), 100), // High load
        (Duration::from_millis(200), 50),  // Medium load
        (Duration::from_millis(200), 200), // Very high load
    ];

    let monitor = PerformanceTestMonitorCollector::new(
        "variable-load",
        SourceCaps::PROCESS,
        LoadProfile::VariableLoad { pattern_changes },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(1000);

    let mut collector = Collector::new(config);
    collector.register(Box::new(monitor.clone())).unwrap();

    let start_time = Instant::now();

    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(1200), collector.run()).await;
    });

    let _ = collector_handle.await;

    let elapsed = start_time.elapsed();
    let summary = monitor.get_performance_summary();

    // Validate variable load performance
    assert!(
        summary.total_events > 50,
        "Variable load should generate events: {}",
        summary.total_events
    );

    // Variable load should show significant throughput variation
    assert!(
        summary.peak_throughput > summary.avg_throughput * 2.0,
        "Variable load should show significant variation: peak={:.2}, avg={:.2}",
        summary.peak_throughput,
        summary.avg_throughput
    );

    info!(
        total_events = summary.total_events,
        elapsed_ms = elapsed.as_millis(),
        avg_throughput = summary.avg_throughput,
        peak_throughput = summary.peak_throughput,
        avg_latency_us = summary.avg_latency.as_micros(),
        p99_latency_us = summary.p99_latency.as_micros(),
        "Variable load performance test completed"
    );
}

// Performance regression detection test

#[tokio::test]
async fn test_performance_regression_detection() {
    let performance_config = PerformanceConfig {
        enabled: true,
        collection_interval: Duration::from_millis(10),
        max_throughput_samples: 1000,
        max_latency_samples: 5000,
        ..Default::default()
    };

    let performance_monitor = PerformanceMonitor::new(performance_config);

    // Establish baseline with good performance
    let baseline_monitor = PerformanceTestMonitorCollector::new(
        "regression-baseline",
        SourceCaps::PROCESS,
        LoadProfile::Baseline {
            events_per_second: 100,
        },
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(500);

    let mut collector = Collector::new(config);
    collector
        .register(Box::new(baseline_monitor.clone()))
        .unwrap();

    // Run baseline
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(1000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let baseline_summary = baseline_monitor.get_performance_summary();

    // Record baseline metrics
    for _ in 0..baseline_summary.total_events {
        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1000,
            ppid: Some(1),
            name: "regression_test".to_string(),
            executable_path: Some("/usr/bin/regression_test".to_string()),
            command_line: vec!["regression_test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("regression_hash".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        performance_monitor.record_event(&event);
    }

    // Establish baseline
    let baseline = performance_monitor.establish_baseline(10).await.unwrap();

    // Simulate performance degradation with CPU intensive load
    let degraded_monitor = PerformanceTestMonitorCollector::new(
        "regression-degraded",
        SourceCaps::PROCESS,
        LoadProfile::CpuIntensive {
            computation_cycles: 50000,
        }, // Much higher CPU load
    );

    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(200);

    let mut collector = Collector::new(config);
    collector
        .register(Box::new(degraded_monitor.clone()))
        .unwrap();

    // Run degraded performance test
    let collector_handle = tokio::spawn(async move {
        let _ = timeout(Duration::from_millis(1000), collector.run()).await;
    });

    let _ = collector_handle.await;

    let degraded_summary = degraded_monitor.get_performance_summary();

    // Update performance monitor with degraded metrics
    for _ in 0..degraded_summary.total_events {
        performance_monitor.update_cpu_usage(80.0); // High CPU usage
        performance_monitor.update_memory_usage(200 * 1024 * 1024); // Higher memory
    }

    // Check for performance degradation
    let comparison = performance_monitor.compare_to_baseline().await;
    let degradation = performance_monitor.check_performance_degradation().await;

    // Validate regression detection
    assert!(
        baseline_summary.avg_throughput > degraded_summary.avg_throughput,
        "Should detect throughput regression: baseline={:.2}, degraded={:.2}",
        baseline_summary.avg_throughput,
        degraded_summary.avg_throughput
    );

    assert!(
        degraded_summary.avg_latency > baseline_summary.avg_latency,
        "Should detect latency regression: baseline={:?}, degraded={:?}",
        baseline_summary.avg_latency,
        degraded_summary.avg_latency
    );

    if let Some(comparison) = comparison {
        info!(
            throughput_ratio = comparison.throughput_ratio,
            cpu_ratio = comparison.cpu_ratio,
            memory_ratio = comparison.memory_ratio,
            "Performance comparison results"
        );
    }

    if let Some(degradation) = degradation {
        info!(
            degradations_count = degradation.degradations.len(),
            "Performance degradations detected"
        );
    }

    info!(
        baseline_throughput = baseline_summary.avg_throughput,
        baseline_latency_us = baseline_summary.avg_latency.as_micros(),
        degraded_throughput = degraded_summary.avg_throughput,
        degraded_latency_us = degraded_summary.avg_latency.as_micros(),
        baseline_established = baseline.baseline_throughput,
        "Performance regression detection test completed"
    );
}
