//! Spawn tokens are minted at the single spawn choke point and die with their process (R9).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use daemoneye_eventbus::process_manager::spawn_token::{SPAWN_TOKEN_ARG, SpawnTokenStore};
use daemoneye_eventbus::process_manager::{
    CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
};
use tempfile::TempDir;

fn mock_binary(dir: &TempDir) -> PathBuf {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.path().join("mock_collector.sh");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"#!/bin/bash\nexec sleep 30\n").unwrap();
    file.sync_all().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A collector that exits promptly, so the process monitor reaps it during the test.
fn short_lived_binary(dir: &TempDir) -> PathBuf {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.path().join("short_lived_collector.sh");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"#!/bin/bash\nexit 0\n").unwrap();
    file.sync_all().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Poll `condition` until it holds or the deadline passes, rather than sleeping a fixed span.
async fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    /// The monitor polls twice a second; ten seconds is far past any healthy reap.
    const DEADLINE: Duration = Duration::from_secs(10);

    let started = std::time::Instant::now();
    while started.elapsed() < DEADLINE {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    condition()
}

fn manager_with_tokens(dir: &TempDir) -> (Arc<CollectorProcessManager>, Arc<SpawnTokenStore>) {
    let store = Arc::new(SpawnTokenStore::new(dir.path()).unwrap());
    let manager = CollectorProcessManager::with_spawn_tokens(
        ProcessManagerConfig::default(),
        None,
        Some(Arc::clone(&store)),
    );
    (manager, store)
}

fn collector_config(binary_path: PathBuf) -> CollectorConfig {
    CollectorConfig {
        binary_path,
        args: vec!["--baseline".to_owned()],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    }
}

#[tokio::test]
async fn spawning_mints_a_token_and_the_stored_config_does_not_accumulate_the_flag() {
    // Arrange
    let dir = TempDir::new().unwrap();
    let (manager, store) = manager_with_tokens(&dir);
    let config = collector_config(mock_binary(&dir));

    // Act
    let _pid = manager
        .start_collector("test-collector", "test", config)
        .await
        .unwrap();

    // Assert: a token exists, and the persisted config is untouched so a restart cannot
    // accumulate a second --spawn-token-file argument.
    assert!(store.expected_token("test-collector").is_some());
    let stored = manager.collector_config("test-collector").await.unwrap();
    assert_eq!(stored.args, ["--baseline"]);
    assert!(!stored.args.iter().any(|arg| arg == SPAWN_TOKEN_ARG));

    let _termination = manager
        .stop_collector("test-collector", true, Duration::from_secs(5))
        .await;
}

#[tokio::test]
async fn a_reaped_collector_loses_its_token_and_a_respawn_mints_a_different_one() {
    // Arrange: a collector that exits on its own, so the monitor reaps it.
    let dir = TempDir::new().unwrap();
    let (manager, store) = manager_with_tokens(&dir);
    let short_lived = short_lived_binary(&dir);
    let _pid = manager
        .start_collector(
            "test-collector",
            "test",
            collector_config(short_lived.clone()),
        )
        .await
        .unwrap();
    let before = store.expected_token("test-collector").unwrap();

    // Act: wait for the monitor to notice the exit and reap it.
    let reaped = wait_until(|| store.expected_token("test-collector").is_none()).await;

    // Assert: the token died with the process it was issued for.
    assert!(
        reaped,
        "monitor did not revoke the token of a reaped collector"
    );

    // Act: the agent respawns the same identity.
    let _respawn_pid = manager
        .start_collector("test-collector", "test", collector_config(short_lived))
        .await
        .unwrap();

    // Assert: the new spawn carries a different token, so a captured one is worthless.
    let after = store.expected_token("test-collector").unwrap();
    assert_ne!(before, after, "a respawn must not reuse the previous token");
}

#[tokio::test]
async fn stopping_a_collector_revokes_its_token() {
    let dir = TempDir::new().unwrap();
    let (manager, store) = manager_with_tokens(&dir);
    let config = collector_config(mock_binary(&dir));
    let _pid = manager
        .start_collector("test-collector", "test", config)
        .await
        .unwrap();

    // The termination itself is timing-sensitive on a shell-script collector and the repo already
    // treats it as flaky; the revocation happens on the decision to stop, before any of that.
    let _termination = manager
        .stop_collector("test-collector", true, Duration::from_secs(5))
        .await;

    assert!(store.expected_token("test-collector").is_none());
}
