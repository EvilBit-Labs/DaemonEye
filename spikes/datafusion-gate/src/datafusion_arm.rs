//! U6 — `DataFusion` arm: `SessionContext` plus the `ShadowHunt` self-join.
//!
//! Registers the U5 provider and executes the lineage join as derived standard
//! SQL, returning the same matches as the control arm (R3, R18).
//!
//! Arrow comes through `datafusion::arrow` (KTD2): `DataFusion` 55.1 pins
//! `arrow ^59.2` while the current release is 60.0, and a direct dependency
//! yields two incompatible `arrow_schema::Schema` types.

use crate::fixture::{SERVICE_NAME, SHELL_NAME};
use crate::provider::EventTable;
use daemoneye_lib::storage::EventStore;
use datafusion::arrow::array::{Array, UInt32Array};
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::prelude::{SessionConfig, SessionContext};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Table name the provider is registered under.
const TABLE: &str = "events";

/// Session settings recorded alongside the measurement; both move the memory
/// profile, so the decision artifact reports them.
#[derive(Debug, Clone, Copy)]
pub struct SessionSettings {
    /// `datafusion.execution.target_partitions`.
    pub target_partitions: usize,
    /// `datafusion.execution.batch_size`.
    pub batch_size: usize,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            target_partitions: 4,
            batch_size: 8192,
        }
    }
}

/// The derived standard SQL the arm executes.
///
/// A parent/child lineage self-join in the `ShadowHunt` shape: a shell process
/// whose parent is a long-running service. The time predicate is what the
/// provider prunes partitions on.
#[must_use]
pub fn lineage_sql(start_ms: i64, end_ms: i64) -> String {
    format!(
        "SELECT c.pid AS child_pid \
         FROM {TABLE} AS c JOIN {TABLE} AS p ON c.ppid = p.pid \
         WHERE c.name = '{SHELL_NAME}' AND p.name = '{SERVICE_NAME}' \
         AND c.ts_ms >= {start_ms} AND c.ts_ms < {end_ms} \
         AND p.ts_ms >= {start_ms} AND p.ts_ms < {end_ms}"
    )
}

/// Build a `SessionContext` with the provider registered.
///
/// # Errors
/// Returns an error when the provider cannot discover buckets or registration
/// fails.
pub fn build_context(
    store: Arc<EventStore>,
    settings: SessionSettings,
) -> DfResult<(SessionContext, usize)> {
    let table = EventTable::try_new(store)?;
    table.assert_granularity()?;
    let buckets = table.bucket_count();

    let config = SessionConfig::new()
        .with_target_partitions(settings.target_partitions)
        .with_batch_size(settings.batch_size);
    let ctx = SessionContext::new_with_config(config);
    ctx.register_table(TABLE, Arc::new(table))?;
    Ok((ctx, buckets))
}

/// Execute the lineage join and collect the matched child pids.
///
/// # Errors
/// Returns an error when planning or execution fails, or when the result
/// schema is not the single `UInt32` column the query projects.
pub async fn run(ctx: &SessionContext, start_ms: i64, end_ms: i64) -> DfResult<BTreeSet<u32>> {
    let df = ctx.sql(&lineage_sql(start_ms, end_ms)).await?;
    let batches = df.collect().await?;

    let mut matches = BTreeSet::new();
    for batch in &batches {
        let col = batch.column(0);
        let ids = col.as_any().downcast_ref::<UInt32Array>().ok_or_else(|| {
            DataFusionError::Execution(format!(
                "expected UInt32 child_pid column, found {:?}",
                col.data_type()
            ))
        })?;
        for i in 0..ids.len() {
            if !ids.is_null(i) {
                matches.insert(ids.value(i));
            }
        }
    }
    Ok(matches)
}
