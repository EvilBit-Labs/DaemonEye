//! Pushdown planning: what may be pushed, what stays residual, and that the split loses no row
//! (R11, R13, R14, R15, R16, R17, R18).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::detection::catalog::{SchemaCatalog, VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::planner::{PlanError, plan_rule};
use daemoneye_lib::detection::rule_health::RuleHealth;
use daemoneye_lib::detection::{DetectionEngine, RegexCache};
use daemoneye_lib::detection_bounds::PUSHDOWN_TASK_TTL;
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};

mod support;
use support::{eval_sql_predicate, pushed_admits};

/// A 64-hex-character token of the shape the agent issues.
fn token() -> String {
    "a".repeat(64)
}

fn verified(collector_id: &str) -> VerifiedRegistration {
    verify_spawn_token(collector_id, Some(&token()), Some(&token())).unwrap()
}

fn int_column(name: &str, ops: &[PredicateOp]) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(ColumnType::Int),
        nullable: false,
        supported_ops: ops.iter().copied().map(i32::from).collect(),
    }
}

fn descriptor(columns: Vec<ColumnDescriptor>) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns,
        }],
        conformance_results: Vec::new(),
    }
}

/// A catalog whose `cpu_usage` and `memory_usage` columns are both advertised and verified.
fn both_verified_catalog() -> SchemaCatalog {
    let mut catalog = SchemaCatalog::new();
    let ops = [PredicateOp::Eq, PredicateOp::Gt, PredicateOp::Lt];
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![
                int_column("cpu_usage", &ops),
                int_column("memory_usage", &ops),
            ]),
        )
        .unwrap();
    for column in ["cpu_usage", "memory_usage"] {
        for op in ops {
            catalog.record_conformance_pass("procmond", "processes", column, op);
        }
    }
    catalog
}

fn rule(sql: &str) -> DetectionRule {
    DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Planner test rule".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::Medium,
    )
}

// --- AE3: disjunction ---------------------------------------------------------------------

#[test]
fn a_disjunction_pushes_neither_term_even_when_both_are_conformance_passed() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90 OR memory_usage > 90"),
        3,
    )
    .unwrap();

    assert!(
        compiled.plan().predicates.is_empty(),
        "a term under OR is not implied by the whole rule and must not be pushed, got {:?}",
        compiled.plan().predicates
    );
    let residual = compiled
        .residual()
        .expect("the whole predicate is residual");
    assert!(residual.contains("cpu_usage"), "residual: {residual}");
    assert!(residual.contains("memory_usage"), "residual: {residual}");
}

#[test]
fn a_nested_and_inside_an_or_pushes_nothing_from_either_branch() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule(
            "SELECT cpu_usage FROM processes \
             WHERE (cpu_usage > 90 AND memory_usage > 90) OR cpu_usage < 5",
        ),
        3,
    )
    .unwrap();

    assert!(
        compiled.plan().predicates.is_empty(),
        "a conjunct nested inside a disjunct is still not implied, got {:?}",
        compiled.plan().predicates
    );
}

// --- AE4: verified plus unverified ---------------------------------------------------------

#[test]
fn a_conjunction_pushes_the_verified_term_and_leaves_the_unverified_one_residual() {
    let mut catalog = SchemaCatalog::new();
    let ops = [PredicateOp::Gt];
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![
                int_column("cpu_usage", &ops),
                int_column("memory_usage", &ops),
            ]),
        )
        .unwrap();
    // Only cpu_usage has a conformance pass; memory_usage is advertised but unverified.
    catalog.record_conformance_pass("procmond", "processes", "cpu_usage", PredicateOp::Gt);

    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90 AND memory_usage > 90"),
        3,
    )
    .unwrap();

    assert_eq!(compiled.plan().predicates.len(), 1);
    assert_eq!(compiled.plan().predicates[0].column, "cpu_usage");
    let residual = compiled.residual().expect("memory_usage stays residual");
    assert!(residual.contains("memory_usage"), "residual: {residual}");
    assert!(!residual.contains("cpu_usage"), "residual: {residual}");
}

