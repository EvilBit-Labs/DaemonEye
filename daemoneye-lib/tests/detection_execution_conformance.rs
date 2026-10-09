//! `DataFusion` parity with the conformance reference (U11; R17).
//!
//! Every collector was certified against `reference_outcome`, and the agent re-applies pushed
//! predicates over stored rows through `DataFusion`. This file runs each applicable corpus case
//! through the real executor session (a one-row `MemTable`, the plan built by `predicate_to_expr`)
//! and compares "the row came back" with the reference. A divergence is fixed in the UDFs or in
//! `derive.rs`, never in the reference.
//!
//! One test per `ConformanceAxis`, so a failure is named by the test that failed. Cases the
//! reference `Refuses` are static plan defects the planner rejects before any row exists; there is
//! no executor outcome to compare and they are counted separately.
#![cfg(feature = "detection-engine")]
// `ColumnType`, `literal::Value` and the outcome enum are `#[non_exhaustive]` to this crate, so a
// wildcard arm is mandatory and `wildcard_enum_match_arm` cannot be satisfied.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm
)]

use std::sync::Arc;

use daemoneye_lib::detection::RegexCache;
use daemoneye_lib::detection::conformance::{
    ConformanceAxis, ConformanceCase, ConformanceOutcome, cases_for, reference_outcome,
};
use daemoneye_lib::detection::execution::derive::predicate_to_expr;
use daemoneye_lib::detection::execution::session::{ExecutorRuntime, LatencySink, session_state};
use daemoneye_lib::proto::{ColumnType, Literal, Predicate, PredicateOp, literal};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::execution::context::SessionContext;
use datafusion::scalar::ScalarValue;

const COLUMN: &str = "c";
const COLUMN_TYPES: [ColumnType; 5] = [
    ColumnType::String,
    ColumnType::Int,
    ColumnType::Uint,
    ColumnType::Float,
    ColumnType::Bool,
];
const OPS: [PredicateOp; 9] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::In,
    PredicateOp::Like,
    PredicateOp::Regexp,
];

/// What one axis run saw.
#[derive(Debug, Default)]
struct Tally {
    /// Pairs compared against the executor.
    exercised: usize,
    /// Of those, how many reference outcomes were `Admits` (positive controls).
    admitted: usize,
    /// Of those, how many were `Excludes`.
    excluded: usize,
    /// Pairs the reference refuses; not compared.
    refused: usize,
    /// Pairs where the executor and the reference disagreed.
    diverged: usize,
    /// The diverging cases, printed on failure so the log names them.
    divergent_cases: Vec<String>,
}

fn arrow_type(column_type: ColumnType) -> DataType {
    match column_type {
        ColumnType::String => DataType::Utf8,
        ColumnType::Int => DataType::Int64,
        ColumnType::Uint => DataType::UInt64,
        ColumnType::Float => DataType::Float64,
        ColumnType::Bool => DataType::Boolean,
        _ => panic!("corpus never uses an unspecified column type"),
    }
}

fn scalar(value: &literal::Value) -> ScalarValue {
    match *value {
        literal::Value::StringValue(ref text) => ScalarValue::Utf8(Some(text.clone())),
        literal::Value::IntValue(n) => ScalarValue::Int64(Some(n)),
        literal::Value::UintValue(n) => ScalarValue::UInt64(Some(n)),
        literal::Value::FloatValue(n) => ScalarValue::Float64(Some(n)),
        literal::Value::BoolValue(b) => ScalarValue::Boolean(Some(b)),
        literal::Value::NullValue(_marker) => ScalarValue::Null,
        _ => panic!("corpus carries a literal kind this test cannot build"),
    }
}

/// Whether the executor returns the case's one row.
async fn executor_returns_row(case: &ConformanceCase, nullable: bool) -> bool {
    let data_type = arrow_type(case.column_type);
    let cell = case
        .observed
        .as_ref()
        .map_or_else(|| ScalarValue::try_from(&data_type).unwrap(), scalar);
    let schema = Arc::new(Schema::new(vec![Field::new(COLUMN, data_type, nullable)]));
    let batch = RecordBatch::try_new(Arc::clone(&schema), vec![cell.to_array().unwrap()]).unwrap();
    let table = MemTable::try_new(Arc::clone(&schema), vec![vec![batch]]).unwrap();

    let runtime = ExecutorRuntime::new().unwrap().env();
    let state = session_state(
        runtime,
        Arc::new(LatencySink::default()),
        Arc::new(RegexCache::new()),
    )
    .unwrap();
    let predicate = Predicate {
        column: COLUMN.to_owned(),
        op: case.op.into(),
        values: case
            .literals
            .iter()
            .map(|value| Literal {
                value: Some(value.clone()),
            })
            .collect(),
    };
    let expr = predicate_to_expr(&state, &predicate).unwrap();
    let ctx = SessionContext::new_with_state(state);
    let frame = ctx
        .read_table(Arc::new(table))
        .unwrap()
        .filter(expr)
        .unwrap();
    frame.count().await.unwrap() == 1
}

