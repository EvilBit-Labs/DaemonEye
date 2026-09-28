//! Spawn-token issuance, file permissions, and per-spawn invalidation (R9).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::create_dir
)]

use daemoneye_eventbus::process_manager::spawn_token::{
    SPAWN_TOKEN_ARG, SpawnTokenError, SpawnTokenStore, read_token_file,
};

#[test]
fn an_issued_token_is_sixty_four_lowercase_hex_characters() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();

    let issued = store.issue("procmond").unwrap();

    let value = store.expected_token("procmond").unwrap();
    assert_eq!(value.len(), 64);
    assert!(
        value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(read_token_file(issued.path()).unwrap(), value);
}

#[test]
fn the_token_file_is_readable_only_by_its_creator() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();
    let issued = store.issue("procmond").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(issued.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o400,
            "token file mode should be 0o400, got {mode:#o}"
        );

        let dir_mode = std::fs::metadata(store.directory())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dir_mode & 0o077,
            0,
            "socket directory must not be group- or world-accessible"
        );
    }
    #[cfg(windows)]
    {
        assert!(issued.path().exists());
    }
}

#[test]
fn respawning_a_collector_invalidates_the_token_issued_for_the_previous_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();

    let _first_issue = store.issue("procmond").unwrap();
    let first = store.expected_token("procmond").unwrap();
    let _second_issue = store.issue("procmond").unwrap();
    let second = store.expected_token("procmond").unwrap();

    assert_ne!(first, second);
    assert_eq!(store.expected_token("procmond").unwrap(), second);
}

#[test]
fn reaping_a_collector_revokes_its_token_and_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();
    let issued = store.issue("procmond").unwrap();

    store.revoke("procmond");

    assert!(store.expected_token("procmond").is_none());
    assert!(!issued.path().exists());
}

#[test]
fn a_collector_that_was_never_spawned_has_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();
    assert!(store.expected_token("ghostmond").is_none());
}

#[test]
fn an_identity_that_would_escape_the_token_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();

    let error = store.issue("../escape").unwrap_err();
    assert!(matches!(error, SpawnTokenError::InvalidCollectorId(_)));
}

#[test]
fn a_group_writable_token_directory_is_refused_at_construction() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("spawn-tokens");
        std::fs::create_dir(&token_dir).unwrap();
        std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o777)).unwrap();

        let error = SpawnTokenStore::new(dir.path()).unwrap_err();
        assert!(matches!(error, SpawnTokenError::InsecureDirectory { .. }));
    }
}

#[test]
fn the_spawn_argument_carries_the_path_and_never_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let store = SpawnTokenStore::new(dir.path()).unwrap();
    let issued = store.issue("procmond").unwrap();

    let args = issued.command_args();
    assert_eq!(args[0], SPAWN_TOKEN_ARG);
    assert_eq!(args[1], issued.path().display().to_string());
    let value = store.expected_token("procmond").unwrap();
    assert!(args.iter().all(|arg| !arg.contains(&value)));
}
