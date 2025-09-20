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

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use prost::Message;
use sentinel_lib::ipc::{Crc32Variant, IpcConfig, ResilientIpcClient, TransportType};
use sentinel_lib::proto::{DetectionResult, DetectionTask, ProtoProcessRecord};
use std::hint::black_box;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Benchmark IPC message serialization and deserialization
fn bench_ipc_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_serialization");

    group.bench_function("serialize_detection_task", |b| {
        b.iter(|| {
            let task = DetectionTask::new_enumerate_processes("benchmark_task", None);

            let start = std::time::Instant::now();
            let serialized = prost::Message::encode_to_vec(&task);
            let duration = start.elapsed();

            black_box((serialized.len(), duration))
        });
    });

    group.bench_function("deserialize_detection_task", |b| {
        let task = DetectionTask::new_enumerate_processes("benchmark_task", None);

        let serialized = prost::Message::encode_to_vec(&task);

        b.iter(|| {
            let start = std::time::Instant::now();
            let deserialized = DetectionTask::decode(&serialized[..]).unwrap();
            let duration = start.elapsed();

            black_box((deserialized.task_id, duration))
        });
    });

    group.bench_function("serialize_detection_result", |b| {
        b.iter(|| {
            let result = DetectionResult {
                task_id: "benchmark_task".to_string(),
                success: true,
                error_message: None,
                processes: vec![ProtoProcessRecord {
                    pid: 1234,
                    ppid: Some(1000),
                    name: "test_process".to_string(),
                    executable_path: Some("/usr/bin/test".to_string()),
                    command_line: vec![
                        "test".to_string(),
                        "--arg".to_string(),
                        "value".to_string(),
                    ],
                    start_time: Some(chrono::Utc::now().timestamp()),
                    cpu_usage: Some(25.5),
                    memory_usage: Some(1024 * 1024),
                    executable_hash: Some("abc123def456".to_string()),
                    hash_algorithm: Some("sha256".to_string()),
                    user_id: Some("1000".to_string()),
                    accessible: true,
                    file_exists: true,
                    collection_time: chrono::Utc::now().timestamp_millis(),
                }],
                hash_result: None,
            };

            let start = std::time::Instant::now();
            let serialized = prost::Message::encode_to_vec(&result);
            let duration = start.elapsed();

            black_box((serialized.len(), duration))
        });
    });

    group.bench_function("deserialize_detection_result", |b| {
        let result = DetectionResult {
            task_id: "benchmark_task".to_string(),
            success: true,
            error_message: None,
            processes: vec![ProtoProcessRecord {
                pid: 1234,
                ppid: Some(1000),
                name: "test_process".to_string(),
                executable_path: Some("/usr/bin/test".to_string()),
                command_line: vec!["test".to_string(), "--arg".to_string(), "value".to_string()],
                start_time: Some(chrono::Utc::now().timestamp()),
                cpu_usage: Some(25.5),
                memory_usage: Some(1024 * 1024),
                executable_hash: Some("abc123def456".to_string()),
                hash_algorithm: Some("sha256".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                collection_time: chrono::Utc::now().timestamp_millis(),
            }],
            hash_result: None,
        };

        let serialized = prost::Message::encode_to_vec(&result);

        b.iter(|| {
            let start = std::time::Instant::now();
            let deserialized = DetectionResult::decode(&serialized[..]).unwrap();
            let duration = start.elapsed();

            black_box((deserialized.task_id, duration))
        });
    });

    group.finish();
}

/// Benchmark CRC32 checksum calculation
fn bench_crc32_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("crc32_calculation");

    let test_data = b"This is test data for CRC32 calculation benchmark";

    group.bench_function("crc32_ieee", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(test_data);
            let checksum = hasher.finalize();
            let duration = start.elapsed();

            black_box((checksum, duration))
        });
    });

    group.bench_function("crc32_castagnoli", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(test_data);
            let checksum = hasher.finalize();
            let duration = start.elapsed();

            black_box((checksum, duration))
        });
    });

    // Test with different data sizes
    let data_sizes = vec![100, 1000, 10000, 100_000];

    for data_size in data_sizes {
        group.bench_with_input(
            BenchmarkId::new("crc32_data_size", data_size),
            &data_size,
            |b, &data_size| {
                let test_data_bytes = vec![0_u8; data_size];

                b.iter(|| {
                    let start = std::time::Instant::now();
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&test_data_bytes);
                    let checksum = hasher.finalize();
                    let duration = start.elapsed();

                    black_box((checksum, duration))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark IPC client creation and connection
fn bench_ipc_client_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_client_operations");

    let rt = Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    group.bench_function("create_ipc_client", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = IpcConfig {
                    endpoint_path: socket_path.to_string_lossy().to_string(),
                    max_frame_bytes: 1024 * 1024,
                    max_connections: 4,
                    accept_timeout_ms: 1000,
                    read_timeout_ms: 5000,
                    write_timeout_ms: 5000,
                    transport: TransportType::Interprocess,
                    crc32_variant: Crc32Variant::Ieee,
                };

                let start = std::time::Instant::now();
                let client = ResilientIpcClient::new(config);
                let duration = start.elapsed();

                black_box((client, duration))
            })
        });
    });

    group.bench_function("get_client_stats", |b| {
        rt.block_on(async {
            let config = IpcConfig {
                endpoint_path: socket_path.to_string_lossy().to_string(),
                max_frame_bytes: 1024 * 1024,
                max_connections: 4,
                accept_timeout_ms: 1000,
                read_timeout_ms: 5000,
                write_timeout_ms: 5000,
                transport: TransportType::Interprocess,
                crc32_variant: Crc32Variant::Ieee,
            };

            let client = ResilientIpcClient::new(config);

            b.iter(|| {
                let start = std::time::Instant::now();
                let stats = client.get_stats();
                let duration = start.elapsed();

                black_box((stats, duration))
            });
        });
    });

    group.finish();
}

