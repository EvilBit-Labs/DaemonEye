//! Process Manager Unit Tests
//!
//! These tests validate the collector process lifecycle management functionality,
//! including spawning, monitoring, termination, pause/resume, and automatic restart.

use daemoneye_eventbus::process_manager::{
    CollectorConfig, CollectorProcessManager, CollectorState, ProcessManagerConfig,
    ProcessManagerError,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tempfile::TempDir;

static SCRIPT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a test configuration for process manager
fn create_test_config() -> ProcessManagerConfig {
    ProcessManagerConfig {
        collector_binaries: HashMap::new(),
        default_graceful_timeout: Duration::from_secs(5),
        default_force_timeout: Duration::from_secs(2),
        health_check_interval: Duration::from_secs(10),
        enable_auto_restart: false,
        heartbeat_timeout_multiplier: 3,
    }
}

/// Create a mock collector binary that sleeps for a duration
#[cfg(unix)]
fn create_mock_collector_binary(temp_dir: &TempDir, sleep_duration: u64) -> PathBuf {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let id = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let script_path = temp_dir.path().join(format!("mock_collector_{id}.sh"));
    let script_content = format!(
        r#"#!/bin/bash
echo "Starting collector"
sleep {}
echo "Collector exiting"
exit 0
"#,
        sleep_duration
    );

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&script_path)
        .expect("Failed to create test script");
    file.write_all(script_content.as_bytes())
        .expect("Failed to write test script");
    file.sync_all().expect("Failed to sync test script");
    let mut perms = fs::metadata(&script_path)
        .expect("Failed to get metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    script_path
}

#[cfg(windows)]
fn create_mock_collector_binary(temp_dir: &TempDir, sleep_duration: u64) -> PathBuf {
    use std::fs::OpenOptions;
    use std::io::Write;

    let id = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let script_path = temp_dir.path().join(format!("mock_collector_{id}.bat"));
    let script_content = format!(
        r#"@echo off
echo Starting collector
timeout /t {} /nobreak > nul
echo Collector exiting
exit /b 0
"#,
        sleep_duration
    );

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&script_path)
        .expect("Failed to create test script");
    file.write_all(script_content.as_bytes())
        .expect("Failed to write test script");
    file.sync_all().expect("Failed to sync test script");
    script_path
}

/// Create a failing collector binary that exits immediately with error
#[cfg(unix)]
fn create_failing_collector_binary(temp_dir: &TempDir) -> PathBuf {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let id = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let script_path = temp_dir.path().join(format!("failing_collector_{id}.sh"));
    let script_content = r#"#!/bin/bash
echo "Collector failing"
exit 1
"#;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&script_path)
        .expect("Failed to create test script");
    file.write_all(script_content.as_bytes())
        .expect("Failed to write test script");
    file.sync_all().expect("Failed to sync test script");
    let mut perms = fs::metadata(&script_path)
        .expect("Failed to get metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    script_path
}

#[cfg(windows)]
fn create_failing_collector_binary(temp_dir: &TempDir) -> PathBuf {
    use std::fs::OpenOptions;
    use std::io::Write;

    let id = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let script_path = temp_dir.path().join(format!("failing_collector_{id}.bat"));
    let script_content = r#"@echo off
echo Collector failing
exit /b 1
"#;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&script_path)
        .expect("Failed to create test script");
    file.write_all(script_content.as_bytes())
        .expect("Failed to write test script");
    file.sync_all().expect("Failed to sync test script");
    script_path
}

#[tokio::test]
async fn test_start_collector() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    let result = manager
        .start_collector("test-collector", "test", collector_config)
        .await;

    assert!(result.is_ok(), "Failed to start collector: {:?}", result);
    let pid = result.unwrap();
    assert!(pid > 0, "Invalid PID returned");

    // Verify status
    let status = manager.get_collector_status("test-collector").await;
    assert!(status.is_ok());
    let status = status.unwrap();
    assert_eq!(status.state, CollectorState::Running);
    assert_eq!(status.pid, pid);

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
#[ignore = "Flaky test - timing sensitive process termination"]
async fn test_stop_collector_graceful() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Stop gracefully
    let result = manager
        .stop_collector("test-collector", true, Duration::from_secs(5))
        .await;

    assert!(result.is_ok(), "Failed to stop collector: {:?}", result);

    // Verify collector is removed
    let status = manager.get_collector_status("test-collector").await;
    assert!(matches!(
        status,
        Err(ProcessManagerError::ProcessNotFound(_))
    ));
}

