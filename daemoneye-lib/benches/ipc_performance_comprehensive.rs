//! Comprehensive performance benchmarks for IPC communication
//!
//! This benchmark suite validates that there are no regressions in message
//! throughput or latency for the interprocess transport implementation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::shadow_unrelated,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use daemoneye_lib::ipc::codec::{IpcCodec, IpcError};
use daemoneye_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
use daemoneye_lib::ipc::{IpcConfig, ResilientIpcClient, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::duplex;
use tokio::runtime::Runtime;

/// Create a test configuration for benchmarks
fn create_benchmark_config(test_name: &str) -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_benchmark_endpoint(&temp_dir, test_name);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 10 * 1024 * 1024, // 10MB for large message tests
        accept_timeout_ms: 5000,
        read_timeout_ms: 30000,
        write_timeout_ms: 30000,
        max_connections: 16,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    (config, temp_dir)
}

/// Create platform-specific benchmark endpoint
fn create_benchmark_endpoint(temp_dir: &TempDir, test_name: &str) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join(format!("bench_{}.sock", test_name))
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bench");
        format!(r"\\.\pipe\daemoneye\bench-{}-{}", test_name, dir_name)
    }
}

/// Create a test detection task
fn create_benchmark_task(task_id: &str, metadata_size: usize) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: (metadata_size > 0).then(|| "x".repeat(metadata_size)),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    }
}

/// Create a test process record
fn create_benchmark_process_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        pid,
        ppid: Some(pid.saturating_sub(1)),
        name: format!("benchmark_process_{}", pid),
        executable_path: Some(format!("/usr/bin/benchmark_{}", pid)),
        command_line: vec![
            format!("benchmark_{}", pid),
            "--test".to_owned(),
            format!("--pid={}", pid),
        ],
        start_time: Some(chrono::Utc::now().timestamp()),
        cpu_usage: Some(25.5),
        memory_usage: Some(1024 * 1024 * u64::from(pid)),
        executable_hash: Some(format!("hash_{:08x}", pid)),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        collection_time: chrono::Utc::now().timestamp_millis(),
    }
}

/// Create a test detection result with specified number of processes
fn create_benchmark_result(task_id: &str, num_processes: usize) -> DetectionResult {
    let mut processes = Vec::with_capacity(num_processes);
    for i in 0..num_processes {
        processes.push(create_benchmark_process_record(i as u32));
    }

    DetectionResult {
        task_id: task_id.to_owned(),
        success: true,
        error_message: None,
        processes,
        hash_result: None,
        network_events: vec![],
        filesystem_events: vec![],
        performance_events: vec![],
    }
}

