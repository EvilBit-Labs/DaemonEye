//! Locked-down `DataFusion` session and allowlisted functions (U6; R4, R6, R7).
//!
//! Scoped to the session and function surface. The executor's own tests (U8) are added below the
//! marked section by that unit.
#![cfg(feature = "detection-engine")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use daemoneye_lib::detection::execution::functions::ALLOWLISTED_UDF_NAMES;
use daemoneye_lib::detection::execution::session::{
    ExecutorRuntime, LatencySink, session_state, session_state_sized,
};
use daemoneye_lib::detection::{ALLOWED_SQL_FUNCTIONS, RegexCache};
use daemoneye_lib::detection_bounds::{EXECUTOR_BATCH_SIZE, EXECUTOR_TARGET_PARTITIONS};
use datafusion::arrow::array::{
    Array, BinaryArray, BooleanArray, Int64Array, RecordBatch, StringArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use datafusion::execution::context::SessionContext;
use datafusion::execution::runtime_env::RuntimeEnv;

struct Harness {
    ctx: SessionContext,
    sink: Arc<LatencySink>,
    cache: Arc<RegexCache>,
}

fn harness_with_runtime(runtime: Arc<RuntimeEnv>, names: Vec<Option<String>>) -> Harness {
    let sink = Arc::new(LatencySink::default());
    let cache = Arc::new(RegexCache::new());
    let state = session_state(runtime, Arc::clone(&sink), Arc::clone(&cache)).unwrap();
    let ctx = SessionContext::new_with_state(state);
    let schema = Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, true),
        Field::new("pid", DataType::Int64, false),
    ]));
    let pids: Vec<i64> = (0..i64::try_from(names.len()).unwrap()).collect();
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(names)),
            Arc::new(Int64Array::from(pids)),
        ],
    )
    .unwrap();
    let table = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
    ctx.register_table("processes", Arc::new(table)).unwrap();
    Harness { ctx, sink, cache }
}

fn harness(names: Vec<Option<String>>) -> Harness {
    let runtime = ExecutorRuntime::new().unwrap().env();
    harness_with_runtime(runtime, names)
}

fn names(values: &[&str]) -> Vec<Option<String>> {
    values.iter().map(|v| Some((*v).to_owned())).collect()
}

async fn run(h: &Harness, sql: &str) -> Result<Vec<RecordBatch>, DataFusionError> {
    h.ctx.sql(sql).await?.collect().await
}

async fn one_i64(h: &Harness, sql: &str) -> Option<i64> {
    let batches = run(h, sql).await.unwrap();
    let col = batches[0].column(0);
    let arr = col.as_any().downcast_ref::<Int64Array>().unwrap();
    arr.is_valid(0).then(|| arr.value(0))
}

async fn one_string(h: &Harness, sql: &str) -> Option<String> {
    let batches = run(h, sql).await.unwrap();
    let col = batches[0].column(0);
    let arr = col.as_any().downcast_ref::<StringArray>().unwrap();
    arr.is_valid(0).then(|| arr.value(0).to_owned())
}

async fn one_bool(h: &Harness, sql: &str) -> Option<bool> {
    let batches = run(h, sql).await.unwrap();
    let col = batches[0].column(0);
    let arr = col.as_any().downcast_ref::<BooleanArray>().unwrap();
    arr.is_valid(0).then(|| arr.value(0))
}

fn err_text(result: Result<impl Sized, DataFusionError>) -> String {
    result.err().expect("expected an error").to_string()
}

// --- R4: the allowlist is the registry ---------------------------------------------------------

