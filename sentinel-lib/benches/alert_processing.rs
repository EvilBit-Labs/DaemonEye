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

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sentinel_lib::alerting::{AlertManager, AlertSink, DeliveryResult};
use sentinel_lib::models::{Alert, AlertSeverity, ProcessRecord, ProcessStatus};
use std::hint::black_box;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Mock alert sink for benchmarking
#[derive(Debug)]
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
    ) -> Result<DeliveryResult, sentinel_lib::alerting::AlertingError> {
        // Simulate network latency
        tokio::time::sleep(tokio::time::Duration::from_millis(self.latency_ms)).await;

        // Simulate success/failure based on success rate
        if rand::random::<f64>() < self.success_rate {
            Ok(DeliveryResult {
                sink_name: self.name.clone(),
                success: true,
                delivered_at: chrono::Utc::now(),
                error_message: None,
                duration_ms: self.latency_ms,
            })
        } else {
            Err(sentinel_lib::alerting::AlertingError::SinkError(format!(
                "Mock sink {} failed to deliver alert",
                self.name
            )))
        }
    }

    async fn health_check(&self) -> Result<(), sentinel_lib::alerting::AlertingError> {
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

/// Benchmark alert manager operations
fn bench_alert_manager_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_manager_operations");

    let rt = Runtime::new().unwrap();

    group.bench_function("create_alert_manager", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();

            let _sinks = [
                Arc::new(MockAlertSink::new("sink1".to_string(), 0.9, 10)),
                Arc::new(MockAlertSink::new("sink2".to_string(), 0.8, 15)),
                Arc::new(MockAlertSink::new("sink3".to_string(), 0.95, 5)),
            ];

            let manager = AlertManager::new();
            let duration = start.elapsed();

            black_box((manager, duration))
        });
    });

    group.bench_function("send_alert_single_sink", |b| {
        rt.block_on(async {
            let _sinks = [Arc::new(MockAlertSink::new("sink1".to_string(), 1.0, 10))];
            let mut manager = AlertManager::new();
            let alert = Alert::new(
                AlertSeverity::High,
                "Test alert",
                "Test alert message",
                "test_rule",
                ProcessRecord::builder()
                    .pid_raw(1234)
                    .name("test_process".to_string())
                    .status(ProcessStatus::Running)
                    .build()
                    .unwrap(),
            );

            b.iter(|| {
                let start = std::time::Instant::now();
                let result = rt.block_on(manager.send_alert(&alert));
                let duration = start.elapsed();

                black_box((result, duration))
            });
        });
    });

    group.bench_function("send_alert_multiple_sinks", |b| {
        rt.block_on(async {
            let _sinks = [
                Arc::new(MockAlertSink::new("sink1".to_string(), 0.9, 10)),
                Arc::new(MockAlertSink::new("sink2".to_string(), 0.8, 15)),
                Arc::new(MockAlertSink::new("sink3".to_string(), 0.95, 5)),
            ];
            let mut manager = AlertManager::new();
            let alert = Alert::new(
                AlertSeverity::High,
                "Test alert",
                "Test alert message",
                "test_rule",
                ProcessRecord::builder()
                    .pid_raw(1234)
                    .name("test_process".to_string())
                    .status(ProcessStatus::Running)
                    .build()
                    .unwrap(),
            );

            b.iter(|| {
                let start = std::time::Instant::now();
                let result = rt.block_on(manager.send_alert(&alert));
                let duration = start.elapsed();

                black_box((result, duration))
            });
        });
    });

    group.finish();
}

