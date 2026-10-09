//! Deterministic stores for the memory measurement, and the arithmetic that says what is in them.
//!
//! Every row is index arithmetic, so a rerun reproduces the store. Timestamps are fixed constants
//! far in the past: every bucket is closed, so the posting-list cache is live, and the store is
//! written completely before any reader exists, so no cached list can predate a write.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::verify_spawn_token;
use daemoneye_lib::models::{AlertSeverity, DetectionRule, ProcessRecord};
use daemoneye_lib::proto::{ColumnDescriptor, ColumnType, SchemaDescriptor, TableDescriptor};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::IngestRecord;

pub const HOUR_MS: u64 = 3_600_000;
/// An hour boundary in 2023; far enough back that every bucket is closed.
pub const BASE_MS: u64 = 472_222 * HOUR_MS;
pub const FULL_BUCKETS: u64 = 168;
pub const HALF_BUCKETS: u64 = 84;
/// More than the default `executor_batch_size` (8,192), so every bucket needs two batches.
pub const ROWS_PER_BUCKET: u64 = 9_000;
/// One planted `nc` row per this many rows, so 168 buckets hold exactly 1,000.
pub const PLANT_STRIDE: u64 = 1_512;
/// Planted pids start here; ordinary pids stay below, which the residual rule's `pid > 100000` uses.
pub const PLANTED_PID_BASE: u64 = 200_000;
/// Ordinary pids cycle through `1..=PID_SPAN`.
const PID_SPAN: u64 = 60_000;
pub const MIN_COVERAGE_PERCENT: u64 = 90;

const NAMES: [&str; 4] = ["bash", "sshd", "nginx", "cron"];

/// The planted rows' global indices that fall in buckets `lo..hi`.
const fn planted_indices(lo: u64, hi: u64) -> std::ops::Range<u64> {
    let first = lo.saturating_mul(ROWS_PER_BUCKET).div_ceil(PLANT_STRIDE);
    let end = hi.saturating_mul(ROWS_PER_BUCKET).div_ceil(PLANT_STRIDE);
    first..end
}

/// How many planted rows buckets `lo..hi` hold.
pub fn planted_in(lo: u64, hi: u64) -> usize {
    planted_indices(lo, hi).count()
}

/// Which buckets of `lo..hi` hold at least one planted row.
pub fn planted_buckets(lo: u64, hi: u64) -> BTreeSet<u64> {
    planted_indices(lo, hi)
        .map(|k| {
            k.saturating_mul(PLANT_STRIDE)
                .saturating_div(ROWS_PER_BUCKET)
        })
        .collect()
}

/// The bucket a planted row (identified by its pid) was written to.
pub fn bucket_of_planted_pid(pid: u32) -> u64 {
    let k = u64::from(pid).saturating_sub(PLANTED_PID_BASE);
    k.saturating_mul(PLANT_STRIDE)
        .saturating_div(ROWS_PER_BUCKET)
}

/// The R13 distribution guard: planted rows must be spread, not clustered.
pub fn assert_spread(buckets_with_matches: usize, window_buckets: u64) {
    let covered = u64::try_from(buckets_with_matches).unwrap();
    assert!(
        covered.saturating_mul(100) >= window_buckets.saturating_mul(MIN_COVERAGE_PERCENT),
        "planted matches must cover at least 90% of the window's buckets"
    );
}

fn timestamp(bucket: u64, index: u64) -> chrono::DateTime<Utc> {
    let step = HOUR_MS.checked_div(ROWS_PER_BUCKET).unwrap_or(1);
    let ts_ms = BASE_MS
        .saturating_add(bucket.saturating_mul(HOUR_MS))
        .saturating_add(index.saturating_mul(step));
    Utc.timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
        .unwrap()
}

/// Row `index` of `bucket`: a realistic process row, a planted `nc` every `PLANT_STRIDE` rows.
fn row(bucket: u64, index: u64) -> ProcessRecord {
    let global = bucket.saturating_mul(ROWS_PER_BUCKET).saturating_add(index);
    let (pid, name) = if global.is_multiple_of(PLANT_STRIDE) {
        let k = global.checked_div(PLANT_STRIDE).unwrap_or(0);
        (PLANTED_PID_BASE.saturating_add(k), "nc")
    } else {
        let slot = global.checked_rem(PID_SPAN).unwrap_or(0);
        let pick = global.checked_rem(4).unwrap_or(0);
        (
            slot.saturating_add(1),
            NAMES
                .get(usize::try_from(pick).unwrap())
                .copied()
                .unwrap_or("bash"),
        )
    };
    let mut r = ProcessRecord::new(u32::try_from(pid).unwrap(), name.to_owned());
    r.collection_time = timestamp(bucket, index);
    r.executable_path = Some(format!("/usr/sbin/{name}").into());
    let marker = if name == "nc" { " planted" } else { "" };
    r.command_line = Some(format!(
        "/usr/sbin/{name}{marker} --config /etc/{name}/{name}.conf --worker {index}"
    ));
    r
}