#[tokio::test]
async fn session_rejects_md5_by_name_through_both_entry_points() {
    let h = harness(names(&["bash"]));
    // Same run: every allowlisted name resolves, so a session that can call nothing cannot pass.
    for name in ALLOWED_SQL_FUNCTIONS {
        assert!(h.ctx.state().scalar_functions().contains_key(*name));
        assert!(
            h.ctx
                .state()
                .scalar_functions()
                .contains_key(&name.to_uppercase().to_lowercase())
        );
    }
    let via_sql = err_text(run(&h, "SELECT md5(name) FROM processes").await);
    assert!(via_sql.to_lowercase().contains("md5"), "{via_sql}");
    let via_state = err_text(
        h.ctx
            .state()
            .create_logical_plan("SELECT md5(name) FROM processes")
            .await,
    );
    assert!(via_state.to_lowercase().contains("md5"), "{via_state}");
}

#[tokio::test]
async fn session_resolves_every_allowlisted_name_case_insensitively() {
    let h = harness(names(&["bash"]));
    let calls = [
        "HEX(CAST('ab' AS BYTEA))",
        "INSTR(name, 'a')",
        "Length(name)",
        "LIKE(name, 'b%')",
        "MATCH(name, 'b')",
        "REGEXP(name, 'b')",
        "UNHEX('00')",
    ];
    assert_eq!(calls.len(), ALLOWED_SQL_FUNCTIONS.len());
    for call in calls {
        let sql = format!("SELECT {call} FROM processes");
        run(&h, &sql).await.unwrap();
    }
    assert_eq!(ALLOWLISTED_UDF_NAMES, ALLOWED_SQL_FUNCTIONS);
}

#[tokio::test]
async fn session_registers_only_allowlisted_scalar_functions() {
    let h = harness(names(&["bash"]));
    let state = h.ctx.state();
    let mut registered: Vec<String> = state.scalar_functions().keys().cloned().collect();
    registered.sort();
    assert_eq!(registered, ALLOWED_SQL_FUNCTIONS);
    assert!(state.aggregate_functions().is_empty());
    assert!(state.window_functions().is_empty());
}

// --- R6: pool, config, no spill ----------------------------------------------------------------

#[tokio::test]
async fn session_uses_default_partitions_and_batch_size() {
    let h = harness(names(&["bash"]));
    let state = h.ctx.state();
    assert_eq!(state.config().target_partitions(), 4);
    assert_eq!(state.config().batch_size(), 8192);
    assert_eq!(EXECUTOR_TARGET_PARTITIONS, 4);
    assert_eq!(EXECUTOR_BATCH_SIZE, 8192);
}

#[test]
fn session_runtime_cannot_create_a_spill_file() {
    let runtime = ExecutorRuntime::new().unwrap().env();
    assert!(!runtime.disk_manager.tmp_files_enabled());
    let err = runtime.disk_manager.create_tmp_file("session test").err();
    assert!(err.is_some_and(|e| matches!(e.find_root(), DataFusionError::ResourcesExhausted(_))));
}

/// 10,000 rows spread over `scan_partitions` scan partitions, `target_partitions` configured, a
/// 1 KiB pool. Separating the two is the whole point: the pool error comes from the
/// `RepartitionExec` the optimizer inserts to close the gap between them, so a scan that already
/// supplies the target count never gets one.
async fn tiny_pool_outcome_split(
    scan_partitions: usize,
    target_partitions: usize,
) -> (Result<usize, DataFusionError>, Arc<RuntimeEnv>) {
    let runtime = ExecutorRuntime::with_pool_bytes(1024).unwrap().env();
    let sink = Arc::new(LatencySink::default());
    let state = session_state_sized(
        Arc::clone(&runtime),
        sink,
        Arc::new(RegexCache::new()),
        target_partitions,
        EXECUTOR_BATCH_SIZE,
    )
    .unwrap();
    let ctx = SessionContext::new_with_state(state);
    let schema = Arc::new(Schema::new(vec![Field::new("name", DataType::Utf8, true)]));
    let per = 10_000_usize.div_ceil(scan_partitions);
    let mut parts: Vec<Vec<RecordBatch>> = Vec::with_capacity(scan_partitions);
    for p in 0..scan_partitions {
        let rows: Vec<String> = (0..10_000)
            .skip(p.saturating_mul(per))
            .take(per)
            .map(|i| format!("proc{i}"))
            .collect();
        let batch =
            RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(StringArray::from(rows))])
                .unwrap();
        parts.push(vec![batch]);
    }
    ctx.register_table(
        "processes",
        Arc::new(MemTable::try_new(Arc::clone(&schema), parts).unwrap()),
    )
    .unwrap();
    let outcome = async {
        let batches = ctx
            .sql("SELECT name FROM processes WHERE length(name) > 3 AND regexp(name, '^proc') LIMIT 5000")
            .await?
            .collect()
            .await?;
        Ok(batches.iter().map(RecordBatch::num_rows).sum())
    }
    .await;
    (outcome, runtime)
}

