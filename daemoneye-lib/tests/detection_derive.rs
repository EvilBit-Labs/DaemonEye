//! Plan derivation: a `CompiledRule` plus a cycle window becomes a `DataFusion` `DataFrame`
//! (U7; R2, R3, R5). The R27 rule-load tests live in `detection_sql_validation.rs`.
//!
//! Split from `detection_execution.rs`, which U6 owns for the session and U8 extends for the
//! executor. These tests share none of its harness: they need a typed catalog and a provider with
//! the real column types, not the session tests' two-column `MemTable`.
#![cfg(feature = "detection-engine")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use daemoneye_lib::detection::catalog::{SchemaCatalog, verify_spawn_token};
use daemoneye_lib::detection::execution::derive::{
    CycleWindow, DeriveError, derive, derive_from_parts, predicate_to_expr, rewrite_residual,
};
use daemoneye_lib::detection::execution::session::{ExecutorRuntime, LatencySink, session_state};
use daemoneye_lib::detection::{CompiledRule, RegexCache, plan_rule};
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, Literal, Predicate, PredicateOp, PushdownPlan, SchemaDescriptor,
    TableDescriptor, literal::Value as LiteralValue,
};
use datafusion::arrow::array::{Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result as DfResult;
use datafusion::execution::context::{SessionContext, SessionState};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use parking_lot::Mutex;

// --- fixtures ----------------------------------------------------------------------------------

/// One stored row of the test table.
#[derive(Clone)]
struct StoredRow {
    pid: u64,
    name: Option<&'static str>,
    cols: [i64; 4],
    collection_time: i64,
}

fn test_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pid", DataType::UInt64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("a", DataType::Int64, false),
        Field::new("b", DataType::Int64, false),
        Field::new("c", DataType::Int64, false),
        Field::new("d", DataType::Int64, false),
        Field::new("collection_time", DataType::Int64, false),
    ]))
}

fn mem_table(rows: &[StoredRow]) -> MemTable {
    let schema = test_schema();
    let int_col = |index: usize| {
        Arc::new(Int64Array::from(
            rows.iter().map(|r| r.cols[index]).collect::<Vec<_>>(),
        ))
    };
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(UInt64Array::from(
                rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.name).collect::<Vec<_>>(),
            )),
            int_col(0),
            int_col(1),
            int_col(2),
            int_col(3),
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.collection_time).collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap();
    MemTable::try_new(schema, vec![vec![batch]]).unwrap()
}

fn named_rows(entries: &[(u64, &'static str, i64)]) -> Vec<StoredRow> {
    entries
        .iter()
        .map(|&(pid, name, collection_time)| StoredRow {
            pid,
            name: Some(name),
            cols: [0; 4],
            collection_time,
        })
        .collect()
}

fn ctx() -> SessionContext {
    let state = session_state(
        ExecutorRuntime::new().unwrap().env(),
        Arc::new(LatencySink::default()),
        Arc::new(RegexCache::new()),
    )
    .unwrap();
    SessionContext::new_with_state(state)
}

fn state() -> SessionState {
    ctx().state()
}

fn column(name: &str, column_type: ColumnType, ops: &[PredicateOp]) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(column_type),
        nullable: false,
        supported_ops: ops.iter().copied().map(i32::from).collect(),
    }
}

/// A catalog for `processes`; every `(column, op)` advertised is also conformance-passed, so the
/// ops listed decide exactly what the planner pushes.
fn catalog(columns: Vec<ColumnDescriptor>) -> SchemaCatalog {
    let token = "a".repeat(64);
    let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
    let mut catalog = SchemaCatalog::new();
    let pairs: Vec<(String, i32)> = columns
        .iter()
        .flat_map(|c| c.supported_ops.iter().map(|op| (c.name.clone(), *op)))
        .collect();
    catalog
        .register(
            &verified,
            SchemaDescriptor {
                collector_id: "procmond".to_owned(),
                descriptor_version: "v1".to_owned(),
                tables: vec![TableDescriptor {
                    name: "processes".to_owned(),
                    columns,
                }],
                conformance_results: Vec::new(),
            },
        )
        .unwrap();
    for (name, op) in pairs {
        catalog.record_conformance_pass(
            "procmond",
            "processes",
            &name,
            PredicateOp::try_from(op).unwrap(),
        );
    }
    catalog
}

