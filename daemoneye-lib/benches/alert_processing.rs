#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::let_underscore_must_use,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::missing_const_for_fn
)]

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use daemoneye_lib::alerting::{AlertManager, AlertSink, DeliveryResult, NoopPublisher};
use daemoneye_lib::config::AlertingConfig;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord, ProcessStatus};
use daemoneye_lib::storage::EventStore;
use std::hint::black_box;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::watch;

/// Mock alert sink for benchmarking
#[derive(Debug, Clone)]
struct MockAlertSink {
    name: String,
    success_rate: f64,
    latency_ms: u64,
}

impl MockAlertSink {
    fn new(name: String, success_rate: f64, latency_ms: u64) -> Self {
        Self {
            name,
            success_rate,
            latency_ms,
        }
    }
}

#[async_trait::async_trait]
impl AlertSink for MockAlertSink {
    async fn send(
        &self,
        _alert: &Alert,
    ) -> Result<DeliveryResult, daemoneye_lib::alerting::AlertingError> {
        // Simulate network latency
        tokio::time::sleep(tokio::time::Duration::from_millis(self.latency_ms)).await;

        // Simulate success/failure via the Result wrapper — the correct semantic.
        // `Ok` means delivery succeeded; `Err` means it failed.
        if rand::random::<f64>() < self.success_rate {
            Ok(DeliveryResult {
                sink_name: self.name.clone(),
                delivered_at: chrono::Utc::now(),
                duration_ms: self.latency_ms,
            })
        } else {
            Err(daemoneye_lib::alerting::AlertingError::SinkError(format!(
                "Simulated failure for sink {}",
                self.name
            )))
        }
    }

    async fn health_check(&self) -> Result<(), daemoneye_lib::alerting::AlertingError> {
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Benchmark alert creation and serialization
fn bench_alert_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_creation");

    group.bench_function("create_alert", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();

            let process_record = ProcessRecord::builder()
                .pid_raw(1234)
                .name("test_process".to_string())
                .status(ProcessStatus::Running)
                .build()
                .unwrap();

            let alert = Alert::new(
                AlertSeverity::High,
                "Test alert",
                "Test alert message",
                "test_rule",
                process_record,
                Completeness::complete(),
            );

            let duration = start.elapsed();
            black_box((alert, duration))
        });
    });

    group.bench_function("serialize_alert", |b| {
        let process_record = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_string())
            .status(ProcessStatus::Running)
            .build()
            .unwrap();

        let alert = Alert::new(
            AlertSeverity::High,
            "Test alert",
            "Test alert message",
            "test_rule",
            process_record,
            Completeness::complete(),
        );

        b.iter(|| {
            let start = std::time::Instant::now();
            let serialized = serde_json::to_string(&alert).unwrap();
            let duration = start.elapsed();

            black_box((serialized.len(), duration))
        });
    });

    group.bench_function("deserialize_alert", |b| {
        let process_record = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_string())
            .status(ProcessStatus::Running)
            .build()
            .unwrap();

        let alert = Alert::new(
            AlertSeverity::High,
            "Test alert",
            "Test alert message",
            "test_rule",
            process_record,
            Completeness::complete(),
        );
        let serialized = serde_json::to_string(&alert).unwrap();

        b.iter(|| {
            let start = std::time::Instant::now();
            let deserialized: Alert = serde_json::from_str(&serialized).unwrap();
            let duration = start.elapsed();

            black_box((deserialized, duration))
        });
    });

    group.finish();
}

/// A started manager over a temp store; the guards keep the store and the cancellation alive.
struct Rig {
    _dir: tempfile::TempDir,
    _cancel: watch::Sender<bool>,
    manager: AlertManager,
}

fn start_rig(rt: &Runtime, sinks: &[MockAlertSink]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("bench.redb")).unwrap());
    let (cancel, cancel_rx) = watch::channel(false);
    let boxed: Vec<Box<dyn AlertSink>> = sinks
        .iter()
        .map(|sink| Box::new(sink.clone()) as Box<dyn AlertSink>)
        .collect();
    let manager = rt
        .block_on(AlertManager::start(
            Arc::clone(&store),
            boxed,
            &AlertingConfig::default(),
            Arc::new(NoopPublisher),
            cancel_rx,
        ))
        .unwrap();
    Rig {
        _dir: dir,
        _cancel: cancel,
        manager,
    }
}

/// An alert whose deduplication key is unique to `i`, so none is suppressed.
fn alert_number(i: usize) -> Alert {
    Alert::new(
        AlertSeverity::Low,
        format!("Alert {}", i),
        format!("Alert message {}", i),
        format!("rule_{}", i),
        ProcessRecord::builder()
            .pid_raw(i as u32)
            .name(format!("process_{}", i))
            .status(ProcessStatus::Running)
            .build()
            .unwrap(),
        Completeness::complete(),
    )
}

