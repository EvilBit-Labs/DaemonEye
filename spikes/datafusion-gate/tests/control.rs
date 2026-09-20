//! U4 verification — the control arm decodes everything and answers correctly.
//!
//! The load-bearing one here is
//! [`decoded_row_count_equals_the_generated_row_count`]: it compares the
//! control arm against what the generator actually wrote, rather than against
//! the other arm. If `scan_range` silently dropped rows at a range edge, both
//! arms would agree on a wrong answer and every equivalence assertion would
//! stay green. The measured-scale equivalence test makes the same ground-truth
//! comparison at 120,000 rows.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::fixture::{FixtureSpec, HOUR_MS};
use datafusion_gate::{control, fixture};
use tempfile::TempDir;

const SPEC: FixtureSpec = FixtureSpec {
    rows: 6_000,
    span_hours: 25,
    planted: 40,
    start_ms: 1_767_225_600_000,
};

struct Fx {
    _dir: TempDir,
    store: EventStore,
    start: u64,
    end: u64,
    written: u64,
}

fn fx() -> Fx {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f.redb");
    let stats = fixture::generate(&path, SPEC).unwrap();
    let store = EventStore::open(&path).unwrap();
    Fx {
        _dir: dir,
        store,
        start: stats.start_ms,
        end: SPEC.start_ms + SPEC.span_hours * HOUR_MS,
        written: stats.rows,
    }
}

#[test]
fn decoded_row_count_equals_the_generated_row_count() {
    let f = fx();
    let decoded = control::decode_only(&f.store, f.start, f.end).unwrap();
    assert_eq!(
        u64::try_from(decoded).unwrap(),
        f.written,
        "the control arm must decode every row the generator wrote; \
         a shortfall means scan_range dropped rows and both arms would agree on a wrong answer"
    );
    assert_eq!(
        u64::try_from(decoded).unwrap(),
        SPEC.rows,
        "and that count must equal the spec's row count"
    );
}

#[test]
fn the_match_count_equals_the_planted_constant() {
    let f = fx();
    let r = control::run(&f.store, f.start, f.end).unwrap();
    assert_eq!(
        u64::try_from(r.matches.len()).unwrap(),
        SPEC.planted,
        "the control arm must find exactly the planted matches, not noise"
    );
    assert_eq!(
        u64::try_from(r.rows_decoded).unwrap(),
        f.written,
        "and decode every row"
    );
}

#[test]
fn a_range_covering_zero_buckets_returns_empty_rather_than_erroring() {
    let f = fx();
    let far = f.end + HOUR_MS * 1_000;
    let r = control::run(&f.store, far, far + HOUR_MS)
        .expect("an empty window is a valid query, not an error");
    assert_eq!(r.rows_decoded, 0, "no rows exist in that window");
    assert!(r.matches.is_empty(), "and therefore no matches");
}

#[test]
fn a_single_bucket_range_returns_only_that_buckets_rows() {
    let f = fx();
    let whole = control::decode_only(&f.store, f.start, f.end).unwrap();
    let one = control::decode_only(&f.store, f.start, f.start + HOUR_MS).unwrap();
    assert!(one > 0, "the first bucket must hold rows");
    assert!(
        one < whole,
        "one bucket must be a strict subset of the full range: {one} vs {whole}"
    );
}

#[test]
fn an_inverted_range_returns_nothing_rather_than_erroring() {
    let f = fx();
    let r = control::run(&f.store, f.end, f.start).expect("an inverted window must not error");
    assert_eq!(r.rows_decoded, 0, "an inverted window selects no rows");
}
