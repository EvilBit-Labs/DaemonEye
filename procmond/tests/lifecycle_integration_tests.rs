//! Integration tests for process lifecycle tracking with ProcessEventSource.
//!
//! These tests verify that the ProcessLifecycleTracker properly integrates with
//! the ProcessEventSource and can detect lifecycle events in realistic scenarios.

use collector_core::ProcessEvent;
use procmond::lifecycle::{
    LifecycleTrackingConfig, ProcessLifecycleEvent, ProcessLifecycleTracker, ProcessSnapshot,
    SuspiciousEventSeverity,
};
use std::time::{Duration, SystemTime};

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

#[test]
fn test_lifecycle_tracker_with_process_events() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Initial process enumeration
    let initial_processes = vec![
        create_test_process_event(1, "init", Some("/sbin/init"), vec!["/sbin/init"]),
        create_test_process_event(
            100,
            "systemd",
            Some("/usr/lib/systemd/systemd"),
            vec!["systemd"],
        ),
        create_test_process_event(200, "bash", Some("/bin/bash"), vec!["bash"]),
    ];

    let events = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();
    assert!(
        events.is_empty(),
        "First enumeration should not generate events"
    );
    assert_eq!(tracker.tracked_process_count(), 3);

    // Second enumeration with changes
    let updated_processes = vec![
        create_test_process_event(1, "init", Some("/sbin/init"), vec!["/sbin/init"]),
        create_test_process_event(
            100,
            "systemd",
            Some("/usr/lib/systemd/systemd"),
            vec!["systemd"],
        ),
        // bash process terminated
        // New process started
        create_test_process_event(300, "vim", Some("/usr/bin/vim"), vec!["vim", "file.txt"]),
    ];

    let events = tracker
        .update_and_detect_changes(updated_processes)
        .unwrap();

    // Should detect one stop event and one start event
    let start_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
        .collect();
    let stop_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
        .collect();

    assert_eq!(start_events.len(), 1, "Should detect one process start");
    assert_eq!(stop_events.len(), 1, "Should detect one process stop");

    // Verify the start event
    match start_events[0] {
        ProcessLifecycleEvent::Start { process, .. } => {
            assert_eq!(process.pid, 300);
            assert_eq!(process.name, "vim");
        }
        _ => panic!("Expected Start event"),
    }

    // Verify the stop event
    match stop_events[0] {
        ProcessLifecycleEvent::Stop {
            process,
            runtime_duration,
            ..
        } => {
            assert_eq!(process.pid, 200);
            assert_eq!(process.name, "bash");
            assert!(runtime_duration.is_some());
        }
        _ => panic!("Expected Stop event"),
    }
}

#[test]
fn test_process_modification_detection_integration() {
    let config = LifecycleTrackingConfig {
        track_command_line_changes: true,
        track_executable_changes: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Initial process
    let initial_processes = vec![create_test_process_event(
        500,
        "editor",
        Some("/usr/bin/editor"),
        vec!["editor"],
    )];

    let events = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();
    assert!(events.is_empty());

    // Process with modified command line
    let modified_processes = vec![create_test_process_event(
        500,
        "editor",
        Some("/usr/bin/editor"),
        vec!["editor", "--verbose", "document.txt"],
    )];

    let events = tracker
        .update_and_detect_changes(modified_processes)
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
            assert_eq!(previous.command_line, vec!["editor"]);
            assert_eq!(
                current.command_line,
                vec!["editor", "--verbose", "document.txt"]
            );
        }
        _ => panic!("Expected Modified event"),
    }
}