fn int_catalog() -> SchemaCatalog {
    let ops = [
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::Lt,
        PredicateOp::Le,
        PredicateOp::Gt,
        PredicateOp::Ge,
    ];
    catalog(
        support::PROPERTY_COLUMNS
            .iter()
            .map(|n| column(n, ColumnType::Int, &ops))
            .collect(),
    )
}

/// `name` advertises no op, so every predicate on it stays residual.
fn residual_only_catalog() -> SchemaCatalog {
    catalog(vec![
        column("pid", ColumnType::Uint, &[]),
        column("name", ColumnType::String, &[]),
    ])
}

fn compile(catalog: &SchemaCatalog, sql: &str) -> CompiledRule {
    let rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Derive test".to_owned(),
        "Derive test rule".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::Low,
    );
    plan_rule(catalog, &RegexCache::new(), &rule, 3).unwrap()
}

const WIDE: CycleWindow = CycleWindow {
    after_ms: 0,
    through_ms: 1_000_000,
};

async fn pids(df: datafusion::prelude::DataFrame) -> Vec<u64> {
    let mut out = Vec::new();
    for batch in df.collect().await.unwrap() {
        let column = batch.column_by_name("pid").expect("pid projected");
        let array = column.as_any().downcast_ref::<UInt64Array>().unwrap();
        out.extend((0..array.len()).map(|i| array.value(i)));
    }
    out.sort_unstable();
    out
}

fn string_literal(text: &str) -> Literal {
    Literal {
        value: Some(LiteralValue::StringValue(text.to_owned())),
    }
}

fn predicate(column: &str, op: PredicateOp, values: Vec<Literal>) -> Predicate {
    Predicate {
        column: column.to_owned(),
        op: i32::from(op),
        values,
    }
}

// --- rewrite_residual: the pinned text ---------------------------------------------------------

#[test]
fn derive_rewrites_regexp_operator_to_the_function_form() {
    assert_eq!(
        rewrite_residual("(name REGEXP '^ba')").unwrap(),
        "(regexp(name, '^ba'))"
    );
    assert_eq!(
        rewrite_residual("(name RLIKE '^ba')").unwrap(),
        "(regexp(name, '^ba'))"
    );
}

#[test]
fn derive_rewrites_a_negated_regexp_to_not_around_the_call() {
    assert_eq!(
        rewrite_residual("(name NOT REGEXP 'x')").unwrap(),
        "(NOT regexp(name, 'x'))"
    );
    // The spelling with an explicit NOT keeps its own parentheses around the rewritten call.
    assert_eq!(
        rewrite_residual("(NOT (name REGEXP 'x'))").unwrap(),
        "(NOT (regexp(name, 'x')))"
    );
}

#[test]
fn derive_leaves_an_already_functional_regexp_unchanged() {
    assert_eq!(
        rewrite_residual("(regexp(name, '^alpha$'))").unwrap(),
        "(regexp(name, '^alpha$'))"
    );
}

#[test]
fn derive_keeps_the_parentheses_of_a_disjunction() {
    assert_eq!(
        rewrite_residual("(a = 1 OR b = 2)").unwrap(),
        "(a = 1 OR b = 2)"
    );
    assert_eq!(
        rewrite_residual("(a = 1 OR b = 2) AND c = 3").unwrap(),
        "(a = 1 OR b = 2) AND c = 3"
    );
}

#[test]
fn derive_rewrites_a_regexp_nested_inside_a_disjunction() {
    assert_eq!(
        rewrite_residual("(a = 1 OR name REGEXP 'x')").unwrap(),
        "(a = 1 OR regexp(name, 'x'))"
    );
}

#[test]
fn derive_refuses_a_residual_that_is_not_one_expression() {
    for fragment in ["a = 1; DROP TABLE processes", "a = ", "a = 1 ORDER BY a"] {
        assert!(
            matches!(
                rewrite_residual(fragment),
                Err(DeriveError::ResidualParse { .. })
            ),
            "{fragment}"
        );
    }
}

