//! `runnable_rules`: the single eligibility site (KTD6, R8).
//!
//! The exhaustive `enabled x plan x health` agreement with `is_rule_covered` is a unit test inside
//! the crate, because it needs combinations the public API cannot reach on purpose (an enabled,
//! planned, latency-latched rule is unconstructible through it). This file pins what a caller of
//! the public API sees.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::{VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};

fn verified() -> VerifiedRegistration {
    let token = "a".repeat(64);
    verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap()
}

fn descriptor() -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![ColumnDescriptor {
                name: "cpu_usage".to_owned(),
                column_type: i32::from(ColumnType::Int),
                nullable: false,
                supported_ops: vec![i32::from(PredicateOp::Gt)],
            }],
        }],
        conformance_results: Vec::new(),
    }
}

fn rule(id: &str) -> DetectionRule {
    DetectionRule::new(
        id.to_owned(),
        "Rule".to_owned(),
        "Eligibility fixture".to_owned(),
        "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
        "test".to_owned(),
        AlertSeverity::Low,
    )
}

fn planned_engine() -> DetectionEngine {
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(&verified(), descriptor())
        .unwrap();
    engine
}

#[test]
fn a_planned_enabled_healthy_rule_is_runnable_with_generation_one() {
    let mut engine = planned_engine();
    engine.load_rule(rule("a")).unwrap();

    let runnable = engine.runnable_rules();

    assert_eq!(runnable.len(), 1);
    let only = &runnable[0];
    assert_eq!(only.rule.id.raw(), "a");
    assert_eq!(only.compiled.rule_id(), "a");
    assert_eq!(only.compiled.collector_id(), "procmond");
    assert_eq!(only.descriptor.name, "processes");
    assert_eq!(only.generation.to_string(), "1");
    assert_eq!(only.pattern_latency_threshold, Duration::from_millis(10));
}

#[test]
fn a_disabled_rule_is_not_runnable() {
    let mut engine = planned_engine();
    engine.load_rule(rule("a")).unwrap();
    engine.set_rule_enabled("a", false).unwrap();

    assert!(engine.runnable_rules().is_empty());
}

#[test]
fn a_rule_deferred_under_r18_is_not_runnable() {
    let mut engine = DetectionEngine::new();
    engine.load_rule(rule("a")).unwrap();
    assert_eq!(engine.deferred_rule_ids(), ["a"]);

    assert!(engine.runnable_rules().is_empty());
}

#[test]
fn a_latency_latched_rule_is_not_runnable() {
    let mut engine = planned_engine();
    engine.load_rule(rule("a")).unwrap();
    let generation = engine.runnable_rules()[0].generation;
    assert!(engine.observe_pattern_latency("a", generation, Duration::from_millis(11)));

    assert!(engine.runnable_rules().is_empty());
}

#[test]
fn only_the_eligible_rules_of_several_are_listed() {
    let mut engine = planned_engine();
    engine.load_rule(rule("a")).unwrap();
    engine.load_rule(rule("b")).unwrap();
    engine.load_rule(rule("c")).unwrap();
    engine.set_rule_enabled("b", false).unwrap();

    let mut ids: Vec<String> = engine
        .runnable_rules()
        .iter()
        .map(|r| r.rule.id.raw().to_owned())
        .collect();
    ids.sort();

    assert_eq!(ids, ["a", "c"]);
}

#[test]
fn a_runnable_rule_is_a_snapshot_not_a_live_view() {
    let mut engine = planned_engine();
    engine.load_rule(rule("a")).unwrap();
    let snapshot = engine.runnable_rules();

    engine.set_rule_enabled("a", false).unwrap();

    assert!(
        snapshot[0].rule.enabled,
        "the snapshot keeps what it cloned"
    );
    assert!(
        !engine.is_runnable("a", snapshot[0].generation),
        "is_runnable re-reads the engine, which is how the agent's result gate sees the change"
    );
}
