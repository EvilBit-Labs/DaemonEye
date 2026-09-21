//! Pushdown task renewal: a task stays alive while its rule is enabled, and an expiry that lands
//! while the rule is still enabled marks the rule unhealthy (R16, R12).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::SystemTime;

use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::{VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::rule_health::RuleHealth;
use daemoneye_lib::detection_bounds::{PUSHDOWN_TASK_RENEWAL_INTERVAL, PUSHDOWN_TASK_TTL};
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};

fn token() -> String {
    "a".repeat(64)
}

fn verified(collector_id: &str) -> VerifiedRegistration {
    verify_spawn_token(collector_id, Some(&token()), Some(&token())).unwrap()
}

fn descriptor() -> SchemaDescriptor {
    let ops = [PredicateOp::Eq, PredicateOp::Gt, PredicateOp::Lt];
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![ColumnDescriptor {
                name: "cpu_usage".to_owned(),
                column_type: i32::from(ColumnType::Int),
                nullable: false,
                supported_ops: ops.iter().copied().map(i32::from).collect(),
            }],
        }],
    }
}

fn rule(enabled: bool) -> DetectionRule {
    let mut rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Renewal test rule".to_owned(),
        "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
        "test".to_owned(),
        AlertSeverity::Medium,
    );
    rule.enabled = enabled;
    rule
}

/// An engine with one enabled rule planned against one registered collector, and its first task
/// already issued at `start`.
fn engine_with_issued_task(start: SystemTime) -> DetectionEngine {
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();
    engine.load_rule(rule(true)).unwrap();
    let cycle = engine.renewal_cycle(start);
    assert_eq!(cycle.due().len(), 1, "the first cycle issues the task");
    assert!(
        cycle.expired_rules().is_empty(),
        "a freshly issued task cannot be expired"
    );
    engine
}

fn task_id() -> String {
    daemoneye_lib::detection::task_renewal::task_id("rule-1", "procmond")
}

#[test]
fn a_task_is_renewed_before_expiry_while_its_rule_stays_enabled() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    assert_eq!(engine.renewal_count(&task_id()), Some(0));

    for round in 1_u32..=3 {
        let now = start + PUSHDOWN_TASK_RENEWAL_INTERVAL * round;
        let cycle = engine.renewal_cycle(now);
        assert!(cycle.expired_rules().is_empty(), "round {round} expired");
        assert_eq!(cycle.due().len(), 1, "round {round} had no renewal due");
        engine.record_renewal(&task_id(), now);
        assert_eq!(engine.renewal_count(&task_id()), Some(u64::from(round)));
    }

    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "a renewed rule stays covered"
    );
    assert_ne!(
        engine.rule_health("rule-1"),
        Some(&RuleHealth::Unhealthy {
            reason: String::new()
        })
    );
}

#[test]
fn disabling_a_rule_stops_renewal_of_its_task_without_marking_it_unhealthy() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);

    engine.load_rule(rule(false)).unwrap();

    let now = start + PUSHDOWN_TASK_RENEWAL_INTERVAL;
    let cycle = engine.renewal_cycle(now);
    assert!(cycle.due().is_empty(), "a disabled rule is not renewed");
    assert!(
        cycle.expired_rules().is_empty(),
        "stopping renewal is silent, not an expiry"
    );
    assert_eq!(engine.active_task_count(), 0);
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            None | Some(&RuleHealth::Unknown | &RuleHealth::Healthy)
        ),
        "a disabled rule is not unhealthy"
    );

    // Re-enabling resumes renewal: the plan is re-made by the load, and the next cycle issues a
    // fresh task under the same identifier rather than reviving a stale lifetime.
    engine.load_rule(rule(true)).unwrap();
    let resumed = engine.renewal_cycle(now + PUSHDOWN_TASK_RENEWAL_INTERVAL);
    assert_eq!(resumed.due().len(), 1, "a re-enabled rule is covered again");
    assert_eq!(engine.active_task_count(), 1);
    assert_eq!(engine.renewal_count(&task_id()), Some(0));
}

#[test]
fn a_renewal_that_does_not_land_before_expiry_marks_the_rule_unhealthy() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);

    // Every cycle inside the TTL offers the renewal; none of them lands.
    let _ignored = engine.renewal_cycle(start + PUSHDOWN_TASK_RENEWAL_INTERVAL);

    let cycle = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert_eq!(cycle.expired_rules(), ["rule-1"]);
    assert!(
        cycle.due().is_empty(),
        "an expired task is not re-offered in the same cycle that retired it"
    );
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy { .. })
        ),
        "an expiry that lands while the rule is enabled marks it unhealthy"
    );
    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "an expired rule is no longer treated as covered"
    );
    assert_eq!(engine.active_task_count(), 0);
}

#[test]
fn a_collector_re_registering_re_issues_the_active_task_set_without_duplicates() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    assert_eq!(engine.active_task_count(), 1);

    let later = start + PUSHDOWN_TASK_RENEWAL_INTERVAL;
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();
    let reissued = engine.issue_tasks_for_collector("procmond", later);

    assert_eq!(reissued.len(), 1);
    assert_eq!(reissued[0].task_id(), task_id());
    assert_eq!(reissued[0].collector_id(), "procmond");
    assert_eq!(
        engine.active_task_count(),
        1,
        "re-issue replaces the task in place rather than accumulating a second one"
    );

    // The re-issue reset the deadline, so the next cycle inside the interval owes nothing.
    let cycle = engine.renewal_cycle(later);
    assert!(
        cycle.due().is_empty(),
        "a task re-issued this instant owes no renewal yet"
    );
    assert!(
        cycle.expired_rules().is_empty(),
        "a freshly issued task cannot be expired"
    );
}

#[test]
fn an_expired_rule_stays_uncovered_until_something_re_plans_it() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let expired = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert_eq!(expired.expired_rules(), ["rule-1"]);

    // A collector re-registering with the same descriptor changes nothing about the catalog, so
    // nothing re-plans the rule. The expiry mark is deliberately sticky: an operator sees it
    // through the CLI and reloads the rule. Silently resurrecting it would hide the lapse.
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();
    let quiet = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert!(
        quiet.due().is_empty(),
        "an uncovered rule has no task to renew"
    );
    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "an unchanged descriptor re-plans nothing"
    );
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy { .. })
        ),
        "the expiry mark survives a no-op re-registration"
    );

    // Re-loading the rule re-plans it, which restores coverage and issues a fresh task.
    let restored = start + PUSHDOWN_TASK_TTL + PUSHDOWN_TASK_RENEWAL_INTERVAL;
    engine.load_rule(rule(true)).unwrap();
    let cycle = engine.renewal_cycle(restored);
    assert_eq!(cycle.due().len(), 1);
    assert_eq!(cycle.due()[0].task_id(), task_id());
    assert_eq!(engine.active_task_count(), 1);
    assert_eq!(engine.renewal_count(&task_id()), Some(0));
    assert!(
        !matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy { .. })
        ),
        "re-planning the rule clears the expiry mark"
    );
}
