//! Lifecycle Tracking Integration Tests.
//!
//! These tests verify process lifecycle detection capabilities:
//! - Start detection: New processes detected
//! - Stop detection: Terminated processes detected
//! - Modification detection: Process changes detected
//!
//! # Test Strategy
//!
//! Tests use a combination of:
//! 1. Real subprocess spawning/termination for realistic scenarios
//! 2. Mock collection results for deterministic edge case testing
//! 3. Multi-cycle tracking to verify state transitions
//!
//! # Running Tests
//!
//! ```bash
//! cargo test --package procmond --test lifecycle_tracking_tests
//! ```

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::inefficient_to_string,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    clippy::pattern_type_mismatch,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::needless_collect,
    clippy::as_conversions,
    clippy::print_stdout,
    clippy::use_debug,
    unused_imports,
    dead_code
)]

use collector_core::ProcessEvent;
use procmond::lifecycle::{
    LifecycleTrackingConfig, ProcessLifecycleEvent, ProcessLifecycleTracker, ProcessSnapshot,
    SuspiciousEventSeverity,
};
use procmond::process_collector::{
    ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::collections::HashSet;
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a test ProcessEvent with the specified parameters.
fn create_test_process_event(
    pid: u32,
    name: &str,
    executable_path: Option<&str>,
    command_line: Vec<&str>,
) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: name.to_string(),
        executable_path: executable_path.map(|s| s.to_string()),
        command_line: command_line.iter().map(|s| s.to_string()).collect(),
        start_time: Some(SystemTime::now() - Duration::from_secs(60)),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("abc123".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Creates a process event with a specific start time for testing.
fn create_process_event_with_start_time(
    pid: u32,
    name: &str,
    start_time: SystemTime,
) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: name.to_string(),
        executable_path: Some(format!("/usr/bin/{name}")),
        command_line: vec![name.to_string()],
        start_time: Some(start_time),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("abc123".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Spawns a sleep process that runs for a specified duration.
/// Returns the child process handle.
#[cfg(unix)]
fn spawn_sleep_process(duration_secs: u64) -> Child {
    Command::new("sleep")
        .arg(duration_secs.to_string())
        .spawn()
        .expect("Failed to spawn sleep process")
}

/// Spawns a sleep process on Windows.
/// Uses PowerShell Start-Sleep to avoid conflict with Unix timeout command
/// that may be present in PATH from Git Bash or similar tools.
#[cfg(windows)]
fn spawn_sleep_process(duration_secs: u64) -> Child {
    Command::new("powershell")
        .args(["-Command", &format!("Start-Sleep -Seconds {duration_secs}")])
        .spawn()
        .expect("Failed to spawn sleep process")
}

// ============================================================================
// Start Detection Tests
// ============================================================================

/// Test: New process appears in the next collection cycle.
///
/// Verifies that when a new process is spawned between collection cycles,
/// a Start event is generated with the correct process metadata.
#[test]
fn test_start_detection_new_process_appears() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: baseline processes
    let initial_processes = vec![
        create_test_process_event(100, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(200, "bash", Some("/bin/bash"), vec!["bash"]),
    ];

    let events = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");
    assert!(
        events.is_empty(),
        "First enumeration should not generate events"
    );
    assert_eq!(tracker.tracked_process_count(), 2);

    // Second cycle: new process added
    let updated_processes = vec![
        create_test_process_event(100, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(200, "bash", Some("/bin/bash"), vec!["bash"]),
        create_test_process_event(300, "vim", Some("/usr/bin/vim"), vec!["vim", "file.txt"]),
    ];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .expect("Second update should succeed");

    // Verify start event generated
    let start_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .collect();

    assert_eq!(
        start_events.len(),
        1,
        "Should detect exactly one process start"
    );

    // Verify start event details
    match start_events[0] {
        ProcessLifecycleEvent::Start {
            process,
            detected_at,
        } => {
            assert_eq!(process.pid, 300, "Start event should have correct PID");
            assert_eq!(process.name, "vim", "Start event should have correct name");
            assert_eq!(
                process.executable_path,
                Some("/usr/bin/vim".to_string()),
                "Start event should have correct executable path"
            );
            assert!(
                *detected_at <= SystemTime::now(),
                "Detection time should not be in the future"
            );
        }
        _ => panic!("Expected Start event"),
    }
}

/// Test: ProcessEvent with Start type is generated correctly.
///
/// Verifies that the Start event contains all expected metadata fields
/// including command line arguments and resource usage.
#[test]
fn test_start_detection_event_has_correct_metadata() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: empty
    let initial_processes = vec![];
    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");

    // Second cycle: new process with full metadata
    let new_process = ProcessEvent {
        pid: 1234,
        ppid: Some(100),
        name: "test_process".to_string(),
        executable_path: Some("/usr/local/bin/test_process".to_string()),
        command_line: vec![
            "test_process".to_string(),
            "--verbose".to_string(),
            "--config=/etc/test.conf".to_string(),
        ],
        start_time: Some(SystemTime::now() - Duration::from_secs(10)),
        cpu_usage: Some(5.5),
        memory_usage: Some(50 * 1024 * 1024), // 50 MB
        executable_hash: Some("sha256:abc123def456".to_string()),
        user_id: Some("1001".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: Some(serde_json::json!({"sandbox": true})),
    };

    let events = tracker
        .update_and_detect_changes(vec![new_process])
        .expect("Second update should succeed");

    assert_eq!(events.len(), 1, "Should generate exactly one event");

    match &events[0] {
        ProcessLifecycleEvent::Start { process, .. } => {
            assert_eq!(process.pid, 1234);
            assert_eq!(process.ppid, Some(100));
            assert_eq!(process.name, "test_process");
            assert_eq!(
                process.executable_path,
                Some("/usr/local/bin/test_process".to_string())
            );
            assert_eq!(process.command_line.len(), 3);
            assert_eq!(process.command_line[0], "test_process");
            assert_eq!(process.command_line[1], "--verbose");
            assert!(process.start_time.is_some());
            assert_eq!(process.cpu_usage, Some(5.5));
            assert_eq!(process.memory_usage, Some(50 * 1024 * 1024));
            assert_eq!(
                process.executable_hash,
                Some("sha256:abc123def456".to_string())
            );
            assert_eq!(process.user_id, Some("1001".to_string()));
            assert!(process.accessible);
            assert!(process.file_exists);
            assert!(process.platform_metadata.is_some());
        }
        _ => panic!("Expected Start event"),
    }
}

/// Test: Multiple new processes detected in single cycle.
///
/// Verifies that multiple processes starting between cycles are all detected.
#[test]
fn test_start_detection_multiple_new_processes() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: one process
    let initial_processes = vec![create_test_process_event(
        1,
        "init",
        Some("/sbin/init"),
        vec!["init"],
    )];
    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");

    // Second cycle: three new processes
    let updated_processes = vec![
        create_test_process_event(1, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
        create_test_process_event(200, "process_b", Some("/bin/b"), vec!["b"]),
        create_test_process_event(300, "process_c", Some("/bin/c"), vec!["c"]),
    ];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .expect("Second update should succeed");

    let start_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .collect();

    assert_eq!(start_events.len(), 3, "Should detect three process starts");

    // Verify all three PIDs are detected
    let started_pids: HashSet<u32> = start_events
        .iter()
        .filter_map(|e| {
            if let ProcessLifecycleEvent::Start { process, .. } = e {
                Some(process.pid)
            } else {
                None
            }
        })
        .collect();

    assert!(started_pids.contains(&100));
    assert!(started_pids.contains(&200));
    assert!(started_pids.contains(&300));
}

/// Test: Start detection with real subprocess spawning.
///
/// Spawns a real subprocess and verifies it can be detected through the collector.
#[tokio::test]
async fn test_start_detection_with_real_subprocess() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: true,
        max_processes: 0, // Unlimited
    };
    let collector = SysinfoProcessCollector::new(config);

    // Get initial process list
    let (initial_events, _) = collector
        .collect_processes()
        .await
        .expect("Initial collection should succeed");

    let initial_pids: HashSet<u32> = initial_events.iter().map(|e| e.pid).collect();

    // Spawn a new process
    let child = spawn_sleep_process(10);
    let child_pid = child.id();

    // Give the process time to start
    sleep(Duration::from_millis(100)).await;

    // Collect again
    let (current_events, _) = collector
        .collect_processes()
        .await
        .expect("Second collection should succeed");

    let current_pids: HashSet<u32> = current_events.iter().map(|e| e.pid).collect();

    // Verify the spawned process is detected
    assert!(
        current_pids.contains(&child_pid),
        "Spawned process (PID {child_pid}) should be detected in collection"
    );

    // Verify it's a new PID (wasn't in initial)
    let new_pids: HashSet<u32> = current_pids.difference(&initial_pids).copied().collect();
    assert!(
        new_pids.contains(&child_pid),
        "Spawned process should be in the set of new PIDs"
    );

    // Cleanup
    drop(child);
}

// ============================================================================
// Stop Detection Tests
// ============================================================================

/// Test: Previously running process terminates.
///
/// Verifies that when a process terminates between collection cycles,
/// a Stop event is generated with the correct process metadata.
#[test]
fn test_stop_detection_process_terminates() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: three processes
    let initial_processes = vec![
        create_test_process_event(100, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(200, "bash", Some("/bin/bash"), vec!["bash"]),
        create_test_process_event(300, "vim", Some("/usr/bin/vim"), vec!["vim"]),
    ];

    let events = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");
    assert!(events.is_empty());
    assert_eq!(tracker.tracked_process_count(), 3);

    // Second cycle: vim process terminated
    let updated_processes = vec![
        create_test_process_event(100, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(200, "bash", Some("/bin/bash"), vec!["bash"]),
        // vim (PID 300) is gone
    ];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .expect("Second update should succeed");

    let stop_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .collect();

    assert_eq!(
        stop_events.len(),
        1,
        "Should detect exactly one process stop"
    );

    match stop_events[0] {
        ProcessLifecycleEvent::Stop {
            process,
            detected_at,
            runtime_duration,
        } => {
            assert_eq!(process.pid, 300, "Stop event should have correct PID");
            assert_eq!(process.name, "vim", "Stop event should have correct name");
            assert!(
                *detected_at <= SystemTime::now(),
                "Detection time should not be in the future"
            );
            assert!(
                runtime_duration.is_some(),
                "Runtime duration should be calculated"
            );
        }
        _ => panic!("Expected Stop event"),
    }

    assert_eq!(
        tracker.tracked_process_count(),
        2,
        "Tracker should now have 2 processes"
    );
}

/// Test: ProcessEvent with Stop type has correct runtime duration.
///
/// Verifies that the Stop event correctly calculates the process runtime.
#[test]
fn test_stop_detection_runtime_duration_calculated() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Create process with known start time
    let process_start_time = SystemTime::now() - Duration::from_secs(120); // Started 2 minutes ago
    let initial_processes = vec![create_process_event_with_start_time(
        500,
        "long_running_process",
        process_start_time,
    )];

    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");

    // Process terminates
    let events = tracker
        .update_and_detect_changes(vec![])
        .expect("Second update should succeed");

    assert_eq!(events.len(), 1);

    match &events[0] {
        ProcessLifecycleEvent::Stop {
            runtime_duration, ..
        } => {
            let duration = runtime_duration.expect("Runtime duration should be present");
            // Should be approximately 120 seconds (with some tolerance)
            assert!(
                duration >= Duration::from_secs(119) && duration <= Duration::from_secs(125),
                "Runtime duration should be approximately 120 seconds, got {duration:?}"
            );
        }
        _ => panic!("Expected Stop event"),
    }
}

/// Test: Multiple processes stop simultaneously.
///
/// Verifies that multiple processes terminating between cycles are all detected.
#[test]
fn test_stop_detection_multiple_processes() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: five processes
    let initial_processes = vec![
        create_test_process_event(1, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
        create_test_process_event(200, "process_b", Some("/bin/b"), vec!["b"]),
        create_test_process_event(300, "process_c", Some("/bin/c"), vec!["c"]),
        create_test_process_event(400, "process_d", Some("/bin/d"), vec!["d"]),
    ];

    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .expect("First update should succeed");

    // Second cycle: processes B, C, and D have stopped
    let updated_processes = vec![
        create_test_process_event(1, "init", Some("/sbin/init"), vec!["init"]),
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
    ];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .expect("Second update should succeed");

    let stop_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .collect();

    assert_eq!(stop_events.len(), 3, "Should detect three process stops");

    // Verify all three PIDs are detected as stopped
    let stopped_pids: HashSet<u32> = stop_events
        .iter()
        .filter_map(|e| {
            if let ProcessLifecycleEvent::Stop { process, .. } = e {
                Some(process.pid)
            } else {
                None
            }
        })
        .collect();

    assert!(stopped_pids.contains(&200));
    assert!(stopped_pids.contains(&300));
    assert!(stopped_pids.contains(&400));
}

/// Test: Stopped process removed from active tracking.
///
/// Verifies that after a process stops, it is no longer tracked.
#[test]
fn test_stop_detection_removes_from_tracking() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let initial_processes = vec![
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
        create_test_process_event(200, "process_b", Some("/bin/b"), vec!["b"]),
    ];
    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();
    assert_eq!(tracker.tracked_process_count(), 2);

    // Second cycle: process_b stops
    let updated = vec![create_test_process_event(
        100,
        "process_a",
        Some("/bin/a"),
        vec!["a"],
    )];
    let events = tracker.update_and_detect_changes(updated).unwrap();

    // Verify stop detected
    assert!(events.iter().any(|e| matches!(
        e,
        ProcessLifecycleEvent::Stop { process, .. } if process.pid == 200
    )));

    // Verify tracking count updated
    assert_eq!(tracker.tracked_process_count(), 1);

    // Third cycle: same state
    let same = vec![create_test_process_event(
        100,
        "process_a",
        Some("/bin/a"),
        vec!["a"],
    )];
    let events = tracker.update_and_detect_changes(same).unwrap();

    // No new stop events for already-stopped process
    let stop_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .collect();
    assert!(
        stop_events.is_empty(),
        "No stop events for already-stopped process"
    );
}

/// Test: Stop detection with real subprocess termination.
///
/// Spawns a real subprocess, terminates it, and verifies detection.
#[tokio::test]
async fn test_stop_detection_with_real_subprocess() {
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: true,
        max_processes: 0,
    };
    let collector = SysinfoProcessCollector::new(config);

    // Spawn a process
    let mut child = spawn_sleep_process(30);
    let child_pid = child.id();

    // Give the process time to start
    sleep(Duration::from_millis(100)).await;

    // Collect to verify it exists
    let (events_before, _) = collector
        .collect_processes()
        .await
        .expect("Collection should succeed");

    let pids_before: HashSet<u32> = events_before.iter().map(|e| e.pid).collect();
    assert!(
        pids_before.contains(&child_pid),
        "Process should exist before termination"
    );

    // Kill the process
    child.kill().expect("Failed to kill child process");
    child.wait().expect("Failed to reap child process");

    // Give system time to clean up
    sleep(Duration::from_millis(200)).await;

    // Collect again
    let (events_after, _) = collector
        .collect_processes()
        .await
        .expect("Collection should succeed");

    let pids_after: HashSet<u32> = events_after.iter().map(|e| e.pid).collect();
    assert!(
        !pids_after.contains(&child_pid),
        "Process should not exist after termination"
    );
}

