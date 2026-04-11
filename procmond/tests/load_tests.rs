//! Load tests for procmond with high process counts and throughput validation.
//!
//! These tests validate stability and resource usage under load,
//! ensuring procmond meets the performance budgets defined in AGENTS.md:
//! - Process enumeration: < 5s for 10,000+ processes
//! - Event publishing throughput: > 1,000 events/sec
//! - Memory usage: < 100 MB resident

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::print_stdout,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::uninlined_format_args,
    clippy::needless_pass_by_value,
    clippy::pattern_type_mismatch,
    clippy::ignored_unit_patterns,
    clippy::missing_const_for_fn,
    clippy::integer_division,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::use_debug,
    clippy::indexing_slicing,
    clippy::let_underscore_must_use,
    clippy::or_fun_call
)]

use collector_core::event::ProcessEvent;
use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};
use procmond::process_collector::{
    ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

/// Creates a test process event with the given PID.
fn create_test_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("load-test-{pid}"),
        executable_path: Some(format!("/usr/bin/load_test_{pid}")),
        command_line: vec!["load-test".to_string(), format!("--id={pid}")],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.0),
        memory_usage: Some(4096),
        executable_hash: None,
        hash_algorithm: None,
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Test real system process collection completes within budget.
///
/// AGENTS.md budget: < 5s for 10,000+ processes.
/// Most systems have 200-1,000 processes, so this is a comfortable margin.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_collection_with_real_system_processes() {
    let collector = SysinfoProcessCollector::new(ProcessCollectionConfig::default());
    let start = std::time::Instant::now();
    let result = collector.collect_processes().await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Collection should succeed");
    let (processes, stats) = result.unwrap();
    let count = processes.len();
    let _ = stats; // Stats available for debugging if needed

    println!("Collected {count} processes in {elapsed:?}");

    // Verify timing is within budget (< 5s per AGENTS.md)
    assert!(
        elapsed < Duration::from_secs(5),
        "Collection took {elapsed:?}, budget is <5s"
    );

    // Should find a reasonable number of processes
    assert!(count > 0, "Should find at least one process");
}

/// Test event publishing throughput meets the > 1,000 events/sec target.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_event_publishing_throughput() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let target_count: u32 = 1000;
    let start = std::time::Instant::now();

    for i in 0..target_count {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    let elapsed = start.elapsed();
    let throughput = f64::from(target_count) / elapsed.as_secs_f64();

    println!("Published {target_count} events in {elapsed:?} ({throughput:.0} events/sec)");

    assert!(
        throughput > 1000.0,
        "Throughput {throughput:.0}/s below 1000/s target"
    );
}

/// Test that repeated collection cycles don't cause unbounded memory growth.
///
/// AGENTS.md budget: < 100 MB resident.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_memory_usage_within_budget() {
    let collector = SysinfoProcessCollector::new(ProcessCollectionConfig::default());

    // Warm up
    let _ = collector.collect_processes().await;

    let initial_rss = get_current_rss_kb();

    // Run 10 collection cycles
    for _ in 0..10 {
        let _ = collector.collect_processes().await;
    }

    let final_rss = get_current_rss_kb();

    if initial_rss > 0 && final_rss > 0 {
        let growth_mb = (final_rss.saturating_sub(initial_rss)) as f64 / 1024.0;
        println!(
            "RSS growth over 10 cycles: {growth_mb:.1}MB (initial: {initial_rss}KB, final: {final_rss}KB)"
        );

        // Should stay well under 100MB budget
        assert!(
            final_rss < 100 * 1024,
            "RSS {final_rss}KB exceeds 100MB budget"
        );
    } else {
        println!("RSS measurement not available on this platform, skipping assertion");
    }
}

/// Test WAL write throughput meets > 500 writes/sec target.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_wal_write_throughput() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let target_count: u32 = 500;
    let start = std::time::Instant::now();

    for i in 0..target_count {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("WAL write should succeed");
    }

    let elapsed = start.elapsed();
    let throughput = f64::from(target_count) / elapsed.as_secs_f64();

    println!("WAL wrote {target_count} events in {elapsed:?} ({throughput:.0} writes/sec)");

    assert!(
        throughput > 500.0,
        "WAL throughput {throughput:.0}/s below 500/s target"
    );
}

