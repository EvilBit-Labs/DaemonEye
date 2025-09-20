#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::shadow_reuse,
    clippy::arithmetic_side_effects,
    clippy::unchecked_duration_subtraction
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures_util::StreamExt;
use sentinel_lib::collection::{ProcessCollectionService, SysinfoProcessCollector};
use sentinel_lib::models::{ProcessRecord, ProcessStatus};
use std::hint::black_box;
use std::time::Instant;
use sysinfo::System;

/// Benchmark process collection performance
/// Target: <5s for 10,000+ processes
fn bench_process_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_collection");

    // Test with different process counts to measure scalability
    let process_counts = vec![100, 1000, 5000, 10000];

    for count in process_counts {
        group.bench_with_input(
            BenchmarkId::new("sysinfo_collector", count),
            &count,
            |b, &processes_count| {
                b.iter(|| {
                    let _collector = SysinfoProcessCollector::default();
                    let mut system = System::new_all();
                    system.refresh_all();

                    let processes: Vec<_> = system
                        .processes()
                        .iter()
                        .take(processes_count)
                        .map(|(pid, process)| {
                            ProcessRecord::builder()
                                .pid_raw(pid.as_u32())
                                .name(process.name().to_string_lossy().to_string())
                                .status(ProcessStatus::Running)
                                .build()
                                .expect("Failed to build process record")
                        })
                        .collect();

                    black_box(processes)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark process collection with streaming
/// Tests the streaming API for memory efficiency
fn bench_process_collection_streaming(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_collection_streaming");

    group.bench_function("stream_small_batch", |b| {
        b.iter(|| {
            let collector = SysinfoProcessCollector::default();
            let deadline = Some(
                Instant::now()
                    .checked_add(std::time::Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            );

            let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
            rt.block_on(async {
                let mut stream = collector.stream_processes(deadline);
                let mut count: u32 = 0;
                while (stream.next().await).is_some() {
                    count = count.saturating_add(1);
                    if count >= 1000 {
                        break;
                    } // Limit for benchmark
                }
                black_box(count)
            })
        });
    });

    group.finish();
}

/// Benchmark process record creation and serialization
fn bench_process_record_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_record_operations");

    group.bench_function("create_process_record", |b| {
        b.iter(|| {
            let record = ProcessRecord::builder()
                .pid_raw(1234)
                .name("test_process".to_owned())
                .status(ProcessStatus::Running)
                .executable_path("/usr/bin/test".to_owned())
                .command_line("test --arg value".to_owned())
                .cpu_usage(25.5)
                .memory_usage(1024 * 1024)
                .build()
                .expect("Failed to build process record");

            black_box(record)
        });
    });

    group.bench_function("serialize_process_record", |b| {
        let record = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_owned())
            .status(ProcessStatus::Running)
            .executable_path("/usr/bin/test".to_owned())
            .command_line("test --arg value".to_owned())
            .cpu_usage(25.5)
            .memory_usage(1024 * 1024)
            .build()
            .expect("Failed to build process record");

        b.iter(|| {
            let serialized = serde_json::to_string(&record).expect("Failed to serialize record");
            black_box(serialized)
        });
    });

    group.bench_function("deserialize_process_record", |b| {
        let record = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_owned())
            .status(ProcessStatus::Running)
            .executable_path("/usr/bin/test".to_owned())
            .command_line("test --arg value".to_owned())
            .cpu_usage(25.5)
            .memory_usage(1024 * 1024)
            .build()
            .expect("Failed to build process record");

        let serialized = serde_json::to_string(&record).expect("Failed to serialize record");

        b.iter(|| {
            let deserialized: ProcessRecord =
                serde_json::from_str(&serialized).expect("Failed to deserialize record");
            black_box(deserialized)
        });
    });

    group.finish();
}

/// Benchmark system information collection
fn bench_system_info_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("system_info_collection");

    group.bench_function("refresh_system", |b| {
        b.iter(|| {
            let mut system = System::new_all();
            system.refresh_all();
            black_box(system.processes().len())
        });
    });

    group.bench_function("refresh_processes_only", |b| {
        b.iter(|| {
            let mut system = System::new_all();
            system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            black_box(system.processes().len())
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_process_collection,
    bench_process_collection_streaming,
    bench_process_record_operations,
    bench_system_info_collection
);
criterion_main!(benches);