// ============================================================================
// Modification Detection Tests
// ============================================================================

/// Test: Process command line modification detected.
///
/// Verifies that when a process's command line changes, a Modified event is generated.
#[test]
fn test_modification_detection_command_line_changed() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: process with initial command line
    let initial_processes = vec![create_test_process_event(
        500,
        "server",
        Some("/usr/bin/server"),
        vec!["server", "--port=8080"],
    )];
    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();

    // Second cycle: command line has changed
    let updated_processes = vec![create_test_process_event(
        500,
        "server",
        Some("/usr/bin/server"),
        vec!["server", "--port=8080", "--verbose", "--debug"],
    )];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .unwrap();

    let modified_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();

    assert_eq!(
        modified_events.len(),
        1,
        "Should detect command line modification"
    );

    match modified_events[0] {
        ProcessLifecycleEvent::Modified {
            previous,
            current,
            modified_fields,
            ..
        } => {
            assert_eq!(previous.pid, 500);
            assert_eq!(current.pid, 500);
            assert!(modified_fields.contains(&"command_line".to_string()));
            assert_eq!(previous.command_line, vec!["server", "--port=8080"]);
            assert_eq!(
                current.command_line,
                vec!["server", "--port=8080", "--verbose", "--debug"]
            );
        }
        _ => panic!("Expected Modified event"),
    }
}

