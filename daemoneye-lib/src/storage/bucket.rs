//! Time-bucket partitioning for `processes.events` (T3 · U3).
//!
//! `processes.events` is partitioned into per-time-window tables named
//! `processes.events@<bucket-id>`, where `bucket-id = ts_ms / granularity_ms`.
//! Whole-bucket eviction is then an O(1) `delete_table` rather than a row scan,
//! and the read handle discovers live buckets via redb's `list_tables()` plus a
//! name-prefix filter — no separate partition registry.
//!
//! All arithmetic uses `checked_*` / `saturating_*` (the workspace denies
//! `arithmetic_side_effects`), so a zero granularity or an extreme timestamp
//! surfaces a [`StorageError`] instead of overflowing.

use crate::storage::StorageError;

/// Table-name prefix shared by every per-bucket process-event table.
pub(super) const BUCKET_PREFIX: &str = "processes.events@";

/// One hour in milliseconds — the default bucket granularity.
pub(super) const HOURLY_MS: u64 = 3_600_000;

/// One day in milliseconds — the coarse granularity for long retention windows.
pub(super) const DAILY_MS: u64 = 86_400_000;

/// Soft ceiling on the number of live buckets before granularity auto-coarsens
/// from hourly to daily (keeps `list_tables()` enumeration cheap).
pub(super) const BUCKET_CEILING: u64 = 1_000;

/// Compute the bucket id for an event timestamp at the given granularity.
pub(super) fn bucket_id(ts_ms: u64, granularity_ms: u64) -> Result<u64, StorageError> {
    ts_ms
        .checked_div(granularity_ms)
        .ok_or_else(|| StorageError::Overflow {
            context: "bucket_id: zero granularity".to_owned(),
        })
}

/// Build the table name for a bucket id.
pub(super) fn bucket_table_name(id: u64) -> String {
    format!("{BUCKET_PREFIX}{id}")
}

/// Parse a bucket id back out of a table name, returning `None` for tables that
/// are not process-event buckets.
pub(super) fn parse_bucket_name(name: &str) -> Option<u64> {
    name.strip_prefix(BUCKET_PREFIX)
        .and_then(|suffix| suffix.parse::<u64>().ok())
}

/// Choose a granularity so the projected live-bucket count for `retention_ms`
/// stays at or under [`BUCKET_CEILING`]; coarsens hourly → daily otherwise.
pub(super) fn choose_granularity(retention_ms: u64) -> u64 {
    let projected_hourly = retention_ms.checked_div(HOURLY_MS).unwrap_or(u64::MAX);
    if projected_hourly > BUCKET_CEILING {
        DAILY_MS
    } else {
        HOURLY_MS
    }
}

/// The highest bucket id that is entirely older than `now_ms - retention_ms`
/// and therefore eligible for retention drop. Buckets with an id strictly less
/// than this cutoff are expired.
pub(super) fn retention_cutoff(
    now_ms: u64,
    retention_ms: u64,
    granularity_ms: u64,
) -> Result<u64, StorageError> {
    let oldest_kept = now_ms.saturating_sub(retention_ms);
    bucket_id(oldest_kept, granularity_ms)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn bucket_id_groups_by_granularity() {
        // Two timestamps in the same hour share a bucket; the next hour differs.
        let a = bucket_id(HOURLY_MS, HOURLY_MS).unwrap();
        let b = bucket_id(HOURLY_MS.saturating_add(59_999), HOURLY_MS).unwrap();
        let c = bucket_id(HOURLY_MS.saturating_mul(2), HOURLY_MS).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(c, a.saturating_add(1));
    }

    #[test]
    fn bucket_id_rejects_zero_granularity() {
        assert!(bucket_id(123, 0).is_err());
    }

    #[test]
    fn name_round_trips() {
        let id = 472_222;
        let name = bucket_table_name(id);
        assert_eq!(name, "processes.events@472222");
        assert_eq!(parse_bucket_name(&name), Some(id));
    }

    #[test]
    fn parse_rejects_foreign_table_names() {
        assert_eq!(parse_bucket_name("detection_rules"), None);
        assert_eq!(parse_bucket_name("processes.events@notanumber"), None);
        assert_eq!(parse_bucket_name("processes.events"), None);
    }

    #[test]
    fn choose_granularity_coarsens_for_long_retention() {
        // 7 days hourly = 168 buckets => stays hourly.
        assert_eq!(choose_granularity(DAILY_MS.saturating_mul(7)), HOURLY_MS);
        // 200 days hourly = 4800 buckets => coarsens to daily.
        assert_eq!(choose_granularity(DAILY_MS.saturating_mul(200)), DAILY_MS);
    }

    #[test]
    fn retention_cutoff_marks_old_buckets_expired() {
        let now = HOURLY_MS.saturating_mul(100);
        let retention = HOURLY_MS.saturating_mul(5);
        // oldest kept = hour 95, so buckets < 95 are expired.
        assert_eq!(retention_cutoff(now, retention, HOURLY_MS).unwrap(), 95);
    }

    #[test]
    fn retention_cutoff_saturates_at_zero() {
        // retention larger than now must not underflow.
        assert_eq!(retention_cutoff(10, 1_000_000, HOURLY_MS).unwrap(), 0);
    }
}
