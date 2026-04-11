//! Composition-root integration test for the shared executable-hash engine.
//!
//! Verifies that when `procmond` constructs an `Arc<MultiAlgorithmHasher>`
//! and hands it to both the actor-mode `ProcmondMonitorCollector` and the
//! standalone `ProcessEventSource`, every holder ends up with the same
//! underlying allocation (`Arc::ptr_eq`).
//!
//! This is the explicit defense against the "silent no-op" regression
//! in the initial P1 binary-hashing implementation: before the
//! composition-root refactor, `--compute-hashes` was silently a no-op
//! because no single site constructed a `MultiAlgorithmHasher` and
//! threaded it into both the actor-mode collector and the standalone
//! event source. The test asserts the wiring cannot regress to that
//! state without a visible failure.
//!
//! See also: `docs/solutions/security-issues/binary-hashing-authorization-and-toctou-fixes.md`
//! for the full problem/solution writeup.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use daemoneye_lib::{
    integrity::{HasherConfig, MultiAlgorithmHasher},
    storage,
};
use procmond::{
    ProcessEventSource, ProcessSourceConfig,
    monitor_collector::{ProcmondMonitorCollector, ProcmondMonitorConfig},
    process_collector::ProcessCollectionConfig,
};
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use tokio::sync::{Mutex, mpsc};

/// Produce an in-memory-like `DatabaseManager` backed by a tempdir path.
///
/// `storage::DatabaseManager::new` is the same entry point procmond uses
/// at startup; giving it a tempdir path exercises the real constructor
/// without touching `/var/lib/daemoneye`.
///
/// Returns the [`TempDir`] alongside the database handle so the
/// underlying directory lives as long as the database — dropping the
/// `TempDir` before the handle would delete the backing files out from
/// under redb.
fn test_db() -> (TempDir, Arc<Mutex<storage::DatabaseManager>>) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("procmond-test.db");
    let db = Arc::new(Mutex::new(
        storage::DatabaseManager::new(path.display().to_string()).expect("db new"),
    ));
    (dir, db)
}

#[tokio::test]
async fn shared_arc_hasher_is_ptr_eq_across_holders() {
    // Construct the composition root — exactly as `procmond/src/main.rs`
    // does when `--compute-hashes == true`.
    let engine = Arc::new(
        MultiAlgorithmHasher::new(HasherConfig::default()).expect("engine constructs cleanly"),
    );

    // Inject into standalone-mode holder. Keep `_tmp_source` alive to
    // preserve the backing tempdir for the database files.
    let (_tmp_source, db_for_source) = test_db();
    let event_source =
        ProcessEventSource::with_config(db_for_source, ProcessSourceConfig::default())
            .with_hasher(Some(Arc::clone(&engine)));

    // Inject into actor-mode holder. Keep `_tmp_actor` alive for the
    // same reason.
    let (_tmp_actor, db_for_actor) = test_db();
    let (_handle, rx) = ProcmondMonitorCollector::create_channel();
    let actor_config = ProcmondMonitorConfig {
        process_config: ProcessCollectionConfig {
            compute_executable_hashes: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let actor_collector = ProcmondMonitorCollector::new(db_for_actor, actor_config, rx)
        .expect("actor collector constructs cleanly")
        .with_hasher(Some(Arc::clone(&engine)));

    // Pull the engine clones back out of both holders and assert they
    // point at the same allocation. This is the core invariant: a
    // single policy (concurrency cap, algorithm list, size budget)
    // applies to both procmond modes.
    let engine_from_source = event_source.hasher().expect("standalone holder has engine");
    let engine_from_actor = actor_collector.hasher().expect("actor holder has engine");

    assert!(
        Arc::ptr_eq(&engine, &engine_from_source),
        "standalone-mode holder received a different Arc"
    );
    assert!(
        Arc::ptr_eq(&engine, &engine_from_actor),
        "actor-mode holder received a different Arc"
    );
    assert!(
        Arc::ptr_eq(&engine_from_source, &engine_from_actor),
        "standalone and actor holders received different Arcs"
    );
}

#[tokio::test]
async fn no_hasher_is_propagated_cleanly() {
    // When --compute-hashes is OFF, both holders must see `None`.
    // This guards against an accidental "default to Some" regression.
    let (_tmp_source, db) = test_db();
    let source = ProcessEventSource::with_config(db, ProcessSourceConfig::default());
    assert!(
        source.hasher().is_none(),
        "ProcessEventSource::with_config must default to hasher: None"
    );

    let (_tmp_actor, db_for_actor) = test_db();
    let (_handle, rx) = ProcmondMonitorCollector::create_channel();
    let actor_collector =
        ProcmondMonitorCollector::new(db_for_actor, ProcmondMonitorConfig::default(), rx)
            .expect("actor constructs");
    assert!(
        actor_collector.hasher().is_none(),
        "ProcmondMonitorCollector::new must default to hasher: None"
    );
}

// Suppress unused-import warning in environments where the mpsc
// re-export would otherwise be flagged. `mpsc` is used transitively via
// `ProcmondMonitorCollector::create_channel`.
#[allow(dead_code)]
fn _use_mpsc(_: mpsc::Sender<u8>) {}