/// Test: Process executable path modification detected.
///
/// Verifies that when a process's executable path changes (suspicious behavior),
/// a Modified event is generated.
#[test]
fn test_modification_detection_executable_path_changed() {
    let config = LifecycleTrackingConfig {
        track_executable_changes: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let initial_processes = vec![create_test_process_event(
        600,
        "app",
        Some("/usr/bin/app"),
        vec!["app"],
    )];
    let _ = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();

    // Second cycle: executable path changed (suspicious!)
    let updated_processes = vec![create_test_process_event(
        600,
        "app",
        Some("/tmp/malicious/app"),
        vec!["app"],
    )];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .unwrap();

    // Should detect either Modified or Suspicious event
    let has_modification = events.iter().any(|e| {
        matches!(
            e,
            ProcessLifecycleEvent::Modified { modified_fields, .. }
            if modified_fields.contains(&"executable_path".to_string())
        ) || matches!(e, ProcessLifecycleEvent::Suspicious { .. })
    });

    assert!(
        has_modification,
        "Should detect executable path modification"
    );
}

/// Test: Memory usage change above threshold generates Modified event.
#[test]
fn test_modification_detection_memory_change() {
    let config = LifecycleTrackingConfig {
        track_memory_changes: true,
        memory_change_threshold: 20.0, // 20% threshold
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: process with 100MB memory
    let mut initial_process = create_test_process_event(
        700,
        "memory_hog",
        Some("/usr/bin/memory_hog"),
        vec!["memory_hog"],
    );
    initial_process.memory_usage = Some(100 * 1024 * 1024); // 100 MB
    let _ = tracker
        .update_and_detect_changes(vec![initial_process])
        .unwrap();

    // Second cycle: memory increased to 150MB (50% increase, above 20% threshold)
    let mut updated_process = create_test_process_event(
        700,
        "memory_hog",
        Some("/usr/bin/memory_hog"),
        vec!["memory_hog"],
    );
    updated_process.memory_usage = Some(150 * 1024 * 1024); // 150 MB

    let events = tracker
        .update_and_detect_changes(vec![updated_process])
        .unwrap();

    let modified_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();

    assert_eq!(
        modified_events.len(),
        1,
        "Should detect memory change above threshold"
    );

    match modified_events[0] {
        ProcessLifecycleEvent::Modified {
            modified_fields, ..
        } => {
            assert!(modified_fields.contains(&"memory_usage".to_string()));
        }
        _ => panic!("Expected Modified event"),
    }
}

/// Test: Minor changes below threshold do not generate events.
#[test]
fn test_modification_detection_below_threshold_ignored() {
    let config = LifecycleTrackingConfig {
        track_memory_changes: true,
        memory_change_threshold: 50.0, // 50% threshold
        track_command_line_changes: false,
        track_executable_changes: false,
        track_cpu_changes: false,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let mut initial = create_test_process_event(800, "stable", Some("/bin/stable"), vec!["stable"]);
    initial.memory_usage = Some(100 * 1024 * 1024); // 100 MB
    let _ = tracker.update_and_detect_changes(vec![initial]).unwrap();

    // Second cycle: 10% memory increase (below 50% threshold)
    let mut updated = create_test_process_event(800, "stable", Some("/bin/stable"), vec!["stable"]);
    updated.memory_usage = Some(110 * 1024 * 1024); // 110 MB (10% increase)

    let events = tracker.update_and_detect_changes(vec![updated]).unwrap();

    let modified_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();

    assert!(
        modified_events.is_empty(),
        "Should not generate event for change below threshold"
    );
}

/// Test: Multiple fields modified in single cycle.
#[test]
fn test_modification_detection_multiple_fields() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: true,
        track_memory_changes: true,
        memory_change_threshold: 20.0,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let mut initial = create_test_process_event(
        900,
        "multi_change",
        Some("/bin/multi"),
        vec!["multi", "--mode=normal"],
    );
    initial.memory_usage = Some(50 * 1024 * 1024);
    let _ = tracker.update_and_detect_changes(vec![initial]).unwrap();

    // Second cycle: both command line and memory changed
    let mut updated = create_test_process_event(
        900,
        "multi_change",
        Some("/bin/multi"),
        vec!["multi", "--mode=turbo", "--extra-flag"],
    );
    updated.memory_usage = Some(100 * 1024 * 1024); // 100% increase

    let events = tracker.update_and_detect_changes(vec![updated]).unwrap();

    let modified_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();

    assert_eq!(
        modified_events.len(),
        1,
        "Should generate one Modified event"
    );

    match modified_events[0] {
        ProcessLifecycleEvent::Modified {
            modified_fields, ..
        } => {
            assert!(
                modified_fields.contains(&"command_line".to_string()),
                "Should include command_line in modified fields"
            );
            assert!(
                modified_fields.contains(&"memory_usage".to_string()),
                "Should include memory_usage in modified fields"
            );
        }
        _ => panic!("Expected Modified event"),
    }
}

/// Test: Modification tracking disabled does not generate events.
#[test]
fn test_modification_detection_disabled() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: false,
        track_executable_changes: false,
        track_memory_changes: false,
        track_cpu_changes: false,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let initial = create_test_process_event(
        1000,
        "no_track",
        Some("/bin/no_track"),
        vec!["no_track", "--arg1"],
    );
    let _ = tracker.update_and_detect_changes(vec![initial]).unwrap();

    // Second cycle: many changes
    let updated = create_test_process_event(
        1000,
        "no_track",
        Some("/opt/no_track"),
        vec!["no_track", "--arg1", "--arg2", "--arg3"],
    );

    let events = tracker.update_and_detect_changes(vec![updated]).unwrap();

    let modified_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();

    assert!(
        modified_events.is_empty(),
        "Should not generate Modified events when tracking is disabled"
    );
}

// ============================================================================
// Suspicious Event Detection Tests
// ============================================================================

/// Test: PID reuse detection generates Suspicious event.
///
/// When a PID is reused by a completely different process, this is suspicious.
#[test]
fn test_suspicious_event_pid_reuse_detected() {
    let config = LifecycleTrackingConfig {
        detect_pid_reuse: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: legitimate process
    let initial = create_test_process_event(
        1100,
        "legitimate_app",
        Some("/usr/bin/legitimate_app"),
        vec!["legitimate_app"],
    );
    let _ = tracker.update_and_detect_changes(vec![initial]).unwrap();

    // Second cycle: same PID but completely different process
    let suspicious = create_test_process_event(
        1100,
        "suspicious_app",
        Some("/tmp/.hidden/suspicious"),
        vec!["suspicious", "--stealth"],
    );

    let events = tracker.update_and_detect_changes(vec![suspicious]).unwrap();

    let suspicious_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Suspicious { .. }))
        .collect();

    assert_eq!(
        suspicious_events.len(),
        1,
        "Should detect suspicious PID reuse"
    );

    match suspicious_events[0] {
        ProcessLifecycleEvent::Suspicious {
            process,
            reason,
            severity,
            ..
        } => {
            assert_eq!(process.pid, 1100);
            assert!(reason.contains("PID reuse") || reason.contains("reuse"));
            assert_eq!(
                *severity,
                SuspiciousEventSeverity::High,
                "PID reuse with executable change should be high severity"
            );
        }
        _ => panic!("Expected Suspicious event"),
    }
}

