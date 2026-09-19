//! U4 — control arm: the scan-and-decode ceiling.
//!
//! Materializes every row in the query's time range with no engine on top
//! (R5), producing three things: the reference result set the `DataFusion` arm is
//! checked against (R18), the RSS ceiling, and the latency floor.
//!
//! It reads through `EventStore::scan_range`, the same public call the provider
//! (U5) uses per partition. Both arms therefore pay the same
//! `Vec<ProcessRecord>` materialization, which is what keeps R6's marginal
//! comparison honest (KTD10).

use crate::fixture::{SERVICE_NAME, SHELL_NAME};
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::storage::{EventStore, StorageError};
use std::collections::{BTreeSet, HashSet};

/// What one control-arm run observed.
#[derive(Debug, Clone)]
pub struct ControlResult {
    /// Rows decoded across the whole range.
    pub rows_decoded: usize,
    /// Child pids whose parent is a service — the reference answer.
    pub matches: BTreeSet<u32>,
}

/// Decode `[start_ms, end_ms)` and compute the `ShadowHunt` lineage matches.
///
/// # Errors
/// Returns [`StorageError`] when the range scan fails.
pub fn run(store: &EventStore, start_ms: u64, end_ms: u64) -> Result<ControlResult, StorageError> {
    let rows = store.scan_range(start_ms, end_ms)?;
    Ok(ControlResult {
        rows_decoded: rows.len(),
        matches: lineage_matches(&rows),
    })
}

/// Decode the range without computing matches, isolating pure materialization
/// cost from the match computation.
///
/// # Errors
/// Returns [`StorageError`] when the range scan fails.
pub fn decode_only(store: &EventStore, start_ms: u64, end_ms: u64) -> Result<usize, StorageError> {
    Ok(store.scan_range(start_ms, end_ms)?.len())
}

/// The reference implementation of the `ShadowHunt` self-join, in plain Rust.
///
/// Mirrors the derived SQL the `DataFusion` arm runs: a shell child whose parent
/// is a long-running service. This is a correctness reference for R18, not a
/// performance claim.
fn lineage_matches(rows: &[ProcessRecord]) -> BTreeSet<u32> {
    let services: HashSet<u32> = rows
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();

    rows.iter()
        .filter(|r| r.name == SHELL_NAME)
        .filter_map(|r| r.ppid.map(|p| (r.pid.raw(), p.raw())))
        .filter(|&(_, ppid)| services.contains(&ppid))
        .map(|(pid, _)| pid)
        .collect()
}
