//! U3 verification — the fixture is reproducible and shaped as the arms expect.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::fixture::{self, FixtureSpec, HOUR_MS, SERVICE_NAME, SHELL_NAME};
use std::collections::HashSet;
use tempfile::TempDir;

const fn small_spec() -> FixtureSpec {
    FixtureSpec {
        rows: 6_000,
        span_hours: 25,
        planted: 40,
        start_ms: 1_767_225_600_000,
    }
}

#[test]
fn generating_twice_with_the_same_spec_produces_identical_stores() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let spec = small_spec();

    let sa = fixture::generate(&a.path().join("f.redb"), spec).unwrap();
    let sb = fixture::generate(&b.path().join("f.redb"), spec).unwrap();

    assert_eq!(sa.rows, sb.rows, "row counts must match across runs");
    assert_eq!(sa.buckets, sb.buckets, "bucket counts must match");
    assert_eq!(sa.end_ms, sb.end_ms, "time spans must match");

    let store_a = EventStore::open(a.path().join("f.redb")).unwrap();
    let store_b = EventStore::open(b.path().join("f.redb")).unwrap();
    let rows_a = store_a.scan_range(sa.start_ms, sa.end_ms).unwrap();
    let rows_b = store_b.scan_range(sb.start_ms, sb.end_ms).unwrap();
    assert_eq!(rows_a.len(), rows_b.len());
    for (x, y) in rows_a.iter().zip(rows_b.iter()) {
        assert_eq!(x.pid, y.pid, "pid sequence must be identical");
        assert_eq!(x.name, y.name, "name sequence must be identical");
        assert_eq!(
            x.collection_time, y.collection_time,
            "timestamps must be identical"
        );
    }
}

#[test]
fn spans_at_least_twenty_four_hourly_buckets() {
    let d = TempDir::new().unwrap();
    let stats = fixture::generate(&d.path().join("f.redb"), small_spec()).unwrap();
    assert!(
        stats.buckets >= 24,
        "fixture must span >=24 buckets so the join crosses partitions, got {}",
        stats.buckets
    );
}

#[test]
fn default_spec_meets_the_hundred_thousand_row_bar() {
    let spec = FixtureSpec::default();
    assert!(
        spec.rows >= 100_000,
        "R2 requires at least 100k events, spec has {}",
        spec.rows
    );
    assert!(
        spec.span_hours >= 24,
        "R2 requires at least 24h of buckets, spec has {}",
        spec.span_hours
    );
}

#[test]
fn every_planted_child_resolves_to_a_service_parent() {
    let d = TempDir::new().unwrap();
    let spec = small_spec();
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    let store = EventStore::open(d.path().join("f.redb")).unwrap();
    let rows = store.scan_range(stats.start_ms, stats.end_ms).unwrap();

    let services: HashSet<u32> = rows
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();
    let planted = rows
        .iter()
        .filter(|r| r.name == SHELL_NAME)
        .filter_map(|r| r.ppid)
        .filter(|p| services.contains(&p.raw()))
        .count();

    assert_eq!(
        u64::try_from(planted).unwrap(),
        spec.planted,
        "planted match count must be exactly the spec's constant"
    );
}

#[test]
fn collection_time_equals_the_bucket_assigning_timestamp() {
    // Load-bearing for U5: `ts_ms` lives only in the redb key, so the provider
    // can only see `collection_time`. If they diverge, pruning reads the wrong
    // windows.
    let d = TempDir::new().unwrap();
    let spec = small_spec();
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    let store = EventStore::open(d.path().join("f.redb")).unwrap();

    let buckets = store.list_buckets().unwrap();
    for bucket in buckets {
        let start = bucket * HOUR_MS;
        let end = start + HOUR_MS;
        for r in store.scan_range(start, end).unwrap() {
            let ct = r.collection_time.timestamp_millis();
            assert!(
                ct >= start.cast_signed() && ct < end.cast_signed(),
                "row in bucket {bucket} carries collection_time {ct} outside [{start},{end})"
            );
        }
    }
    assert!(stats.rows > 0);
}

#[test]
fn refuses_to_append_to_a_store_that_already_holds_events() {
    let d = TempDir::new().unwrap();
    let path = d.path().join("f.redb");
    fixture::generate(&path, small_spec()).unwrap();

    let err = fixture::generate(&path, small_spec()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already holds") && msg.contains("refusing to append"),
        "a stale fixture must be rejected by name, got: {msg}"
    );
}