/// Pins which side of the pool question each production regime lands on.
///
/// Neither assumption A3 nor A7 is right on its own. Nothing in the filter, projection or limit
/// reserves pool memory, as A7 says — but the physical optimizer inserts a `RepartitionExec` to
/// close any gap between the scan's partition count and `target_partitions`, and *that* reserves
/// batch-sized memory and attempts a spill the disabled disk manager refuses. So the pool error is
/// a function of the gap, not of the plan's operators, and R11's "at most
/// `executor_target_partitions`" puts the two production regimes on opposite sides:
///
/// - **Normal detection cycle.** R3 evaluates only the rows that cycle stored, so the window falls
///   inside one bucket: one scan partition against a target of four. The repartition is inserted,
///   and `ResourcesExhausted` is reachable. This is the common path, so R6's degraded-reason
///   mapping in U10 is covering real behaviour rather than a shape this plan excludes.
/// - **Full-retention measurement (U9).** The window spans every bucket, so the provider supplies
///   the target count, no repartition is inserted, and a 1 KiB pool completes the query.
///
/// Both are asserted below so a `DataFusion` change that moves the reservation is visible, and so
/// U5 knows its partition count decides which regime a query lands in.
#[tokio::test]
async fn session_pool_survives_when_the_scan_already_supplies_target_partitions() {
    let (matched, _r) =
        tiny_pool_outcome_split(EXECUTOR_TARGET_PARTITIONS, EXECUTOR_TARGET_PARTITIONS).await;
    assert_eq!(
        matched.ok(),
        Some(5000),
        "a scan already at target_partitions needs no repartition, so the tiny pool suffices"
    );

    // The contrast: the same target with a single-partition scan does need one, and fails.
    let (gapped, _r2) = tiny_pool_outcome_split(1, EXECUTOR_TARGET_PARTITIONS).await;
    assert!(
        gapped.is_err(),
        "a one-partition scan under the same target must still need a repartition"
    );
}

/// 10,000 rows, `n` partitions, a 1 KiB pool: the A3 scenario over the filter/limit plan.
async fn tiny_pool_outcome(partitions: usize) -> (Result<usize, DataFusionError>, Arc<RuntimeEnv>) {
    let runtime = ExecutorRuntime::with_pool_bytes(1024).unwrap().env();
    let sink = Arc::new(LatencySink::default());
    let state = session_state_sized(
        Arc::clone(&runtime),
        sink,
        Arc::new(RegexCache::new()),
        partitions,
        EXECUTOR_BATCH_SIZE,
    )
    .unwrap();
    let ctx = SessionContext::new_with_state(state);
    let rows: Vec<String> = (0..10_000).map(|i| format!("proc{i}")).collect();
    let schema = Arc::new(Schema::new(vec![Field::new("name", DataType::Utf8, true)]));
    let batch =
        RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(StringArray::from(rows))]).unwrap();
    ctx.register_table(
        "processes",
        Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
    )
    .unwrap();
    let outcome = async {
        let batches = ctx
            .sql("SELECT name FROM processes WHERE length(name) > 3 AND regexp(name, '^proc') LIMIT 5000")
            .await?
            .collect()
            .await?;
        Ok(batches.iter().map(RecordBatch::num_rows).sum())
    }
    .await;
    (outcome, runtime)
}