/// Test: PID reuse detection disabled does not generate events.
#[test]
fn test_suspicious_event_pid_reuse_disabled() {
    let config = LifecycleTrackingConfig {
        detect_pid_reuse: false,
        track_command_line_changes: false,
        track_executable_changes: false,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle
    let initial =
        create_test_process_event(1200, "app_v1", Some("/usr/bin/app_v1"), vec!["app_v1"]);
    let _ = tracker.update_and_detect_changes(vec![initial]).unwrap();

    // Second cycle: PID reused (but detection disabled)
    let reused = create_test_process_event(1200, "app_v2", Some("/usr/bin/app_v2"), vec!["app_v2"]);

    let events = tracker.update_and_detect_changes(vec![reused]).unwrap();

    let suspicious_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Suspicious { .. }))
        .collect();

    assert!(
        suspicious_events.is_empty(),
        "Should not generate Suspicious events when PID reuse detection is disabled"
    );
}

// ============================================================================
// Combined Lifecycle Tests
// ============================================================================

/// Test: Mixed start, stop, and modify events in single cycle.
///
/// Verifies that the tracker can correctly detect multiple event types simultaneously.
#[test]
fn test_combined_lifecycle_events() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // First cycle: three processes
    let initial = vec![
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
        create_test_process_event(200, "process_b", Some("/bin/b"), vec!["b"]),
        create_test_process_event(300, "process_c", Some("/bin/c"), vec!["c"]),
    ];
    let _ = tracker.update_and_detect_changes(initial).unwrap();

    // Second cycle:
    // - process_a: still running (no change)
    // - process_b: stopped (removed)
    // - process_c: modified (command line changed)
    // - process_d: started (new)
    let updated = vec![
        create_test_process_event(100, "process_a", Some("/bin/a"), vec!["a"]),
        // process_b (200) is gone
        create_test_process_event(300, "process_c", Some("/bin/c"), vec!["c", "--modified"]),
        create_test_process_event(400, "process_d", Some("/bin/d"), vec!["d"]),
    ];

    let events = tracker.update_and_detect_changes(updated).unwrap();

    // Count event types
    let start_count = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .count();
    let stop_count = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .count();
    let modified_count = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .count();

    assert_eq!(start_count, 1, "Should detect one start event (process_d)");
    assert_eq!(stop_count, 1, "Should detect one stop event (process_b)");
    assert_eq!(
        modified_count, 1,
        "Should detect one modified event (process_c)"
    );

    // Verify specific events
    assert!(events.iter().any(|e| matches!(
        e,
        ProcessLifecycleEvent::Start { process, .. } if process.pid == 400
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ProcessLifecycleEvent::Stop { process, .. } if process.pid == 200
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ProcessLifecycleEvent::Modified { current, .. } if current.pid == 300
    )));
}

