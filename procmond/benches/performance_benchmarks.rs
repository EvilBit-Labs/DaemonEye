//! Performance baseline benchmarks using Criterion.
//!
//! This benchmark suite establishes performance baselines for critical operations
//! in procmond. The results help ensure the system meets its performance budgets:
//!
//! - Process enumeration: < 5s for 10,000+ processes
//! - DB Writes: > 1,000 records/sec
//! - Alert latency: < 100ms per rule
//! - CPU Usage: < 5% sustained
//! - Memory: < 100 MB resident
//!
//! # Running benchmarks
//!
//! ```bash
//! cargo bench --package procmond --bench performance_benchmarks
//! ```

#![allow(
    clippy::doc_markdown,
    clippy::unreadable_literal,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::missing_const_for_fn,
    clippy::uninlined_format_args,
    clippy::print_stdout,
    clippy::map_unwrap_or,
    clippy::non_ascii_literal,
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::as_conversions,
    clippy::panic,
    clippy::option_if_let_else,
    clippy::wildcard_enum_match_arm,
    clippy::large_enum_variant,
    clippy::integer_division,
    clippy::clone_on_ref_ptr,
    clippy::unused_self,
    clippy::modulo_arithmetic,
    clippy::explicit_iter_loop,
    clippy::semicolon_if_nothing_returned,
    clippy::missing_assert_message,
    clippy::pattern_type_mismatch,
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee,
    clippy::if_not_else,
    clippy::indexing_slicing,
    clippy::cast_lossless,
    clippy::items_after_statements,
    clippy::let_underscore_must_use,
    clippy::redundant_closure_for_method_calls
)]

use collector_core::ProcessEvent;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use procmond::wal::{WalEntry, WriteAheadLog};
use std::hint::black_box;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a test process event with the given PID.
fn create_test_event(pid: u32) -> ProcessEvent {
    let now = SystemTime::now();
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("benchmark_process_{}", pid),
        executable_path: Some(format!("/usr/bin/benchmark_{}", pid)),
        command_line: vec![
            format!("benchmark_{}", pid),
            "--test".to_owned(),
            format!("--id={}", pid),
        ],
        start_time: Some(now),
        cpu_usage: Some(1.5 + (pid as f64 * 0.1) % 10.0),
        memory_usage: Some(1_048_576_u64.saturating_add((pid as u64).saturating_mul(4096))),
        executable_hash: Some(format!("hash_{:08x}", pid)),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: now,
        platform_metadata: None,
    }
}

/// Create a minimal test process event (smaller serialization size).
fn create_minimal_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: None,
        name: "min".to_owned(),
        executable_path: None,
        command_line: vec![],
        start_time: None,
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: None,
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Create a large test process event (larger serialization size).
fn create_large_event(pid: u32) -> ProcessEvent {
    let now = SystemTime::now();
    let long_args: Vec<String> = (0..50).map(|i| format!("--arg{}=value{}", i, i)).collect();

    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("large_process_with_very_long_name_{}", pid),
        executable_path: Some(format!(
            "/usr/local/bin/very/deep/nested/path/benchmark_{}",
            pid
        )),
        command_line: long_args,
        start_time: Some(now),
        cpu_usage: Some(99.9),
        memory_usage: Some(1_073_741_824), // 1 GB
        executable_hash: Some(format!(
            "sha256:{}",
            "a".repeat(64) // Realistic SHA-256 length
        )),
        user_id: Some("root".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: now,
        // Note: platform_metadata with serde_json::Value is not directly serializable
        // with postcard (WontImplement error), so we leave it as None for benchmarks
        platform_metadata: None,
    }
}

// ============================================================================
// WAL Operations Benchmarks
// ============================================================================