/// Compare the executor with `reference` over every applicable pair on `axis`.
async fn compare_axis<R>(axis: ConformanceAxis, reference: R) -> Tally
where
    R: Fn(&ConformanceCase) -> ConformanceOutcome,
{
    let mut tally = Tally::default();
    for column_type in COLUMN_TYPES {
        for nullable in [false, true] {
            for op in OPS {
                for case in cases_for(column_type, nullable, op).filter(|c| c.axis == axis) {
                    let expected = reference(case);
                    if expected == ConformanceOutcome::Refuses {
                        tally.refused = tally.refused.saturating_add(1);
                        continue;
                    }
                    tally.exercised = tally.exercised.saturating_add(1);
                    let admits = expected == ConformanceOutcome::Admits;
                    if admits {
                        tally.admitted = tally.admitted.saturating_add(1);
                    } else {
                        tally.excluded = tally.excluded.saturating_add(1);
                    }
                    if executor_returns_row(case, nullable).await != admits {
                        tally.diverged = tally.diverged.saturating_add(1);
                        tally
                            .divergent_cases
                            .push(format!("{case:?} nullable={nullable}"));
                    }
                }
            }
        }
    }
    tally
}

/// Assert full agreement on one axis, with floors proving the run compared something.
async fn assert_axis_agrees(axis: ConformanceAxis, min_exercised: usize) {
    let tally = compare_axis(axis, reference_outcome).await;
    eprintln!(
        "exercised={} admitted={} excluded={} refused={} diverged={}\n{}",
        tally.exercised,
        tally.admitted,
        tally.excluded,
        tally.refused,
        tally.diverged,
        tally.divergent_cases.join("\n"),
    );
    assert_eq!(tally.diverged, 0, "executor disagrees with the reference");
    assert!(tally.exercised >= min_exercised, "too few pairs exercised");
    assert!(tally.admitted > 0, "no positive case matched");
    assert!(tally.excluded > 0, "no negative case matched");
}

#[tokio::test]
async fn null_axis_agrees_with_reference() {
    assert_axis_agrees(ConformanceAxis::Null, 32).await;
}

#[tokio::test]
async fn coercion_axis_agrees_with_reference() {
    // Every coercion case is a static plan defect: the reference refuses it and the planner
    // rejects it before a row exists, so there is no executor outcome to compare. `DataFusion`
    // itself would coerce these silently, which is why that refusal is load-bearing.
    let tally = compare_axis(ConformanceAxis::Coercion, reference_outcome).await;
    assert_eq!(tally.exercised, 0, "a coercion case produced an outcome");
    assert!(tally.refused >= 200, "too few coercion refusals counted");
}

#[tokio::test]
async fn collation_axis_agrees_with_reference() {
    assert_axis_agrees(ConformanceAxis::Collation, 24).await;
}

#[tokio::test]
async fn boundary_axis_agrees_with_reference() {
    assert_axis_agrees(ConformanceAxis::Boundary, 120).await;
}

/// Control: a reference that is wrong on purpose must be caught, or the suite compares nothing.
#[tokio::test]
async fn inverted_reference_diverges_on_every_pair() {
    let invert = |case: &ConformanceCase| match reference_outcome(case) {
        ConformanceOutcome::Admits => ConformanceOutcome::Excludes,
        ConformanceOutcome::Excludes => ConformanceOutcome::Admits,
        other => other,
    };
    let tally = compare_axis(ConformanceAxis::Null, invert).await;
    assert!(tally.exercised > 0);
    assert_eq!(tally.diverged, tally.exercised);
}

/// Control: the classic NULL mistake (`!=` over NULL admitting) is caught by the NULL axis.
#[tokio::test]
async fn null_ne_admitting_reference_diverges() {
    let sloppy = |case: &ConformanceCase| {
        if case.observed.is_none() && case.op == PredicateOp::Ne {
            ConformanceOutcome::Admits
        } else {
            reference_outcome(case)
        }
    };
    let tally = compare_axis(ConformanceAxis::Null, sloppy).await;
    assert!(tally.diverged > 0);
}

/// The behaviours the task names are in the exercised set, so "all agree" covers them.
#[test]
fn named_behaviours_are_in_the_corpus() {
    let any = |column_type, op, keep: &dyn Fn(&ConformanceCase) -> bool| {
        cases_for(column_type, true, op).any(keep)
    };
    let null_literal = |case: &ConformanceCase| {
        case.literals
            .iter()
            .any(|value| matches!(*value, literal::Value::NullValue(_marker)))
    };
    assert!(any(ColumnType::String, PredicateOp::Eq, &|c| c
        .observed
        .is_none()));
    assert!(any(ColumnType::String, PredicateOp::In, &null_literal));
    assert!(any(ColumnType::String, PredicateOp::Like, &|c| {
        reference_outcome(c) == ConformanceOutcome::Admits
    }));
    assert!(any(ColumnType::String, PredicateOp::Regexp, &|c| {
        reference_outcome(c) == ConformanceOutcome::Excludes
    }));
}

/// `NULL = NULL`, `NULL != NULL`, and a NULL on one side only: no match in either engine. The
/// corpus has no scalar NULL literal (the planner owns that), so these cases are built here.
#[tokio::test]
async fn null_on_either_side_of_a_comparison_matches_nothing() {
    let null = literal::Value::NullValue(true);
    let text = || literal::Value::StringValue("x".to_owned());
    for op in [
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::Lt,
        PredicateOp::Ge,
    ] {
        for (observed, literals) in [
            (None, vec![null.clone()]),
            (None, vec![text()]),
            (Some(text()), vec![null.clone()]),
        ] {
            let case = ConformanceCase {
                axis: ConformanceAxis::Null,
                column_type: ColumnType::String,
                op,
                observed,
                literals,
            };
            assert_eq!(reference_outcome(&case), ConformanceOutcome::Excludes);
            assert!(!executor_returns_row(&case, true).await);
        }
    }
}