#[tokio::test]
async fn session_exhausted_pool_is_a_resource_error_and_never_a_spill() {
    let (outcome, runtime) = tiny_pool_outcome(EXECUTOR_TARGET_PARTITIONS).await;
    let error = outcome.expect_err("a 1 KiB pool cannot carry a 4-way repartition");
    assert!(
        matches!(error.find_root(), DataFusionError::ResourcesExhausted(_)),
        "expected ResourcesExhausted"
    );
    // The error came from a spill attempt that the disabled disk manager refused. Asserting on the
    // disk manager rather than diffing the shared temp directory: nextest runs tests in parallel
    // and other tests create temp files, so a before/after diff would be racy.
    assert!(error.to_string().contains("DiskManager is disabled"));
    assert!(!runtime.disk_manager.tmp_files_enabled());
    assert_eq!(runtime.disk_manager.used_disk_space(), 0);
    assert!(runtime.disk_manager.temp_dir_paths().is_empty());
    assert_eq!(
        runtime.memory_pool.reserved(),
        0,
        "reservations are released on failure"
    );
}

/// Characterises where the pool error comes from, so a `DataFusion` change that moves it is seen.
///
/// The plan is filter + limit with no sort, aggregate or join. The only operator that reserves
/// batch-sized memory is the `RepartitionExec` the physical optimizer inserts to reach
/// `target_partitions` over a one-partition scan. At one partition there is none, and the same
/// query fits a 1 KiB pool.
#[tokio::test]
async fn session_pool_error_needs_a_repartition_and_vanishes_at_one_partition() {
    let (single, _single_runtime) = tiny_pool_outcome(1).await;
    assert_eq!(single.ok(), Some(5000));
    let (many, _many_runtime) = tiny_pool_outcome(2).await;
    assert!(many.is_err());
}