// --- predicate_to_expr -------------------------------------------------------------------------

#[test]
fn derive_plans_an_in_predicate_with_three_literals_as_an_in_list() {
    let p = predicate(
        "name",
        PredicateOp::In,
        vec![
            string_literal("a"),
            string_literal("b"),
            string_literal("c"),
        ],
    );
    let expr = predicate_to_expr(&state(), &p).unwrap();
    let Expr::InList(list) = expr else {
        panic!("expected an IN list, got {expr:?}");
    };
    assert_eq!(list.list.len(), 3);
    assert!(!list.negated);
}

#[test]
fn derive_refuses_an_unspecified_or_unknown_op_rather_than_reading_it_as_equality() {
    let mut p = predicate("pid", PredicateOp::Unspecified, vec![string_literal("x")]);
    assert!(matches!(
        predicate_to_expr(&state(), &p),
        Err(DeriveError::UnspecifiedOp { ref column }) if column == "pid"
    ));
    p.op = 9_999;
    assert!(matches!(
        predicate_to_expr(&state(), &p),
        Err(DeriveError::UnspecifiedOp { .. })
    ));
}

#[test]
fn derive_refuses_a_predicate_with_the_wrong_number_of_values() {
    let none = predicate("pid", PredicateOp::Eq, vec![]);
    let two = predicate(
        "name",
        PredicateOp::Like,
        vec![string_literal("a"), string_literal("b")],
    );
    let empty_in = predicate("name", PredicateOp::In, vec![]);
    for p in [none, two, empty_in] {
        assert!(
            matches!(
                predicate_to_expr(&state(), &p),
                Err(DeriveError::BadPredicate { .. })
            ),
            "{p:?}"
        );
    }
}

#[test]
fn derive_refuses_a_literal_with_no_value() {
    let p = predicate("pid", PredicateOp::Eq, vec![Literal { value: None }]);
    assert!(matches!(
        predicate_to_expr(&state(), &p),
        Err(DeriveError::BadPredicate { .. })
    ));
}

#[tokio::test]
async fn derive_a_null_literal_matches_no_row_as_sql_null_does() {
    let c = ctx();
    // An empty name is the literal a NULL could silently degrade into; it must not match either.
    let provider: Arc<dyn datafusion::datasource::TableProvider> =
        Arc::new(mem_table(&named_rows(&[(1, "bash", 10), (2, "", 11)])));
    let control = predicate("name", PredicateOp::Eq, vec![string_literal("bash")]);
    let expr = predicate_to_expr(&c.state(), &control).unwrap();
    let df = c
        .read_table(Arc::clone(&provider))
        .unwrap()
        .filter(expr)
        .unwrap();
    assert_eq!(
        pids(df).await,
        vec![1],
        "the control literal matches its row"
    );

    let null = Literal {
        value: Some(LiteralValue::NullValue(true)),
    };
    let null_expr =
        predicate_to_expr(&c.state(), &predicate("name", PredicateOp::Eq, vec![null])).unwrap();
    let null_df = c.read_table(provider).unwrap().filter(null_expr).unwrap();
    assert!(pids_or_empty(null_df).await.is_empty());
}

async fn pids_or_empty(df: datafusion::prelude::DataFrame) -> Vec<u64> {
    pids(df).await
}

// --- derive: window, pushed, residual, projection, cap -----------------------------------------

#[tokio::test]
async fn derive_the_window_is_half_open_after_exclusive_through_inclusive() {
    let c = ctx();
    let rows = named_rows(&[
        (1, "bash", 200),
        (2, "bash", 201),
        (3, "bash", 600),
        (4, "bash", 601),
    ]);
    let compiled = compile(&residual_only_catalog(), "SELECT pid FROM processes");
    let window = CycleWindow {
        after_ms: 200,
        through_ms: 600,
    };
    let df = derive(&c, Arc::new(mem_table(&rows)), &compiled, window, 100).unwrap();
    assert_eq!(pids(df).await, vec![2, 3]);
}