#[test]
fn test_suspicious_behavior_detection_integration() {
    let config = LifecycleTrackingConfig {
        detect_pid_reuse: true,
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Initial process
    let initial_processes = vec![create_test_process_event(
        1000,
        "legitimate_app",
        Some("/usr/bin/legitimate_app"),
        vec!["legitimate_app"],
    )];

    let events = tracker
        .update_and_detect_changes(initial_processes)
        .unwrap();
    assert!(events.is_empty());

    // Same PID but completely different process (potential PID reuse)
    let suspicious_processes = vec![create_test_process_event(
        1000,
        "malicious_app",
        Some("/tmp/malicious_app"),
        vec!["malicious_app", "--stealth"],
    )];

    let events = tracker
        .update_and_detect_changes(suspicious_processes)
        .unwrap();

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
            assert_eq!(process.pid, 1000);
            assert_eq!(process.name, "malicious_app");
            assert!(reason.contains("PID reuse"));
            assert_eq!(*severity, SuspiciousEventSeverity::High); // High because executable path changed
        }
        _ => panic!("Expected Suspicious event"),
    }
}

#[test]
fn test_lifecycle_tracker_statistics_integration() {
    let config = LifecycleTrackingConfig::default();
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Perform multiple updates to generate statistics
    let processes1 = vec![
        create_test_process_event(1, "process1", Some("/bin/process1"), vec!["process1"]),
        create_test_process_event(2, "process2", Some("/bin/process2"), vec!["process2"]),
    ];
    let _events1 = tracker.update_and_detect_changes(processes1).unwrap();

    let processes2 = vec![
        create_test_process_event(1, "process1", Some("/bin/process1"), vec!["process1"]),
        // process2 stopped
        create_test_process_event(3, "process3", Some("/bin/process3"), vec!["process3"]),
    ];
    let _events2 = tracker.update_and_detect_changes(processes2).unwrap();

    let processes3 = vec![
        create_test_process_event(
            1,
            "process1",
            Some("/bin/process1"),
            vec!["process1", "--flag"],
        ), // modified
        create_test_process_event(3, "process3", Some("/bin/process3"), vec!["process3"]),
    ];
    let _events3 = tracker.update_and_detect_changes(processes3).unwrap();

    // Verify statistics
    let stats = tracker.stats();
    assert_eq!(stats.total_updates, 3);
    assert_eq!(stats.start_events, 1); // process3 started
    assert_eq!(stats.stop_events, 1); // process2 stopped
    assert_eq!(stats.modification_events, 1); // process1 modified
    assert!(stats.avg_processes_tracked > 0.0);
}

#[test]
fn test_lifecycle_tracker_memory_management() {
    let config = LifecycleTrackingConfig {
        max_snapshots: 3, // Small limit to test cleanup
        ..Default::default()
    };
    let mut tracker = ProcessLifecycleTracker::new(config);

    // Add processes up to the limit in a single update
    let processes = vec![
        create_test_process_event(1, "process1", Some("/bin/process1"), vec!["process1"]),
        create_test_process_event(2, "process2", Some("/bin/process2"), vec!["process2"]),
        create_test_process_event(3, "process3", Some("/bin/process3"), vec!["process3"]),
    ];
    let _events = tracker.update_and_detect_changes(processes).unwrap();
    assert_eq!(tracker.tracked_process_count(), 3);

    // Add more processes to exceed the limit
    let processes = vec![
        create_test_process_event(1, "process1", Some("/bin/process1"), vec!["process1"]),
        create_test_process_event(2, "process2", Some("/bin/process2"), vec!["process2"]),
        create_test_process_event(3, "process3", Some("/bin/process3"), vec!["process3"]),
        create_test_process_event(4, "process4", Some("/bin/process4"), vec!["process4"]),
    ];
    let _events = tracker.update_and_detect_changes(processes).unwrap();

    // Should still work without crashing (cleanup should have occurred)
    // The tracker should have 4 processes now, but previous snapshots should be cleared
    assert_eq!(tracker.tracked_process_count(), 4);
}