/// Benchmark WAL single write latency.
fn bench_wal_write_single(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_write_single");

    // Different event sizes
    let event_types = [("minimal", 0), ("standard", 1), ("large", 2)];

    for (event_type, type_id) in event_types.iter() {
        group.bench_function(BenchmarkId::new("write_latency", event_type), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    // Use 10MB rotation threshold for benchmarks
                    let wal = WriteAheadLog::with_rotation_threshold(
                        temp_dir.path().to_path_buf(),
                        10 * 1024 * 1024,
                    )
                    .await
                    .expect("Failed to create WAL");

                    let event = match type_id {
                        0 => create_minimal_event(1),
                        1 => create_test_event(1),
                        _ => create_large_event(1),
                    };

                    let sequence = wal.write(event).await.expect("Failed to write");
                    black_box(sequence)
                })
            });
        });
    }

    group.finish();
}

/// Benchmark WAL batch write throughput.
fn bench_wal_write_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_write_throughput");

    // Set longer measurement time for throughput tests
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10);

    // Test different batch sizes
    let batch_sizes = [100, 500, 1000, 5000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_write", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        // Use larger rotation threshold for batch tests
                        let wal = WriteAheadLog::with_rotation_threshold(
                            temp_dir.path().to_path_buf(),
                            50 * 1024 * 1024,
                        )
                        .await
                        .expect("Failed to create WAL");

                        let start = std::time::Instant::now();
                        for i in 0..batch_size {
                            let event = create_test_event(i as u32);
                            wal.write(event).await.expect("Failed to write");
                        }
                        let duration = start.elapsed();

                        let rate = batch_size as f64 / duration.as_secs_f64();
                        if batch_size >= 1000 {
                            println!(
                                "WAL write throughput: {} events in {:.2}ms, rate: {:.1} events/sec",
                                batch_size,
                                duration.as_millis(),
                                rate
                            );
                        }

                        black_box((duration, rate))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WAL replay latency.
fn bench_wal_replay(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_replay");

    group.measurement_time(Duration::from_secs(20));
    group.sample_size(10);

    let event_counts = [100, 500, 1000, 2500];

    for event_count in event_counts.iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("replay_latency", event_count),
            event_count,
            |b, &event_count| {
                // Pre-populate WAL with events before each benchmark iteration
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let wal = WriteAheadLog::with_rotation_threshold(
                            temp_dir.path().to_path_buf(),
                            50 * 1024 * 1024,
                        )
                        .await
                        .expect("Failed to create WAL");

                        // Write events
                        for i in 0..event_count {
                            let event = create_test_event(i as u32);
                            wal.write(event).await.expect("Failed to write");
                        }

                        // Measure replay
                        let start = std::time::Instant::now();
                        let events = wal.replay().await.expect("Failed to replay");
                        let duration = start.elapsed();

                        assert_eq!(events.len(), event_count);

                        let rate = events.len() as f64 / duration.as_secs_f64();
                        if event_count >= 1000 {
                            println!(
                                "WAL replay: {} events in {:.2}ms, rate: {:.1} events/sec",
                                events.len(),
                                duration.as_millis(),
                                rate
                            );
                        }

                        black_box((duration, events.len()))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WAL file rotation time.
fn bench_wal_rotation(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_rotation");

    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10);

    // Use a very small rotation threshold to trigger rotation
    let rotation_threshold = 10 * 1024; // 10KB - will rotate frequently

    group.bench_function("rotation_triggered", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let wal = WriteAheadLog::with_rotation_threshold(
                    temp_dir.path().to_path_buf(),
                    rotation_threshold,
                )
                .await
                .expect("Failed to create WAL");

                // Write events until rotation happens multiple times
                let mut rotations_triggered = 0_u32;
                let mut last_sequence = 0_u64;

                for i in 0..500 {
                    let event = create_large_event(i);
                    let seq = wal.write(event).await.expect("Failed to write");

                    // Detect rotation by checking if sequence reset behavior or file count
                    if seq < last_sequence {
                        rotations_triggered = rotations_triggered.saturating_add(1);
                    }
                    last_sequence = seq;
                }

                black_box(rotations_triggered)
            })
        });
    });

    group.finish();
}

// ============================================================================
// EventBusConnector Benchmarks (simulated, without actual broker connection)
// ============================================================================