#[tokio::test]
async fn derive_a_row_in_one_window_is_not_in_the_next() {
    let c = ctx();
    let rows = named_rows(&[(1, "bash", 500)]);
    let compiled = compile(&residual_only_catalog(), "SELECT pid FROM processes");
    let first = CycleWindow {
        after_ms: 400,
        through_ms: 600,
    };
    let second = CycleWindow {
        after_ms: 600,
        through_ms: 800,
    };
    let table: Arc<dyn TableProvider> = Arc::new(mem_table(&rows));
    let in_first = derive(&c, Arc::clone(&table), &compiled, first, 100).unwrap();
    assert_eq!(pids(in_first).await, vec![1]);
    let in_second = derive(&c, table, &compiled, second, 100).unwrap();
    assert!(pids(in_second).await.is_empty());
}

#[tokio::test]
async fn derive_a_window_bound_beyond_i64_saturates_instead_of_wrapping() {
    let c = ctx();
    let rows = named_rows(&[(1, "bash", i64::MAX)]);
    let compiled = compile(&residual_only_catalog(), "SELECT pid FROM processes");
    let window = CycleWindow {
        after_ms: 0,
        through_ms: u64::MAX,
    };
    let df = derive(&c, Arc::new(mem_table(&rows)), &compiled, window, 100).unwrap();
    assert_eq!(pids(df).await, vec![1]);
}

#[tokio::test]
async fn derive_the_residual_regexp_operator_filters_rows() {
    let c = ctx();
    let rows = named_rows(&[(1, "bash", 10), (2, "zsh", 10), (3, "bat", 10)]);
    let compiled = compile(
        &residual_only_catalog(),
        "SELECT pid FROM processes WHERE name REGEXP '^ba'",
    );
    assert!(compiled.residual().unwrap().contains("REGEXP"));
    let matching = derive(&c, Arc::new(mem_table(&rows)), &compiled, WIDE, 100).unwrap();
    assert_eq!(pids(matching).await, vec![1, 3]);

    let negated = compile(
        &residual_only_catalog(),
        "SELECT pid FROM processes WHERE name NOT REGEXP '^ba'",
    );
    let rest = derive(&c, Arc::new(mem_table(&rows)), &negated, WIDE, 100).unwrap();
    assert_eq!(pids(rest).await, vec![2]);
}

#[tokio::test]
async fn derive_a_pushed_regexp_predicate_uses_the_regexp_udf() {
    let c = ctx();
    let rows = named_rows(&[(1, "bash", 10), (2, "zsh", 10)]);
    let catalog = catalog(vec![
        column("pid", ColumnType::Uint, &[]),
        column("name", ColumnType::String, &[PredicateOp::Regexp]),
    ]);
    let compiled = compile(
        &catalog,
        "SELECT pid FROM processes WHERE name REGEXP '^ba'",
    );
    assert_eq!(compiled.plan().predicates.len(), 1, "the regexp is pushed");
    assert!(compiled.residual().is_none());
    let df = derive(&c, Arc::new(mem_table(&rows)), &compiled, WIDE, 100).unwrap();
    assert_eq!(pids(df).await, vec![1]);
}

#[tokio::test]
async fn derive_pushed_like_and_in_predicates_filter_rows() {
    let c = ctx();
    let rows = named_rows(&[(1, "bash", 10), (2, "zsh", 10), (3, "fish", 10)]);
    let catalog = catalog(vec![
        column("pid", ColumnType::Uint, &[]),
        column(
            "name",
            ColumnType::String,
            &[PredicateOp::Like, PredicateOp::In],
        ),
    ]);
    let table: Arc<dyn TableProvider> = Arc::new(mem_table(&rows));
    let like = compile(&catalog, "SELECT pid FROM processes WHERE name LIKE 'b%'");
    assert_eq!(like.plan().predicates.len(), 1);
    let liked = derive(&c, Arc::clone(&table), &like, WIDE, 100).unwrap();
    assert_eq!(pids(liked).await, vec![1]);

    let in_list = compile(
        &catalog,
        "SELECT pid FROM processes WHERE name IN ('zsh', 'fish', 'nope')",
    );
    assert_eq!(in_list.plan().predicates.len(), 1);
    let listed = derive(&c, table, &in_list, WIDE, 100).unwrap();
    assert_eq!(pids(listed).await, vec![2, 3]);
}