/// Test: Statistics tracking across multiple cycles.
#[test]
fn test_lifecycle_statistics_tracking() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Cycle 1: initial
    let _ = tracker
        .update_and_detect_changes(vec![
            create_test_process_event(1, "p1", Some("/bin/p1"), vec!["p1"]),
            create_test_process_event(2, "p2", Some("/bin/p2"), vec!["p2"]),
        ])
        .unwrap();

    // Cycle 2: one stop, one start
    let _ = tracker
        .update_and_detect_changes(vec![
            create_test_process_event(1, "p1", Some("/bin/p1"), vec!["p1"]),
            create_test_process_event(3, "p3", Some("/bin/p3"), vec!["p3"]),
        ])
        .unwrap();

    // Cycle 3: one modification
    let _ = tracker
        .update_and_detect_changes(vec![
            create_test_process_event(1, "p1", Some("/bin/p1"), vec!["p1", "--modified"]),
            create_test_process_event(3, "p3", Some("/bin/p3"), vec!["p3"]),
        ])
        .unwrap();

    let stats = tracker.stats();
    assert_eq!(stats.total_updates, 3);
    assert_eq!(
        stats.start_events, 1,
        "Should have recorded one start event"
    );
    assert_eq!(stats.stop_events, 1, "Should have recorded one stop event");
    assert_eq!(
        stats.modification_events, 1,
        "Should have recorded one modification event"
    );
    assert!(stats.avg_processes_tracked > 0.0);
}

