//! Criterion benchmarks for collector-core framework.
//!
//! This benchmark suite measures performance characteristics critical for
//! production deployment, including event batching, backpressure handling,
//! and runtime overhead.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{runtime::Runtime, sync::mpsc, time::timeout};

/// High-throughput event source for benchmarking collector performance.
///
/// This mock event source generates synthetic process events at configurable
/// rates to measure collector framework performance under various load conditions.
struct BenchmarkEventSource {
    name: &'static str,
    capabilities: SourceCaps,
    events_to_send: usize,
    events_sent: Arc<AtomicUsize>,
    delay_between_events: Duration,
}

impl BenchmarkEventSource {
    /// Creates a new benchmark event source with specified parameters.
    ///
    /// # Arguments
    /// * `name` - Static name for the event source
    /// * `capabilities` - Source capabilities to advertise
    /// * `events_to_send` - Total number of events to generate
    fn new(name: &'static str, capabilities: SourceCaps, events_to_send: usize) -> Self {
        Self {
            name,
            capabilities,
            events_to_send,
            events_sent: Arc::new(AtomicUsize::new(0)),
            delay_between_events: Duration::from_nanos(100), // Very fast for benchmarking
        }
    }

    /// Sets the delay between event generation for rate limiting.
    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay_between_events = delay;
        self
    }

    /// Returns the current count of events sent by this source.
    fn events_sent(&self) -> usize {
        self.events_sent.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EventSource for BenchmarkEventSource {
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
        for i in 0..self.events_to_send {
            if shutdown_signal.load(Ordering::Relaxed) {
                break;
            }

            let event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + (i as u32),
                ppid: Some(1),
                name: format!("bench_process_{}", i),
                executable_path: Some(format!("/usr/bin/bench_{}", i)),
                command_line: vec![format!("bench_{}", i), "--test".to_owned()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(1.5),
                memory_usage: Some(1024 * 1024),
                executable_hash: Some("bench_hash".to_owned()),
                user_id: Some("1000".to_owned()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            });

            if tx.send(event).await.is_err() {
                break; // Channel closed
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);

            if !self.delay_between_events.is_zero() {
                tokio::time::sleep(self.delay_between_events).await;
            }
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Benchmark event batching performance with different batch sizes.
fn bench_event_batching(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("event_batching");

    for batch_size in [10, 50, 100, 500, 1000].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_processing", batch_size),
            batch_size,
            |b, &batch_size| {
                // Setup outside the iteration
                let config = CollectorConfig::default()
                    .with_max_batch_size(batch_size)
                    .with_event_buffer_size(batch_size * 2)
                    .with_batch_timeout(Duration::from_millis(10));

                b.iter(|| {
                    rt.block_on(async {
                        let mut collector = Collector::new(config.clone());
                        let source = BenchmarkEventSource::new(
                            "batch-bench",
                            SourceCaps::PROCESS,
                            batch_size,
                        );

                        collector.register(Box::new(source)).unwrap();

                        // Measure the time to process a batch of events
                        let start = std::time::Instant::now();

                        // Run collector briefly to process events
                        let collector_handle = tokio::spawn(async move {
                            let _ = timeout(Duration::from_millis(100), collector.run()).await;
                        });

                        let _ = collector_handle.await;
                        black_box(start.elapsed());
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark backpressure handling under different load conditions.
fn bench_backpressure_handling(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("backpressure_handling");

    for buffer_size in [100, 500, 1000, 2000].iter() {
        group.throughput(Throughput::Elements(*buffer_size as u64));
        group.bench_with_input(
            BenchmarkId::new("backpressure_buffer", buffer_size),
            buffer_size,
            |b, &buffer_size| {
                // Setup outside the iteration
                let backpressure_threshold = buffer_size * 80 / 100; // 80% threshold
                let config = CollectorConfig::default()
                    .with_event_buffer_size(buffer_size)
                    .with_backpressure_threshold(backpressure_threshold)
                    .with_max_backpressure_wait(Duration::from_millis(10));

                b.iter(|| {
                    rt.block_on(async {
                        let mut collector = Collector::new(config.clone());
                        let source = BenchmarkEventSource::new(
                            "backpressure-bench",
                            SourceCaps::PROCESS,
                            buffer_size * 2, // Generate more events than buffer can hold
                        )
                        .with_delay(Duration::from_nanos(10)); // Very fast generation

                        collector.register(Box::new(source)).unwrap();

                        // Measure backpressure handling performance
                        let start = std::time::Instant::now();

                        let collector_handle = tokio::spawn(async move {
                            let _ = timeout(Duration::from_millis(200), collector.run()).await;
                        });

                        let _ = collector_handle.await;
                        black_box(start.elapsed());
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark graceful shutdown coordination with multiple sources.
fn bench_graceful_shutdown(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("graceful_shutdown");

    for source_count in [1, 2, 4, 8].iter() {
        group.throughput(Throughput::Elements(*source_count as u64));
        group.bench_with_input(
            BenchmarkId::new("shutdown_sources", source_count),
            source_count,
            |b, &source_count| {
                // Setup outside the iteration
                let config = CollectorConfig::default()
                    .with_max_event_sources(source_count)
                    .with_shutdown_timeout(Duration::from_millis(100));

                b.iter(|| {
                    rt.block_on(async {
                        let mut collector = Collector::new(config.clone());

                        // Store source names in a Vec to avoid memory leaks
                        let mut source_names = Vec::with_capacity(source_count);

                        // Register multiple sources
                        for i in 0..source_count {
                            let name = format!("shutdown-bench-{}", i);
                            source_names.push(name);

                            // Use a static string pattern similar to chaos testing
                            let static_name = match source_names.last().unwrap().as_str() {
                                "shutdown-bench-0" => "shutdown-bench-0",
                                "shutdown-bench-1" => "shutdown-bench-1",
                                "shutdown-bench-2" => "shutdown-bench-2",
                                "shutdown-bench-3" => "shutdown-bench-3",
                                "shutdown-bench-4" => "shutdown-bench-4",
                                "shutdown-bench-5" => "shutdown-bench-5",
                                "shutdown-bench-6" => "shutdown-bench-6",
                                "shutdown-bench-7" => "shutdown-bench-7",
                                _ => "shutdown-bench-unknown",
                            };

                            let source = BenchmarkEventSource::new(
                                static_name,
                                SourceCaps::PROCESS,
                                1000, // Long-running sources
                            )
                            .with_delay(Duration::from_millis(1));

                            collector.register(Box::new(source)).unwrap();
                        }
                        // Measure shutdown coordination time
                        let start = std::time::Instant::now();

                        let collector_handle = tokio::spawn(async move {
                            let _ = timeout(Duration::from_millis(150), collector.run()).await;
                        });

                        let _ = collector_handle.await;
                        black_box(start.elapsed());
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark collector runtime overhead with different configurations.
fn bench_runtime_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("runtime_overhead");

    let configurations = [
        ("minimal", CollectorConfig::default()),
        (
            "standard",
            CollectorConfig::default()
                .with_telemetry(true)
                .with_health_check_interval(Duration::from_millis(100)),
        ),
        (
            "high_throughput",
            CollectorConfig::default()
                .with_event_buffer_size(2000)
                .with_max_batch_size(500)
                .with_telemetry(true)
                .with_debug_logging(false),
        ),
    ];

    for (config_name, config) in configurations.iter() {
        group.bench_function(BenchmarkId::new("runtime_config", config_name), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let mut collector = Collector::new(config.clone());
                    let source = BenchmarkEventSource::new(
                        "runtime-bench",
                        SourceCaps::PROCESS | SourceCaps::REALTIME,
                        100,
                    );

                    collector.register(Box::new(source)).unwrap();

                    // Measure runtime overhead
                    let start = std::time::Instant::now();

                    let collector_handle = tokio::spawn(async move {
                        let _ = timeout(Duration::from_millis(50), collector.run()).await;
                    });

                    let _ = collector_handle.await;
                    black_box(start.elapsed());
                })
            });
        });
    }
    group.finish();
}

/// Benchmark event throughput under sustained load.
fn bench_event_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("event_throughput");

    for events_per_second in [100, 500, 1000, 2000, 5000].iter() {
        group.throughput(Throughput::Elements(*events_per_second as u64));
        group.bench_with_input(
            BenchmarkId::new("sustained_throughput", events_per_second),
            events_per_second,
            |b, &events_per_second| {
                b.iter(|| {
                    rt.block_on(async {
                        // Validate events_per_second to prevent division by zero and overflow
                        let delay_between_events = if events_per_second > 0 {
                            // Use f64 for precise calculation to avoid overflow
                            Duration::from_secs_f64(1.0 / events_per_second as f64)
                        } else {
                            Duration::ZERO // Handle zero case explicitly
                        };

                        let test_duration = Duration::from_millis(100);
                        let expected_events =
                            (events_per_second * test_duration.as_millis() as usize) / 1000;

                        let config = CollectorConfig::default()
                            .with_event_buffer_size(events_per_second * 2)
                            .with_max_batch_size(events_per_second / 10);

                        let mut collector = Collector::new(config);
                        let source = BenchmarkEventSource::new(
                            "throughput-bench",
                            SourceCaps::PROCESS,
                            expected_events,
                        )
                        .with_delay(delay_between_events);

                        let events_sent = source.events_sent();
                        collector.register(Box::new(source)).unwrap();
                        // Measure sustained throughput
                        let start = std::time::Instant::now();

                        let collector_handle = tokio::spawn(async move {
                            let _ =
                                timeout(test_duration + Duration::from_millis(50), collector.run())
                                    .await;
                        });

                        let _ = collector_handle.await;
                        let elapsed = start.elapsed();

                        black_box((elapsed, events_sent));
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark performance monitoring overhead.
fn bench_performance_monitoring_overhead(c: &mut Criterion) {
    use collector_core::{
        AnalysisType, PerformanceConfig, PerformanceMonitor, TriggerPriority, TriggerRequest,
    };
    use std::collections::HashMap;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("performance_monitoring");

    // Test different monitoring configurations
    let configs = [
        (
            "disabled",
            PerformanceConfig {
                enabled: false,
                ..Default::default()
            },
        ),
        (
            "minimal",
            PerformanceConfig {
                enabled: true,
                enable_trigger_latency_tracking: false,
                collection_interval: Duration::from_secs(60),
                ..Default::default()
            },
        ),
        ("full", PerformanceConfig::default()),
    ];

    for (config_name, config) in configs.iter() {
        group.bench_function(BenchmarkId::new("monitoring_overhead", config_name), |b| {
            // Setup outside the iteration
            let monitor = PerformanceMonitor::new(config.clone());

            b.iter(|| {
                rt.block_on(async {
                    // Simulate event processing with monitoring
                    let start = std::time::Instant::now();

                    for i in 0..1000 {
                        let event = CollectionEvent::Process(ProcessEvent {
                            pid: 1000 + i,
                            ppid: Some(1),
                            name: format!("bench_process_{}", i),
                            executable_path: Some(format!("/usr/bin/bench_{}", i)),
                            command_line: vec![format!("bench_{}", i)],
                            start_time: Some(SystemTime::now()),
                            cpu_usage: Some(1.5),
                            memory_usage: Some(1024 * 1024),
                            executable_hash: Some("bench_hash".to_owned()),
                            user_id: Some("1000".to_owned()),
                            accessible: true,
                            file_exists: true,
                            timestamp: SystemTime::now(),
                            platform_metadata: None,
                        });

                        monitor.record_event(&event);

                        // Simulate trigger processing
                        if config.enable_trigger_latency_tracking && i % 10 == 0 {
                            let trigger_id = format!("trigger_{}", i);
                            monitor.record_trigger_start(&trigger_id);

                            let trigger = TriggerRequest {
                                trigger_id: trigger_id.clone(),
                                target_collector: "test_collector".to_string(),
                                analysis_type: AnalysisType::YaraScan,
                                priority: TriggerPriority::Normal,
                                target_pid: Some(1000 + i),
                                target_path: None,
                                correlation_id: "test_correlation".to_string(),
                                metadata: HashMap::new(),
                                timestamp: SystemTime::now(),
                            };

                            monitor.record_trigger_completion(&trigger_id, &trigger);
                        }

                        // Simulate resource updates
                        if i % 50 == 0 {
                            monitor.update_cpu_usage(25.0 + (i as f64 % 10.0));
                            monitor.update_memory_usage(1024 * 1024 + (i as u64 * 1024));
                        }
                    }

                    let elapsed = start.elapsed();
                    black_box(elapsed);
                })
            });
        });
    }
    group.finish();
}

/// Benchmark high process churn scenarios (10,000+ processes).
fn bench_high_process_churn(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("high_process_churn");
    group.sample_size(10); // Reduce sample size for expensive benchmarks

    for process_count in [1000, 5000, 10000, 20000].iter() {
        group.throughput(Throughput::Elements(*process_count as u64));
        group.bench_with_input(
            BenchmarkId::new("process_churn", process_count),
            process_count,
            |b, &process_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let config = CollectorConfig::default()
                            .with_event_buffer_size(process_count * 2)
                            .with_max_batch_size(1000)
                            .with_telemetry(true);

                        let mut collector = Collector::new(config);

                        // Create high-churn event source
                        let source = BenchmarkEventSource::new(
                            "churn-bench",
                            SourceCaps::PROCESS | SourceCaps::REALTIME,
                            process_count,
                        )
                        .with_delay(Duration::from_nanos(1)); // Very fast churn

                        collector.register(Box::new(source)).unwrap();
                        // Measure processing time for high churn
                        let start = std::time::Instant::now();

                        let collector_handle = tokio::spawn(async move {
                            let _ = timeout(Duration::from_millis(500), collector.run()).await;
                        });

                        let _ = collector_handle.await;
                        let elapsed = start.elapsed();

                        black_box(elapsed);
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark trigger event latency measurement.
fn bench_trigger_latency_measurement(c: &mut Criterion) {
    use collector_core::{
        AnalysisType, PerformanceConfig, PerformanceMonitor, TriggerPriority, TriggerRequest,
    };
    use std::collections::HashMap;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("trigger_latency");

    for trigger_count in [10, 100, 1000, 5000].iter() {
        group.throughput(Throughput::Elements(*trigger_count as u64));
        group.bench_with_input(
            BenchmarkId::new("latency_tracking", trigger_count),
            trigger_count,
            |b, &trigger_count| {
                // Setup outside the iteration
                let config = PerformanceConfig {
                    enable_trigger_latency_tracking: true,
                    max_latency_samples: trigger_count * 2,
                    ..Default::default()
                };
                let monitor = PerformanceMonitor::new(config);

                b.iter(|| {
                    rt.block_on(async {
                        let start = std::time::Instant::now();

                        // Simulate trigger processing with latency measurement
                        for i in 0..trigger_count {
                            let trigger_id = format!("trigger_{}", i);
                            monitor.record_trigger_start(&trigger_id);

                            // Simulate variable processing time
                            tokio::time::sleep(Duration::from_micros(10 + (i % 100) as u64)).await;

                            let trigger = TriggerRequest {
                                trigger_id: trigger_id.clone(),
                                target_collector: "test_collector".to_string(),
                                analysis_type: AnalysisType::YaraScan,
                                priority: TriggerPriority::Normal,
                                target_pid: Some(2000 + i as u32),
                                target_path: None,
                                correlation_id: "test_correlation".to_string(),
                                metadata: HashMap::new(),
                                timestamp: SystemTime::now(),
                            };

                            monitor.record_trigger_completion(&trigger_id, &trigger);
                        }

                        // Calculate latency metrics
                        let _metrics = monitor.calculate_trigger_latency_metrics();
                        let elapsed = start.elapsed();

                        black_box(elapsed);
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark resource usage tracking overhead.
fn bench_resource_usage_tracking(c: &mut Criterion) {
    use collector_core::{PerformanceConfig, PerformanceMonitor};

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("resource_tracking");

    for update_frequency in [10, 100, 1000, 5000].iter() {
        group.throughput(Throughput::Elements(*update_frequency as u64));
        group.bench_with_input(
            BenchmarkId::new("resource_updates", update_frequency),
            update_frequency,
            |b, &update_frequency| {
                // Setup outside the iteration
                let config = PerformanceConfig::default();
                let monitor = PerformanceMonitor::new(config);

                b.iter(|| {
                    rt.block_on(async {
                        let start = std::time::Instant::now();

                        // Simulate frequent resource updates
                        for i in 0..update_frequency {
                            let cpu_percent = 10.0 + (i as f64 % 50.0);
                            let memory_bytes = 1024 * 1024 + (i as u64 * 1024);

                            monitor.update_cpu_usage(cpu_percent);
                            monitor.update_memory_usage(memory_bytes);

                            // Periodically collect metrics
                            if i % 100 == 0 {
                                let _metrics = monitor.collect_resource_metrics().await;
                            }
                        }

                        let elapsed = start.elapsed();
                        black_box(elapsed);
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark baseline establishment performance.
fn bench_baseline_establishment(c: &mut Criterion) {
    use collector_core::{PerformanceConfig, PerformanceMonitor};

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("baseline_establishment");
    group.sample_size(10); // Reduce sample size for expensive benchmarks

    for sample_count in [10, 25, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("establish_baseline", sample_count),
            sample_count,
            |b, &sample_count| {
                // Setup outside the iteration
                let config = PerformanceConfig {
                    collection_interval: Duration::from_millis(10), // Fast for benchmarking
                    ..Default::default()
                };
                let monitor = PerformanceMonitor::new(config);

                b.iter(|| {
                    rt.block_on(async {
                        // Simulate some activity before establishing baseline
                        for i in 0..100 {
                            monitor.update_cpu_usage(20.0 + (i as f64 % 10.0));
                            monitor.update_memory_usage(1024 * 1024 + (i as u64 * 1024));
                        }

                        let start = std::time::Instant::now();
                        let _baseline = monitor.establish_baseline(sample_count).await.unwrap();
                        let elapsed = start.elapsed();

                        black_box(elapsed);
                    })
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_event_batching,
    bench_backpressure_handling,
    bench_graceful_shutdown,
    bench_runtime_overhead,
    bench_event_throughput,
    bench_performance_monitoring_overhead,
    bench_high_process_churn,
    bench_trigger_latency_measurement,
    bench_resource_usage_tracking,
    bench_baseline_establishment
);
criterion_main!(benches);