/// Benchmark alert manager operations
fn bench_alert_manager_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_manager_operations");

    let rt = Runtime::new().unwrap();
    let three = [
        MockAlertSink::new("sink1".to_string(), 0.9, 10),
        MockAlertSink::new("sink2".to_string(), 0.8, 15),
        MockAlertSink::new("sink3".to_string(), 0.95, 5),
    ];

    group.bench_function("start_alert_manager", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let rig = start_rig(&rt, &three);
            rt.block_on(rig.manager.shutdown(std::time::Duration::ZERO));
            black_box(start.elapsed())
        });
    });

    let single = start_rig(&rt, &three[..1]);
    let mut single_count = 0_usize;
    group.bench_function("enqueue_single_sink", |b| {
        b.iter(|| {
            single_count += 1;
            let alert = alert_number(single_count);
            let start = std::time::Instant::now();
            let result = rt.block_on(single.manager.enqueue(&alert));
            black_box((result, start.elapsed()))
        });
    });

    let multi = start_rig(&rt, &three);
    let mut multi_count = 0_usize;
    group.bench_function("enqueue_multiple_sinks", |b| {
        b.iter(|| {
            multi_count += 1;
            let alert = alert_number(multi_count);
            let start = std::time::Instant::now();
            let result = rt.block_on(multi.manager.enqueue(&alert));
            black_box((result, start.elapsed()))
        });
    });

    group.finish();
}

