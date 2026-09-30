//! Re-validation and re-plan seam on descriptor change (R12).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::SystemTime;

use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::{SchemaCatalog, verify_spawn_token};
use daemoneye_lib::detection::rule_health::{RuleHealth, RuleHealthRegistry, UnhealthyCause};
use daemoneye_lib::detection::task_renewal::task_id;
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};

fn token() -> String {
    "a".repeat(64)
}

fn descriptor(columns: &[&str]) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: columns
                .iter()
                .map(|name| ColumnDescriptor {
                    name: (*name).to_owned(),
                    column_type: i32::from(ColumnType::String),
                    nullable: false,
                    supported_ops: vec![i32::from(PredicateOp::Eq)],
                })
                .collect(),
        }],
        conformance_results: Vec::new(),
    }
}

fn register(
    catalog: &mut SchemaCatalog,
    columns: &[&str],
) -> daemoneye_lib::detection::catalog::CatalogChange {
    let verified = verify_spawn_token("procmond", Some(&token()), Some(&token())).unwrap();
    catalog.register(&verified, descriptor(columns)).unwrap()
}

/// A rule whose only filter is `column = 'x'`, so dropping that column makes it stop validating.
fn rule(id: &str, column: &str) -> DetectionRule {
    DetectionRule::new(
        id.to_owned(),
        "Health test rule".to_owned(),
        "Revalidation fixture".to_owned(),
        format!("SELECT {column} FROM processes WHERE {column} = 'x'"),
        "test".to_owned(),
        AlertSeverity::Medium,
    )
}

#[test]
fn first_registration_replans_rules_that_were_waiting_on_an_empty_catalog() {
    // Arrange
    let mut catalog = SchemaCatalog::new();
    let mut rules = RuleHealthRegistry::new();
    rules.track("waiting", [("processes", "name")]);

    // Act
    let change = register(&mut catalog, &["name"]);
    let outcome = rules.revalidate(&catalog, &change);

    // Assert
    assert!(change.is_first_registration());
    assert_eq!(outcome.to_replan(), ["waiting"]);
    assert!(outcome.newly_unhealthy().is_empty());
    assert_eq!(rules.health("waiting"), Some(&RuleHealth::Healthy));
}

#[test]
fn a_dropped_column_leaves_only_the_rule_that_filters_on_it_unhealthy() {
    // Arrange
    let mut catalog = SchemaCatalog::new();
    let mut rules = RuleHealthRegistry::new();
    rules.track("needs-cmdline", [("processes", "cmdline")]);
    rules.track("needs-name", [("processes", "name")]);
    let initial = register(&mut catalog, &["name", "cmdline"]);
    let _first = rules.revalidate(&catalog, &initial);

    // Act: the collector restarts without `cmdline`.
    let narrowed = register(&mut catalog, &["name"]);
    let outcome = rules.revalidate(&catalog, &narrowed);

    // Assert
    assert_eq!(outcome.newly_unhealthy(), ["needs-cmdline"]);
    assert_eq!(outcome.to_replan(), ["needs-name"]);
    assert!(matches!(
        rules.health("needs-cmdline"),
        Some(&RuleHealth::Unhealthy { .. })
    ));
    assert_eq!(rules.health("needs-name"), Some(&RuleHealth::Healthy));
}

#[test]
fn an_unhealthy_rule_names_the_reference_that_stopped_resolving() {
    let mut catalog = SchemaCatalog::new();
    let mut rules = RuleHealthRegistry::new();
    rules.track("needs-cmdline", [("processes", "cmdline")]);
    let initial = register(&mut catalog, &["cmdline"]);
    let _first = rules.revalidate(&catalog, &initial);

    let narrowed = register(&mut catalog, &["name"]);
    let _second = rules.revalidate(&catalog, &narrowed);

    let health = rules.health("needs-cmdline").unwrap();
    match *health {
        RuleHealth::Unhealthy { ref reason, .. } => assert!(reason.contains("cmdline"), "{reason}"),
        RuleHealth::Healthy | RuleHealth::Unknown => panic!("rule should be unhealthy"),
        ref other => panic!("rule should be unhealthy, was {other:?}"),
    }
}

#[test]
fn a_rejected_descriptor_triggers_no_replanning() {
    // A refused descriptor never produces a CatalogChange, so nothing can be revalidated from it.
    let mut catalog = SchemaCatalog::new();
    let mut rules = RuleHealthRegistry::new();
    rules.track("needs-name", [("processes", "name")]);
    let verified = verify_spawn_token("procmond", Some(&token()), Some(&token())).unwrap();

    let before = rules.health("needs-name").cloned();

    let mut oversized = descriptor(&["name"]);
    oversized.tables[0].name = "t".repeat(200);
    assert!(catalog.register(&verified, oversized).is_err());

    // The claim is that nothing moved, so compare against what health actually was beforehand
    // rather than against a named variant: pinning the literal state here made this test fail for
    // an unrelated correction to what `track` records, which is not what it is guarding.
    assert_eq!(rules.health("needs-name").cloned(), before);
    assert!(catalog.is_empty());
}