#[test]
fn test_process_snapshot_conversion_integration() {
    let original_event = create_test_process_event(
        1234,
        "test_process",
        Some("/usr/bin/test"),
        vec!["test", "--arg1", "--arg2"],
    );

    // Convert to snapshot and back
    let snapshot = ProcessSnapshot::from(original_event.clone());
    let converted_event = ProcessEvent::from(snapshot.clone());

    // Verify all fields are preserved
    assert_eq!(original_event.pid, converted_event.pid);
    assert_eq!(original_event.ppid, converted_event.ppid);
    assert_eq!(original_event.name, converted_event.name);
    assert_eq!(
        original_event.executable_path,
        converted_event.executable_path
    );
    assert_eq!(original_event.command_line, converted_event.command_line);
    assert_eq!(original_event.start_time, converted_event.start_time);
    assert_eq!(original_event.cpu_usage, converted_event.cpu_usage);
    assert_eq!(original_event.memory_usage, converted_event.memory_usage);
    assert_eq!(
        original_event.executable_hash,
        converted_event.executable_hash
    );
    assert_eq!(original_event.user_id, converted_event.user_id);
    assert_eq!(original_event.accessible, converted_event.accessible);
    assert_eq!(original_event.file_exists, converted_event.file_exists);
    assert_eq!(
        original_event.platform_metadata,
        converted_event.platform_metadata
    );

    // Verify snapshot-specific fields
    assert_eq!(snapshot.pid, 1234);
    assert_eq!(snapshot.name, "test_process");
    assert_eq!(snapshot.executable_path, Some("/usr/bin/test".to_string()));
    assert_eq!(snapshot.command_line, vec!["test", "--arg1", "--arg2"]);
}

#[test]
fn test_lifecycle_tracker_configuration_validation() {
    // Test various configuration options
    let config1 = LifecycleTrackingConfig {
        track_command_line_changes: false,
        track_executable_changes: false,
        track_memory_changes: false,
        track_cpu_changes: false,
        ..Default::default()
    };
    let mut tracker1 = ProcessLifecycleTracker::new(config1);

    let processes1 = vec![create_test_process_event(
        1,
        "process1",
        Some("/bin/process1"),
        vec!["process1"],
    )];
    let _events1 = tracker1.update_and_detect_changes(processes1).unwrap();

    // Modify everything but tracking is disabled
    let mut modified_process = create_test_process_event(
        1,
        "process1",
        Some("/bin/process1"),
        vec!["process1", "--new-arg"],
    );
    modified_process.cpu_usage = Some(50.0);
    modified_process.memory_usage = Some(10 * 1024 * 1024);
    let processes2 = vec![modified_process];
    let events2 = tracker1.update_and_detect_changes(processes2).unwrap();

    // Should not detect any modifications since tracking is disabled
    let modified_events: Vec<_> = events2
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();
    assert_eq!(
        modified_events.len(),
        0,
        "Should not detect modifications when tracking is disabled"
    );

    // Test with tracking enabled
    let config2 = LifecycleTrackingConfig {
        track_command_line_changes: true,
        track_memory_changes: true,
        memory_change_threshold: 10.0, // 10% threshold
        ..Default::default()
    };
    let mut tracker2 = ProcessLifecycleTracker::new(config2);

    let processes3 = vec![create_test_process_event(
        1,
        "process1",
        Some("/bin/process1"),
        vec!["process1"],
    )];
    let _events3 = tracker2.update_and_detect_changes(processes3).unwrap();

    let mut modified_process2 = create_test_process_event(
        1,
        "process1",
        Some("/bin/process1"),
        vec!["process1", "--new-arg"],
    );
    modified_process2.memory_usage = Some(10 * 1024 * 1024); // 10x increase
    let processes4 = vec![modified_process2];
    let events4 = tracker2.update_and_detect_changes(processes4).unwrap();

    // Should detect modifications since tracking is enabled
    let modified_events2: Vec<_> = events4
        .iter()
        .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
        .collect();
    assert_eq!(
        modified_events2.len(),
        1,
        "Should detect modifications when tracking is enabled"
    );

    match modified_events2[0] {
        ProcessLifecycleEvent::Modified {
            modified_fields, ..
        } => {
            assert!(modified_fields.contains(&"command_line".to_string()));
            assert!(modified_fields.contains(&"memory_usage".to_string()));
        }
        _ => panic!("Expected Modified event"),
    }
}
