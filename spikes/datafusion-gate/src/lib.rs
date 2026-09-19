//! T4 · M3 `DataFusion` feasibility spike.
//!
//! Disposable. Measures what `DataFusion` costs over a real T3 event store against
//! a no-engine control arm, so the maintainer can decide whether T6 builds
//! detection execution on `DataFusion` or on the hand-rolled fallback.
//!
//! Delete this directory, its `.gitignore` negation, the root `[workspace]
//! exclude` entry, and its `just` recipes to remove the spike entirely (R12).

pub mod control;
pub mod datafusion_arm;
pub mod fixture;
pub mod measure;
pub mod provider;

use std::path::PathBuf;

/// Where both arms expect the generated fixture store.
///
/// Overridable with `DATAFUSION_GATE_FIXTURE` so a run can point at a store on
/// another volume without recompiling.
#[must_use]
pub fn fixture_path() -> PathBuf {
    std::env::var_os("DATAFUSION_GATE_FIXTURE").map_or_else(
        || std::env::temp_dir().join("datafusion-gate-fixture.redb"),
        PathBuf::from,
    )
}

/// Fail early when the fixture has not been generated.
///
/// # Errors
/// Returns a message naming the missing path and how to create it.
pub fn require_fixture(path: &std::path::Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    Err(format!(
        "fixture missing at {}; run `just spike-datafusion-fixture` first",
        path.display()
    ))
}

/// Repetitions each arm times, so R7 reports a distribution rather than one sample.
pub const REPEATS: usize = 10;