#[test]
fn an_advertised_but_unverified_operation_is_treated_as_unverified() {
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![int_column("cpu_usage", &[PredicateOp::Gt])]),
        )
        .unwrap();

    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90"),
        3,
    )
    .unwrap();

    assert!(compiled.plan().predicates.is_empty());
    assert!(compiled.residual().is_some());
}

// --- R17: a projection-only task is a plan -------------------------------------------------

#[test]
fn a_rule_with_no_pushable_predicate_loads_as_a_projection_only_task() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90 OR memory_usage > 90"),
        3,
    )
    .unwrap();

    assert!(compiled.plan().predicates.is_empty());
    assert_eq!(compiled.plan().table, "processes");
    assert_eq!(compiled.collector_id(), "procmond");
    // The residual reads memory_usage, so the projection has to carry it back.
    assert!(
        compiled
            .plan()
            .projection
            .contains(&"memory_usage".to_owned()),
        "projection must cover every column the residual reads, got {:?}",
        compiled.plan().projection
    );
}

/// The bug the property test found: two ineligible disjuncts joined into one residual string.
///
/// `a = 1 OR a = 0` renders without parentheses, so joining two such conjuncts with ` AND `
/// reassociated under SQL precedence into `a = 1 OR (a = 0 AND a = 0) OR a = 2` — a different
/// predicate. Pinned deterministically here so the fix does not rest on a random seed.
#[test]
fn two_disjunct_conjuncts_keep_their_grouping_in_the_residual() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule(
            "SELECT cpu_usage FROM processes \
             WHERE (cpu_usage = 1 OR cpu_usage = 0) AND (cpu_usage = 0 OR cpu_usage = 2)",
        ),
        3,
    )
    .unwrap();

    let residual = compiled.residual().expect("neither disjunct is pushable");
    assert!(
        residual.contains("(cpu_usage = 1 OR cpu_usage = 0)")
            && residual.contains("(cpu_usage = 0 OR cpu_usage = 2)"),
        "each disjunct must stay parenthesised, got {residual}"
    );
}

// --- Lowering shapes beyond binary comparison ----------------------------------------------

/// A catalog whose `name` column is a string with `IN`, `LIKE` and `REGEXP` all verified.
fn string_catalog() -> SchemaCatalog {
    let ops = [PredicateOp::In, PredicateOp::Like, PredicateOp::Regexp];
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![ColumnDescriptor {
                name: "name".to_owned(),
                column_type: i32::from(ColumnType::String),
                nullable: false,
                supported_ops: ops.iter().copied().map(i32::from).collect(),
            }]),
        )
        .unwrap();
    for op in ops {
        catalog.record_conformance_pass("procmond", "processes", "name", op);
    }
    catalog
}

#[test]
fn an_in_list_lowers_to_one_predicate_carrying_every_candidate() {
    let catalog = string_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT name FROM processes WHERE name IN ('bash', 'sh')"),
        3,
    )
    .unwrap();

    assert_eq!(compiled.plan().predicates.len(), 1);
    let predicate = &compiled.plan().predicates[0];
    assert_eq!(predicate.op, i32::from(PredicateOp::In));
    assert_eq!(predicate.values.len(), 2);
    assert!(compiled.residual().is_none());
}

#[test]
fn a_regexp_predicate_over_a_verified_column_is_pushed() {
    let catalog = string_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT name FROM processes WHERE name REGEXP '^ba(sh)?$'"),
        3,
    )
    .unwrap();

    assert_eq!(compiled.plan().predicates.len(), 1);
    assert_eq!(
        compiled.plan().predicates[0].op,
        i32::from(PredicateOp::Regexp)
    );
}

#[test]
fn a_regexp_pattern_that_does_not_compile_fails_the_rule_at_load() {
    let catalog = string_catalog();
    let cache = RegexCache::new();
    let error = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT name FROM processes WHERE name REGEXP '(unclosed'"),
        3,
    )
    .unwrap_err();

    assert!(
        matches!(error, PlanError::Pattern(_)),
        "unexpected error: {error}"
    );
}

// --- R16: TTL ------------------------------------------------------------------------------