/// AE8's executor half. `plan_rule` refuses a rule calling `md5` (it validates the SQL first), so
/// this drives derivation from parts, which is the only way an unvetted residual can arrive.
#[tokio::test]
async fn derive_a_residual_calling_md5_fails_naming_md5() {
    let c = ctx();
    let plan = PushdownPlan {
        table: "processes".to_owned(),
        predicates: vec![],
        projection: vec![],
        ttl_ms: 0,
    };
    let error = derive_from_parts(
        &c,
        Arc::new(mem_table(&named_rows(&[(1, "bash", 10)]))),
        &plan,
        Some("md5(name) = 'x'"),
        WIDE,
        100,
    )
    .expect_err("md5 is not registered, so the residual must not plan");
    assert!(matches!(error, DeriveError::Plan(_)), "{error:?}");
    assert!(error.to_string().to_lowercase().contains("md5"), "{error}");
}

#[tokio::test]
async fn derive_projection_keeps_only_the_columns_the_rule_needs() {
    let c = ctx();
    let compiled = compile(
        &residual_only_catalog(),
        "SELECT pid FROM processes WHERE name = 'bash'",
    );
    let df = derive(
        &c,
        Arc::new(mem_table(&named_rows(&[(1, "bash", 10)]))),
        &compiled,
        WIDE,
        100,
    )
    .unwrap();
    let fields: Vec<String> = df
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(fields, compiled.plan().projection);
    assert_eq!(fields, vec!["name".to_owned(), "pid".to_owned()]);
}

#[tokio::test]
async fn derive_returns_cap_plus_one_rows_so_the_executor_can_tell_over_from_exact() {
    let c = ctx();
    let rows: Vec<_> = (1..=5).map(|pid| (pid, "bash", 10)).collect();
    let table: Arc<dyn TableProvider> = Arc::new(mem_table(&named_rows(&rows)));
    let compiled = compile(&residual_only_catalog(), "SELECT pid FROM processes");
    for (cap, expected) in [(3_u32, 4_usize), (5, 5), (4, 5), (10, 5)] {
        let df = derive(&c, Arc::clone(&table), &compiled, WIDE, cap).unwrap();
        assert_eq!(pids(df).await.len(), expected, "cap {cap}");
    }
}

#[tokio::test]
async fn derive_explain_shows_the_window_one_filter_and_the_cap_plus_one() {
    let c = ctx();
    let compiled = compile(&residual_only_catalog(), "SELECT pid FROM processes");
    let window = CycleWindow {
        after_ms: 200,
        through_ms: 600,
    };
    let df = derive(
        &c,
        Arc::new(mem_table(&named_rows(&[]))),
        &compiled,
        window,
        3,
    )
    .unwrap();
    let batches = df.explain(false, false).unwrap().collect().await.unwrap();
    let mut text = String::new();
    for batch in &batches {
        for column in batch.columns() {
            if let Some(strings) = column.as_any().downcast_ref::<StringArray>() {
                for i in 0..strings.len() {
                    text.push_str(strings.value(i));
                    text.push('\n');
                }
            }
        }
    }
    assert!(text.contains("collection_time > Int64(200)"), "{text}");
    assert!(text.contains("collection_time <= Int64(600)"), "{text}");
    assert_eq!(text.matches("FilterExec").count(), 1, "{text}");
    assert!(text.contains("fetch=4"), "{text}");
}

// --- what the provider is actually handed ------------------------------------------------------

/// A provider that records the filters `scan` receives, so the shapes can be inspected after the
/// optimizer has run, which is what `EventStoreTableProvider::supports_filters_pushdown` sees.
#[derive(Debug)]
struct Recording {
    inner: MemTable,
    seen: Mutex<Vec<Expr>>,
}