/// Test sustained load with mixed event types doesn't degrade over time.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_sustained_mixed_event_load() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let event_types = [
        ProcessEventType::Start,
        ProcessEventType::Stop,
        ProcessEventType::Modify,
    ];

    let batch_size: u32 = 200;
    let batch_count: u32 = 5;
    let mut batch_times = Vec::with_capacity(batch_count as usize);

    for batch in 0..batch_count {
        let batch_start = std::time::Instant::now();
        let base_pid = batch * batch_size;

        for i in 0..batch_size {
            let event = create_test_event(base_pid + i);
            let event_type = event_types[(i % 3) as usize];
            connector
                .publish(event, event_type)
                .await
                .expect("Publish should succeed");
        }

        let batch_elapsed = batch_start.elapsed();
        batch_times.push(batch_elapsed);
        println!("Batch {batch}: {batch_size} events in {batch_elapsed:?}");
    }

    // Verify no significant degradation: last batch shouldn't be >3x slower than first
    let first_batch = batch_times[0];
    let last_batch = batch_times[batch_times.len() - 1];

    if first_batch > Duration::from_millis(1) {
        let ratio = last_batch.as_secs_f64() / first_batch.as_secs_f64();
        println!("Degradation ratio (last/first): {ratio:.2}x");
        assert!(
            ratio < 3.0,
            "Performance degradation {ratio:.2}x exceeds 3x threshold"
        );
    }

    let total_events = batch_size * batch_count;
    println!("Sustained load test: {total_events} events across {batch_count} batches");
}

/// Test synthetic 10,000+ process event publishing.
///
/// AGENTS.md budget: support 10,000+ processes without crashes or memory leaks.
/// This test creates synthetic events (not real processes) to validate the
/// publish pipeline handles high event counts without degradation.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_synthetic_10k_process_events() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let target_count: u32 = 10_000;
    let start = std::time::Instant::now();

    let event_types = [
        ProcessEventType::Start,
        ProcessEventType::Modify,
        ProcessEventType::Stop,
    ];

    for i in 0..target_count {
        let event = create_test_event(i);
        let event_type = event_types[(i % 3) as usize];
        connector
            .publish(event, event_type)
            .await
            .expect("Publish should succeed for 10K events");
    }

    let elapsed = start.elapsed();
    let throughput = f64::from(target_count) / elapsed.as_secs_f64();

    println!(
        "Published {target_count} synthetic events in {elapsed:?} ({throughput:.0} events/sec)"
    );

    // Should complete within 5s budget (AGENTS.md: <5s for 10,000+ processes)
    assert!(
        elapsed < Duration::from_secs(5),
        "10K events took {elapsed:?}, budget is <5s"
    );

    // Throughput should still exceed 1,000 events/sec
    assert!(
        throughput > 1000.0,
        "Throughput {throughput:.0}/s below 1000/s at 10K scale"
    );
}

/// Test backpressure activation timing meets <1s target.
///
/// AGENTS.md budget: Backpressure activation <1s to adjust interval.
/// This test fills the buffer to trigger backpressure and measures the
/// time until the signal is received.
#[tokio::test]
#[ignore = "load test — run via `cargo test --test load_tests -- --ignored`"]
async fn test_backpressure_activation_timing() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut bp_rx = connector
        .take_backpressure_receiver()
        .expect("Should have backpressure receiver");

    let start = std::time::Instant::now();
    let mut backpressure_detected = false;
    let mut events_published = 0_u32;

    // Publish events until backpressure triggers or we exceed a reasonable count
    for i in 0..50_000_u32 {
        let event = create_test_event(i);
        match connector.publish(event, ProcessEventType::Start).await {
            Ok(_seq) => {
                events_published += 1;
                // Check for backpressure signal (non-blocking)
                if bp_rx.try_recv().is_ok() {
                    backpressure_detected = true;
                    break;
                }
            }
            Err(_e) => {
                // Buffer overflow also indicates backpressure activation
                backpressure_detected = true;
                break;
            }
        }
    }

    let elapsed = start.elapsed();

    println!(
        "Backpressure after {events_published} events in {elapsed:?} (detected: {backpressure_detected})"
    );

    if backpressure_detected {
        // Backpressure should activate within 1 second
        assert!(
            elapsed < Duration::from_secs(1),
            "Backpressure took {elapsed:?}, budget is <1s"
        );
    } else {
        println!(
            "Backpressure not triggered after {events_published} events - buffer capacity may be very large"
        );
    }
}

/// Get current RSS in KB (Linux only, returns 0 on other platforms).
fn get_current_rss_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0)
    }
    #[cfg(target_os = "macos")]
    {
        // Use sysinfo to get current process RSS on macOS
        use sysinfo::System;
        let pid = sysinfo::get_current_pid().unwrap_or(sysinfo::Pid::from(0));
        let mut sys = System::new_all();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).map_or(0, |p| p.memory() / 1024)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}