/// Benchmark message framing and unframing
fn bench_message_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_framing");

    let test_message = b"This is a test message for framing benchmark";

    group.bench_function("frame_message", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();

            // Simulate message framing with length prefix and CRC32
            let length = test_message.len() as u32;
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(test_message);
            let crc = hasher.finalize();

            let mut framed = Vec::new();
            framed.extend_from_slice(&length.to_le_bytes());
            framed.extend_from_slice(&crc.to_le_bytes());
            framed.extend_from_slice(test_message);

            let duration = start.elapsed();

            black_box((framed.len(), duration))
        });
    });

    group.bench_function("unframe_message", |b| {
        // Create framed message
        let length = test_message.len() as u32;
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(test_message);
        let crc = hasher.finalize();

        let mut framed = Vec::new();
        framed.extend_from_slice(&length.to_le_bytes());
        framed.extend_from_slice(&crc.to_le_bytes());
        framed.extend_from_slice(test_message);

        b.iter(|| {
            let start = std::time::Instant::now();

            // Simulate message unframing
            let length_bytes = [framed[0], framed[1], framed[2], framed[3]];
            let crc_bytes = [framed[4], framed[5], framed[6], framed[7]];
            let length = u32::from_le_bytes(length_bytes);
            let crc = u32::from_le_bytes(crc_bytes);
            let message = &framed[8..8 + length as usize];

            // Verify CRC32
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(message);
            let calculated_crc = hasher.finalize();

            let duration = start.elapsed();

            black_box((crc == calculated_crc, duration))
        });
    });

    group.finish();
}

/// Benchmark concurrent IPC operations
fn bench_concurrent_ipc_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_ipc_operations");

    let rt = Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    group.bench_function("concurrent_client_creation", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = IpcConfig {
                    endpoint_path: socket_path.to_string_lossy().to_string(),
                    max_frame_bytes: 1024 * 1024,
                    max_connections: 4,
                    accept_timeout_ms: 1000,
                    read_timeout_ms: 5000,
                    write_timeout_ms: 5000,
                    transport: TransportType::Interprocess,
                    crc32_variant: Crc32Variant::Ieee,
                };

                let start = std::time::Instant::now();

                // Create multiple clients concurrently
                let handles: Vec<_> = (0..10)
                    .map(|_| {
                        let config = config.clone();
                        tokio::spawn(async move { ResilientIpcClient::new(config) })
                    })
                    .collect();

                let clients = futures::future::join_all(handles).await;
                let duration = start.elapsed();

                black_box((clients.len(), duration))
            })
        });
    });

    group.finish();
}

/// Benchmark IPC message throughput
fn bench_ipc_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_throughput");

    // Test with different message sizes
    let message_sizes = vec![100, 1000, 10000, 100_000];

    for message_size in message_sizes {
        group.bench_with_input(
            BenchmarkId::new("message_throughput", message_size),
            &message_size,
            |b, &message_size| {
                b.iter(|| {
                    let start = std::time::Instant::now();

                    // Create test message
                    let test_message = vec![0_u8; message_size];

                    // Simulate full IPC message processing
                    let length = test_message.len() as u32;
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&test_message);
                    let crc = hasher.finalize();

                    // Frame message
                    let mut framed = Vec::new();
                    framed.extend_from_slice(&length.to_le_bytes());
                    framed.extend_from_slice(&crc.to_le_bytes());
                    framed.extend_from_slice(&test_message);

                    // Unframe message
                    let unframed_length_bytes = [framed[0], framed[1], framed[2], framed[3]];
                    let unframed_crc_bytes = [framed[4], framed[5], framed[6], framed[7]];
                    let unframed_length = u32::from_le_bytes(unframed_length_bytes);
                    let unframed_crc = u32::from_le_bytes(unframed_crc_bytes);
                    let unframed_message = &framed[8..8 + unframed_length as usize];

                    // Verify CRC32
                    let mut crc_hasher = crc32fast::Hasher::new();
                    crc_hasher.update(unframed_message);
                    let calculated_crc = crc_hasher.finalize();

                    let duration = start.elapsed();
                    let throughput = message_size as f64 / duration.as_secs_f64();

                    black_box((calculated_crc == unframed_crc, throughput))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_ipc_serialization,
    bench_crc32_calculation,
    bench_ipc_client_operations,
    bench_message_framing,
    bench_concurrent_ipc_operations,
    bench_ipc_throughput
);
criterion_main!(benches);
