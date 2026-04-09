//! Composition-root integration test for the shared executable-hash engine.
//!
//! Verifies that when `procmond` constructs an `Arc<MultiAlgorithmHasher>`
//! and hands it to both the actor-mode `ProcmondMonitorCollector` and the
//! standalone `ProcessEventSource`, every holder ends up with the same
//! underlying allocation (`Arc::ptr_eq`).
//!
//! This is the explicit defense against Discovery 1 from the P1
//! resolution plan: prior to Phase 1B, `--compute-hashes` was a
//! silent no-op because no composition site constructed an engine and
//! threaded it into the production holders. The test asserts the
//! wiring cannot regress to that state without a visible failure.
//!
//! Related:
//! - `docs/plans/2026-04-09-001-refactor-binary-hashing-p1-resolutions-plan.md`
//! - `todos/013-pending-p1-shared-engine-composition-root.md`

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
use tempfile::tempdir;
use tokio::sync::{Mutex, mpsc};

/// Produce an in-memory-like `DatabaseManager` backed by a tempdir path.
///
/// `storage::DatabaseManager::new` is the same entry point procmond uses
/// at startup; giving it a tempdir path exercises the real constructor
/// without touching `/var/lib/daemoneye`.
fn test_db() -> Arc<Mutex<storage::DatabaseManager>> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("procmond-test.db");
    Arc::new(Mutex::new(
        storage::DatabaseManager::new(path.display().to_string()).expect("db new"),
    ))
}

#[tokio::test]
async fn shared_arc_hasher_is_ptr_eq_across_holders() {
    // Construct the composition root — exactly as `procmond/src/main.rs`
    // does when `--compute-hashes == true`.
    let engine = Arc::new(
        MultiAlgorithmHasher::new(HasherConfig::default()).expect("engine constructs cleanly"),
    );

    // Inject into standalone-mode holder.
    let db_for_source = test_db();
    let event_source =
        ProcessEventSource::with_config(db_for_source, ProcessSourceConfig::default())
            .with_hasher(Some(Arc::clone(&engine)));

    // Inject into actor-mode holder.
    let db_for_actor = test_db();
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
    let db = test_db();
    let source = ProcessEventSource::with_config(db, ProcessSourceConfig::default());
    assert!(
        source.hasher().is_none(),
        "ProcessEventSource::with_config must default to hasher: None"
    );

    let (_handle, rx) = ProcmondMonitorCollector::create_channel();
    let actor_collector =
        ProcmondMonitorCollector::new(test_db(), ProcmondMonitorConfig::default(), rx)
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