/// Benchmark alert delivery with different sink configurations
fn bench_alert_delivery_configurations(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_delivery_configurations");

    let rt = Runtime::new().unwrap();
    let alert = Alert::new(
        AlertSeverity::High,
        "Test alert",
        "Test alert message",
        "test_rule",
        ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_string())
            .status(ProcessStatus::Running)
            .build()
            .unwrap(),
    );

    // Test with different numbers of sinks
    let sink_counts = vec![1, 3, 5, 10, 20];

    for sink_count in sink_counts {
        group.bench_with_input(
            BenchmarkId::new("delivery_sinks", sink_count),
            &sink_count,
            |b, &sink_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let _sinks: Vec<_> = (0..sink_count)
                            .map(|i| {
                                Arc::new(MockAlertSink::new(
                                    format!("sink_{}", i),
                                    0.9,
                                    10 + (i as u64 * 5), // Varying latency
                                ))
                            })
                            .collect();

                        let mut manager = AlertManager::new();

                        let start = std::time::Instant::now();
                        let result = manager.send_alert(&alert).await;
                        let duration = start.elapsed();

                        black_box((result, duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark alert delivery with different success rates
fn bench_alert_delivery_success_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_delivery_success_rates");

    let rt = Runtime::new().unwrap();
    let alert = Alert::new(
        AlertSeverity::High,
        "Test alert",
        "Test alert message",
        "test_rule",
        ProcessRecord::builder()
            .pid_raw(1234)
            .name("test_process".to_string())
            .status(ProcessStatus::Running)
            .build()
            .unwrap(),
    );

    // Test with different success rates
    let success_rates = vec![0.5, 0.7, 0.8, 0.9, 0.95, 1.0];

    for success_rate in success_rates {
        group.bench_with_input(
            BenchmarkId::new("delivery_success_rate", (success_rate * 100.0) as u64),
            &success_rate,
            |b, &success_rate| {
                b.iter(|| {
                    rt.block_on(async {
                        let _sinks = [
                            Arc::new(MockAlertSink::new("sink1".to_string(), success_rate, 10)),
                            Arc::new(MockAlertSink::new("sink2".to_string(), success_rate, 15)),
                            Arc::new(MockAlertSink::new("sink3".to_string(), success_rate, 5)),
                        ];

                        let mut manager = AlertManager::new();

                        let start = std::time::Instant::now();
                        let result = manager.send_alert(&alert).await;
                        let duration = start.elapsed();

                        black_box((result, duration))
                    })
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

    group.bench_function("concurrent_alert_sending", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _sinks = [
                    Arc::new(MockAlertSink::new("sink1".to_string(), 0.9, 10)),
                    Arc::new(MockAlertSink::new("sink2".to_string(), 0.8, 15)),
                ];

                let _manager = AlertManager::new();

                // Create multiple alerts
                let alerts: Vec<Alert> = (0..100)
                    .map(|i| {
                        Alert::new(
                            AlertSeverity::Medium,
                            format!("Alert {}", i),
                            format!("Alert message {}", i),
                            format!("rule_{}", i),
                            ProcessRecord::builder()
                                .pid_raw(i as u32)
                                .name(format!("process_{}", i))
                                .status(ProcessStatus::Running)
                                .build()
                                .unwrap(),
                        )
                    })
                    .collect();

                let start = std::time::Instant::now();

                // Send alerts concurrently
                let handles: Vec<_> = alerts
                    .iter()
                    .map(|alert| {
                        let alert = alert.clone();
                        tokio::spawn(async move {
                            let mut manager = AlertManager::new();
                            manager.send_alert(&alert).await
                        })
                    })
                    .collect();

                let results = futures::future::join_all(handles).await;
                let duration = start.elapsed();

                black_box((results.len(), duration))
            })
        });
    });

    group.finish();
}

/// Benchmark alert throughput
fn bench_alert_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("alert_throughput");

    let rt = Runtime::new().unwrap();

    // Test with different alert volumes
    let alert_counts = vec![10, 100, 1000, 5000];

    for alert_count in alert_counts {
        group.bench_with_input(
            BenchmarkId::new("throughput", alert_count),
            &alert_count,
            |b, &alert_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let _sinks = [
                            Arc::new(MockAlertSink::new("sink1".to_string(), 0.9, 5)),
                            Arc::new(MockAlertSink::new("sink2".to_string(), 0.8, 8)),
                        ];

                        let mut manager = AlertManager::new();

                        let start = std::time::Instant::now();

                        // Send alerts sequentially
                        for i in 0..alert_count {
                            let alert = Alert::new(
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
                            );

                            let _ = manager.send_alert(&alert).await;
                        }

                        let duration = start.elapsed();
                        let throughput = alert_count as f64 / duration.as_secs_f64();

                        black_box((throughput, duration))
                    })
                });
            },
        );
    }

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
    bench_alert_serialization_performance
);
criterion_main!(benches);