/// Benchmark event buffering latency (disconnected mode).
fn bench_eventbus_buffer_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_buffer");

    use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};

    // Buffer single event
    group.bench_function("buffer_single_event", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                    .await
                    .expect("Failed to create connector");

                let event = create_test_event(1);
                let start = std::time::Instant::now();
                let sequence = connector
                    .publish(event, ProcessEventType::Start)
                    .await
                    .expect("Failed to publish");
                let duration = start.elapsed();

                black_box((sequence, duration))
            })
        });
    });

    group.finish();
}

/// Benchmark event buffering throughput.
fn bench_eventbus_buffer_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_buffer_throughput");

    use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};

    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10);

    let batch_sizes = [100, 500, 1000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("buffer_batch", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                            .await
                            .expect("Failed to create connector");

                        let start = std::time::Instant::now();
                        for i in 0..batch_size {
                            let event = create_test_event(i as u32);
                            // Note: This will buffer events since we're not connected
                            let _ = connector.publish(event, ProcessEventType::Start).await;
                        }
                        let duration = start.elapsed();

                        let rate = batch_size as f64 / duration.as_secs_f64();
                        if batch_size >= 500 {
                            println!(
                                "EventBus buffer throughput: {} events in {:.2}ms, rate: {:.1} events/sec",
                                batch_size,
                                duration.as_millis(),
                                rate
                            );
                        }

                        black_box((duration, rate))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WAL replay through EventBusConnector.
fn bench_eventbus_wal_replay(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_wal_replay");

    use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};

    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10);

    let event_counts = [100, 500, 1000];

    for event_count in event_counts.iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("wal_replay_through_connector", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                            .await
                            .expect("Failed to create connector");

                        // Publish events (they go to WAL and buffer)
                        for i in 0..event_count {
                            let event = create_test_event(i as u32);
                            connector
                                .publish(event, ProcessEventType::Start)
                                .await
                                .expect("Failed to publish");
                        }

                        // Measure replay time (while disconnected, events go to buffer)
                        let start = std::time::Instant::now();
                        let replayed = connector.replay_wal().await.expect("Failed to replay");
                        let duration = start.elapsed();

                        black_box((duration, replayed))
                    })
                });
            },
        );
    }

    group.finish();
}

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
    group.measurement_time(Duration::from_secs(30));
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
                        println!(
                            "Process collection ({}): {} processes in {:.2}ms, rate: {:.1} proc/sec",
                            config_name,
                            events.len(),
                            duration.as_millis(),
                            rate
                        );
                        black_box((duration, events.len(), stats));
                    } else {
                        println!("Process collection failed: {:?}", result.err());
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
// Serialization Benchmarks
// ============================================================================

/// Benchmark event serialization using postcard (as used by WAL).
fn bench_serialization_postcard(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_postcard");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        // Serialize benchmark
        group.bench_function(BenchmarkId::new("serialize", event_type), |b| {
            let entry = WalEntry::new(1, event.clone());
            b.iter(|| {
                let serialized = postcard::to_allocvec(&entry).expect("Failed to serialize");
                black_box(serialized)
            });
        });

        // Deserialize benchmark
        let entry = WalEntry::new(1, event.clone());
        let serialized = postcard::to_allocvec(&entry).expect("Failed to serialize");

        group.bench_function(BenchmarkId::new("deserialize", event_type), |b| {
            b.iter(|| {
                let deserialized: WalEntry =
                    postcard::from_bytes(&serialized).expect("Failed to deserialize");
                black_box(deserialized)
            });
        });
    }

    group.finish();
}

/// Benchmark event serialization using serde_json (for comparison and debugging).
fn bench_serialization_json(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_json");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        // Serialize benchmark
        group.bench_function(BenchmarkId::new("serialize", event_type), |b| {
            b.iter(|| {
                let serialized = serde_json::to_string(&event).expect("Failed to serialize");
                black_box(serialized)
            });
        });

        // Deserialize benchmark
        let serialized = serde_json::to_string(&event).expect("Failed to serialize");

        group.bench_function(BenchmarkId::new("deserialize", event_type), |b| {
            b.iter(|| {
                let deserialized: ProcessEvent =
                    serde_json::from_str(&serialized).expect("Failed to deserialize");
                black_box(deserialized)
            });
        });
    }

    group.finish();
}

