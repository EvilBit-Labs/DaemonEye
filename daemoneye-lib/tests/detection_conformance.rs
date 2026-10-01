//! Conformance vectors: an advertised operation becomes pushable only once a vector passed for it
//! (R15, R22).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use daemoneye_lib::detection::RegexCache;
use daemoneye_lib::detection::catalog::{SchemaCatalog, VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::planner::plan_rule;
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::literal::Value as LiteralValue;
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, ConformanceResult, PredicateOp, SchemaDescriptor, TableDescriptor,
};

// `support` is shared with `detection_planner.rs`, which already allows `indexing_slicing`
// crate-wide because it owns this same module. Scoping the allow to the module declaration here
// keeps every new property test in this file subject to the lint, and only covers indexing that
// is already load-bearing, pre-existing behaviour in `support::render`.
#[allow(clippy::indexing_slicing)]
mod support;

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

// --- Property gate: planner fallback for a generated conformance-passed set (R6) ------------

/// The comparison operators a generated leaf may use, in the order its index selects from.
const PROPERTY_OPS_TEXT: [&str; 6] = ["=", "!=", "<", "<=", ">", ">="];

/// [`PROPERTY_OPS_TEXT`]'s operators as the enum the catalog and plan speak in.
const PROPERTY_OPS: [PredicateOp; 6] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
];

/// A descriptor advertising every [`support::PROPERTY_COLUMNS`] column for every
/// [`PROPERTY_OPS`] operation, mirroring `int_column` in `detection_planner.rs`.
fn property_int_column(name: &str) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(ColumnType::Int),
        nullable: false,
        supported_ops: PROPERTY_OPS.iter().copied().map(i32::from).collect(),
    }
}