/// Test: ProcessSnapshot conversion preserves all fields.
#[test]
fn test_snapshot_conversion_roundtrip() {
    let original = ProcessEvent {
        pid: 9999,
        ppid: Some(1234),
        name: "test_process".to_string(),
        executable_path: Some("/usr/local/bin/test".to_string()),
        command_line: vec![
            "test".to_string(),
            "--arg1".to_string(),
            "--arg2=value".to_string(),
        ],
        start_time: Some(SystemTime::now() - Duration::from_secs(3600)),
        cpu_usage: Some(25.5),
        memory_usage: Some(256 * 1024 * 1024),
        executable_hash: Some("sha256:fedcba987654321".to_string()),
        user_id: Some("user123".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: Some(serde_json::json!({"key": "value"})),
    };

    // Convert to snapshot
    let snapshot = ProcessSnapshot::from(original.clone());

    // Convert back to event
    let roundtrip = ProcessEvent::from(snapshot);

    // Verify all fields preserved
    assert_eq!(original.pid, roundtrip.pid);
    assert_eq!(original.ppid, roundtrip.ppid);
    assert_eq!(original.name, roundtrip.name);
    assert_eq!(original.executable_path, roundtrip.executable_path);
    assert_eq!(original.command_line, roundtrip.command_line);
    assert_eq!(original.start_time, roundtrip.start_time);
    assert_eq!(original.cpu_usage, roundtrip.cpu_usage);
    assert_eq!(original.memory_usage, roundtrip.memory_usage);
    assert_eq!(original.executable_hash, roundtrip.executable_hash);
    assert_eq!(original.user_id, roundtrip.user_id);
    assert_eq!(original.accessible, roundtrip.accessible);
    assert_eq!(original.file_exists, roundtrip.file_exists);
    assert_eq!(original.platform_metadata, roundtrip.platform_metadata);
}

/// Test: Tracker handles empty process lists gracefully.
#[test]
fn test_lifecycle_handles_empty_lists() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Start with empty
    let events1 = tracker.update_and_detect_changes(vec![]).unwrap();
    assert!(events1.is_empty());
    assert_eq!(tracker.tracked_process_count(), 0);

    // Add some processes
    let events2 = tracker
        .update_and_detect_changes(vec![create_test_process_event(
            1,
            "p1",
            Some("/bin/p1"),
            vec!["p1"],
        )])
        .unwrap();
    let start_count = events2
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .count();
    assert_eq!(start_count, 1);

    // Back to empty (all stop)
    let events3 = tracker.update_and_detect_changes(vec![]).unwrap();
    let stop_count = events3
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .count();
    assert_eq!(stop_count, 1);
    assert_eq!(tracker.tracked_process_count(), 0);
}

