//! Alert enrichment (R19): ancestry through `latest_by_pid`, host context, partial marking,
//! and the invariants an enrichment must not touch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::non_ascii_literal
)]

use daemoneye_lib::alerting::{EnrichContext, enrich};
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, CompletenessReason, ProcessId};
use daemoneye_lib::storage::EventStore;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

const HOUR_MS: u64 = 3_600_000;
const BASE_HOUR: u64 = 1_000;

fn open() -> (TempDir, EventStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = EventStore::new(dir.path().join("events.redb")).unwrap();
    (dir, store)
}

fn ctx() -> EnrichContext {
    EnrichContext {
        hostname: "host-a".to_owned(),
        agent_version: "9.9.9".to_owned(),
    }
}

fn record(pid: u32, ppid: Option<u32>, name: &str, start_hour: u64) -> ProcessRecord {
    let mut r = ProcessRecord::new(pid, name.to_owned());
    r.ppid = ppid.map(ProcessId::new);
    r.start_time = Some(UNIX_EPOCH + Duration::from_millis(start_hour * HOUR_MS));
    r
}

/// Store `r` as an event in the bucket of `hour`.
fn put(store: &EventStore, hour: u64, r: &ProcessRecord) {
    store.put_event(hour * HOUR_MS, 0, r).unwrap();
}

fn alert_for(r: ProcessRecord) -> Alert {
    Alert::new(
        AlertSeverity::High,
        "t",
        "d",
        "rule-1",
        r,
        Completeness::complete(),
    )
}

/// `depth` processes pid 100.. each the child of the next; the last is a root. Returns the child.
fn store_chain(store: &EventStore, depth: u32) -> ProcessRecord {
    let mut child = None;
    for k in 0..depth {
        let ppid = (k + 1 < depth).then_some(101 + k);
        let hour = BASE_HOUR + u64::from(depth - k);
        let r = record(100 + k, ppid, &format!("p{k}"), hour);
        put(store, hour, &r);
        if k == 0 {
            child = Some(r);
        }
    }
    child.unwrap()
}

fn is_partial(a: &Alert) -> bool {
    a.context.tags.iter().any(|t| t == "enrichment=partial")
}

fn hops(a: &Alert) -> usize {
    a.context
        .data
        .get("ancestry")
        .map_or(0, |v| v.split('>').count())
}

#[test]
fn latest_by_pid_honours_bound_and_newest_first() {
    let (_d, store) = open();
    for (hour, name) in [(1, "old"), (3, "mid"), (5, "new")] {
        put(
            &store,
            BASE_HOUR + hour,
            &record(7, None, name, BASE_HOUR + hour),
        );
    }
    let at = |hour: u64| {
        store
            .latest_by_pid(7, (BASE_HOUR + hour) * HOUR_MS)
            .unwrap()
            .map(|r| r.name)
    };
    assert_eq!(at(9).as_deref(), Some("new"));
    assert_eq!(at(4).as_deref(), Some("mid"));
    assert_eq!(at(1).as_deref(), Some("old"));
    assert_eq!(at(0), None);
    assert!(store.latest_by_pid(8, u64::MAX).unwrap().is_none());
}

#[test]
fn latest_by_pid_falls_back_to_collection_time_without_start() {
    let (_d, store) = open();
    let mut r = ProcessRecord::new(9, "nostart".to_owned());
    r.start_time = None;
    put(&store, BASE_HOUR, &r);
    let collected = u64::try_from(r.collection_time.timestamp_millis()).unwrap();
    assert!(store.latest_by_pid(9, collected).unwrap().is_some());
    assert!(store.latest_by_pid(9, collected - 1).unwrap().is_none());
}

#[test]
fn recycled_pid_does_not_name_an_unrelated_process() {
    let (_d, store) = open();
    let child = record(10, Some(5), "child", BASE_HOUR + 10);
    put(&store, BASE_HOUR + 10, &child);
    // Real parent, then a later unrelated process that reused pid 5.
    put(
        &store,
        BASE_HOUR + 2,
        &record(5, None, "real-parent", BASE_HOUR + 2),
    );
    put(
        &store,
        BASE_HOUR + 20,
        &record(5, None, "recycled", BASE_HOUR + 20),
    );

    let out = enrich(&store, &alert_for(child), &ctx());
    assert_eq!(out.context.data["ancestry"], "5:real-parent");
    assert!(!is_partial(&out));
}