/// A catalog advertising every property column and operation, with conformance passed only for
/// the `(column, op)` pairs named in `passed` — the fallback case `property_catalog` in
/// `detection_planner.rs` does not exercise, since that one passes everything.
fn property_catalog(passed: &BTreeSet<(usize, usize)>) -> SchemaCatalog {
    let mut catalog = SchemaCatalog::new();
    let columns = support::PROPERTY_COLUMNS
        .iter()
        .map(|name| property_int_column(name))
        .collect();
    catalog
        .register(
            &verified("procmond"),
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
    for &(column, op_index) in passed {
        let column_name = support::PROPERTY_COLUMNS
            .get(column)
            .copied()
            .expect("column index is drawn from 0..PROPERTY_COLUMNS.len()");
        let op = PROPERTY_OPS
            .get(op_index)
            .copied()
            .expect("op index is drawn from 0..PROPERTY_OPS.len()");
        catalog.record_conformance_pass("procmond", "processes", column_name, op);
    }
    catalog
}

/// The planner pushes exactly the conjuncts whose `(column, op)` passed conformance, keeps every
/// other conjunct in the residual, and the pushed-then-residual split loses no row over the
/// fixture — the same row-equivalence idiom as `detection_planner.rs:565-585`, replayed here with
/// a generated conformance-passed set standing in for the all-passed catalog.
#[test]
fn planner_pushes_exactly_the_conjuncts_whose_operation_conformance_passed() {
    use proptest::prelude::*;
    use support::{Pred, eval_sql_predicate, pushed_admits, render, rows_fixture};

    let cache = RegexCache::new();
    let rows = rows_fixture();

    proptest!(|(
        passed in prop::collection::btree_set(
            (0..support::PROPERTY_COLUMNS.len(), 0..PROPERTY_OPS.len()),
            0..=24_usize,
        ),
        leaves in prop::collection::vec(
            (0..support::PROPERTY_COLUMNS.len(), 0..PROPERTY_OPS.len(), 0..4_i64),
            1..=6,
        ),
    )| {
        let catalog = property_catalog(&passed);

        let where_sql = leaves
            .iter()
            .map(|&(column, op_index, value)| {
                let op = PROPERTY_OPS_TEXT
                    .get(op_index)
                    .copied()
                    .expect("op index is drawn from 0..PROPERTY_OPS_TEXT.len()");
                render(&Pred::Cmp { column, op, value })
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!("SELECT a FROM processes WHERE {where_sql}");
        let compiled = plan_rule(&catalog, &cache, &rule(&sql), 3).unwrap();

        let pushed: BTreeSet<(usize, i32, i64)> = compiled
            .plan()
            .predicates
            .iter()
            .map(|predicate| {
                let column = support::PROPERTY_COLUMNS
                    .iter()
                    .position(|&name| name == predicate.column)
                    .expect("a pushed predicate must read one of the property columns");
                let value = match predicate
                    .values
                    .first()
                    .and_then(|literal| literal.value.clone())
                {
                    Some(LiteralValue::IntValue(value)) => value,
                    ref other => panic!("expected an integer literal, got {other:?}"),
                };
                (column, predicate.op, value)
            })
            .collect();

        let expected: BTreeSet<(usize, i32, i64)> = leaves
            .iter()
            .filter(|&&(column, op_index, _value)| passed.contains(&(column, op_index)))
            .map(|&(column, op_index, value)| {
                let op = PROPERTY_OPS
                    .get(op_index)
                    .copied()
                    .expect("op index is drawn from 0..PROPERTY_OPS.len()");
                (column, i32::from(op), value)
            })
            .collect();

        prop_assert_eq!(pushed, expected, "the pushed set must be exactly the passed leaves");

        let every_leaf_passed = leaves
            .iter()
            .all(|&(column, op_index, _value)| passed.contains(&(column, op_index)));
        prop_assert_eq!(
            compiled.residual().is_none(),
            every_leaf_passed,
            "residual is empty iff every leaf's operation conformance-passed"
        );

        let whole: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|&(_index, row)| eval_sql_predicate(&where_sql, row))
            .map(|(index, _row)| index)
            .collect();

        let projection = &compiled.plan().projection;
        let split: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|&(_index, row)| pushed_admits(compiled.plan(), row))
            .map(|(index, row)| (index, row.project(projection)))
            .filter(|&(_index, ref row)| {
                compiled
                    .residual()
                    .is_none_or(|residual| eval_sql_predicate(residual, row))
            })
            .map(|(index, _row)| index)
            .collect();

        prop_assert_eq!(
            whole,
            split,
            "predicate {:?} lost or gained rows; plan {:?} residual {:?}",
            where_sql,
            compiled.plan(),
            compiled.residual()
        );
    });
}

// --- Property gate: the corpus itself, for any applicable (column_type, nullable, op) (R6) ---

/// For any corpus case of any applicable operation, a probe that disagrees on exactly that one
/// case fails the operation, and a probe that merely cannot represent it does not.
#[test]
fn a_probe_disagreeing_on_one_case_fails_and_one_that_only_skips_it_does_not() {
    use proptest::prelude::*;

    let column_types = [
        ColumnType::String,
        ColumnType::Int,
        ColumnType::Uint,
        ColumnType::Float,
        ColumnType::Bool,
    ];
    let predicate_ops = [
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

    let strategy = (
        prop::sample::select(column_types.to_vec()),
        any::<bool>(),
        prop::sample::select(predicate_ops.to_vec()),
    )
        .prop_filter(
            "the combination must have at least one applicable corpus case",
            |&(column_type, nullable, op)| cases_for(column_type, nullable, op).count() > 0,
        )
        .prop_flat_map(|(column_type, nullable, op)| {
            let count = cases_for(column_type, nullable, op).count();
            (Just(column_type), Just(nullable), Just(op), 0..count)
        });

    proptest!(|((column_type, nullable, op, index) in strategy)| {
        let count = cases_for(column_type, nullable, op).count();
        let target = cases_for(column_type, nullable, op)
            .nth(index)
            .expect("index is drawn from 0..count so a case exists at it");

        let disagree_at_target = |case: &ConformanceCase| -> Option<ConformanceOutcome> {
            if std::ptr::eq(case, target) {
                let reference = reference_outcome(case);
                Some(if reference == ConformanceOutcome::Admits {
                    ConformanceOutcome::Excludes
                } else {
                    ConformanceOutcome::Admits
                })
            } else {
                Some(reference_outcome(case))
            }
        };
        prop_assert!(
            !verify_operation(column_type, nullable, op, disagree_at_target),
            "a probe disagreeing on one case must fail the operation"
        );

        let skip_target = |case: &ConformanceCase| -> Option<ConformanceOutcome> {
            if std::ptr::eq(case, target) {
                None
            } else {
                Some(reference_outcome(case))
            }
        };
        // Every `(column_type, nullable, op)` this strategy can select has at least five corpus
        // cases today, so comparing against `count > 1` would be true on every generated input and
        // the assertion would only ever exercise one side. Pin that precondition rather than
        // hiding it, so a corpus that later admits a single-case operation fails here instead of
        // quietly narrowing what this property proves. The zero-case half is proved directly by
        // `an_operation_whose_cases_are_all_skipped_does_not_pass`.
        prop_assert!(
            count > 1,
            "this property assumes every selectable operation has several corpus cases"
        );
        prop_assert!(
            verify_operation(column_type, nullable, op, skip_target),
            "skipping one case of several must still pass the operation"
        );
    });
}