/// Test: Tracker clear resets all state.
#[test]
fn test_lifecycle_tracker_clear() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Add processes
    let _ = tracker
        .update_and_detect_changes(vec![
            create_test_process_event(1, "p1", Some("/bin/p1"), vec!["p1"]),
            create_test_process_event(2, "p2", Some("/bin/p2"), vec!["p2"]),
        ])
        .unwrap();
    assert_eq!(tracker.tracked_process_count(), 2);

    // Clear
    tracker.clear();
    assert_eq!(tracker.tracked_process_count(), 0);

    // After clear, next update should be treated as first (no events)
    let events = tracker
        .update_and_detect_changes(vec![create_test_process_event(
            3,
            "p3",
            Some("/bin/p3"),
            vec!["p3"],
        )])
        .unwrap();
    assert!(
        events.is_empty(),
        "First update after clear should not generate events"
    );
}

/// Test: High volume of processes handled efficiently.
#[test]
fn test_lifecycle_high_volume_processes() {
    let config = LifecycleTrackingConfig {
        max_snapshots: 20000,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Create 1000 processes
    let processes: Vec<ProcessEvent> = (1..=1000)
        .map(|i| {
            create_test_process_event(
                i,
                &format!("process_{i}"),
                Some(&format!("/bin/p{i}")),
                vec![&format!("p{i}")],
            )
        })
        .collect();

    // First cycle
    let start = std::time::Instant::now();
    let events1 = tracker
        .update_and_detect_changes(processes.clone())
        .unwrap();
    let duration1 = start.elapsed();

    assert!(events1.is_empty());
    assert_eq!(tracker.tracked_process_count(), 1000);
    println!("First cycle (1000 processes): {duration1:?}");

    // Second cycle: 100 stopped, 100 started
    let mut updated: Vec<ProcessEvent> = processes[100..].to_vec();
    for i in 1001..=1100 {
        updated.push(create_test_process_event(
            i,
            &format!("process_{i}"),
            Some(&format!("/bin/p{i}")),
            vec![&format!("p{i}")],
        ));
    }

    let start = std::time::Instant::now();
    let events2 = tracker.update_and_detect_changes(updated).unwrap();
    let duration2 = start.elapsed();

    let start_count = events2
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .count();
    let stop_count = events2
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .count();

    assert_eq!(start_count, 100);
    assert_eq!(stop_count, 100);
    println!("Second cycle (1000 processes, 100 start, 100 stop): {duration2:?}");

    // Performance should be reasonable (under 1 second for this volume)
    assert!(
        duration2 < Duration::from_secs(1),
        "Lifecycle detection should complete in under 1 second"
    );
}