#[test]
fn the_plan_carries_the_fixed_pushdown_task_ttl() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let compiled = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90"),
        3,
    )
    .unwrap();

    let expected = u64::try_from(PUSHDOWN_TASK_TTL.as_millis()).unwrap();
    assert_eq!(compiled.plan().ttl_ms, expected);
}

// --- R11: unknown references ---------------------------------------------------------------

#[test]
fn an_unknown_table_is_named_in_the_planning_error() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let error = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM sockets WHERE cpu_usage > 90"),
        3,
    )
    .unwrap_err();

    assert!(
        matches!(error, PlanError::UnknownReference(_)),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("sockets"), "error: {error}");
}

#[test]
fn an_unknown_column_is_named_in_the_planning_error() {
    let catalog = both_verified_catalog();
    let cache = RegexCache::new();
    let error = plan_rule(
        &catalog,
        &cache,
        &rule("SELECT cpu_usage FROM processes WHERE nonesuch > 90"),
        3,
    )
    .unwrap_err();

    assert!(error.to_string().contains("nonesuch"), "error: {error}");
}

// --- AE2 + R18: the engine gate ------------------------------------------------------------

#[test]
fn a_rule_naming_an_unregistered_table_fails_first_load_and_stays_out_of_the_enabled_set() {
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(
            &verified("procmond"),
            descriptor(vec![int_column("cpu_usage", &[PredicateOp::Gt])]),
        )
        .unwrap();

    let error = engine
        .load_rule(rule("SELECT cpu_usage FROM sockets WHERE cpu_usage > 90"))
        .unwrap_err();

    assert!(error.to_string().contains("sockets"), "error: {error}");
    assert!(engine.compiled_rule("rule-1").is_none());
    assert!(engine.get_rule("rule-1").is_none());
    assert_eq!(engine.rejection_log().records().len(), 1);
}

#[test]
fn a_rule_loaded_before_any_collector_registers_is_deferred_and_planned_on_registration() {
    let mut engine = DetectionEngine::new();
    engine
        .load_rule(rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90"))
        .unwrap();

    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "R18: nothing is planned against an empty catalog"
    );
    assert_eq!(engine.deferred_rule_ids(), vec!["rule-1".to_owned()]);

    engine
        .register_collector(
            &verified("procmond"),
            descriptor(vec![int_column("cpu_usage", &[PredicateOp::Gt])]),
        )
        .unwrap();

    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "a deferred rule is planned once a collector arrives, never dropped"
    );
    assert!(engine.deferred_rule_ids().is_empty());
}

#[test]
fn a_deferred_rule_that_cannot_be_planned_is_rejected_on_the_drain_not_marked_unhealthy() {
    let mut engine = DetectionEngine::new();
    engine
        .load_rule(rule("SELECT cpu_usage FROM sockets WHERE cpu_usage > 90"))
        .unwrap();

    engine
        .register_collector(
            &verified("procmond"),
            descriptor(vec![int_column("cpu_usage", &[PredicateOp::Gt])]),
        )
        .unwrap();

    assert!(
        engine.get_rule("rule-1").is_none(),
        "first load, so rejected"
    );
    assert_eq!(engine.rejection_log().records().len(), 1);
}

/// A rule carrying a CTE is refused rather than planned against the table the CTE shadows.
///
/// Regression test. The planner resolves the FROM name against the catalog and never reads
/// `query.with`, so this did not merely ignore the CTE — it planned against the real `processes`
/// table and discarded the CTE's own `pid > 100` filter, admitting rows the operator excluded.
/// That is the same silent mis-lowering as a dropped LIMIT, reached by a different route.
///
/// The validation gate still accepts CTEs on purpose: three tests in `detection_sql_validation.rs`
/// exist to prove the gate reaches constructs hidden inside a CTE body, and refusing `WITH` there
/// would delete that coverage. The refusal belongs where the mis-lowering happens.
#[test]
fn a_rule_carrying_a_cte_is_refused_rather_than_planned_against_the_shadowed_table() {
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified("procmond"),
            descriptor(vec![int_column("pid", &[PredicateOp::Eq, PredicateOp::Gt])]),
        )
        .unwrap();
    let cache = RegexCache::new();

    let shadowing = rule(
        "WITH processes AS (SELECT pid FROM processes WHERE pid > 100) \
         SELECT pid FROM processes WHERE pid = 1",
    );
    assert!(
        matches!(
            plan_rule(&catalog, &cache, &shadowing, 3),
            Err(PlanError::CommonTableExpression)
        ),
        "a CTE shadowing a catalog table must be refused, not silently planned against the table"
    );

    // A CTE that shadows nothing is refused by the same gate: the planner cannot lower any of them.
    let unshadowing = rule("WITH recent AS (SELECT pid FROM processes) SELECT pid FROM processes");
    assert!(matches!(
        plan_rule(&catalog, &cache, &unshadowing, 3),
        Err(PlanError::CommonTableExpression)
    ));
}