fn ingest(records: impl Iterator<Item = (u64, ProcessRecord)>) -> Vec<IngestRecord> {
    records
        .map(|(seq, record)| {
            IngestRecord::new("memory", seq, u32::try_from(seq).unwrap(), record).unwrap()
        })
        .collect()
}

/// Write the 168-bucket retention store at `path`, one `put_batch` per bucket.
pub fn build_retention_store(path: &Path) {
    let store = EventStore::new(path).unwrap();
    for bucket in 0..FULL_BUCKETS {
        let first = bucket.saturating_mul(ROWS_PER_BUCKET);
        let rows = (0..ROWS_PER_BUCKET).map(|i| (first.saturating_add(i), row(bucket, i)));
        store.put_batch(&ingest(rows)).unwrap();
    }
}

/// Rows in the max-size store: enough for several byte-bounded batches.
pub const MAX_SIZE_ROWS: u64 = 24;
pub const MAX_COMMAND_LINE_BYTES: usize = 1_048_576;
/// `DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES`'s own composition.
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 4_096;
const HASH_BYTES: usize = 64;

/// One bucket of maximum-admitted rows: 255-byte name, 4,096-byte path, 64-byte hash, 1 MiB
/// command line. Every command line carries the `planted` marker so the full-scan rule matches each row.
pub fn build_max_size_store(path: &Path) {
    let store = EventStore::new(path).unwrap();
    let rows = (0..MAX_SIZE_ROWS).map(|i| {
        let pid = u32::try_from(PLANTED_PID_BASE.saturating_add(i)).unwrap();
        let mut r = ProcessRecord::new(pid, "n".repeat(MAX_NAME_BYTES));
        r.collection_time = timestamp(0, i);
        r.executable_path = Some(format!("/{}", "p".repeat(MAX_PATH_BYTES - 1)).into());
        let marker = "planted ";
        r.command_line = Some(format!(
            "{marker}{}",
            "c".repeat(MAX_COMMAND_LINE_BYTES.saturating_sub(marker.len()))
        ));
        r.executable_hash = Some("f".repeat(HASH_BYTES));
        (i, r)
    });
    store.put_batch(&ingest(rows)).unwrap();
}

fn column(name: &str, column_type: ColumnType) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(column_type),
        nullable: false,
        supported_ops: Vec::new(),
    }
}

fn schema() -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![
                column("pid", ColumnType::Uint),
                column("name", ColumnType::String),
                ColumnDescriptor {
                    nullable: true,
                    ..column("command_line", ColumnType::String)
                },
                column("collection_time", ColumnType::Int),
            ],
        }],
        conformance_results: Vec::new(),
    }
}

/// The rules every run evaluates each cycle: an index-served equality, the same with a residual,
/// and a `LIKE` on `command_line`, which no index serves, so it reads every row of every bucket in
/// the window. (`pid > 100000` looked like the unindexed choice and is not: the pid index serves
/// range predicates, and the first run of this harness read 1,000 rows instead of 1,512,000.)
pub const RULES: [(&str, &str); 3] = [
    (
        "indexed",
        "SELECT pid, name FROM processes WHERE name = 'nc'",
    ),
    (
        "residual",
        "SELECT pid, name FROM processes WHERE name = 'nc' AND pid > 100000",
    ),
    (
        "full-scan",
        "SELECT pid, name FROM processes WHERE command_line LIKE '%planted%'",
    ),
];

/// The id of the rule that reads every row.
pub const FULL_SCAN_ID: &str = "full-scan";

/// An engine with [`RULES`] loaded, and the store it reads.
pub fn engine_over(path: &Path, config: &DetectionConfig) -> (Arc<EventStore>, DetectionEngine) {
    let store = Arc::new(EventStore::open(path).unwrap());
    let mut engine = DetectionEngine::with_config(config);
    let token = "a".repeat(64);
    let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
    engine.register_collector(&verified, schema()).unwrap();
    for (id, sql) in RULES {
        let rule = DetectionRule::new(
            id.to_owned(),
            format!("Rule {id}"),
            "memory measurement".to_owned(),
            sql.to_owned(),
            "test".to_owned(),
            AlertSeverity::High,
        );
        engine.load_rule(rule).unwrap();
    }
    (store, engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planting_covers_at_least_90_percent_of_the_full_and_half_windows() {
        assert_eq!(
            planted_in(0, FULL_BUCKETS),
            1_000,
            "the plan's 1,000 matches"
        );
        assert_spread(planted_buckets(0, FULL_BUCKETS).len(), FULL_BUCKETS);
        assert_spread(
            planted_buckets(FULL_BUCKETS - HALF_BUCKETS, FULL_BUCKETS).len(),
            HALF_BUCKETS,
        );
    }

    #[test]
    fn the_spread_guard_rejects_matches_clustered_in_one_bucket() {
        let clustered: BTreeSet<u64> = BTreeSet::from([0]);
        let result = std::panic::catch_unwind(|| assert_spread(clustered.len(), FULL_BUCKETS));
        assert!(result.is_err(), "one bucket of 168 is not a spread");
    }
}