#[test]
fn only_a_recycled_parent_record_marks_partial() {
    let (_d, store) = open();
    let child = record(10, Some(5), "child", BASE_HOUR + 10);
    put(&store, BASE_HOUR + 10, &child);
    put(
        &store,
        BASE_HOUR + 20,
        &record(5, None, "recycled", BASE_HOUR + 20),
    );

    let out = enrich(&store, &alert_for(child), &ctx());
    assert!(!out.context.data.contains_key("ancestry"));
    assert!(is_partial(&out));
}

#[test]
fn three_deep_chain_lists_ancestors_without_partial() {
    let (_d, store) = open();
    let child = store_chain(&store, 4);
    let out = enrich(&store, &alert_for(child), &ctx());
    assert_eq!(out.context.data["ancestry"], "101:p1>102:p2>103:p3");
    assert_eq!(out.context.data["host"], "host-a");
    assert_eq!(out.context.data["agent_version"], "9.9.9");
    assert!(out.context.tags.contains(&"enriched".to_owned()));
    assert!(!is_partial(&out));
}

#[test]
fn twenty_deep_chain_stops_at_sixteen_and_is_partial() {
    let (_d, store) = open();
    let child = store_chain(&store, 21);
    let out = enrich(&store, &alert_for(child), &ctx());
    assert_eq!(hops(&out), 16);
    assert!(is_partial(&out));
}

#[test]
fn missing_parent_keeps_found_hops_and_marks_partial() {
    let (_d, store) = open();
    let child = record(10, Some(11), "c", BASE_HOUR + 10);
    put(&store, BASE_HOUR + 10, &child);
    put(
        &store,
        BASE_HOUR + 5,
        &record(11, Some(12), "b", BASE_HOUR + 5),
    );
    // pid 12 was never stored.
    let out = enrich(&store, &alert_for(child), &ctx());
    assert_eq!(out.context.data["ancestry"], "11:b");
    assert!(is_partial(&out));
}

#[test]
fn root_alert_has_no_ancestry_and_no_partial() {
    let (_d, store) = open();
    let out = enrich(
        &store,
        &alert_for(record(1, None, "init", BASE_HOUR)),
        &ctx(),
    );
    assert!(!out.context.data.contains_key("ancestry"));
    assert!(!is_partial(&out));
}

#[test]
fn exe_hash_is_copied_from_the_alert_record() {
    let (_d, store) = open();
    let mut r = record(1, None, "init", BASE_HOUR);
    r.executable_hash = Some("ab".repeat(32));
    let out = enrich(&store, &alert_for(r), &ctx());
    assert_eq!(out.context.data["exe_hash"], "ab".repeat(32));
}

#[test]
fn dedup_key_and_completeness_are_untouched() {
    let (_d, store) = open();
    let child = store_chain(&store, 2);
    let degraded = Alert::new(
        AlertSeverity::High,
        "t",
        "d",
        "rule-1",
        child.clone(),
        Completeness::degraded(vec![CompletenessReason::ResourceLimit {
            detail: "x".to_owned(),
        }])
        .unwrap(),
    );
    let mut suffixed = alert_for(child);
    suffixed.deduplication_key.push_str(":integrity:abc");
    for input in [
        alert_for(record(1, None, "p", BASE_HOUR)),
        degraded,
        suffixed,
    ] {
        let out = enrich(&store, &input, &ctx());
        assert_eq!(out.deduplication_key, input.deduplication_key);
        assert_eq!(out.completeness, input.completeness);
        assert_eq!(out.id, input.id);
        assert!(input.context.data.is_empty(), "input must not be mutated");
    }
}

#[test]
fn long_values_are_capped_on_a_char_boundary() {
    let (_d, store) = open();
    let child = record(10, Some(11), "c", BASE_HOUR + 10);
    put(&store, BASE_HOUR + 10, &child);
    put(
        &store,
        BASE_HOUR + 5,
        &record(11, None, &"é".repeat(500), BASE_HOUR + 5),
    );
    let out = enrich(&store, &alert_for(child), &ctx());
    let v = &out.context.data["ancestry"];
    assert!(v.len() <= 256);
    assert!(v.starts_with("11:é"));
}

#[test]
fn latest_by_pid_prefers_the_later_posting_within_a_bucket() {
    let (_d, store) = open();
    let start = BASE_HOUR + 1;
    store
        .put_event(start * HOUR_MS, 0, &record(3, None, "first", start))
        .unwrap();
    store
        .put_event(start * HOUR_MS + 1, 0, &record(3, None, "second", start))
        .unwrap();
    let hit = store.latest_by_pid(3, u64::MAX).unwrap().unwrap();
    assert_eq!(hit.name, "second");
}