/// Benchmark codec serialization performance
fn bench_codec_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec_serialization");

    // Test different message sizes
    let message_sizes = vec![
        ("small", 1),
        ("medium", 100),
        ("large", 1000),
        ("xlarge", 10000),
    ];

    for (size_name, num_processes) in message_sizes {
        group.throughput(Throughput::Elements(num_processes as u64));

        group.bench_with_input(
            BenchmarkId::new("serialize_detection_result", size_name),
            &num_processes,
            |b, &num_processes| {
                let result = create_benchmark_result("benchmark", num_processes);

                b.iter(|| {
                    let serialized = prost::Message::encode_to_vec(&result);
                    black_box(serialized.len())
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("deserialize_detection_result", size_name),
            &num_processes,
            |b, &num_processes| {
                let result = create_benchmark_result("benchmark", num_processes);
                let serialized = prost::Message::encode_to_vec(&result);

                b.iter(|| {
                    let deserialized: DetectionResult =
                        prost::Message::decode(&serialized[..]).unwrap();
                    black_box(deserialized.processes.len())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark CRC32 calculation performance
fn bench_crc32_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("crc32_performance");

    // Test different data sizes
    let data_sizes = vec![
        ("1kb", 1024),
        ("10kb", 10 * 1024),
        ("100kb", 100 * 1024),
        ("1mb", 1024 * 1024),
    ];

    for (size_name, data_size) in data_sizes {
        group.throughput(Throughput::Bytes(data_size as u64));

        group.bench_with_input(
            BenchmarkId::new("crc32c", size_name),
            &data_size,
            |b, &data_size| {
                let test_data = vec![0_u8; data_size];
                let _codec = IpcCodec::new(10 * 1024 * 1024);

                b.iter(|| {
                    let crc = crc32c::crc32c(&test_data);
                    black_box(crc)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark message framing and unframing
fn bench_message_framing_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_framing");

    let rt = Runtime::new().unwrap();

    // Test different message sizes
    let message_sizes = vec![("small", 100), ("medium", 10000), ("large", 1_000_000)];

    for (size_name, num_processes) in message_sizes {
        group.throughput(Throughput::Elements(num_processes as u64));

        group.bench_with_input(
            BenchmarkId::new("frame_message", size_name),
            &num_processes,
            |b, &num_processes| {
                let result = create_benchmark_result("benchmark", num_processes);
                let codec = IpcCodec::new(10 * 1024 * 1024);

                b.iter(|| {
                    rt.block_on(async {
                        let (mut client, _server) = duplex(8192);
                        let timeout = Duration::from_secs(1);

                        let write_result = codec.write_message(&mut client, &result, timeout).await;
                        black_box(write_result.is_ok())
                    })
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("unframe_message", size_name),
            &num_processes,
            |b, &num_processes| {
                let result = create_benchmark_result("benchmark", num_processes);
                let mut codec = IpcCodec::new(10 * 1024 * 1024);

                b.iter(|| {
                    rt.block_on(async {
                        let (mut client, mut server) = duplex(8192);
                        let timeout = Duration::from_secs(1);

                        // Write message
                        let _write_result =
                            codec.write_message(&mut client, &result, timeout).await;

                        // Read message back
                        let read_result: Result<DetectionResult, IpcError> =
                            codec.read_message(&mut server, timeout).await;
                        black_box(read_result.is_ok())
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark end-to-end IPC communication
fn bench_end_to_end_ipc(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end_ipc");
    group.sample_size(20); // Reduce sample size for slower end-to-end tests

    let rt = Runtime::new().unwrap();

    // Test different response sizes
    let response_sizes = vec![("small", 1), ("medium", 100), ("large", 1000)];

    for (size_name, num_processes) in response_sizes {
        group.throughput(Throughput::Elements(num_processes as u64));

        group.bench_with_input(
            BenchmarkId::new("client_server_roundtrip", size_name),
            &num_processes,
            |b, &num_processes| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("roundtrip_{}", size_name));

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());
                        let response_size = num_processes;

                        server.set_handler(move |task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, response_size))
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        // Send request
                        let mut client = InterprocessClient::new(config.clone());
                        let task = create_benchmark_task("benchmark", 0);

                        let start_time = Instant::now();
                        let result = client.send_task(task).await.expect("Request failed");
                        let duration = start_time.elapsed();

                        server.stop();

                        black_box((result.success, duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent IPC operations
fn bench_concurrent_ipc(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_ipc");
    group.sample_size(10); // Reduce sample size for concurrent tests

    let rt = Runtime::new().unwrap();

    // Test different concurrency levels
    let concurrency_levels = vec![1, 2, 4, 8];

    for concurrency in concurrency_levels {
        group.bench_with_input(
            BenchmarkId::new("concurrent_requests", concurrency),
            &concurrency,
            |b, &concurrency| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("concurrent_{}", concurrency));
                        let request_counter = Arc::new(AtomicU32::new(0));
                        let handler_counter = Arc::clone(&request_counter);

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());

                        server.set_handler(move |task: DetectionTask| {
                            let counter = Arc::clone(&handler_counter);
                            async move {
                                let _request_num = counter.fetch_add(1, Ordering::SeqCst);
                                Ok(create_benchmark_result(&task.task_id, 10))
                            }
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(200)).await;

                        // Send concurrent requests
                        let mut handles = vec![];
                        let start_time = Instant::now();

                        for i in 0..concurrency {
                            let client_config = config.clone();
                            let handle = tokio::spawn(async move {
                                let mut client = InterprocessClient::new(client_config);
                                let task = create_benchmark_task(&format!("concurrent_{}", i), 0);
                                client.send_task(task).await
                            });
                            handles.push(handle);
                        }

                        // Wait for all requests to complete
                        let mut successful_requests = 0;
                        for handle in handles {
                            if let Ok(Ok(_)) = handle.await {
                                successful_requests += 1;
                            }
                        }

                        let total_duration = start_time.elapsed();
                        server.stop();

                        black_box((successful_requests, total_duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark resilient client performance
fn bench_resilient_client(c: &mut Criterion) {
    let mut group = c.benchmark_group("resilient_client");

    let rt = Runtime::new().unwrap();

    group.bench_function("client_creation", |b| {
        b.iter(|| {
            let (config, _temp_dir) = create_benchmark_config("client_creation");
            let client = ResilientIpcClient::new(config);
            black_box(client)
        });
    });

    group.bench_function("client_stats_retrieval", |b| {
        let (config, _temp_dir) = create_benchmark_config("client_stats");
        let client = ResilientIpcClient::new(config);

        b.iter(|| {
            rt.block_on(async {
                let stats = client.get_stats().await;
                black_box(stats)
            })
        });
    });

    group.bench_function("connection_state_check", |b| {
        let (config, _temp_dir) = create_benchmark_config("connection_state");
        let client = ResilientIpcClient::new(config);

        b.iter(|| {
            rt.block_on(async {
                let state = client.get_connection_state().await;
                black_box(state)
            })
        });
    });

    group.finish();
}

/// Benchmark message throughput
fn bench_message_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_throughput");

    let rt = Runtime::new().unwrap();

    // Test sustained throughput with different message sizes
    let throughput_tests = vec![
        ("small_burst", 100, 10),  // 100 small messages
        ("medium_burst", 50, 100), // 50 medium messages
        ("large_burst", 10, 1000), // 10 large messages
    ];

    for (test_name, num_messages, processes_per_message) in throughput_tests {
        group.throughput(Throughput::Elements(
            (num_messages * processes_per_message) as u64,
        ));

        group.bench_with_input(
            BenchmarkId::new("sustained_throughput", test_name),
            &(num_messages, processes_per_message),
            |b, &(num_messages, processes_per_message)| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("throughput_{}", test_name));

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());
                        let response_size = processes_per_message;

                        server.set_handler(move |task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, response_size))
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(200)).await;

                        // Send burst of messages
                        let mut client = InterprocessClient::new(config.clone());
                        let start_time = Instant::now();

                        for i in 0..num_messages {
                            let task = create_benchmark_task(&format!("throughput_{}", i), 0);
                            let _result = client.send_task(task).await.expect("Request failed");
                        }

                        let duration = start_time.elapsed();
                        server.stop();

                        black_box(duration)
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark latency characteristics
fn bench_latency_characteristics(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency_characteristics");
    group.sample_size(50); // More samples for latency measurements

    let rt = Runtime::new().unwrap();

    // Test latency with different message sizes
    let latency_tests = vec![
        ("minimal", 0), // No processes
        ("small", 1),   // 1 process
        ("medium", 10), // 10 processes
        ("large", 100), // 100 processes
    ];

    for (test_name, num_processes) in latency_tests {
        group.bench_with_input(
            BenchmarkId::new("request_latency", test_name),
            &num_processes,
            |b, &num_processes| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("latency_{}", test_name));

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());
                        let response_size = num_processes;

                        server.set_handler(move |task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, response_size))
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        // Measure single request latency
                        let mut client = InterprocessClient::new(config.clone());
                        let task = create_benchmark_task("latency_test", 0);

                        let start_time = Instant::now();
                        let _result = client.send_task(task).await.expect("Request failed");
                        let latency = start_time.elapsed();

                        server.stop();

                        black_box(latency)
                    })
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_codec_serialization,
    bench_crc32_performance,
    bench_message_framing_performance,
    bench_end_to_end_ipc,
    bench_concurrent_ipc,
    bench_resilient_client,
    bench_message_throughput,
    bench_latency_characteristics
);
criterion_main!(benches);