/// Benchmark enqueue cost with different sink counts
fn bench_alert_delivery_configurations(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_delivery_configurations");

    let rt = Runtime::new().unwrap();
    let sink_counts = vec![1, 3, 5, 10, 20];

    for sink_count in sink_counts {
        group.bench_with_input(
            BenchmarkId::new("delivery_sinks", sink_count),
            &sink_count,
            |b, &sink_count| {
                let sinks: Vec<_> = (0..sink_count)
                    .map(|i| {
                        MockAlertSink::new(
                            format!("sink_{}", i),
                            0.9,
                            10 + (i as u64 * 5), // Varying latency
                        )
                    })
                    .collect();
                let rig = start_rig(&rt, &sinks);
                let mut count = 0_usize;

                b.iter(|| {
                    count += 1;
                    let alert = alert_number(count);
                    let start = std::time::Instant::now();
                    let result = rt.block_on(rig.manager.enqueue(&alert));
                    black_box((result, start.elapsed()))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark enqueue cost while sinks fail at different rates (workers retry in the background)
fn bench_alert_delivery_success_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_delivery_success_rates");

    let rt = Runtime::new().unwrap();
    let success_rates = vec![0.5, 0.7, 0.8, 0.9, 0.95, 1.0];

    for success_rate in success_rates {
        group.bench_with_input(
            BenchmarkId::new("delivery_success_rate", (success_rate * 100.0) as u64),
            &success_rate,
            |b, &success_rate| {
                let sinks = [
                    MockAlertSink::new("sink1".to_string(), success_rate, 10),
                    MockAlertSink::new("sink2".to_string(), success_rate, 15),
                    MockAlertSink::new("sink3".to_string(), success_rate, 5),
                ];
                let rig = start_rig(&rt, &sinks);
                let mut count = 0_usize;

                b.iter(|| {
                    count += 1;
                    let alert = alert_number(count);
                    let start = std::time::Instant::now();
                    let result = rt.block_on(rig.manager.enqueue(&alert));
                    black_box((result, start.elapsed()))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent alert processing
fn bench_concurrent_alert_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_alert_processing");

    let rt = Runtime::new().unwrap();
    let rig = start_rig(&rt, &[MockAlertSink::new("sink1".to_string(), 1.0, 5)]);
    let manager = Arc::new(rig.manager);
    let mut round = 0_usize;

    group.bench_function("concurrent_alert_enqueue", |b| {
        b.iter(|| {
            round += 1;
            let start = std::time::Instant::now();
            let results = rt.block_on(async {
                let handles: Vec<_> = (0..100)
                    .map(|i| {
                        let alert = alert_number(round * 100 + i);
                        let manager = Arc::clone(&manager);
                        tokio::spawn(async move { manager.enqueue(&alert).await })
                    })
                    .collect();

                futures::future::join_all(handles).await
            });
            black_box((results.len(), start.elapsed()))
        });
    });
    group.finish();
}

/// Benchmark alert enqueue throughput
fn bench_alert_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_throughput");

    let rt = Runtime::new().unwrap();
    // Each enqueue is a durable commit, so the old in-memory volumes would run for minutes.
    group.sample_size(10);
    let alert_counts = vec![10, 100, 500];

    for alert_count in alert_counts {
        group.bench_with_input(
            BenchmarkId::new("throughput", alert_count),
            &alert_count,
            |b, &alert_count| {
                let sinks = [
                    MockAlertSink::new("sink1".to_string(), 0.9, 5),
                    MockAlertSink::new("sink2".to_string(), 0.8, 8),
                ];
                let rig = start_rig(&rt, &sinks);
                let mut round = 0_usize;

                b.iter(|| {
                    round += 1;
                    let start = std::time::Instant::now();
                    for i in 0..alert_count {
                        let alert = alert_number(round * 10_000 + i);
                        let _ = rt.block_on(rig.manager.enqueue(&alert));
                    }
                    let duration = start.elapsed();
                    let throughput = alert_count as f64 / duration.as_secs_f64();
                    black_box((throughput, duration))
                });
            },
        );
    }

    group.finish();
}

/// Rows per second through the outbox: enqueue, claim and complete 1,000 alerts on one sink.
fn bench_outbox_rows_per_second(c: &mut Criterion) {
    use daemoneye_lib::models::DeliveryState;

    const ALERTS: usize = 1_000;
    const MAX_OPEN: usize = 10_000;
    let mut group = c.benchmark_group("alert_outbox");
    group.throughput(Throughput::Elements(ALERTS as u64));
    group.sample_size(10);

    // A fresh store per iteration keeps the table size constant; `_dir` rides in the tuple so the
    // directory outlives the store, and returning the tuple drops it outside the timed region.
    group.bench_function("enqueue_claim_complete_1000", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let store = EventStore::new(dir.path().join("outbox.redb")).unwrap();
                (dir, store)
            },
            |(dir, store)| {
                let now_ms = 1_000_u64;
                for i in 0..ALERTS {
                    let alert = alert_number(i);
                    store
                        .enqueue_alert(&alert, &["sink"], None, MAX_OPEN, now_ms)
                        .unwrap();
                    let claimed = store.claim("sink", alert.id, now_ms).unwrap();
                    let mut done = claimed.record.clone();
                    done.state = DeliveryState::Delivered;
                    done.terminal_at_ms = Some(now_ms);
                    store
                        .complete("sink", alert.id, claimed.expected, &done)
                        .unwrap();
                }
                (dir, store)
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

/// Benchmark alert serialization performance
fn bench_alert_serialization_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_serialization_performance");

    // Test with different alert sizes
    let alert_sizes = vec![100, 500, 1000, 5000];

    for alert_size in alert_sizes {
        group.bench_with_input(
            BenchmarkId::new("serialization_size", alert_size),
            &alert_size,
            |b, &alert_size| {
                b.iter(|| {
                    let alert = Alert::new(
                        AlertSeverity::High,
                        "Test alert",
                        "x".repeat(alert_size),
                        "test_rule",
                        ProcessRecord::builder()
                            .pid_raw(1234)
                            .name("test_process".to_string())
                            .status(ProcessStatus::Running)
                            .build()
                            .unwrap(),
                        Completeness::complete(),
                    );

                    let start = std::time::Instant::now();
                    let serialized = serde_json::to_string(&alert).unwrap();
                    let duration = start.elapsed();

                    black_box((serialized.len(), duration))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_alert_creation,
    bench_alert_manager_operations,
    bench_alert_delivery_configurations,
    bench_alert_delivery_success_rates,
    bench_concurrent_alert_processing,
    bench_alert_throughput,
    bench_outbox_rows_per_second,
    bench_alert_serialization_performance
);
criterion_main!(benches, enrich_benches);

/// Enrichment of a 16-deep ancestry chain, one record per hourly bucket, over 168 buckets.
fn bench_enrich_16_deep_chain_over_168_buckets(c: &mut Criterion) {
    use daemoneye_lib::alerting::{EnrichContext, enrich};
    use daemoneye_lib::models::ProcessId;
    use daemoneye_lib::storage::EventStore;
    use std::time::{Duration, UNIX_EPOCH};

    const HOUR_MS: u64 = 3_600_000;
    const BASE_HOUR: u64 = 1_000;
    const BUCKETS: u64 = 168;
    const DEPTH: u32 = 17;

    let dir = tempfile::tempdir().unwrap();
    let store = EventStore::new(dir.path().join("events.redb")).unwrap();
    let mut child = None;
    for k in 0..DEPTH {
        let hour = BASE_HOUR + BUCKETS - 1 - u64::from(k) * 10;
        let mut record = ProcessRecord::new(100 + k, format!("p{k}"));
        record.ppid = (k + 1 < DEPTH).then(|| ProcessId::new(101 + k));
        record.start_time = Some(UNIX_EPOCH + Duration::from_millis(hour * HOUR_MS));
        store.put_event(hour * HOUR_MS, 0, &record).unwrap();
        if k == 0 {
            child = Some(record);
        }
    }
    // Fill the remaining hourly buckets so the scan crosses all 168.
    for offset in 0..BUCKETS {
        let hour = BASE_HOUR + offset;
        store
            .put_event(
                hour * HOUR_MS + 1,
                0,
                &ProcessRecord::new(9, "filler".to_owned()),
            )
            .unwrap();
    }
    let alert = Alert::new(
        AlertSeverity::High,
        "t",
        "d",
        "rule-1",
        child.unwrap(),
        Completeness::complete(),
    );
    let ctx = EnrichContext {
        hostname: "bench-host".to_owned(),
        agent_version: "0.0.0".to_owned(),
    };
    c.bench_function("enrich_16_deep_chain_over_168_buckets", |b| {
        b.iter(|| black_box(enrich(&store, black_box(&alert), &ctx)));
    });
}

criterion_group!(enrich_benches, bench_enrich_16_deep_chain_over_168_buckets);