#[async_trait]
impl TableProvider for Recording {
    fn schema(&self) -> SchemaRef {
        self.inner.schema()
    }
    fn table_type(&self) -> TableType {
        TableType::Base
    }
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> DfResult<Vec<TableProviderFilterPushDown>> {
        Ok(vec![TableProviderFilterPushDown::Inexact; filters.len()])
    }
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        self.seen.lock().extend(filters.iter().cloned());
        self.inner.scan(state, projection, filters, limit).await
    }
}

/// Unsized coercion in a function body: `Arc::clone(&x)` would infer `Arc<dyn _>` from the target.
fn recording_provider(recording: &Arc<Recording>) -> Arc<dyn TableProvider> {
    let provider: Arc<Recording> = Arc::clone(recording);
    provider
}

/// The `IN` list has four values on purpose: `DataFusion`'s simplifier rewrites an `IN` of three
/// or fewer into `x = a OR x = b`, which `filters.rs` does not recognise, so a short list is
/// still correct (the `FilterExec` judges it) but is not pruned by the provider.
///
/// Confirms the claim that matters for pruning: the window and the pushed predicates reach the
/// provider as one conjunct each, in the `column op literal` and `column IN (literals)` shapes
/// `provider/filters.rs` recognises, with literals of the column's own Arrow type.
#[tokio::test]
async fn derive_window_and_pushed_filters_reach_the_provider_in_pushable_shapes() {
    let c = ctx();
    let catalog = catalog(vec![
        column("pid", ColumnType::Uint, &[PredicateOp::Eq]),
        column("name", ColumnType::String, &[PredicateOp::In]),
    ]);
    let compiled = compile(
        &catalog,
        "SELECT pid FROM processes WHERE pid = 7 AND name IN ('a', 'b', 'c', 'd')",
    );
    assert_eq!(compiled.plan().predicates.len(), 2);
    let recording = Arc::new(Recording {
        inner: mem_table(&named_rows(&[(7, "a", 300)])),
        seen: Mutex::new(Vec::new()),
    });
    let provider = recording_provider(&recording);
    let window = CycleWindow {
        after_ms: 200,
        through_ms: 600,
    };
    let df = derive(&c, provider, &compiled, window, 10).unwrap();
    assert_eq!(pids(df).await, vec![7]);

    let rendered: Vec<String> = recording.seen.lock().iter().map(Expr::to_string).collect();
    for want in [
        "collection_time > Int64(200)",
        "collection_time <= Int64(600)",
        "pid = UInt64(7)",
        "name IN ([Utf8(\"a\"), Utf8(\"b\"), Utf8(\"c\"), Utf8(\"d\")])",
    ] {
        assert!(
            rendered.iter().any(|r| r == want),
            "{want} not among {rendered:?}"
        );
    }
    for expr in recording.seen.lock().iter() {
        assert!(
            matches!(expr, Expr::BinaryExpr(b) if b.op != datafusion::logical_expr::Operator::And)
                || matches!(expr, Expr::InList(_)),
            "a conjunction reached the provider unsplit: {expr:?}"
        );
    }
}

// --- parity: the derived plan admits exactly what the reference evaluator admits ---------------

/// Each fixture row is stored four times, at `collection_time` 200 and 601 (outside the window)
/// and 500 and 600 (inside it), so the window is exercised at both of its edges.
fn parity_rows() -> Vec<StoredRow> {
    let mut out = Vec::new();
    for row in &support::rows_fixture() {
        for collection_time in [200_i64, 500, 600, 601] {
            out.push(StoredRow {
                pid: u64::try_from(out.len()).unwrap(),
                name: None,
                cols: [row.get("a"), row.get("b"), row.get("c"), row.get("d")],
                collection_time,
            });
        }
    }
    out
}

async fn admitted_tuples(df: datafusion::prelude::DataFrame) -> Vec<[i64; 4]> {
    let mut out = Vec::new();
    for batch in df.collect().await.unwrap() {
        let get = |name: &str| {
            let column = batch.column_by_name(name).expect("a..d projected");
            column
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .to_vec()
        };
        let (a, b, c, d) = (get("a"), get("b"), get("c"), get("d"));
        out.extend((0..batch.num_rows()).map(|i| [a[i], b[i], c[i], d[i]]));
    }
    out.sort_unstable();
    out
}

