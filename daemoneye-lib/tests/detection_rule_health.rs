//! Re-validation and re-plan seam on descriptor change (R12).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::detection::catalog::{SchemaCatalog, verify_spawn_token};
use daemoneye_lib::detection::rule_health::{RuleHealth, RuleHealthRegistry};
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
        RuleHealth::Unhealthy { ref reason } => assert!(reason.contains("cmdline"), "{reason}"),
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

    let mut oversized = descriptor(&["name"]);
    oversized.tables[0].name = "t".repeat(200);
    assert!(catalog.register(&verified, oversized).is_err());

    assert_eq!(rules.health("needs-name"), Some(&RuleHealth::Unknown));
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