/// Benchmark CRC32 checksum computation (used by WAL entries).
fn bench_checksum_computation(c: &mut Criterion) {
    use std::hash::Hasher;

    let mut group = c.benchmark_group("checksum_crc32c");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        let serialized = postcard::to_allocvec(&event).expect("Failed to serialize");
        let data_size = serialized.len();

        group.throughput(Throughput::Bytes(data_size as u64));
        group.bench_function(BenchmarkId::new("compute", event_type), |b| {
            b.iter(|| {
                let mut crc = crc32c::Crc32cHasher::new(0);
                crc.write(&serialized);
                let checksum = crc.finish() as u32;
                black_box(checksum)
            });
        });
    }

    group.finish();
}

/// Benchmark batch serialization throughput.
fn bench_serialization_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_throughput");

    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    let batch_sizes = [100, 1000, 5000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));

        // Postcard batch serialization
        group.bench_with_input(
            BenchmarkId::new("postcard_batch", batch_size),
            batch_size,
            |b, &batch_size| {
                let events: Vec<ProcessEvent> = (0..batch_size)
                    .map(|i| create_test_event(i as u32))
                    .collect();

                b.iter(|| {
                    let mut total_bytes = 0_usize;
                    for event in &events {
                        let entry = WalEntry::new(1, event.clone());
                        let serialized =
                            postcard::to_allocvec(&entry).expect("Failed to serialize");
                        total_bytes = total_bytes.saturating_add(serialized.len());
                    }
                    black_box(total_bytes)
                });
            },
        );
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

    group.measurement_time(Duration::from_secs(20));
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
                            println!(
                                "Combined workload: {} events in {:.2}ms, rate: {:.1} events/sec",
                                event_count,
                                duration.as_millis(),
                                rate
                            );

                            // Performance budget check: > 1000 records/sec
                            if rate < 1000.0 {
                                println!(
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

    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10); // Criterion requires at least 10 samples

    let batch_sizes = [1000, 5000, 10000];

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
                        let memory_before =
                            system_before.process(pid).map(|p| p.memory()).unwrap_or(0);

                        // Create batch of events
                        let events: Vec<ProcessEvent> = (0..batch_size)
                            .map(|i| create_test_event(i as u32))
                            .collect();

                        // Measure memory after
                        let system_after = System::new_with_specifics(
                            RefreshKind::nothing()
                                .with_processes(ProcessRefreshKind::nothing().with_memory()),
                        );
                        let memory_after =
                            system_after.process(pid).map(|p| p.memory()).unwrap_or(0);

                        let memory_delta = memory_after.saturating_sub(memory_before);
                        let memory_per_event = if !events.is_empty() {
                            memory_delta / events.len() as u64
                        } else {
                            0
                        };

                        println!(
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

// ============================================================================
// Criterion Groups and Main
// ============================================================================

criterion_group!(
    wal_benchmarks,
    bench_wal_write_single,
    bench_wal_write_throughput,
    bench_wal_replay,
    bench_wal_rotation
);

criterion_group!(
    eventbus_benchmarks,
    bench_eventbus_buffer_operations,
    bench_eventbus_buffer_throughput,
    bench_eventbus_wal_replay
);

criterion_group!(
    process_benchmarks,
    bench_process_collection_real,
    bench_process_collection_single
);

criterion_group!(
    serialization_benchmarks,
    bench_serialization_postcard,
    bench_serialization_json,
    bench_checksum_computation,
    bench_serialization_throughput
);

criterion_group!(
    combined_benchmarks,
    bench_combined_workload,
    bench_memory_efficiency
);

criterion_main!(
    wal_benchmarks,
    eventbus_benchmarks,
    process_benchmarks,
    serialization_benchmarks,
    combined_benchmarks
);
