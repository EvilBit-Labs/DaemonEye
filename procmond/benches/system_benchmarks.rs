//! System-level performance benchmarks using Criterion.
//!
//! This benchmark suite covers real process collection, RPC health check
//! latency, data sanitization, combined workloads, and memory efficiency.
//!
//! # Running benchmarks
//!
//! ```bash
//! cargo bench --package procmond --bench system_benchmarks
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::print_stdout,
    clippy::use_debug,
    clippy::uninlined_format_args,
    clippy::missing_assert_message,
    clippy::indexing_slicing,
    clippy::semicolon_if_nothing_returned,
    clippy::significant_drop_tightening,
    clippy::pattern_type_mismatch,
    clippy::explicit_iter_loop,
    clippy::shadow_reuse,
    clippy::cast_lossless,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls,
    clippy::if_not_else,
    clippy::integer_division,
    clippy::items_after_statements
)]

#[allow(dead_code)]
#[path = "bench_helpers.rs"]
mod bench_helpers;

use bench_helpers::create_test_event;
use collector_core::ProcessEvent;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ============================================================================
// Process Collection Benchmarks
// ============================================================================

/// Benchmark process collection using real system processes.
fn bench_process_collection_real(c: &mut Criterion) {
    use procmond::process_collector::{
        ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
    };

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("process_collection_real");

    // Allow longer measurement for real system collection
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    let configs = [
        (
            "basic",
            ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 10000,
            },
        ),
        (
            "enhanced",
            ProcessCollectionConfig {
                collect_enhanced_metadata: true,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 10000,
            },
        ),
    ];

    for (config_name, config) in configs {
        group.bench_function(BenchmarkId::new("sysinfo_collector", config_name), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let collector = SysinfoProcessCollector::new(config.clone());

                    let start = std::time::Instant::now();
                    let result = collector.collect_processes().await;
                    let duration = start.elapsed();

                    if let Ok((events, stats)) = result {
                        let rate = events.len() as f64 / duration.as_secs_f64();
                        eprintln!(
                            "Process collection ({}): {} processes in {:.2}ms, rate: {:.1} proc/sec",
                            config_name,
                            events.len(),
                            duration.as_millis(),
                            rate
                        );
                        black_box((duration, events.len(), stats));
                    } else {
                        eprintln!("Process collection failed: {:?}", result.err());
                        black_box(duration);
                    }
                })
            });
        });
    }

    group.finish();
}

/// Benchmark single process collection.
fn bench_process_collection_single(c: &mut Criterion) {
    use procmond::process_collector::{
        ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
    };

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("process_collection_single");

    let config = ProcessCollectionConfig::default();

    // Get current process PID for benchmark
    let current_pid = std::process::id();

    group.bench_function("collect_single_process", |b| {
        b.iter(|| {
            rt.block_on(async {
                let collector = SysinfoProcessCollector::new(config.clone());

                let start = std::time::Instant::now();
                let result = collector.collect_process(current_pid).await;
                let duration = start.elapsed();

                if let Ok(event) = result {
                    black_box((duration, event.pid));
                } else {
                    black_box(duration);
                }
            })
        });
    });

    group.finish();
}

// ============================================================================
// RPC Service Benchmarks
// ============================================================================

/// Benchmark RPC health check request/response latency.
///
/// Uses a lightweight actor channel (no full collector) to measure the RPC
/// handler's request processing overhead in isolation.
fn bench_rpc_health_check_latency(c: &mut Criterion) {
    use daemoneye_eventbus::rpc::{
        CollectorOperation, RpcCorrelationMetadata, RpcPayload, RpcRequest,
    };
    use procmond::event_bus_connector::EventBusConnector;
    use procmond::monitor_collector::{ActorHandle, ActorMessage};
    use procmond::rpc_service::RpcServiceHandler;
    use std::sync::Arc;
    use tokio::sync::{RwLock, mpsc};

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("rpc_latency");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    group.bench_function("health_check", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");

                // Create a lightweight actor channel — no full collector needed
                let (tx, mut rx) = mpsc::channel::<ActorMessage>(32);
                let actor_handle = ActorHandle::new(tx);

                // Drain actor messages in background so sends don't block
                let drain_task = tokio::spawn(async move { while rx.recv().await.is_some() {} });

                let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                    .await
                    .expect("connector");
                let event_bus = Arc::new(RwLock::new(connector));

                let handler = RpcServiceHandler::with_defaults(actor_handle.clone(), event_bus);

                let request = RpcRequest {
                    request_id: "bench-1".to_owned(),
                    client_id: "bench-client".to_owned(),
                    target: "control.collector.procmond".to_owned(),
                    operation: CollectorOperation::HealthCheck,
                    payload: RpcPayload::Empty,
                    timestamp: SystemTime::now(),
                    deadline: SystemTime::now() + Duration::from_secs(30),
                    correlation_metadata: RpcCorrelationMetadata::new("bench-corr".to_owned()),
                };

                let start = std::time::Instant::now();
                let response = handler.handle_request(request).await;
                let latency = start.elapsed();

                // Drop handler and actor_handle before awaiting drain_task
                // so all senders are dropped and the drain task can terminate.
                drop(handler);
                drop(actor_handle);
                drain_task.abort();

                black_box((response, latency))
            })
        });
    });

    group.finish();
}