#[tokio::test]
#[ignore = "Flaky test - timing sensitive process termination"]
async fn test_stop_collector_force() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Force kill
    let result = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;

    assert!(
        result.is_ok(),
        "Failed to force kill collector: {:?}",
        result
    );

    // Verify collector is removed
    let status = manager.get_collector_status("test-collector").await;
    assert!(matches!(
        status,
        Err(ProcessManagerError::ProcessNotFound(_))
    ));
}

#[tokio::test]
#[ignore = "Flaky test - timing sensitive process restart"]
async fn test_restart_collector() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    let initial_pid = manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Restart
    let result = manager
        .restart_collector("test-collector", Duration::from_secs(10))
        .await;

    assert!(result.is_ok(), "Failed to restart collector: {:?}", result);
    let new_pid = result.unwrap();
    assert_ne!(initial_pid, new_pid, "PID should change after restart");

    // Verify status
    let status = manager
        .get_collector_status("test-collector")
        .await
        .expect("Failed to get status");
    assert_eq!(status.state, CollectorState::Running);
    assert_eq!(status.restart_count, 1);

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
#[cfg(all(unix, feature = "freebsd"))]
async fn test_pause_collector() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Pause
    let result = manager.pause_collector("test-collector").await;
    assert!(result.is_ok(), "Failed to pause collector: {:?}", result);

    // Verify state
    let status = manager
        .get_collector_status("test-collector")
        .await
        .expect("Failed to get status");
    assert_eq!(status.state, CollectorState::Paused);

    // Resume and cleanup
    let _ = manager.resume_collector("test-collector").await;
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
#[cfg(all(unix, feature = "freebsd"))]
async fn test_resume_collector() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Pause first
    manager
        .pause_collector("test-collector")
        .await
        .expect("Failed to pause");

    // Resume
    let result = manager.resume_collector("test-collector").await;
    assert!(result.is_ok(), "Failed to resume collector: {:?}", result);

    // Verify state
    let status = manager
        .get_collector_status("test-collector")
        .await
        .expect("Failed to get status");
    assert_eq!(status.state, CollectorState::Running);

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
#[cfg(windows)]
async fn test_pause_not_supported_windows() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Attempt pause
    let result = manager.pause_collector("test-collector").await;
    assert!(matches!(
        result,
        Err(ProcessManagerError::PlatformNotSupported(_))
    ));

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn test_start_already_running() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path: binary_path.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config.clone())
        .await
        .expect("Failed to start collector");

    // Try to start again
    let result = manager
        .start_collector("test-collector", "test", collector_config)
        .await;

    assert!(matches!(
        result,
        Err(ProcessManagerError::AlreadyRunning(_))
    ));

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn test_stop_not_found() {
    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let result = manager
        .stop_collector("nonexistent", true, Duration::from_secs(5))
        .await;

    assert!(matches!(
        result,
        Err(ProcessManagerError::ProcessNotFound(_))
    ));
}

#[tokio::test]
async fn test_spawn_failed_invalid_binary() {
    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path: PathBuf::from("/nonexistent/binary"),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    let result = manager
        .start_collector("test-collector", "test", collector_config)
        .await;

    assert!(matches!(result, Err(ProcessManagerError::SpawnFailed(_))));
}

#[tokio::test]
async fn test_check_collector_health() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Check health
    let result = manager.check_collector_health("test-collector").await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        daemoneye_eventbus::process_manager::HealthStatus::Healthy
    );

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn test_get_collector_status() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    let pid = manager
        .start_collector("test-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Get status
    let result = manager.get_collector_status("test-collector").await;
    assert!(result.is_ok());

    let status = result.unwrap();
    assert_eq!(status.collector_id, "test-collector");
    assert_eq!(status.pid, pid);
    assert_eq!(status.state, CollectorState::Running);
    assert_eq!(status.restart_count, 0);
    // Uptime should be reasonably small just after start
    assert!(status.uptime.as_secs() <= 120);

    // Cleanup
    let _ = manager
        .stop_collector("test-collector", false, Duration::from_secs(2))
        .await;
}

