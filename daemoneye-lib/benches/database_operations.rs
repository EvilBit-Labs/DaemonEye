#![allow(
    clippy::unwrap_used, // We allow unwraps in benchmarks
    clippy::expect_used, // We allow expects in benchmarks
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::needless_collect,
    clippy::shadow_unrelated
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use daemoneye_lib::models::{
    Alert, AlertSeverity, DetectionRule, ProcessRecord, ProcessStatus, RuleId,
};
use daemoneye_lib::storage::DatabaseManager;
use std::hint::black_box;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Benchmark database write performance
/// Target: >1,000 records/sec write rate
fn bench_database_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("database_writes");

    let rt = Runtime::new().expect("Failed to create runtime");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("bench.db");

    // Test with different batch sizes
    let batch_sizes = vec![10, 100, 1000, 5000];

    for batch_size in batch_sizes {
        group.bench_with_input(
            BenchmarkId::new("batch_write_processes", batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    rt.block_on(async {
                        let db_manager = DatabaseManager::new(&db_path)
                            .expect("Failed to create database manager");

                        // Create test data
                        let processes: Vec<ProcessRecord> = (0..size)
                            .map(|i| {
                                ProcessRecord::builder()
                                    .pid_raw(i.try_into().unwrap_or(0))
                                    .name(format!("process_{i}"))
                                    .status(ProcessStatus::Running)
                                    .build()
                                    .expect("Failed to build process record")
                            })
                            .collect();

                        // Benchmark batch write
                        let start = std::time::Instant::now();
                        let processes_with_ids: Vec<(u64, ProcessRecord)> = processes
                            .into_iter()
                            .enumerate()
                            .map(|(i, p)| (u64::try_from(i).unwrap_or(0), p))
                            .collect();
                        db_manager
                            .store_processes_batch(&processes_with_ids)
                            .expect("Failed to store processes batch");
                        let duration = start.elapsed();

                        // Calculate records per second
                        let records_per_sec = f64::from(size) / duration.as_secs_f64();
                        black_box(records_per_sec)
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark database read performance
fn bench_database_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("database_reads");

    let rt = Runtime::new().expect("Failed to create runtime");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("bench.db");

    // Setup test data
    rt.block_on(async {
        let db_manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Insert test data
        let processes_with_ids: Vec<(u64, ProcessRecord)> = (0..10000)
            .map(|i| {
                let process = ProcessRecord::builder()
                    .pid_raw(i.try_into().unwrap_or(0))
                    .name(format!("process_{i}"))
                    .status(ProcessStatus::Running)
                    .build()
                    .expect("Failed to build process record");
                (u64::try_from(i).unwrap_or(0), process)
            })
            .collect();

        db_manager
            .store_processes_batch(&processes_with_ids)
            .expect("Failed to store processes batch");
    });

    group.bench_function("get_all_processes", |b| {
        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");
                let processes = db_manager
                    .get_all_processes()
                    .expect("Failed to get processes");
                black_box(processes.len())
            })
        });
    });

    group.bench_function("get_stats", |b| {
        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");
                let stats = db_manager.get_stats().expect("Failed to get stats");
                black_box(stats)
            })
        });
    });

    group.finish();
}