/// The other half of `parser_level_constructs_do_not_reach_the_function_allowlist`.
///
/// `SUBSTR`, `SUBSTRING` and `TRIM` parse into their own `sqlparser` variants, so the load-time
/// allowlist never sees them and a rule using one loads. The session registers exactly
/// [`ALLOWED_SQL_FUNCTIONS`] and replaces `DataFusion`'s defaults, so there is no implementation
/// behind them and the rule fails when it runs instead. Operator docs state that asymmetry
/// (`docs/src/technical/sql-dialect-reference.md`); this is what makes the statement checkable,
/// and it fails if either side of it moves.
#[tokio::test]
async fn parser_level_constructs_load_but_have_no_implementation() {
    let h = harness(names(&["bash"]));
    for sql in [
        "SELECT pid FROM processes WHERE substr(name, 1, 3) = 'bas'",
        "SELECT pid FROM processes WHERE SUBSTRING(name FROM 1 FOR 3) = 'bas'",
        "SELECT pid FROM processes WHERE trim(name) = 'bash'",
    ] {
        assert!(
            run(&h, sql).await.is_err(),
            "{sql} loads but must not execute: no implementation is registered for it"
        );
    }
    // The contrast: an allowlisted function over the same column does execute.
    let ok = run(&h, "SELECT pid FROM processes WHERE length(name) = 4")
        .await
        .unwrap();
    assert_eq!(ok.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
}

// --- R7: sink and measurement site -------------------------------------------------------------

#[tokio::test]
async fn session_regexp_records_one_nonzero_entry_keyed_by_pattern() {
    let rows: Vec<Option<String>> = (0..10_000).map(|i| Some(format!("proc{i}"))).collect();
    let h = harness(rows);
    run(
        &h,
        "SELECT pid FROM processes WHERE regexp(name, '^proc(1|2)')",
    )
    .await
    .unwrap();
    let drained = h.sink.drain();
    assert_eq!(drained.len(), 1);
    let elapsed = drained.get("^proc(1|2)").expect("keyed by pattern text");
    assert!(*elapsed > Duration::ZERO);
    assert!(h.sink.drain().is_empty(), "drain clears the sink");
    assert!(h.cache.is_cached("^proc(1|2)"));
}

#[tokio::test]
async fn session_match_and_regexp_share_one_entry_per_pattern() {
    let h = harness(names(&["bash", "zsh"]));
    run(&h, "SELECT pid FROM processes WHERE regexp(name, '^b')")
        .await
        .unwrap();
    run(&h, "SELECT pid FROM processes WHERE match(name, '^b')")
        .await
        .unwrap();
    let drained = h.sink.drain();
    assert_eq!(drained.keys().collect::<Vec<_>>(), vec!["^b"]);
}

#[test]
fn session_sink_keeps_the_max_per_pattern_until_drained() {
    let sink = LatencySink::default();
    sink.record("p", Duration::from_millis(5));
    sink.record("p", Duration::from_millis(2));
    sink.record("q", Duration::from_millis(1));
    sink.record("p", Duration::from_millis(9));
    let drained = sink.drain();
    assert_eq!(drained.get("p"), Some(&Duration::from_millis(9)));
    assert_eq!(drained.get("q"), Some(&Duration::from_millis(1)));
    sink.record("p", Duration::from_millis(1));
    assert_eq!(sink.drain().get("p"), Some(&Duration::from_millis(1)));
}

#[test]
fn session_sink_latches_a_breach_only_above_its_threshold_and_keeps_it_after_drain() {
    let sink = LatencySink::with_threshold(Duration::from_millis(5));
    sink.record("p", Duration::from_millis(5));
    assert!(!sink.is_breached());
    sink.record("p", Duration::from_millis(6));
    assert!(sink.is_breached());
    sink.drain();
    assert!(sink.is_breached());
    assert!(!LatencySink::default().is_breached());
}

#[tokio::test]
async fn session_regexp_invalid_pattern_errors_and_records_nothing() {
    let h = harness(names(&["bash"]));
    let text = err_text(run(&h, "SELECT regexp(name, '(unclosed') FROM processes").await);
    assert!(text.to_lowercase().contains("regexp"), "{text}");
    assert!(h.sink.drain().is_empty());
    assert!(!h.cache.is_cached("(unclosed"));
}

#[tokio::test]
async fn session_regexp_refuses_a_non_literal_pattern() {
    let h = harness(names(&["bash"]));
    let text = err_text(run(&h, "SELECT regexp(name, name) FROM processes").await);
    assert!(text.to_lowercase().contains("literal"), "{text}");
}

// --- function semantics ------------------------------------------------------------------------

#[tokio::test]
async fn session_length_counts_chars_not_bytes() {
    let h = harness(names(&["h\u{e9}llo\u{1f980}"]));
    assert_eq!(
        one_i64(&h, "SELECT length(name) FROM processes").await,
        Some(6)
    );
    assert_eq!(one_i64(&h, "SELECT length('abc')").await, Some(3));
    assert_eq!(one_i64(&h, "SELECT length('')").await, Some(0));
}

#[tokio::test]
async fn session_instr_is_one_based_in_chars_and_zero_when_absent() {
    let h = harness(names(&["bash"]));
    assert_eq!(one_i64(&h, "SELECT instr('abc','c')").await, Some(3));
    assert_eq!(one_i64(&h, "SELECT instr('abc','z')").await, Some(0));
    // Empty needle is found at position 1, as in SQLite and MySQL.
    assert_eq!(one_i64(&h, "SELECT instr('abc','')").await, Some(1));
    assert_eq!(one_i64(&h, "SELECT instr('','')").await, Some(1));
    // 'e-acute' is 2 bytes; the char position of 'l' is 3, its byte position would be 4.
    assert_eq!(one_i64(&h, "SELECT instr('h\u{e9}llo','l')").await, Some(3));
    assert_eq!(one_i64(&h, "SELECT instr('\u{1f980}x','x')").await, Some(2));
}

#[tokio::test]
async fn session_hex_matches_known_bytes_and_unhex_inverts_it() {
    let h = harness(names(&["bash"]));
    assert_eq!(
        one_string(&h, "SELECT hex(unhex('deadbeef'))")
            .await
            .as_deref(),
        Some("deadbeef")
    );
    // Independent known value: 0x01 0xAB 0xFF, so hex is pinned without unhex.
    assert_eq!(
        one_string(&h, "SELECT hex(CAST(X'01ABFF' AS BYTEA))")
            .await
            .as_deref(),
        Some("01abff")
    );
    // And unhex pinned without hex.
    let batches = run(&h, "SELECT unhex('DEADbeef')").await.unwrap();
    let bin = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap()
        .clone();
    assert_eq!(bin.value(0), [0xde, 0xad, 0xbe, 0xef]);
}

#[tokio::test]
async fn session_unhex_rejects_odd_length_and_non_hex() {
    let h = harness(names(&["bash"]));
    let odd = err_text(run(&h, "SELECT unhex('abc')").await);
    assert!(odd.to_lowercase().contains("odd"), "{odd}");
    let bad = err_text(run(&h, "SELECT unhex('zz')").await);
    assert!(bad.to_lowercase().contains("hex"), "{bad}");
    let wide = err_text(run(&h, "SELECT unhex('\u{e9}\u{e9}')").await);
    assert!(wide.to_lowercase().contains("hex"), "{wide}");
}

#[tokio::test]
async fn session_like_delegates_to_the_arrow_kernel() {
    let h = harness(names(&["bash", "zsh", "b_sh"]));
    let batches = run(
        &h,
        "SELECT name FROM processes WHERE like(name, 'b%') ORDER BY name",
    )
    .await
    .unwrap();
    let col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .clone();
    assert_eq!(col.len(), 2);
    assert_eq!(
        one_bool(&h, "SELECT like('b_sh','b\\_sh')").await,
        Some(true)
    );
    assert_eq!(one_bool(&h, "SELECT like('abc','A%')").await, Some(false));
}

#[tokio::test]
async fn session_every_function_propagates_null() {
    let h = harness(vec![None]);
    let probes = [
        "SELECT length(name) FROM processes",
        "SELECT instr(name, 'a') FROM processes",
        "SELECT instr('a', name) FROM processes",
        "SELECT hex(CAST(name AS BYTEA)) FROM processes",
        "SELECT unhex(name) FROM processes",
        "SELECT like(name, 'a') FROM processes",
        "SELECT like('a', name) FROM processes",
        "SELECT length(NULL) FROM processes",
        "SELECT regexp(name, 'a') FROM processes",
        "SELECT match(name, 'a') FROM processes",
        "SELECT regexp('a', CAST(NULL AS VARCHAR)) FROM processes",
    ];
    for sql in probes {
        let batches = run(&h, sql).await.unwrap();
        assert_eq!(batches[0].num_rows(), 1);
        assert!(batches[0].column(0).is_null(0), "{sql}");
    }
}

#[tokio::test]
async fn session_wrong_arity_and_wrong_types_fail_at_planning() {
    let h = harness(names(&["bash"]));
    for sql in [
        "SELECT length() FROM processes",
        "SELECT length(name, name) FROM processes",
        "SELECT instr(name) FROM processes",
        "SELECT hex() FROM processes",
        "SELECT unhex(name, name) FROM processes",
        "SELECT like(name) FROM processes",
        "SELECT regexp(name) FROM processes",
        "SELECT regexp(name, 'a', 'b') FROM processes",
        "SELECT length(pid) FROM processes",
        "SELECT instr(pid, 'a') FROM processes",
        "SELECT unhex(pid) FROM processes",
        "SELECT hex(name) FROM processes",
        "SELECT hex(pid) FROM processes",
        "SELECT regexp(pid, 'a') FROM processes",
        "SELECT like(pid, 'a') FROM processes",
    ] {
        assert!(run(&h, sql).await.is_err(), "{sql}");
    }
}
