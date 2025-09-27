//! Criterion benchmarks for ProcessCollector implementations.
//!
//! This benchmark suite measures performance characteristics of all ProcessCollector
//! implementations under various load conditions, including high process counts
//! (10,000+ processes) to establish baseline performance metrics.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use procmond::process_collector::{
    CollectionStats, ProcessCollectionError, ProcessCollectionResult, ProcessCollector,
    ProcessCollectorCapabilities,
};
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::runtime::Runtime;

/// Mock ProcessCollector for benchmarking with configurable process counts.
///
/// This collector generates synthetic process data to test performance
/// characteristics without being limited by actual system process counts.
///
/// # Examples
///
/// ```rust
/// let collector = BenchmarkProcessCollector::new("test", 1000);
/// let (events, stats) = collector.collect_processes().await?;
/// assert_eq!(events.len(), 1000);
/// ```
struct BenchmarkProcessCollector {
    name: &'static str,
    process_count: usize,
    delay_per_process: Duration,
    capabilities: ProcessCollectorCapabilities,
}

impl BenchmarkProcessCollector {
    /// Creates a new benchmark collector with the specified name and process count.
    fn new(name: &'static str, process_count: usize) -> Self {
        Self {
            name,
            process_count,
            delay_per_process: Duration::from_nanos(100), // Minimal delay for benchmarking
            capabilities: ProcessCollectorCapabilities {
                basic_info: true,
                enhanced_metadata: true,
                executable_hashing: true,
                system_processes: true,
                kernel_threads: true,
                realtime_collection: true,
            },
        }
    }

    /// Configures a delay per process to simulate system load.
    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay_per_process = delay;
        self
    }

    /// Generates a synthetic process event for benchmarking purposes.
    fn generate_process_event(&self, index: usize) -> ProcessEvent {
        ProcessEvent {
            pid: 1000_u32.saturating_add(index as u32),
            ppid: Some(1),
            name: format!("benchmark_process_{}", index),
            executable_path: Some(format!("/usr/bin/benchmark_{}", index)),
            command_line: vec![
                format!("benchmark_{}", index),
                "--test".to_owned(),
                format!("--id={}", index),
            ],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.5 + (index as f64 * 0.1) % 10.0),
            memory_usage: Some(1_048_576_u64.saturating_add((index * 4096) as u64)),
            executable_hash: Some(format!("hash_{:08x}", index)),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }
    }
}

#[async_trait]
impl ProcessCollector for BenchmarkProcessCollector {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> ProcessCollectorCapabilities {
        self.capabilities
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        let start_time = std::time::Instant::now();
        let mut events = Vec::with_capacity(self.process_count);

        for i in 0..self.process_count {
            if !self.delay_per_process.is_zero() {
                tokio::time::sleep(self.delay_per_process).await;
            }
            events.push(self.generate_process_event(i));
        }

        let stats = CollectionStats {
            total_processes: self.process_count,
            successful_collections: self.process_count,
            inaccessible_processes: 0,
            invalid_processes: 0,
            collection_duration_ms: start_time.elapsed().as_millis() as u64,
        };

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        if pid >= 1000 && pid < 1000_u32.saturating_add(self.process_count as u32) {
            Ok(self.generate_process_event((pid - 1000) as usize))
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        Ok(())
    }
}

/// Benchmark process enumeration with different process counts.
fn bench_process_enumeration_scale(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("process_enumeration_scale");

    // Test with various process counts including high counts (10,000+)
    for process_count in [100, 500, 1000, 5000, 10000, 20000, 50000].iter() {
        group.throughput(Throughput::Elements(*process_count as u64));
        group.bench_with_input(
            BenchmarkId::new("enumerate_processes", process_count),
            process_count,
            |b, &process_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let collector =
                            BenchmarkProcessCollector::new("scale-benchmark", process_count);

                        let start = std::time::Instant::now();
                        let result = collector.collect_processes().await;
                        let duration = start.elapsed();

                        assert!(result.is_ok(), "Collection should succeed");
                        let (events, stats) = result.unwrap();

                        assert_eq!(events.len(), process_count);
                        assert_eq!(stats.total_processes, process_count);

                        black_box((duration, events.len(), stats));
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark process enumeration with simulated system load.
fn bench_process_enumeration_with_load(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("process_enumeration_load");

    // Simulate different system loads with delays
    let load_scenarios = [
        ("no_load", Duration::ZERO),
        ("light_load", Duration::from_nanos(10)),
        ("medium_load", Duration::from_nanos(100)),
        ("heavy_load", Duration::from_micros(1)),
    ];

    for (load_name, delay) in load_scenarios.iter() {
        group.bench_function(BenchmarkId::new("system_load", load_name), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let collector =
                        BenchmarkProcessCollector::new("load-benchmark", 10000).with_delay(*delay);

                    let start = std::time::Instant::now();
                    let result = collector.collect_processes().await;
                    let duration = start.elapsed();

                    assert!(result.is_ok(), "Collection should succeed under load");
                    let (events, stats) = result.unwrap();

                    black_box((duration, events.len(), stats));
                })
            });
        });
    }
    group.finish();
}

