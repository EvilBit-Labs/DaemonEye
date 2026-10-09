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

use criterion::{Criterion, criterion_group, criterion_main};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::models::{AlertSeverity, DetectionRule, RuleId};
use std::hint::black_box;

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

criterion_group!(benches, bench_rule_loading);
criterion_main!(benches);