/// Benchmark data sanitization throughput.
fn bench_sanitization(c: &mut Criterion) {
    use procmond::security::{sanitize_command_line, sanitize_file_path};

    let mut group = c.benchmark_group("sanitization");

    let test_cases = [
        (
            "clean_cmd",
            "app --verbose --output file.txt --debug --workers 4",
        ),
        (
            "sensitive_cmd",
            "app --password secret123 --token test_token_value --api-key abc --verbose",
        ),
        ("long_cmd", &"arg ".repeat(200)),
    ];

    for (name, cmd) in &test_cases {
        group.bench_function(BenchmarkId::new("command_line", name), |b| {
            b.iter(|| black_box(sanitize_command_line(cmd)));
        });
    }

    let path_cases = [
        ("safe_path", "/usr/bin/bash"),
        ("sensitive_ssh", "/home/user/.ssh/id_rsa"),
        ("sensitive_aws", "/home/user/.aws/credentials"),
        ("windows_path", "C:\\Users\\admin\\.ssh\\id_rsa"),
    ];

    for (name, path) in &path_cases {
        group.bench_function(BenchmarkId::new("file_path", name), |b| {
            b.iter(|| black_box(sanitize_file_path(path)));
        });
    }

    group.finish();
}

// ============================================================================
// Combined Workload Benchmarks
// ============================================================================

/// Benchmark a realistic workload combining collection, serialization, and WAL writes.
fn bench_combined_workload(c: &mut Criterion) {
    use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("combined_workload");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let event_counts = [100, 500, 1000];

    for event_count in event_counts.iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("collect_serialize_write", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                            .await
                            .expect("Failed to create connector");

                        let start = std::time::Instant::now();

                        // Simulate collection + publish workflow
                        for i in 0..event_count {
                            let event = create_test_event(i as u32);
                            connector
                                .publish(event, ProcessEventType::Start)
                                .await
                                .expect("Failed to publish");
                        }

                        let duration = start.elapsed();
                        let rate = event_count as f64 / duration.as_secs_f64();

                        if event_count >= 500 {
                            eprintln!(
                                "Combined workload: {} events in {:.2}ms, rate: {:.1} events/sec",
                                event_count,
                                duration.as_millis(),
                                rate
                            );

                            // Performance budget check: > 1000 records/sec
                            if rate < 1000.0 {
                                eprintln!(
                                    "WARNING: Combined workload rate {:.1}/sec is below 1000/sec budget",
                                    rate
                                );
                            }
                        }

                        black_box((duration, rate))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark memory usage patterns during batch operations.
fn bench_memory_efficiency(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("memory_efficiency");

    use sysinfo::{ProcessRefreshKind, RefreshKind, System};

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10); // Criterion requires at least 10 samples

    let batch_sizes = [1000, 5000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("event_batch_memory", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    rt.block_on(async {
                        // Measure memory before
                        let system_before = System::new_with_specifics(
                            RefreshKind::nothing()
                                .with_processes(ProcessRefreshKind::nothing().with_memory()),
                        );
                        let pid = sysinfo::Pid::from_u32(std::process::id());
                        let memory_before = system_before
                            .process(pid)
                            .map(|p| p.memory())
                            .unwrap_or_else(|| {
                                eprintln!("WARNING: Could not measure memory for current process");
                                0
                            });

                        // Create batch of events
                        let events: Vec<ProcessEvent> = (0..batch_size)
                            .map(|i| create_test_event(i as u32))
                            .collect();

                        // Measure memory after
                        let system_after = System::new_with_specifics(
                            RefreshKind::nothing()
                                .with_processes(ProcessRefreshKind::nothing().with_memory()),
                        );
                        let memory_after = system_after
                            .process(pid)
                            .map(|p| p.memory())
                            .unwrap_or_else(|| {
                                eprintln!("WARNING: Could not measure memory for current process");
                                0
                            });

                        let memory_delta = memory_after.saturating_sub(memory_before);
                        let memory_per_event = if !events.is_empty() {
                            memory_delta / events.len() as u64
                        } else {
                            0
                        };

                        eprintln!(
                            "Memory efficiency: {} events, {}KB total delta, {}B per event",
                            batch_size,
                            memory_delta / 1024,
                            memory_per_event
                        );

                        black_box((events.len(), memory_delta))
                    })
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    process_benchmarks,
    bench_process_collection_real,
    bench_process_collection_single
);

criterion_group!(
    rpc_benchmarks,
    bench_rpc_health_check_latency,
    bench_sanitization
);

criterion_group!(
    combined_benchmarks,
    bench_combined_workload,
    bench_memory_efficiency
);

criterion_main!(process_benchmarks, rpc_benchmarks, combined_benchmarks);