/// Benchmark single process collection performance.
fn bench_single_process_collection(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("single_process_collection");

    for total_processes in [1000, 10000, 50000].iter() {
        group.bench_with_input(
            BenchmarkId::new("collect_single", total_processes),
            total_processes,
            |b, &total_processes| {
                b.iter(|| {
                    rt.block_on(async {
                        let collector =
                            BenchmarkProcessCollector::new("single-benchmark", total_processes);

                        // Test collecting a process from the middle of the range
                        let target_pid = 1000_u32.saturating_add((total_processes / 2) as u32);

                        let start = std::time::Instant::now();
                        let result = collector.collect_process(target_pid).await;
                        let duration = start.elapsed();

                        assert!(result.is_ok(), "Single process collection should succeed");
                        let event = result.unwrap();

                        assert_eq!(event.pid, target_pid);
                        black_box((duration, event));
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark concurrent process collection operations.
fn bench_concurrent_collection(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("concurrent_collection");

    for concurrent_tasks in [1, 2, 4, 8, 16].iter() {
        group.throughput(Throughput::Elements(*concurrent_tasks as u64));
        group.bench_with_input(
            BenchmarkId::new("concurrent_enumerate", concurrent_tasks),
            concurrent_tasks,
            |b, &concurrent_tasks| {
                b.iter(|| {
                    rt.block_on(async {
                        let collector = Arc::new(BenchmarkProcessCollector::new(
                            "concurrent-benchmark",
                            10000,
                        ));

                        let start = std::time::Instant::now();

                        // Run multiple concurrent collection operations
                        let mut handles = Vec::with_capacity(concurrent_tasks);
                        for _ in 0..concurrent_tasks {
                            let collector_clone = Arc::clone(&collector);
                            let handle =
                                tokio::spawn(
                                    async move { collector_clone.collect_processes().await },
                                );
                            handles.push(handle);
                        }

                        // Wait for all tasks to complete
                        let mut results = Vec::with_capacity(concurrent_tasks);
                        for handle in handles {
                            let result = handle.await.unwrap();
                            results.push(result);
                        }

                        let duration = start.elapsed();

                        // Verify all collections succeeded
                        for result in &results {
                            assert!(result.is_ok(), "Concurrent collection should succeed");
                        }

                        black_box((duration, results.len()));
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark memory usage patterns during large collections.
fn bench_memory_usage_patterns(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("memory_usage_patterns");

    // Test different collection strategies that might affect memory usage
    let strategies = [
        ("batch_1000", 1000),
        ("batch_5000", 5000),
        ("batch_10000", 10000),
        ("batch_25000", 25000),
    ];

    for (strategy_name, batch_size) in strategies.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_function(BenchmarkId::new("memory_pattern", strategy_name), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let collector = BenchmarkProcessCollector::new("memory-benchmark", *batch_size);

                    let start = std::time::Instant::now();
                    let result = collector.collect_processes().await;
                    let duration = start.elapsed();

                    assert!(result.is_ok(), "Memory pattern test should succeed");
                    let (events, stats) = result.unwrap();

                    // Measure memory characteristics
                    let event_size = std::mem::size_of::<ProcessEvent>();
                    let total_memory = events.len().saturating_mul(event_size);

                    black_box((duration, events.len(), total_memory, stats));
                })
            });
        });
    }
    group.finish();
}

/// Benchmark health check performance under different conditions.
fn bench_health_check_performance(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("health_check_performance");

    for process_count in [1000, 10000, 50000].iter() {
        group.bench_with_input(
            BenchmarkId::new("health_check", process_count),
            process_count,
            |b, &process_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let collector =
                            BenchmarkProcessCollector::new("health-benchmark", process_count);

                        let start = std::time::Instant::now();
                        let result = collector.health_check().await;
                        let duration = start.elapsed();

                        assert!(result.is_ok(), "Health check should succeed");
                        black_box((duration, result.unwrap()));
                    })
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_process_enumeration_scale,
    bench_process_enumeration_with_load,
    bench_single_process_collection,
    bench_concurrent_collection,
    bench_memory_usage_patterns,
    bench_health_check_performance
);
criterion_main!(benches);