#[test]
fn a_rule_untouched_by_the_change_is_left_alone() {
    let mut catalog = SchemaCatalog::new();
    let mut rules = RuleHealthRegistry::new();
    rules.track("elsewhere", [("network.connections", "remote_ip")]);
    let first = register(&mut catalog, &["name"]);
    let first_outcome = rules.revalidate(&catalog, &first);

    // First registration touches every rule, and this one cannot resolve, so it is unhealthy.
    assert_eq!(first_outcome.newly_unhealthy(), ["elsewhere"]);

    // A later, unrelated re-registration of `processes` must not revisit it.
    let widened = register(&mut catalog, &["name", "pid"]);
    let second_outcome = rules.revalidate(&catalog, &widened);
    assert!(second_outcome.newly_unhealthy().is_empty());
    assert!(second_outcome.to_replan().is_empty());
}

/// A rule marked unhealthy by revalidation must also lose its compiled plan, or the renewal clock
/// re-issues a task the current collector can no longer accept (R12 into R16).
#[test]
fn revalidation_takes_the_plan_away_from_the_rule_it_marks_unhealthy() {
    // Arrange
    let mut engine = DetectionEngine::new();
    let verified = verify_spawn_token("procmond", Some(&token()), Some(&token())).unwrap();
    engine
        .register_collector(&verified, descriptor(&["name", "cmdline"]))
        .unwrap();
    engine.load_rule(rule("needs-cmdline", "cmdline")).unwrap();
    engine.load_rule(rule("needs-name", "name")).unwrap();
    assert!(engine.compiled_rule("needs-cmdline").is_some());

    // Act: the collector re-registers without `cmdline`.
    engine
        .register_collector(&verified, descriptor(&["name"]))
        .unwrap();

    // Assert
    assert!(matches!(
        engine.rule_health("needs-cmdline"),
        Some(&RuleHealth::Unhealthy { .. })
    ));
    assert!(
        engine.compiled_rule("needs-cmdline").is_none(),
        "an unhealthy rule must not keep a plan a task could be issued from"
    );
    assert!(
        engine.compiled_rule("needs-name").is_some(),
        "the still-healthy sibling keeps its plan"
    );

    let issued: Vec<String> = engine
        .issue_tasks_for_collector("procmond", SystemTime::UNIX_EPOCH)
        .iter()
        .map(|pending| pending.task_id().to_owned())
        .collect();
    assert_eq!(issued, [task_id("needs-name", "procmond")]);
}

/// A recoverable cause must not displace a resisting one.
///
/// `mark_unhealthy` is the one health writer the surrounding policy does not otherwise reach: it
/// overwrites in place, so without this guard a later `TaskExpiry` mark on a latency-breached rule
/// would downgrade the verdict to a cause `revalidate` clears, laundering the breach. Production
/// cannot currently make that call — `renewal_cycle` prunes uncovered rules before expiring them —
/// but that is statement order, not policy, and a second caller arriving with T6 would not know it.
#[test]
fn a_recoverable_cause_does_not_displace_a_resisting_one() {
    let mut health = RuleHealthRegistry::new();
    health.track("rule-1", [("processes", "pid")]);

    assert!(health.mark_unhealthy("rule-1", "too slow", UnhealthyCause::LatencyBreach));
    assert!(health.mark_unhealthy("rule-1", "task expired", UnhealthyCause::TaskExpiry));

    let RuleHealth::Unhealthy {
        ref reason, cause, ..
    } = *health
        .health("rule-1")
        .expect("the rule is tracked and unhealthy")
    else {
        panic!("expected the rule to be unhealthy");
    };
    assert_eq!(
        cause,
        UnhealthyCause::LatencyBreach,
        "the resisting cause must survive a later recoverable mark"
    );
    assert_eq!(
        reason, "too slow",
        "the breach's own reason must survive too"
    );

    // The reverse order still overwrites: a breach is the stronger verdict and must win.
    let mut reversed = RuleHealthRegistry::new();
    reversed.track("rule-2", [("processes", "pid")]);
    assert!(reversed.mark_unhealthy("rule-2", "task expired", UnhealthyCause::TaskExpiry));
    assert!(reversed.mark_unhealthy("rule-2", "too slow", UnhealthyCause::LatencyBreach));
    assert!(
        matches!(
            reversed.health("rule-2"),
            Some(&RuleHealth::Unhealthy {
                cause: UnhealthyCause::LatencyBreach,
                ..
            })
        ),
        "a breach must displace a recoverable cause"
    );
}
