//! Shared helper functions for benchmark suites.
//!
//! This module provides common test event constructors used across
//! all benchmark files. Include it with:
//!
//! ```ignore
//! #[path = "bench_helpers.rs"]
//! mod bench_helpers;
//! use bench_helpers::*;
//! ```

#![allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::modulo_arithmetic,
    clippy::uninlined_format_args,
    clippy::str_to_string
)]

use collector_core::ProcessEvent;
use std::time::SystemTime;

/// Create a test process event with the given PID.
pub fn create_test_event(pid: u32) -> ProcessEvent {
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
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: now,
        platform_metadata: None,
    }
}

/// Create a minimal test process event (smaller serialization size).
pub fn create_minimal_event(pid: u32) -> ProcessEvent {
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
        hash_algorithm: None,
        user_id: None,
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Create a large test process event (larger serialization size).
pub fn create_large_event(pid: u32) -> ProcessEvent {
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
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("root".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: now,
        // Note: platform_metadata with serde_json::Value is not directly serializable
        // with postcard (WontImplement error), so we leave it as None for benchmarks
        platform_metadata: None,
    }
}