#[test]
fn derive_admits_exactly_the_rows_the_reference_evaluator_admits_within_the_window() {
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use support::{Row, eval_sql_predicate, pred_strategy, render, rows_fixture};

    const CASES: u32 = 256;
    // Floors for the non-vacuity checks below: half the cases match a row, an eighth push a
    // predicate, a quarter keep a residual.
    const MIN_MATCHED: u32 = 128;
    const MIN_PUSHED: u32 = 32;
    const MIN_RESIDUAL: u32 = 64;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let catalog = int_catalog();
    let fixture_rows = rows_fixture();
    let table: Arc<dyn TableProvider> = Arc::new(mem_table(&parity_rows()));
    let window = CycleWindow {
        after_ms: 200,
        through_ms: 600,
    };
    let non_empty = AtomicU32::new(0);
    let with_pushed_half = AtomicU32::new(0);
    let with_residual_half = AtomicU32::new(0);
    let mut config = ProptestConfig::with_cases(CASES);
    config.failure_persistence = None;

    proptest!(config, |(generated in pred_strategy())| {
        let where_sql = render(&generated);
        let sql = format!("SELECT a, b, c, d FROM processes WHERE {where_sql}");
        let compiled = compile(&catalog, &sql);

        // Two in-window copies of every fixture row satisfying the whole predicate.
        let mut expected: Vec<[i64; 4]> = fixture_rows
            .iter()
            .filter(|row: &&Row| eval_sql_predicate(&where_sql, row))
            .flat_map(|row| {
                let tuple = [row.get("a"), row.get("b"), row.get("c"), row.get("d")];
                [tuple, tuple]
            })
            .collect();
        expected.sort_unstable();

        let df = derive(&ctx(), Arc::clone(&table), &compiled, window, 100_000).unwrap();
        let actual = runtime.block_on(admitted_tuples(df));

        if !expected.is_empty() {
            non_empty.fetch_add(1, Ordering::Relaxed);
        }
        if !compiled.plan().predicates.is_empty() {
            with_pushed_half.fetch_add(1, Ordering::Relaxed);
        }
        if compiled.residual().is_some() {
            with_residual_half.fetch_add(1, Ordering::Relaxed);
        }
        prop_assert_eq!(
            actual,
            expected,
            "predicate {} (plan {:?}, residual {:?})",
            where_sql,
            compiled.plan().predicates,
            compiled.residual()
        );
    });

    // Without these the property could pass by comparing empty to empty, or by never splitting.
    let matched = non_empty.load(Ordering::Relaxed);
    let pushed = with_pushed_half.load(Ordering::Relaxed);
    let residual = with_residual_half.load(Ordering::Relaxed);
    assert!(
        matched >= MIN_MATCHED,
        "only {matched}/{CASES} cases matched a row"
    );
    assert!(
        pushed >= MIN_PUSHED,
        "only {pushed}/{CASES} cases had a pushed half"
    );
    assert!(
        residual >= MIN_RESIDUAL,
        "only {residual}/{CASES} cases had a residual"
    );
}

#[tokio::test]
async fn derive_a_known_split_predicate_admits_the_expected_in_window_rows() {
    let c = ctx();
    let compiled = compile(
        &int_catalog(),
        "SELECT a, b, c, d FROM processes WHERE a = 1 AND (b > 1 OR c = 0)",
    );
    assert_eq!(compiled.plan().predicates.len(), 1, "a = 1 is pushed");
    assert!(compiled.residual().is_some(), "the disjunction stays");
    let window = CycleWindow {
        after_ms: 200,
        through_ms: 600,
    };
    let df = derive(
        &c,
        Arc::new(mem_table(&parity_rows())),
        &compiled,
        window,
        1000,
    )
    .unwrap();
    let tuples = admitted_tuples(df).await;
    // a = 1 and (b > 1 or c = 0): b in {2, 3}, or (1 + b) % 4 == 0 i.e. b == 3, so b in {2, 3}.
    assert_eq!(
        tuples.len(),
        2 * 2,
        "two fixture rows, two in-window copies"
    );
    assert!(tuples.iter().all(|t| t[0] == 1 && (t[1] > 1 || t[2] == 0)));
}
