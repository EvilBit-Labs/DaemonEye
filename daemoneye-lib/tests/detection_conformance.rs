//! Conformance vectors: an advertised operation becomes pushable only once a vector passed for it
//! (R15, R22).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use daemoneye_lib::detection::RegexCache;
use daemoneye_lib::detection::catalog::{SchemaCatalog, VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::planner::plan_rule;
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, ConformanceResult, PredicateOp, SchemaDescriptor, TableDescriptor,
};

fn token() -> String {
    "a".repeat(64)
}

fn verified(collector_id: &str) -> VerifiedRegistration {
    verify_spawn_token(collector_id, Some(&token()), Some(&token())).unwrap()
}

fn result(column: &str, op: PredicateOp, passed: bool) -> ConformanceResult {
    ConformanceResult {
        table: "processes".to_owned(),
        column: column.to_owned(),
        op: i32::from(op),
        passed,
    }
}

/// A descriptor advertising `name` for `=` and `LIKE`, carrying the results named.
fn descriptor(results: Vec<ConformanceResult>) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "processes-v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![ColumnDescriptor {
                name: "name".to_owned(),
                column_type: i32::from(ColumnType::String),
                nullable: false,
                supported_ops: vec![i32::from(PredicateOp::Eq), i32::from(PredicateOp::Like)],
            }],
        }],
        conformance_results: results,
    }
}

fn rule(sql: &str) -> DetectionRule {
    DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Conformance test rule".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::Medium,
    )
}

const MIXED_RULE: &str = "SELECT name FROM processes WHERE name = 'x' AND name LIKE 'y%'";

#[test]
fn a_failing_vector_keeps_predicates_over_that_operation_in_the_residual() {
    // Arrange: `=` passed its vector, `LIKE` reported a failure. Both are advertised.
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![
                result("name", PredicateOp::Eq, true),
                result("name", PredicateOp::Like, false),
            ]),
        )
        .unwrap();

    // Act
    let compiled = plan_rule(&catalog, &RegexCache::new(), &rule(MIXED_RULE), 3).unwrap();

    // Assert: the passing operation is pushed, the failing one stays behind.
    let pushed: Vec<PredicateOp> = compiled
        .plan()
        .predicates
        .iter()
        .map(daemoneye_lib::proto::Predicate::op)
        .collect();
    assert_eq!(pushed, vec![PredicateOp::Eq], "only `=` may be pushed");
    let residual = compiled.residual().expect("LIKE stays in the residual");
    assert!(residual.contains("LIKE"), "residual: {residual}");
}

#[test]
fn an_operation_whose_vector_passed_is_pushable_and_one_with_no_result_is_not() {
    // Arrange: only `=` reports a result at all; `LIKE` reports nothing.
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![result("name", PredicateOp::Eq, true)]),
        )
        .unwrap();

    // Assert
    assert!(catalog.is_advertised("processes", "name", PredicateOp::Like));
    assert!(!catalog.is_conformance_passed("processes", "name", PredicateOp::Like));
    assert!(catalog.is_pushable("processes", "name", PredicateOp::Eq));
    assert!(!catalog.is_pushable("processes", "name", PredicateOp::Like));
}

#[test]
fn results_carried_on_the_descriptor_survive_the_registration_that_stored_it() {
    // Arrange + Act: the very registration that stores a descriptor evicts this collector's old
    // conformance entries. Results riding on that descriptor must land after the eviction.
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![result("name", PredicateOp::Eq, true)]),
        )
        .unwrap();

    // Assert
    assert!(
        catalog.is_conformance_passed("processes", "name", PredicateOp::Eq),
        "a result carried by the descriptor must not be wiped by its own registration"
    );
}

// --- The corpus itself, and what a disagreeing collector does to it ----------------------------

use daemoneye_lib::detection::conformance::{
    ConformanceAxis, ConformanceCase, ConformanceOutcome, cases_for, corpus, reference_outcome,
    verify_operation,
};

/// A collector that agrees with the reference on everything. `probe`'s signature demands the
/// `Option`, which is how a collector reports a case it cannot represent; this one represents all
/// of them.
#[allow(clippy::unnecessary_wraps)]
fn honest(case: &ConformanceCase) -> Option<ConformanceOutcome> {
    Some(reference_outcome(case))
}

/// A collector that treats SQL NULL as an ordinary value, so `column != 'x'` over a NULL returns
/// the row. This is the commonest three-valued-logic mistake and it flips admission.
#[allow(clippy::unnecessary_wraps)]
fn null_as_value(case: &ConformanceCase) -> Option<ConformanceOutcome> {
    if case.observed.is_none() && case.op == PredicateOp::Ne {
        return Some(ConformanceOutcome::Admits);
    }
    honest(case)
}

