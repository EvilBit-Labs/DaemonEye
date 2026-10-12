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

use std::hint::black_box;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::verify_spawn_token;
use daemoneye_lib::detection::execution::completeness::{
    CollectorHealth, CycleSignals, IngestSnapshot,
};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::RuleExecutor;
use daemoneye_lib::models::{AlertSeverity, DetectionRule, ProcessRecord, RuleId};
use daemoneye_lib::proto::{ColumnDescriptor, ColumnType, SchemaDescriptor, TableDescriptor};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::IngestRecord;

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

/// An hour boundary far enough in the past that the bucket is closed.
const BASE_MS: u64 = 472_222 * 3_600_000;
/// Every `PLANT_STRIDE`th row is the `nc` the rules look for.
const PLANT_STRIDE: u64 = 100;
const NAMES: [&str; 4] = ["bash", "sshd", "nginx", "cron"];

/// The three shapes the executor sees: an index-served equality, the same with a residual the
/// agent evaluates, and a `LIKE` no index serves, which reads every row in the window.
const RULES: [(&str, &str); 3] = [
    (
        "indexed",
        "SELECT pid, name FROM processes WHERE name = 'nc'",
    ),
    (
        "residual",
        "SELECT pid, name FROM processes WHERE name = 'nc' AND pid > 100000",
    ),
    (
        "full_scan",
        "SELECT pid, name FROM processes WHERE command_line LIKE '%planted%'",
    ),
];

fn column(name: &str, column_type: ColumnType, nullable: bool) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(column_type),
        nullable,
        supported_ops: Vec::new(),
    }
}

fn schema() -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![
                column("pid", ColumnType::Uint, false),
                column("name", ColumnType::String, false),
                column("command_line", ColumnType::String, true),
                column("collection_time", ColumnType::Int, false),
            ],
        }],
        conformance_results: Vec::new(),
    }
}

fn row(index: u64) -> ProcessRecord {
    let (pid, name) = if index.is_multiple_of(PLANT_STRIDE) {
        (200_000 + index.checked_div(PLANT_STRIDE).unwrap_or(0), "nc")
    } else {
        let pick = usize::try_from(index % 4).unwrap();
        (
            1 + index % 60_000,
            NAMES.get(pick).copied().unwrap_or("bash"),
        )
    };
    let mut record = ProcessRecord::new(u32::try_from(pid).unwrap(), name.to_owned());
    record.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(BASE_MS + index).unwrap())
        .unwrap();
    let marker = if name == "nc" { " planted" } else { "" };
    record.command_line = Some(format!("/usr/sbin/{name}{marker} --worker {index}"));
    record
}

/// A store of `rows` rows in one bucket, and an executor with [`RULES`] loaded over it.
fn executor_over(dir: &std::path::Path, rows: u64) -> (RuleExecutor, DetectionEngine) {
    let store = Arc::new(EventStore::new(dir.join(format!("{rows}.redb"))).unwrap());
    let batch: Vec<IngestRecord> = (0..rows)
        .map(|i| IngestRecord::new("bench", i, i as u32, row(i)).unwrap())
        .collect();
    store.put_batch(&batch).unwrap();

    let config = DetectionConfig::default();
    let mut engine = DetectionEngine::with_config(&config);
    let token = "a".repeat(64);
    let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
    engine.register_collector(&verified, schema()).unwrap();
    for (id, sql) in RULES {
        let rule = DetectionRule::new(
            RuleId::new(id),
            format!("Rule {id}"),
            "benchmark",
            sql,
            "benchmark",
            AlertSeverity::High,
        );
        engine.load_rule(rule).unwrap();
    }
    let executor = RuleExecutor::new(store, engine.regex_cache(), &config).unwrap();
    (executor, engine)
}

fn healthy() -> CycleSignals {
    CycleSignals {
        collection: [("procmond".to_owned(), Ok(()))].into(),
        heartbeat: [("procmond".to_owned(), CollectorHealth::Healthy)].into(),
        ingest: IngestSnapshot::default(),
    }
}

/// Benchmark one cycle of the executor: every loaded rule over a window holding `rows` rows.
fn bench_rule_execution(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let window = CycleWindow {
        after_ms: BASE_MS - 1,
        through_ms: BASE_MS + 3_600_000 - 1,
    };
    let mut group = c.benchmark_group("rule_execution");

    for rows in [1_000_u64, 10_000, 50_000] {
        let (executor, engine) = executor_over(dir.path(), rows);
        let rules = engine.runnable_rules();
        group.throughput(Throughput::Elements(rows));
        group.bench_with_input(BenchmarkId::new("cycle", rows), &rows, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let outcome = executor.evaluate(&rules, window, &healthy()).await;
                black_box(outcome.evaluations.len())
            });
        });
        for (rule, (id, _)) in rules.iter().zip(RULES) {
            let one = std::slice::from_ref(rule);
            group.bench_with_input(BenchmarkId::new(id, rows), &rows, |b, _| {
                b.to_async(&runtime).iter(|| async {
                    let outcome = executor.evaluate(one, window, &healthy()).await;
                    black_box(outcome.evaluations.len())
                });
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_rule_loading, bench_rule_execution);
criterion_main!(benches);