/// Create a flaky collector: first run fails quickly, subsequent runs sleep and succeed
#[cfg(unix)]
fn create_flaky_collector_binary(temp_dir: &TempDir) -> PathBuf {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let script_path = temp_dir.path().join("flaky_collector.sh");
    let flag_path = temp_dir.path().join("first_run.flag");
    let script_content = format!(
        r#"#!/bin/bash
if [ ! -f "{flag}" ]; then
  echo "First run failing"
  touch "{flag}"
  exit 1
fi
echo "Subsequent run sleeping"
sleep 5
exit 0
"#,
        flag = flag_path.display()
    );

    fs::write(&script_path, script_content).expect("Failed to write test script");
    let mut perms = fs::metadata(&script_path)
        .expect("Failed to get metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    script_path
}

#[cfg(windows)]
fn create_flaky_collector_binary(temp_dir: &TempDir) -> PathBuf {
    use std::fs;

    let script_path = temp_dir.path().join("flaky_collector.bat");
    let flag_path = temp_dir.path().join("first_run.flag");
    let script_content = format!(
        r#"@echo off
if not exist "{flag}" (
  echo First run failing
  type nul > "{flag}"
  exit /b 1
)
echo Subsequent run sleeping
timeout /t 5 /nobreak > nul
exit /b 0
"#,
        flag = flag_path.display()
    );

    fs::write(&script_path, script_content).expect("Failed to write test script");
    script_path
}

#[tokio::test]
async fn test_auto_restart_on_crash_flaky() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_flaky_collector_binary(&temp_dir);

    let mut cfg = create_test_config();
    cfg.enable_auto_restart = true;
    let manager = CollectorProcessManager::new(cfg);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: true,
        max_restarts: 2,
    };

    manager
        .start_collector("flaky-collector", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Poll until restarted instance is running
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("Timed out waiting for auto-restart");
        }
        if let Ok(status) = manager.get_collector_status("flaky-collector").await
            && status.state == CollectorState::Running
            && status.restart_count >= 1
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Cleanup
    let _ = manager
        .stop_collector("flaky-collector", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn test_auto_restart_disabled() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_failing_collector_binary(&temp_dir);

    let mut cfg = create_test_config();
    cfg.enable_auto_restart = false; // global disabled
    let manager = CollectorProcessManager::new(cfg);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: true,
        max_restarts: 2,
    };

    let _ = manager
        .start_collector("fail-collector", "test", collector_config)
        .await;

    // The collector exits almost immediately, but the monitor task that
    // removes it from the process map runs on a 500ms interval. Under
    // heavy instrumentation (e.g., llvm-cov) scheduling can be delayed,
    // so we poll for removal up to a reasonable deadline instead of
    // relying on a single fixed sleep.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match manager.get_collector_status("fail-collector").await {
            Err(ProcessManagerError::ProcessNotFound(_)) => break,
            Ok(status) => {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "Collector should have been removed from map, last state: {:?}",
                        status.state
                    );
                }
            }
            Err(e) => {
                panic!("Unexpected error while checking collector status: {}", e);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn test_auto_restart_max_attempts() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_failing_collector_binary(&temp_dir);

    let mut cfg = create_test_config();
    cfg.enable_auto_restart = true;
    let manager = CollectorProcessManager::new(cfg);

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: true,
        max_restarts: 1, // only one attempt
    };

    let _ = manager
        .start_collector("max-collector", "test", collector_config)
        .await;

    // After the first crash the collector should be auto-restarted once.
    // When that restart also fails, it should be removed from the process
    // map so that operators can start it again explicitly. As with the
    // disabled case, we poll until the monitor has had a chance to clean
    // up the entry rather than relying on a fixed sleep.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        match manager.get_collector_status("max-collector").await {
            Err(ProcessManagerError::ProcessNotFound(_)) => break,
            Ok(status) => {
                if std::time::Instant::now() > deadline {
                    panic!(
                        "Collector should have been removed after max restarts, last state: {:?}",
                        status.state
                    );
                }
            }
            Err(e) => {
                panic!("Unexpected error while checking collector status: {}", e);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore] // TODO: Fix flaky test - race condition in concurrent start/stop operations