/// A collector that is correct everywhere except the inclusive edge of `>=`, which it evaluates
/// as `>`. Nothing about its NULL or coercion behaviour differs.
#[allow(clippy::unnecessary_wraps)]
fn exclusive_ge(case: &ConformanceCase) -> Option<ConformanceOutcome> {
    if case.op != PredicateOp::Ge {
        return honest(case);
    }
    let shifted = ConformanceCase {
        op: PredicateOp::Gt,
        ..case.clone()
    };
    Some(reference_outcome(&shifted))
}

#[test]
fn a_collector_that_agrees_with_the_reference_passes() {
    assert!(verify_operation(
        ColumnType::String,
        true,
        PredicateOp::Ne,
        honest
    ));
    assert!(verify_operation(
        ColumnType::Int,
        true,
        PredicateOp::Ge,
        honest
    ));
}

#[test]
fn a_collector_disagreeing_only_on_null_handling_fails_that_operation() {
    assert!(
        !verify_operation(ColumnType::String, true, PredicateOp::Ne, null_as_value),
        "NULL != 'x' returning the row must fail the vector"
    );
    // The divergence is confined to the operation it touches: `=` over a NULL is unaffected.
    assert!(verify_operation(
        ColumnType::String,
        true,
        PredicateOp::Eq,
        null_as_value
    ));
}

#[test]
fn a_collector_disagreeing_only_on_a_boundary_value_fails_rather_than_passing() {
    assert!(
        !verify_operation(ColumnType::Int, true, PredicateOp::Ge, exclusive_ge),
        "an exclusive `>=` must fail the vector, not pass on the strength of every other case"
    );
    assert!(verify_operation(
        ColumnType::Int,
        true,
        PredicateOp::Gt,
        exclusive_ge
    ));
}

#[test]
fn an_operation_whose_cases_are_all_skipped_does_not_pass() {
    // Arrange: a collector that can represent nothing.
    let skip_everything = |_case: &ConformanceCase| None;

    // Assert: "no disagreement" is not "verified".
    assert!(!verify_operation(
        ColumnType::Int,
        true,
        PredicateOp::Eq,
        skip_everything
    ));
}

#[test]
fn the_corpus_covers_all_four_axes_for_every_operation_it_names() {
    // Arrange: every (column_type, op) pair the corpus speaks about at all.
    let mut pairs: Vec<(ColumnType, PredicateOp)> = corpus()
        .iter()
        .map(|case| (case.column_type, case.op))
        .collect();
    pairs.sort_unstable_by_key(|&(column_type, op)| (i32::from(column_type), i32::from(op)));
    pairs.dedup();
    assert!(!pairs.is_empty());

    // Assert
    for (column_type, op) in pairs {
        let axes: Vec<ConformanceAxis> = cases_for(column_type, true, op)
            .map(|case| case.axis)
            .collect();
        for required in [
            ConformanceAxis::Null,
            ConformanceAxis::Coercion,
            ConformanceAxis::Boundary,
        ] {
            assert!(
                axes.contains(&required),
                "{column_type:?}/{op:?} has no {required:?} case"
            );
        }
        if column_type == ColumnType::String {
            assert!(
                axes.contains(&ConformanceAxis::Collation),
                "{column_type:?}/{op:?} has no Collation case"
            );
        }
    }
}

#[test]
fn a_case_needing_null_is_not_offered_to_a_non_nullable_column() {
    // Arrange + Act
    let nullable = cases_for(ColumnType::Int, true, PredicateOp::Eq).count();
    let non_nullable = cases_for(ColumnType::Int, false, PredicateOp::Eq).count();

    // Assert: a NULL row against a `NOT NULL` column is an acceptance-time plan defect, not a
    // semantics question, so it is never asked of one.
    assert!(non_nullable < nullable);
    assert!(
        cases_for(ColumnType::Int, false, PredicateOp::Eq).all(|case| !case.requires_nullable())
    );
}

#[test]
fn a_descriptor_version_bump_drops_stale_passes_even_when_no_column_name_changed() {
    // Arrange: `name` passes its vector under v1.
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![result("name", PredicateOp::Eq, true)]),
        )
        .unwrap();
    assert!(catalog.is_pushable("processes", "name", PredicateOp::Eq));

    // Act: v2 keeps every column *name* but changes the column's type, which changes what `=`
    // means. Nothing was added or removed, so the reference diff sees no change at all.
    let mut retyped = descriptor(Vec::new());
    retyped.descriptor_version = "processes-v2".to_owned();
    for column in retyped
        .tables
        .iter_mut()
        .flat_map(|table| table.columns.iter_mut())
    {
        column.column_type = i32::from(ColumnType::Int);
    }
    catalog.register(&verified("procmond"), retyped).unwrap();

    // Assert: a result is bound to the descriptor_version it was produced against.
    assert!(catalog.is_advertised("processes", "name", PredicateOp::Eq));
    assert!(
        !catalog.is_pushable("processes", "name", PredicateOp::Eq),
        "a pass produced against v1 must not survive into v2"
    );
}
