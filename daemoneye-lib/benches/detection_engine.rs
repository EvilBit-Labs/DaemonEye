#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::modulo_arithmetic,
    clippy::cast_lossless
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::models::{AlertSeverity, DetectionRule, ProcessRecord, ProcessStatus, RuleId};
use std::hint::black_box;

/// Benchmark detection engine rule execution
/// Target: <100ms per rule
fn bench_detection_engine_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("detection_engine_execution");

    // Create Tokio runtime once outside the benchmark loop
    let rt = tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime for detection engine benchmark");

    // Test with different numbers of processes
    let process_counts = vec![100, 1000, 5000, 10000];

    for process_count in process_counts {
        group.bench_with_input(
            BenchmarkId::new("execute_rules", process_count),
            &process_count,
            |b, &process_count| {
                b.iter(|| {
                    rt.block_on(async {
                        // Create test processes
                        let processes: Vec<ProcessRecord> = (0..process_count)
                            .map(|i| {
                                ProcessRecord::builder()
                                    .pid_raw(i as u32)
                                    .name(format!("process_{}", i))
                                    .status(ProcessStatus::Running)
                                    .executable_path(format!("/usr/bin/process_{}", i))
                                    .command_line(format!("process_{} --arg value", i))
                                    .cpu_usage((i as f64 % 100.0) / 100.0)
                                    .memory_usage(1024 * 1024 + (i as u64 * 1024))
                                    .build()
                                    .expect("Failed to build ProcessRecord in benchmark")
                            })
                            .collect();

                        // Create detection engine with test rules
                        let mut engine = DetectionEngine::new();

                        // Add simple rule
                        let rule1 = DetectionRule::new(
                            RuleId::new("high_cpu_rule"),
                            "High CPU Rule",
                            "High CPU usage detection",
                            "SELECT * FROM processes WHERE cpu_usage > 0.8",
                            "performance",
                            AlertSeverity::High,
                        );
                        engine
                            .load_rule(rule1)
                            .expect("Failed to load rule1 in benchmark");

                        // Add memory rule
                        let rule2 = DetectionRule::new(
                            RuleId::new("high_memory_rule"),
                            "High Memory Rule",
                            "High memory usage detection",
                            "SELECT * FROM processes WHERE memory_usage > 100000000",
                            "performance",
                            AlertSeverity::High,
                        );
                        engine
                            .load_rule(rule2)
                            .expect("Failed to load rule2 in benchmark");

                        // Add name pattern rule
                        let rule3 = DetectionRule::new(
                            RuleId::new("suspicious_name_rule"),
                            "Suspicious Name Rule",
                            "Suspicious process name detection",
                            "SELECT * FROM processes WHERE name LIKE '%suspicious%'",
                            "security",
                            AlertSeverity::Medium,
                        );
                        engine
                            .load_rule(rule3)
                            .expect("Failed to load rule3 in benchmark");

                        // Benchmark rule execution
                        let start = std::time::Instant::now();
                        let alerts = engine.execute_rules(&processes);
                        let duration = start.elapsed();

                        black_box((alerts.len(), duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark individual rule execution
fn bench_single_rule_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_rule_execution");

    let rt = tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime for single rule benchmark");

    // Create test processes
    let processes: Vec<ProcessRecord> = (0..10000)
        .map(|i| {
            ProcessRecord::builder()
                .pid_raw(i as u32)
                .name(format!("process_{}", i))
                .status(ProcessStatus::Running)
                .executable_path(format!("/usr/bin/process_{}", i))
                .command_line(format!("process_{} --arg value", i))
                .cpu_usage((i as f64 % 100.0) / 100.0)
                .memory_usage(1024 * 1024 + (i as u64 * 1024))
                .build()
                .expect("Failed to build ProcessRecord in single rule benchmark")
        })
        .collect();

    // Test different rule complexities
    let rules = vec![
        ("simple_rule", "SELECT * FROM processes WHERE pid > 1000"),
        ("cpu_rule", "SELECT * FROM processes WHERE cpu_usage > 0.5"),
        (
            "memory_rule",
            "SELECT * FROM processes WHERE memory_usage > 50000000",
        ),
        (
            "complex_rule",
            "SELECT * FROM processes WHERE cpu_usage > 0.3 AND memory_usage > 10000000 AND name LIKE '%test%'",
        ),
        (
            "join_rule",
            "SELECT p1.* FROM processes p1, processes p2 WHERE p1.pid != p2.pid AND p1.cpu_usage > p2.cpu_usage",
        ),
    ];

    for (rule_name, sql_query) in rules {
        group.bench_function(rule_name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    let mut engine = DetectionEngine::new();
                    let rule = DetectionRule::new(
                        RuleId::new(rule_name),
                        format!("{} Rule", rule_name),
                        format!("{} rule", rule_name),
                        sql_query.to_string(),
                        "benchmark",
                        AlertSeverity::Medium,
                    );
                    engine
                        .load_rule(rule)
                        .expect("Failed to load rule in single rule benchmark");

                    let start = std::time::Instant::now();
                    let alerts = engine.execute_rules(&processes);
                    let duration = start.elapsed();

                    black_box((alerts.len(), duration))
                })
            });
        });
    }

    group.finish();
}

/// Benchmark rule loading and validation
fn bench_rule_loading(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_loading");

    group.bench_function("load_simple_rule", |b| {
        b.iter(|| {
            let mut engine = DetectionEngine::new();
            let rule = DetectionRule::new(
                RuleId::new("simple_rule"),
                "Simple Rule",
                "Simple rule",
                "SELECT * FROM processes WHERE pid > 1000",
                "benchmark",
                AlertSeverity::Low,
            );

            let start = std::time::Instant::now();
            engine
                .load_rule(rule)
                .expect("Failed to load simple rule in rule loading benchmark");
            let duration = start.elapsed();

            black_box(duration)
        });
    });

    group.bench_function("load_complex_rule", |b| {
        b.iter(|| {
            let mut engine = DetectionEngine::new();
            let rule = DetectionRule::new(
                RuleId::new("complex_rule"),
                "Complex Rule",
                "Complex rule",
                "SELECT * FROM processes WHERE cpu_usage > 0.3 AND memory_usage > 10000000 AND name LIKE '%test%' AND executable_path LIKE '/usr/bin/%'",
                "benchmark",
                AlertSeverity::Medium,
            );

            let start = std::time::Instant::now();
            engine.load_rule(rule).expect("Failed to load complex rule in rule loading benchmark");
            let duration = start.elapsed();

            black_box(duration)
        });
    });

    group.bench_function("load_invalid_rule", |b| {
        b.iter(|| {
            let mut engine = DetectionEngine::new();
            let rule = DetectionRule::new(
                RuleId::new("invalid_rule"),
                "Invalid Rule",
                "Invalid rule",
                "INVALID SQL SYNTAX",
                "benchmark",
                AlertSeverity::Low,
            );

            let start = std::time::Instant::now();
            let result = engine.load_rule(rule);
            let duration = start.elapsed();

            black_box((result.is_err(), duration))
        });
    });

    group.finish();
}

/// Benchmark multiple rules execution
fn bench_multiple_rules_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiple_rules_execution");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    // Create test processes
    let processes: Vec<ProcessRecord> = (0..5000)
        .map(|i| {
            ProcessRecord::builder()
                .pid_raw(i as u32)
                .name(format!("process_{i}"))
                .status(ProcessStatus::Running)
                .executable_path(format!("/usr/bin/process_{i}"))
                .command_line(format!("process_{i} --arg value"))
                .cpu_usage((f64::from(i) % 100.0) / 100.0)
                .memory_usage(1024 * 1024 + (i as u64 * 1024))
                .build()
                .expect("Failed to build process record")
        })
        .collect();

    // Test with different numbers of rules
    let rule_counts = vec![1, 5, 10, 20, 50];

    for rule_count in rule_counts {
        group.bench_with_input(
            BenchmarkId::new("execute_multiple_rules", rule_count),
            &rule_count,
            |b, &rule_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut engine = DetectionEngine::new();

                        // Add multiple rules
                        for i in 0..rule_count {
                            let rule = DetectionRule::new(
                                RuleId::new(format!("rule_{i}")),
                                format!("Rule {i}"),
                                format!("Rule {i}"),
                                format!(
                                    "SELECT * FROM processes WHERE pid > {} AND cpu_usage > 0.{}",
                                    i * 100,
                                    i % 10
                                ),
                                "benchmark",
                                AlertSeverity::Medium,
                            );
                            engine.load_rule(rule).expect("Failed to load rule");
                        }

                        let start = std::time::Instant::now();
                        let alerts = engine.execute_rules(&processes);
                        let duration = start.elapsed();

                        black_box((alerts.len(), duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark rule execution with different process data sizes
fn bench_rule_execution_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_execution_scaling");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    // Test with different process data sizes
    let data_sizes = vec![100, 500, 1000, 2000, 5000];

    for data_size in data_sizes {
        group.bench_with_input(
            BenchmarkId::new("scaling_test", data_size),
            &data_size,
            |b, &data_size| {
                b.iter(|| {
                    rt.block_on(async {
                        // Create processes with varying data sizes
                        let processes: Vec<ProcessRecord> = (0..data_size)
                            .map(|i| {
                                ProcessRecord::builder()
                                    .pid_raw(i as u32)
                                    .name(format!("process_with_very_long_name_{i}"))
                                    .status(ProcessStatus::Running)
                                    .executable_path(format!("/very/long/path/to/executable/process_{i}"))
                                    .command_line(format!("process_{i} --very-long-argument-name value_{i} --another-argument"))
                                    .cpu_usage((f64::from(i) % 100.0) / 100.0)
                                    .memory_usage(1024 * 1024 + (i as u64 * 1024))
                                    .build()
                                    .expect("Failed to build process record")
                            })
                            .collect();

                        let mut engine = DetectionEngine::new();
                        let rule = DetectionRule::new(
                            RuleId::new("scaling_rule"),
                            "Scaling Rule",
                            "Scaling test rule",
                            "SELECT * FROM processes WHERE cpu_usage > 0.5 AND memory_usage > 1000000",
                            "benchmark",
                            AlertSeverity::Medium,
                        );
                        engine.load_rule(rule).expect("Failed to load rule");

                        let start = std::time::Instant::now();
                        let alerts = engine.execute_rules(&processes);
                        let duration = start.elapsed();

                        black_box((alerts.len(), duration))
                    })
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_detection_engine_execution,
    bench_single_rule_execution,
    bench_rule_loading,
    bench_multiple_rules_execution,
    bench_rule_execution_scaling
);
criterion_main!(benches);