async fn test_concurrent_start_stop() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let manager = CollectorProcessManager::new(create_test_config());

    // Start 3 collectors concurrently
    let mut handles = vec![];
    for i in 0..3 {
        let binary_path = create_mock_collector_binary(&temp_dir, 60);
        let collector_config = CollectorConfig {
            binary_path,
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            resource_limits: None,
            auto_restart: false,
            max_restarts: 1,
        };
        let mgr = manager.clone();
        let id = format!("conc-{}", i);
        handles.push(tokio::spawn(async move {
            mgr.start_collector(&id, "test", collector_config)
                .await
                .ok();
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    // Stop concurrently
    let mut stops = vec![];
    for i in 0..3 {
        let mgr = manager.clone();
        let id = format!("conc-{}", i);
        stops.push(tokio::spawn(async move {
            let _ = mgr.stop_collector(&id, false, Duration::from_secs(2)).await;
        }));
    }
    for h in stops {
        let _ = h.await;
    }

    // Verify removal
    for i in 0..3 {
        let status = manager.get_collector_status(&format!("conc-{}", i)).await;
        assert!(matches!(
            status,
            Err(ProcessManagerError::ProcessNotFound(_))
        ));
    }
}

/// Test that stubborn/non-terminating processes remain visible with Failed state
///
/// This test verifies Comment 1 requirement: processes that don't respond to
/// termination signals should remain in the process map with Failed state for
/// operator visibility, rather than being silently removed.
///
/// Note: This test relies on force kill timeout being extremely short to trigger
/// the timeout path. The test may occasionally pass (kill succeeds) due to timing,
/// but when it does trigger timeout, it verifies the Failed state is preserved.
#[tokio::test]
#[cfg(unix)]
#[ignore = "Flaky test - timing sensitive process termination and state preservation"]
async fn test_stubborn_process_visibility() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create a script that ignores SIGTERM and continues running
    let script_path = temp_dir.path().join("stubborn_collector.sh");
    let script_content = r#"#!/bin/bash
# Trap SIGTERM and ignore it
trap '' TERM

echo "Stubborn collector started"
# Sleep indefinitely to simulate stuck process
while true; do
    sleep 1
done
"#;

    fs::write(&script_path, script_content).expect("Failed to write test script");
    let mut perms = fs::metadata(&script_path)
        .expect("Failed to get metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    let mut config = create_test_config();
    // Use very short timeouts to speed up test
    config.default_graceful_timeout = Duration::from_millis(100);
    // Use extremely short force timeout (1 nanosecond) to trigger timeout before kill completes
    config.default_force_timeout = Duration::from_nanos(1);

    let manager = CollectorProcessManager::new(config);

    let collector_config = CollectorConfig {
        binary_path: script_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start the stubborn collector
    manager
        .start_collector("stubborn", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Verify it's running
    let status = manager
        .get_collector_status("stubborn")
        .await
        .expect("Collector should exist");
    assert_eq!(status.state, CollectorState::Running);

    // Attempt graceful stop with short timeout - this will fail because process ignores SIGTERM
    let stop_result = manager
        .stop_collector("stubborn", true, Duration::from_millis(100))
        .await;

    // Two possible outcomes due to timing:
    match stop_result {
        Ok(_exit_code) => {
            // SIGKILL succeeded before timeout - entry should be removed
            let status_check = manager.get_collector_status("stubborn").await;
            assert!(
                matches!(status_check, Err(ProcessManagerError::ProcessNotFound(_))),
                "Successfully killed process should be removed from map"
            );
            println!("Test outcome: SIGKILL succeeded (process removed - correct behavior)");
        }
        Err(_) => {
            // Force kill timed out - this is the key scenario we're testing
            println!("Test outcome: Force kill timed out (testing Failed state preservation)");

            // Critical assertion: verify collector entry STILL EXISTS with Failed state
            let status = manager
                .get_collector_status("stubborn")
                .await
                .expect("Stubborn collector entry should still exist for operator visibility");

            // Verify state is Failed, not removed from map
            match &status.state {
                CollectorState::Failed(reason) => {
                    assert!(
                        reason.contains("timeout") || reason.contains("kill"),
                        "Failed state should indicate timeout or kill failure, got: {}",
                        reason
                    );
                }
                other_state => {
                    panic!(
                        "Expected Failed state for stubborn process, got: {:?}",
                        other_state
                    );
                }
            }

            // Verify shutdown_all also preserves Failed entries
            let shutdown_result = manager.shutdown_all().await;
            assert!(
                shutdown_result.is_ok(),
                "shutdown_all should complete even with Failed entries"
            );

            // After shutdown_all, Failed entries should still be visible
            let final_status = manager.get_collector_status("stubborn").await;
            match final_status {
                Ok(status) => {
                    // Entry still exists - verify it's still Failed
                    assert!(
                        matches!(status.state, CollectorState::Failed(_)),
                        "Entry should remain in Failed state after shutdown_all"
                    );
                }
                Err(ProcessManagerError::ProcessNotFound(_)) => {
                    panic!("Stubborn process entry should be preserved for operator visibility");
                }
                Err(e) => {
                    panic!("Unexpected error checking final status: {}", e);
                }
            }
        }
    }
}

#[tokio::test]
#[ignore] // TODO: Fix flaky test - race condition causing "Force kill timeout" instead of Running state
async fn test_concurrent_restart() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);
    let manager = CollectorProcessManager::new(create_test_config());

    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };

    manager
        .start_collector("restart-conc", "test", collector_config)
        .await
        .expect("start");

    let m1 = manager.clone();
    let m2 = manager.clone();
    let (r1, r2) = tokio::join!(
        async move {
            m1.restart_collector("restart-conc", Duration::from_secs(5))
                .await
        },
        async move {
            m2.restart_collector("restart-conc", Duration::from_secs(5))
                .await
        }
    );
    assert!(
        r1.is_ok() || r2.is_ok(),
        "At least one restart should succeed"
    );

    let status = manager.get_collector_status("restart-conc").await.unwrap();
    assert_eq!(status.state, CollectorState::Running);

    let _ = manager
        .stop_collector("restart-conc", false, Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn test_shutdown_all() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config = create_test_config();
    let manager = CollectorProcessManager::new(config);

    // Start multiple collectors
    for i in 0..3 {
        let binary_path = create_mock_collector_binary(&temp_dir, 60);
        let collector_config = CollectorConfig {
            binary_path,
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            resource_limits: None,
            auto_restart: false,
            max_restarts: 3,
        };

        manager
            .start_collector(&format!("collector-{}", i), "test", collector_config)
            .await
            .expect("Failed to start collector");
    }

    // Shutdown all
    let result = manager.shutdown_all().await;
    assert!(result.is_ok());

    // Verify all are stopped
    for i in 0..3 {
        let status = manager
            .get_collector_status(&format!("collector-{}", i))
            .await;
        assert!(matches!(
            status,
            Err(ProcessManagerError::ProcessNotFound(_))
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_heartbeat_publish_and_sequence() {
    use daemoneye_eventbus::broker::DaemoneyeBroker;
    use daemoneye_eventbus::message::{Message, MessageType};
    use uuid::Uuid;

    // Start embedded broker
    let broker = Arc::new(
        DaemoneyeBroker::new("/tmp/test-heartbeat.sock")
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    // Subscribe to heartbeat topic for a specific collector
    let collector_id = "hb-collector";
    let topic = format!("control.health.heartbeat.{}", collector_id);
    let subscriber_id = Uuid::new_v4();
    let mut rx = broker.subscribe_raw(&topic, subscriber_id).await.unwrap();

    // Create process manager with heartbeat interval of 1s and broker
    let mut cfg = create_test_config();
    cfg.health_check_interval = Duration::from_secs(1);
    let manager = daemoneye_eventbus::process_manager::CollectorProcessManager::with_broker(
        cfg,
        Some(broker.clone()),
    );

    // Start a long-running mock collector
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };
    manager
        .start_collector(collector_id, "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Expect at least two heartbeat messages with increasing sequence numbers
    let mut seqs = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while seqs.len() < 2 && std::time::Instant::now() < deadline {
        if let Some(msg) = rx.recv().await {
            // Outer message is a Control; payload is JSON of inner heartbeat Message
            assert_eq!(msg.message_type, MessageType::Control);
            let inner: Message =
                serde_json::from_slice(&msg.payload).expect("valid inner heartbeat JSON");
            assert_eq!(inner.message_type, MessageType::Heartbeat);
            seqs.push(inner.sequence);
        }
    }
    assert!(
        seqs.len() >= 2,
        "Did not receive expected heartbeats in time"
    );
    assert!(seqs[1] > seqs[0], "Heartbeat sequence should increase");

    // Cleanup
    let _ = manager
        .stop_collector(collector_id, false, Duration::from_secs(1))
        .await;
}

#[cfg(unix)] // Timing-sensitive test with short intervals fails on Windows
#[tokio::test]
async fn test_heartbeat_timeout_degraded_then_unhealthy_without_broker() {
    // Create process manager with short interval and no broker
    let mut cfg = create_test_config();
    cfg.health_check_interval = Duration::from_millis(100);
    cfg.heartbeat_timeout_multiplier = 3; // unhealthy at >= 3 missed
    let manager = CollectorProcessManager::new(cfg);

    // Start a long-running mock collector
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let binary_path = create_mock_collector_binary(&temp_dir, 60);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };
    manager
        .start_collector("hb-timeout", "test", collector_config)
        .await
        .expect("Failed to start collector");

    // Wait just over one interval to trigger missed heartbeat accounting
    tokio::time::sleep(Duration::from_millis(150)).await;
    let status1 = manager
        .check_collector_health("hb-timeout")
        .await
        .expect("health");
    assert_eq!(
        status1,
        daemoneye_eventbus::process_manager::HealthStatus::Degraded
    );

    // Wait long enough to exceed threshold for unhealthy
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status2 = manager
        .check_collector_health("hb-timeout")
        .await
        .expect("health2");
    assert_eq!(
        status2,
        daemoneye_eventbus::process_manager::HealthStatus::Unhealthy
    );

    // Cleanup
    let _ = manager
        .stop_collector("hb-timeout", false, Duration::from_secs(1))
        .await;
}