/// Benchmark alert storage and retrieval
fn bench_alert_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_operations");

    let rt = Runtime::new().expect("Failed to create runtime");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("bench.db");

    group.bench_function("store_alert", |b| {
        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");

                let process_record = ProcessRecord::builder()
                    .pid_raw(1234)
                    .name("test_process".to_owned())
                    .status(ProcessStatus::Running)
                    .build()
                    .expect("Failed to build process record");

                let alert = Alert::new(
                    AlertSeverity::High,
                    "Test alert",
                    "Test alert description",
                    "test_rule",
                    process_record,
                );

                let start = std::time::Instant::now();
                db_manager
                    .store_alert(0, &alert)
                    .expect("Failed to store alert");
                let duration = start.elapsed();
                black_box(duration)
            })
        });
    });

    group.bench_function("get_all_alerts", |b| {
        // Setup test data
        rt.block_on(async {
            let db_manager =
                DatabaseManager::new(&db_path).expect("Failed to create database manager");

            for i in 0..1000 {
                let process_record = ProcessRecord::builder()
                    .pid_raw(i.try_into().unwrap_or(0))
                    .name(format!("process_{i}"))
                    .status(ProcessStatus::Running)
                    .build()
                    .expect("Failed to build process record");

                let alert = Alert::new(
                    AlertSeverity::Medium,
                    format!("Alert {i}"),
                    format!("Alert description {i}"),
                    format!("rule_{i}"),
                    process_record,
                );
                db_manager
                    .store_alert(0, &alert)
                    .expect("Failed to store alert");
            }
        });

        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");
                let alerts = db_manager.get_all_alerts().expect("Failed to get alerts");
                black_box(alerts.len())
            })
        });
    });

    group.finish();
}

/// Benchmark rule storage and retrieval
fn bench_rule_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_operations");

    let rt = Runtime::new().expect("Failed to create runtime");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("bench.db");

    group.bench_function("store_rule", |b| {
        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");

                let rule = DetectionRule::new(
                    RuleId::new("test_rule"),
                    "Test Rule",
                    "Test rule",
                    "SELECT * FROM processes WHERE name = 'test'",
                    "benchmark",
                    AlertSeverity::Low,
                );

                let start = std::time::Instant::now();
                db_manager.store_rule(&rule).expect("Failed to store rule");
                let duration = start.elapsed();
                black_box(duration)
            })
        });
    });

    group.bench_function("get_all_rules", |b| {
        // Setup test data
        rt.block_on(async {
            let db_manager =
                DatabaseManager::new(&db_path).expect("Failed to create database manager");

            for i in 0..100 {
                let rule = DetectionRule::new(
                    RuleId::new(format!("rule_{i}")),
                    format!("Rule {i}"),
                    format!("Rule {i}"),
                    format!("SELECT * FROM processes WHERE name = 'process_{i}'"),
                    "benchmark",
                    AlertSeverity::Medium,
                );
                db_manager.store_rule(&rule).expect("Failed to store rule");
            }
        });

        b.iter(|| {
            rt.block_on(async {
                let db_manager =
                    DatabaseManager::new(&db_path).expect("Failed to create database manager");
                let rules = db_manager.get_all_rules().expect("Failed to get rules");
                black_box(rules.len())
            })
        });
    });

    group.finish();
}

/// Benchmark concurrent database operations
fn bench_concurrent_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_operations");

    let rt = Runtime::new().expect("Failed to create runtime");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("bench.db");

    group.bench_function("concurrent_reads", |b| {
        // Setup test data once outside the benchmark loop
        let db_manager =
            Arc::new(DatabaseManager::new(&db_path).expect("Failed to create database manager"));

        // Create test data
        let processes_with_ids: Vec<(u64, ProcessRecord)> = (0..1000)
            .map(|i| {
                let process = ProcessRecord::builder()
                    .pid_raw(i.try_into().unwrap_or(0))
                    .name(format!("process_{i}"))
                    .status(ProcessStatus::Running)
                    .build()
                    .expect("Failed to build process record");
                (u64::try_from(i).unwrap_or(0), process)
            })
            .collect();

        // Store test data in database once
        db_manager
            .store_processes_batch(&processes_with_ids)
            .expect("Failed to store processes batch");

        b.iter(|| {
            rt.block_on(async {
                // Concurrent reads on the pre-populated database
                let handles: Vec<_> = (0..10)
                    .map(|_| {
                        let db_manager_clone = Arc::clone(&db_manager);
                        tokio::spawn(async move {
                            db_manager_clone
                                .get_all_processes()
                                .expect("Failed to get processes")
                        })
                    })
                    .collect();

                let results = futures::future::join_all(handles).await;
                black_box(results.len())
            })
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_database_writes,
    bench_database_reads,
    bench_alert_operations,
    bench_rule_operations,
    bench_concurrent_operations
);
criterion_main!(benches);
