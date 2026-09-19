//! U3 verification — the fixture is reproducible and shaped as the arms expect.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::needless_borrows_for_generic_args
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

#[test]
fn planted_matches_are_spread_across_the_whole_span_not_clustered_in_one_bucket() {
    // This is the guard on the gate itself. An earlier layout emitted every
    // planted pair as the first 2*planted rows, which put all of them inside
    // the first hourly bucket: the measured query still decoded all 26 buckets,
    // so the cost numbers were real, but the cross-arm match count only ever
    // exercised one of them. A pruning or decode defect in any later bucket
    // would have moved every measured number while leaving the match count
    // exactly right, and nothing would have caught it.
    let d = TempDir::new().unwrap();
    let spec = small_spec();
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    let store = EventStore::open(&d.path().join("f.redb")).unwrap();

    let services: HashSet<u32> = store
        .scan_range(stats.start_ms, stats.end_ms)
        .unwrap()
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();

    let mut buckets_with_matches = 0_usize;
    let mut total_matches = 0_u64;
    for bucket in store.list_buckets().unwrap() {
        let start = bucket * HOUR_MS;
        let here = store
            .scan_range(start, start + HOUR_MS)
            .unwrap()
            .iter()
            .filter(|r| r.name == SHELL_NAME)
            .filter_map(|r| r.ppid)
            .filter(|p| services.contains(&p.raw()))
            .count();
        if here > 0 {
            buckets_with_matches += 1;
        }
        total_matches += u64::try_from(here).unwrap();
    }

    assert_eq!(
        total_matches, spec.planted,
        "every planted match must be found exactly once across the buckets"
    );
    assert!(
        buckets_with_matches >= stats.buckets.saturating_sub(2),
        "planted matches must reach nearly every bucket, not cluster in one: \
         only {buckets_with_matches} of {} buckets hold a match",
        stats.buckets
    );
}

#[test]
fn the_spread_guard_holds_at_the_spec_the_measurements_use() {
    // The guard above runs the small spec. The recorded numbers come from
    // FixtureSpec::default(), and a guard that only checks a smaller fixture is
    // not guarding the published result — the same gap this crate already had
    // once for the equivalence check.
    let d = TempDir::new().unwrap();
    let spec = FixtureSpec::default();
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    let store = EventStore::open(&d.path().join("f.redb")).unwrap();

    let services: HashSet<u32> = store
        .scan_range(stats.start_ms, stats.end_ms)
        .unwrap()
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();

    let mut buckets_with_matches = 0_usize;
    let mut total = 0_u64;
    for bucket in store.list_buckets().unwrap() {
        let start = bucket * HOUR_MS;
        let here = store
            .scan_range(start, start + HOUR_MS)
            .unwrap()
            .iter()
            .filter(|r| r.name == SHELL_NAME)
            .filter_map(|r| r.ppid)
            .filter(|p| services.contains(&p.raw()))
            .count();
        if here > 0 {
            buckets_with_matches += 1;
        }
        total += u64::try_from(here).unwrap();
    }

    assert_eq!(
        total, stats.planted,
        "every planted match found exactly once"
    );
    assert!(
        buckets_with_matches >= stats.buckets.saturating_sub(2),
        "at the measured spec, matches must reach nearly every bucket: \
         only {buckets_with_matches} of {} hold a match",
        stats.buckets
    );
}

#[test]
fn the_reported_planted_count_is_what_was_written_not_what_was_asked_for() {
    // `planted` used to echo the request. `plant_stride` clamps for an
    // infeasible spec, so the two can diverge and only a counted value is safe
    // to read into the decision artifact.
    let d = TempDir::new().unwrap();
    // Deliberately infeasible: more planted pairs than the row budget allows.
    let spec = FixtureSpec {
        rows: 100,
        span_hours: 25,
        planted: 400,
        start_ms: 1_767_225_600_000,
    };
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    assert!(
        stats.planted < spec.planted,
        "an infeasible spec must report the smaller count it actually wrote, \
         got {} for a request of {}",
        stats.planted,
        spec.planted
    );

    let store = EventStore::open(&d.path().join("f.redb")).unwrap();
    let rows = store.scan_range(stats.start_ms, stats.end_ms).unwrap();
    let services: HashSet<u32> = rows
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();
    let observed = rows
        .iter()
        .filter(|r| r.name == SHELL_NAME)
        .filter_map(|r| r.ppid)
        .filter(|p| services.contains(&p.raw()))
        .count();
    assert_eq!(
        u64::try_from(observed).unwrap(),
        stats.planted,
        "the reported count must equal what is actually in the store"
    );
}

#[test]
fn no_noise_row_can_resolve_to_a_service_parent() {
    // Noise parents point at a reserved pid that is never emitted, so an
    // accidental match cannot inflate the planted constant for any spec.
    let d = TempDir::new().unwrap();
    let spec = small_spec();
    let stats = fixture::generate(&d.path().join("f.redb"), spec).unwrap();
    let store = EventStore::open(&d.path().join("f.redb")).unwrap();
    let rows = store.scan_range(stats.start_ms, stats.end_ms).unwrap();

    let services: HashSet<u32> = rows
        .iter()
        .filter(|r| r.name == SERVICE_NAME)
        .map(|r| r.pid.raw())
        .collect();
    let matching_shells = rows
        .iter()
        .filter(|r| r.name == SHELL_NAME)
        .filter_map(|r| r.ppid)
        .filter(|p| services.contains(&p.raw()))
        .count();

    assert_eq!(
        u64::try_from(matching_shells).unwrap(),
        spec.planted,
        "exactly the planted shells may resolve to a service; \
         a noise row matching would silently inflate the gate's answer"
    );
}