/// A rule planned against a live catalog reports as healthy, not as unjudged.
///
/// Regression test. Health was reset to `Unknown` every time the planner tracked a rule's
/// references, which happens *after* a registration re-plans it — so a rule that had just been
/// validated against a real descriptor still read `Unknown`, and no rule was ever observably
/// `Healthy` following a registration. Nothing caught it because each unit asserted only on
/// `Unhealthy`, the state that does get set. T10 renders this field, and would have shown every
/// rule as unjudged forever.
#[test]
fn a_rule_planned_against_a_live_catalog_reports_healthy() {
    let mut engine = DetectionEngine::new();
    engine
        .load_rule(rule("SELECT cpu_usage FROM processes WHERE cpu_usage > 90"))
        .unwrap();

    // Deferred: R18 holds rule load until a collector has advertised something.
    assert_eq!(engine.rule_health("rule-1"), None, "nothing judged it yet");

    engine
        .register_collector(
            &verified("procmond"),
            descriptor(vec![int_column("cpu_usage", &[PredicateOp::Gt])]),
        )
        .unwrap();

    assert_eq!(
        engine.rule_health("rule-1"),
        Some(&RuleHealth::Healthy),
        "the rule planned against a real descriptor, so it has been judged and passed"
    );
}

// --- The property: pushed then residual loses no row ---------------------------------------

/// The plan's sharpest gate, and the honest statement of its limit.
///
/// For a generated rule and a fixed row set, filtering by the pushed half, projecting to the
/// planned column list, and then filtering by the residual yields exactly the rows the whole
/// predicate yields. This proves the **decomposition** is logically sound over one row set.
///
/// It does **not** prove the two halves observe the same rows in production. The collector
/// evaluates live process state while the residual reads the event store. The plan's position is
/// that the residual applies only to rows the pushed half admitted and that were then stored. An
/// implementation that lets the residual range over the store independently breaks that
/// assumption, and this test would not catch it.
#[test]
fn pushed_then_residual_is_match_equivalent_to_the_whole_predicate() {
    use proptest::prelude::*;
    use support::{pred_strategy, render, rows_fixture};

    let catalog = property_catalog();
    let cache = RegexCache::new();
    let rows = rows_fixture();

    proptest!(|(predicate in pred_strategy())| {
        let where_sql = render(&predicate);
        let sql = format!("SELECT a FROM processes WHERE {where_sql}");
        let compiled = plan_rule(&catalog, &cache, &rule(&sql), 3).unwrap();

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
            predicate,
            compiled.plan(),
            compiled.residual()
        );
    });
}

/// Four int columns, every operation advertised and conformance-passed: the harshest case, since
/// the R15 gate never rescues a predicate the R14 test should have refused.
fn property_catalog() -> SchemaCatalog {
    let mut catalog = SchemaCatalog::new();
    let ops = [
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::Lt,
        PredicateOp::Le,
        PredicateOp::Gt,
        PredicateOp::Ge,
    ];
    let columns = support::PROPERTY_COLUMNS
        .iter()
        .map(|name| int_column(name, &ops))
        .collect();
    catalog
        .register(&verified("procmond"), descriptor(columns))
        .unwrap();
    for name in support::PROPERTY_COLUMNS {
        for op in ops {
            catalog.record_conformance_pass("procmond", "processes", name, op);
        }
    }
    catalog
}
