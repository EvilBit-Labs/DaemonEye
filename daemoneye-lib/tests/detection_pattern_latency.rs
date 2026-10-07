//! The latency threshold's fail-closed consequence (R2, R3, R8).
//!
//! `DetectionEngine::observe_pattern_latency` is the entry point T6 will call with an
//! already-measured [`Duration`]. This file proves the consequence side of that contract without
//! T6's executor: a breach disables the owning rule in one unconditional pass, and only
//! `load_rule` — never `set_rule_enabled(true)` nor a collector registering — restores it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::{Duration, SystemTime};

use tracing_test::traced_test;

use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::catalog::{VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::rule_health::{RuleHealth, UnhealthyCause};
use daemoneye_lib::detection::{DetectionEngine, DetectionEngineError, Generation};
use daemoneye_lib::detection_bounds::{PUSHDOWN_TASK_RENEWAL_INTERVAL, PUSHDOWN_TASK_TTL};
use daemoneye_lib::models::{AlertSeverity, DetectionRule, ProcessRecord};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::rejection_log::RejectionReason;

fn token() -> String {
    "a".repeat(64)
}

fn verified(collector_id: &str) -> VerifiedRegistration {
    verify_spawn_token(collector_id, Some(&token()), Some(&token())).unwrap()
}

/// procmond's descriptor: the one table `rule()` below filters on.
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
        conformance_results: Vec::new(),
    }
}

/// A second, distinct collector identity registering for the first time. `is_first_registration`
/// is true purely because this `collector_id` has never registered before — nothing about the
/// tables it declares matters, so an empty table list is enough to trigger it (verified against
/// `CatalogChange::is_first_registration` in `catalog.rs`).
fn other_collector_descriptor(collector_id: &str) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: collector_id.to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: Vec::new(),
        conformance_results: Vec::new(),
    }
}

fn rule(enabled: bool) -> DetectionRule {
    let mut rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Pattern latency test rule".to_owned(),
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
    engine
}

/// The generation `runnable_rules` issues for `rule-1`. The only way to obtain one: the type has
/// no public constructor, so a test cannot invent a generation any more than production can.
fn generation_of(engine: &DetectionEngine) -> Generation {
    engine
        .runnable_rules()
        .iter()
        .find(|runnable| runnable.rule.id.raw() == "rule-1")
        .expect("rule-1 must be runnable to have a generation")
        .generation
}

/// The reason text of a rule's current `Unhealthy` health, panicking on any other state — used
/// only after a test has already asserted the rule is unhealthy for a specific reason.
fn unhealthy_reason(engine: &DetectionEngine, rule_id: &str) -> String {
    let Some(health) = engine.rule_health(rule_id) else {
        panic!("expected rule {rule_id} to be tracked");
    };
    match *health {
        RuleHealth::Unhealthy { ref reason, .. } => reason.clone(),
        RuleHealth::Healthy | RuleHealth::Unknown => {
            panic!("expected rule {rule_id} to be Unhealthy")
        }
        ref other => panic!("expected rule {rule_id} to be Unhealthy, was {other:?}"),
    }
}

#[test]
fn with_config_carries_the_configured_threshold_and_new_reports_the_default() {
    let configured = DetectionEngine::with_config(&DetectionConfig {
        pattern_latency_threshold_ms: 25,
        ..DetectionConfig::default()
    });
    assert_eq!(
        configured.pattern_latency_threshold(),
        Duration::from_millis(25)
    );

    let default_engine = DetectionEngine::new();
    assert_eq!(
        default_engine.pattern_latency_threshold(),
        Duration::from_millis(10)
    );
}

/// AE1: a breach flips `enabled`, drops the plan, marks the rule unhealthy with both millisecond
/// figures in the reason, and stops the rule from ever being reported as expired or re-issued.
#[test]
fn a_breaching_observation_disables_the_rule_removes_its_plan_and_stops_its_task() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    let breached = engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11));
    assert!(
        breached,
        "11 ms against a 10 ms default threshold breaches it"
    );

    assert!(
        !engine.get_rule("rule-1").expect("rule tracked").enabled,
        "a breach disables the rule"
    );
    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "a breach removes the compiled plan"
    );

    let reason = unhealthy_reason(&engine, "rule-1");
    assert!(
        reason.contains("11ms"),
        "reason must name the observed duration: {reason}"
    );
    assert!(
        reason.contains("10ms budget"),
        "reason must name the threshold duration: {reason}"
    );

    let cycle = engine.renewal_cycle(start + PUSHDOWN_TASK_RENEWAL_INTERVAL);
    assert!(
        cycle.due().is_empty(),
        "no task is issued for a disabled rule"
    );

    // Run past the TTL, not just the renewal interval. At the renewal interval nothing could be
    // reported expired whatever the disable did, so asserting an empty expiry set there proves
    // nothing. Past the TTL the expiry path is genuinely live, and this pins two things: the rule
    // never re-enters the coverable set, and — should it ever be reported expired — the breach
    // verdict is not downgraded to the recoverable `TaskExpiry` cause, which `revalidate` would
    // then clear. Today `renewal_cycle` prunes uncovered rules before expiring them, so the
    // downgrade call is not reachable; that is statement order, not policy, and this asserts the
    // outcome either way.
    let expired = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert!(
        expired.expired_rules().is_empty(),
        "a rule the latency guard disabled was never covered, so it cannot be reported expired"
    );
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy {
                cause: UnhealthyCause::LatencyBreach,
                ..
            })
        ),
        "the breach verdict must survive the expiry path: {:?}",
        engine.rule_health("rule-1")
    );

    let last_rejection = engine.rejection_log().records().back().expect(
        "a latency breach must be recorded in the rejection log: it is the one queryable trace \
         that survives a reload clearing the health row",
    );
    assert!(
        matches!(
            last_rejection.reason,
            RejectionReason::RuleOther { ref rule_id, ref message }
                if rule_id == "rule-1" && message == &reason
        ),
        "expected a RuleOther rejection naming rule-1 with the breach reason, got {:?}",
        last_rejection.reason
    );
}

/// AE2: an observation exactly at the threshold changes nothing.
#[test]
fn an_observation_at_the_threshold_changes_nothing() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    let breached = engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(10));
    assert!(!breached, "an observation at the threshold is not a breach");

    assert!(engine.get_rule("rule-1").expect("rule tracked").enabled);
    assert!(engine.compiled_rule("rule-1").is_some());
    assert_eq!(engine.rule_health("rule-1"), Some(&RuleHealth::Healthy));
}

/// A second breach on an already-disabled rule still reports `true` (the observation itself did
/// breach) but does not overwrite the first breach's reason with a second, different one.
#[test]
fn a_second_breach_on_an_already_disabled_rule_leaves_the_reason_byte_identical() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));
    let first_reason = unhealthy_reason(&engine, "rule-1");

    let second_breach =
        engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(15));
    assert!(
        second_breach,
        "the observation itself still breached the threshold"
    );
    assert_eq!(
        unhealthy_reason(&engine, "rule-1"),
        first_reason,
        "an already-disabled rule is left exactly as it is"
    );
}

/// A rule id the engine does not hold changes nothing and is not tracked afterward.
#[test]
fn an_unknown_rule_id_changes_nothing() {
    // A generation can only come from `runnable_rules`, so borrow a real one from an engine that
    // does hold a rule; the engine under test holds nothing and must refuse it.
    let generation = generation_of(&engine_with_issued_task(SystemTime::UNIX_EPOCH));
    let mut engine = DetectionEngine::new();
    let breached =
        engine.observe_pattern_latency("does-not-exist", generation, Duration::from_millis(999));
    assert!(!breached);
    assert_eq!(engine.rule_health("does-not-exist"), None);
}

/// AE3: after a breach, re-enabling the rule directly is refused; only `load_rule` restores it.
#[test]
fn only_reloading_the_rule_restores_it_after_a_latency_breach() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));
    let reason_before = unhealthy_reason(&engine, "rule-1");

    let result = engine.set_rule_enabled("rule-1", true);
    assert!(
        matches!(result, Err(DetectionEngineError::RuleLatched { .. })),
        "re-enabling a latency-disabled rule must be refused with RuleLatched, got {result:?}"
    );
    assert!(
        !engine.get_rule("rule-1").expect("rule tracked").enabled,
        "a refused enable must leave the rule disabled"
    );
    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "a refused enable does not restore a plan"
    );
    assert_eq!(
        unhealthy_reason(&engine, "rule-1"),
        reason_before,
        "a refused enable does not touch the health reason"
    );

    engine.load_rule(rule(true)).unwrap();
    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "reloading restores a plan"
    );
    assert!(engine.get_rule("rule-1").expect("rule tracked").enabled);
    assert_eq!(engine.rule_health("rule-1"), Some(&RuleHealth::Healthy));
}

/// AE5: `RuleHealthRegistry::revalidate` checks `resists_auto_recovery` before either the
/// first-registration or the `touches()` branch, so a cause that resists it is skipped
/// unconditionally regardless of why the rule was unhealthy. A second collector registering for
/// the first time must not resurrect a rule the latency guard disabled.
#[test]
fn a_new_collectors_first_registration_does_not_re_heal_a_latency_disabled_rule() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));
    let reason_before = unhealthy_reason(&engine, "rule-1");

    engine
        .register_collector(
            &verified("collector-2"),
            other_collector_descriptor("collector-2"),
        )
        .expect("a fresh collector identity with no conflicting tables registers cleanly");

    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "the plan must not reappear after an unrelated collector's first registration"
    );
    assert_eq!(
        unhealthy_reason(&engine, "rule-1"),
        reason_before,
        "the reason must be byte-identical after revalidation"
    );

    let result = engine.set_rule_enabled("rule-1", true);
    assert!(
        matches!(result, Err(DetectionEngineError::RuleLatched { .. })),
        "re-enabling a latency-disabled rule must be refused with RuleLatched, got {result:?}"
    );
    let cycle = engine.renewal_cycle(start + PUSHDOWN_TASK_RENEWAL_INTERVAL);
    assert!(
        cycle.due().is_empty(),
        "no task is issued for a rule the latency guard disabled"
    );
}

/// The blanket re-heal a new collector's first registration performs must still work for the
/// mechanism it was written for: a rule whose reference genuinely stopped resolving (here, via
/// pushed-task expiry, which carries `UnhealthyCause::TaskExpiry`) is re-healed and re-planned once
/// a later registration shows its reference still resolves.
#[test]
fn a_task_expiry_reference_failure_is_still_re_healed_by_a_later_registration() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);

    let expired = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert_eq!(expired.expired_rules(), ["rule-1"]);
    assert!(engine.compiled_rule("rule-1").is_none());
    assert!(matches!(
        engine.rule_health("rule-1"),
        Some(&RuleHealth::Unhealthy { .. })
    ));

    engine
        .register_collector(
            &verified("collector-2"),
            other_collector_descriptor("collector-2"),
        )
        .expect("a fresh collector identity with no conflicting tables registers cleanly");

    assert_eq!(
        engine.rule_health("rule-1"),
        Some(&RuleHealth::Healthy),
        "a reference failure is re-healed once revalidation runs"
    );
    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "re-healing re-plans the rule"
    );
}

/// P1-A: `set_rule_enabled(id, false)` on a rule that is still `Healthy` must not give a
/// subsequent breach a bypass. `observe_pattern_latency` keys its idempotency check on health, not
/// on `rule.enabled`, so an operator-disabled-but-healthy rule still loses its plan and its
/// `Healthy` verdict on a later breach.
#[test]
fn an_operator_disabled_still_healthy_rule_still_loses_its_plan_and_health_on_breach() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    engine
        .set_rule_enabled("rule-1", false)
        .expect("disabling is always allowed");
    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "disabling alone must not drop the compiled plan"
    );
    assert_eq!(engine.rule_health("rule-1"), Some(&RuleHealth::Healthy));

    let breached = engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11));
    assert!(
        breached,
        "11 ms against a 10 ms default threshold breaches it"
    );

    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "a breach on an operator-disabled-but-healthy rule must still drop the compiled plan"
    );
    let reason = unhealthy_reason(&engine, "rule-1");
    assert!(
        reason.contains("11ms"),
        "reason must name the observed ms: {reason}"
    );
}

/// P1-B: a rule loaded before any collector registered sits in `deferred`, untracked in health.
/// A breach on it must be a durable verdict that survives the later registration draining
/// `deferred`, which plans the rule for the first time and calls `RuleHealthRegistry::track`.
#[test]
fn a_breach_on_a_deferred_rule_survives_the_registration_that_drains_it() {
    // A deferred rule has no plan, so `runnable_rules` never lists it and no executor could
    // report on it. The defence-in-depth path is still worth pinning, so the generation is the
    // one an identically-loaded sibling engine issues for its planned rule (both are the first
    // load of `rule-1`).
    let generation = generation_of(&engine_with_issued_task(SystemTime::UNIX_EPOCH));
    let mut engine = DetectionEngine::new();
    engine.load_rule(rule(true)).unwrap();
    assert_eq!(engine.deferred_rule_ids(), ["rule-1"]);
    assert_eq!(
        engine.rule_health("rule-1"),
        None,
        "an untracked deferred rule has no health yet"
    );

    let breached = engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11));
    assert!(breached);
    assert!(!engine.get_rule("rule-1").expect("rule tracked").enabled);

    engine
        .register_collector(&verified("procmond"), descriptor())
        .expect("the first registration drains the deferred rule");

    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "the breach verdict must not be laundered into a compiled plan by the drain"
    );
    assert!(
        !engine.get_rule("rule-1").expect("rule tracked").enabled,
        "the rule must stay disabled after the drain"
    );
    let reason = unhealthy_reason(&engine, "rule-1");
    assert!(
        reason.contains("11ms"),
        "the latency reason must survive the drain: {reason}"
    );
}

/// P1-C: after a breach, re-enabling must be refused outright, and `execute_rules` must not alert
/// for the rule while it stays disabled. This does *not* exercise `execute_rules`' own
/// `resists_auto_recovery` gate — the refused enable leaves `enabled == false`, so the pre-existing
/// `if !rule.enabled { continue; }` check short-circuits before that gate is ever reached. The
/// `resists_auto_recovery` gate in `execute_rules` is what
/// `a_task_expired_rule_still_alerts_locally_and_can_be_re_enabled` actually exercises, via a
/// recoverable cause that leaves `enabled == true`.
#[test]
fn a_breached_rule_refuses_re_enable_and_produces_no_alerts() {
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();

    let mut high_cpu_rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Pattern latency test rule".to_owned(),
        "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
        "high_cpu".to_owned(),
        AlertSeverity::Medium,
    );
    high_cpu_rule.enabled = true;
    engine.load_rule(high_cpu_rule).unwrap();
    let generation = generation_of(&engine);

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));

    let result = engine.set_rule_enabled("rule-1", true);
    assert!(
        matches!(result, Err(DetectionEngineError::RuleLatched { .. })),
        "re-enabling a latency-disabled rule must be refused with RuleLatched, got {result:?}"
    );
    assert!(
        !engine.get_rule("rule-1").expect("rule tracked").enabled,
        "a refused enable must not flip the flag"
    );

    let mut process = ProcessRecord::new(1234, "hog".to_owned());
    process.cpu_usage = Some(95.0);
    let alerts = engine.execute_rules(&[process]);
    assert!(
        alerts.is_empty(),
        "a latency-disabled rule must not alert even though its category would otherwise match"
    );
}

/// Reliability's flagged interleaving: a rule already `Unhealthy { TaskExpiry }` then breaches on
/// latency. The latency verdict must win over the task-expiry one already recorded, and — unlike
/// task expiry alone — must now resist being re-healed by a later registration.
#[test]
fn a_latency_breach_on_a_task_expired_rule_overrides_and_then_resists_re_heal() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    let expired = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert_eq!(expired.expired_rules(), ["rule-1"]);
    assert!(matches!(
        engine.rule_health("rule-1"),
        Some(&RuleHealth::Unhealthy {
            cause: UnhealthyCause::TaskExpiry,
            ..
        })
    ));

    let breached = engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11));
    assert!(breached);
    let reason = unhealthy_reason(&engine, "rule-1");
    assert!(
        reason.contains("11ms"),
        "the latency reason must have replaced the task-expiry one: {reason}"
    );

    engine
        .register_collector(
            &verified("collector-2"),
            other_collector_descriptor("collector-2"),
        )
        .expect("a fresh collector identity with no conflicting tables registers cleanly");

    assert_eq!(
        unhealthy_reason(&engine, "rule-1"),
        reason,
        "the latency verdict must resist the re-heal that would have cleared task expiry alone"
    );
    assert!(engine.compiled_rule("rule-1").is_none());
}

/// `RuleHealthRegistry::revalidate`'s `LatencyBreach` skip must fire on the `touches()` branch
/// too, not only on the `is_first_registration()` branch the earlier test above exercises: the
/// *same*, already-registered collector re-registering with a descriptor that still reaches the
/// latency-disabled rule's table must not re-heal it either.
#[test]
fn a_same_collectors_re_registration_that_touches_the_table_does_not_re_heal_a_latency_disabled_rule()
 {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));
    let reason_before = unhealthy_reason(&engine, "rule-1");

    // Same collector identity re-registering with an extra column on the same table: this
    // changes the reference set for `processes`, so `touches()` returns true for `rule-1` even
    // though `cpu_usage` still resolves and this is not a first registration.
    let mut widened = descriptor();
    widened.tables[0].columns.push(ColumnDescriptor {
        name: "pid".to_owned(),
        column_type: i32::from(ColumnType::Int),
        nullable: false,
        supported_ops: vec![i32::from(PredicateOp::Eq)],
    });
    let change = engine
        .register_collector(&verified("procmond"), widened)
        .expect("the same collector re-registering with a superset of columns registers cleanly");
    assert!(
        !change.is_first_registration(),
        "procmond already registered in engine_with_issued_task"
    );
    assert!(
        change.affected_tables().contains("processes"),
        "adding a column must make this registration touch `processes`"
    );

    assert!(
        engine.compiled_rule("rule-1").is_none(),
        "the plan must not reappear after a same-collector re-registration that touches its table"
    );
    assert_eq!(
        unhealthy_reason(&engine, "rule-1"),
        reason_before,
        "the reason must be byte-identical after revalidation via the touches() branch"
    );
}

/// The recoverable-cause path through `execute_rules`' own `resists_auto_recovery` gate, which no
/// other test in this file reaches. A task-expired rule keeps `enabled == true` — expiry never
/// touches it — unlike the P1-C test above, whose refused enable leaves `enabled == false` and
/// short-circuits on the earlier `if !rule.enabled` check. Here the gate is genuinely evaluated.
/// Broadening it to match any `Unhealthy` cause (the natural-looking simplification of dropping
/// the `resists_auto_recovery()` guard) would pass every other test in this file, since none of
/// them reach a rule that is `Unhealthy` yet still `enabled == true` — and would silently stop
/// alerting for every rule whose pushed task merely lapsed on transient IPC loss, a detection
/// blackout for a condition that isn't even the rule's fault.
#[test]
fn a_task_expired_rule_still_alerts_locally_and_can_be_re_enabled() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();

    let mut high_cpu_rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Pattern latency test rule".to_owned(),
        "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
        "high_cpu".to_owned(),
        AlertSeverity::Medium,
    );
    high_cpu_rule.enabled = true;
    engine.load_rule(high_cpu_rule).unwrap();
    assert_eq!(
        engine.renewal_cycle(start).due().len(),
        1,
        "the first cycle issues the task"
    );

    let expired = engine.renewal_cycle(start + PUSHDOWN_TASK_TTL);
    assert_eq!(expired.expired_rules(), ["rule-1"]);
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy {
                cause: UnhealthyCause::TaskExpiry,
                ..
            })
        ),
        "expiry must mark the rule unhealthy for a recoverable cause: {:?}",
        engine.rule_health("rule-1")
    );
    assert!(
        engine.get_rule("rule-1").expect("rule tracked").enabled,
        "expiry never touches enabled -- this is what actually exercises execute_rules' gate"
    );

    let mut process = ProcessRecord::new(1234, "hog".to_owned());
    process.cpu_usage = Some(95.0);
    let alerts = engine.execute_rules(&[process]);
    assert_eq!(
        alerts.len(),
        1,
        "a recoverable cause (TaskExpiry) must not stop local alerting"
    );

    assert!(
        engine.set_rule_enabled("rule-1", true).is_ok(),
        "TaskExpiry is recoverable; re-enabling it must succeed"
    );
}

/// `set_rule_enabled(id, true)` returning `Ok` on a genuinely `Healthy`, tracked rule — the
/// documented, unrefused path. (A deferred, untracked rule also happens to return `Ok` here, since
/// an untracked rule has no health for the guard to refuse against, but that is a weaker case than
/// this one.)
#[test]
fn set_rule_enabled_true_succeeds_on_a_healthy_tracked_rule() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    assert_eq!(engine.rule_health("rule-1"), Some(&RuleHealth::Healthy));

    engine
        .set_rule_enabled("rule-1", false)
        .expect("disabling a healthy rule is always allowed");
    assert!(!engine.get_rule("rule-1").expect("rule tracked").enabled);

    assert!(
        engine.set_rule_enabled("rule-1", true).is_ok(),
        "re-enabling a healthy rule must succeed"
    );
    assert!(engine.get_rule("rule-1").expect("rule tracked").enabled);
}

/// An unknown rule id returns `Err` for either `enabled` value, as the specific `ExecutionError`
/// variant rather than the `RuleLatched` refusal a health check would produce.
#[test]
fn set_rule_enabled_on_an_unknown_id_returns_execution_error() {
    let mut engine = DetectionEngine::new();
    let result = engine.set_rule_enabled("does-not-exist", true);
    assert!(
        matches!(result, Err(DetectionEngineError::ExecutionError(_))),
        "an unknown id must be refused as ExecutionError, not RuleLatched: {result:?}"
    );
}

/// The operator-facing reason must keep the sub-millisecond precision `Duration`'s own `Debug`
/// rendering carries, not round down to a smaller, less alarming whole-number breach.
#[test]
fn a_fractional_millisecond_observation_is_not_truncated_in_the_reason() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);

    let observed = Duration::from_micros(10_400); // 10.4ms, strictly over the 10ms default
    assert!(engine.observe_pattern_latency("rule-1", generation, observed));

    let reason = unhealthy_reason(&engine, "rule-1");
    assert!(
        reason.contains("10.4ms"),
        "a fractional ms observation must not be truncated to a whole number: {reason}"
    );
}

/// A removed, latency-breached rule leaves no health row behind. A `LatencyBreach` row resists
/// every self-heal, so unlike a `TaskExpiry` row it is never revalidated back to healthy on its
/// own — a leaked row would sit unhealthy forever.
#[test]
fn removing_a_latency_breached_rule_leaves_no_health_row() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);
    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));

    assert!(engine.remove_rule("rule-1").is_some());
    assert_eq!(
        engine.rule_health("rule-1"),
        None,
        "removal must forget the rule's health row"
    );
}

/// AE3 (ADR-0012): a report measured against a superseded instance is discarded before it can
/// touch the fresh one. All four consequences are asserted, because the dangerous failure is a
/// stale report that returns `false` yet still flips `enabled`, drops the plan or marks health.
#[test]
#[traced_test]
fn a_report_for_a_superseded_generation_is_discarded_and_logged() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let stale = generation_of(&engine);
    let health_before = engine.rule_health("rule-1").cloned();

    engine.load_rule(rule(true)).unwrap();
    let fresh = generation_of(&engine);
    assert_ne!(stale, fresh, "a reload must issue a new generation");
    assert_eq!(stale.to_string(), "1");
    assert_eq!(fresh.to_string(), "2");

    let breached = engine.observe_pattern_latency("rule-1", stale, Duration::from_millis(500));

    assert!(!breached, "a stale report must return false");
    assert!(
        engine.get_rule("rule-1").expect("rule tracked").enabled,
        "a stale report must not disable the fresh instance"
    );
    assert!(
        engine.compiled_rule("rule-1").is_some(),
        "a stale report must not drop the fresh instance's plan"
    );
    assert_eq!(
        engine.rule_health("rule-1").cloned(),
        health_before,
        "a stale report must not mark the fresh instance unhealthy"
    );
    assert!(
        engine
            .runnable_rules()
            .iter()
            .any(|r| r.generation == fresh),
        "the fresh instance must still be runnable"
    );
    assert!(
        logs_contain("pattern latency report for a superseded rule instance discarded"),
        "the discard is logged"
    );
    assert!(
        logs_contain("reported_generation=1") && logs_contain("current_generation=2"),
        "the log names both generations"
    );

    assert!(
        engine.observe_pattern_latency("rule-1", fresh, Duration::from_millis(500)),
        "the same measurement against the current generation still breaches"
    );
}

/// Removing a rule retires its generation: a report for it is discarded, not applied to a
/// later rule that happens to reuse the id.
#[test]
fn a_removed_rules_generation_does_not_apply_to_a_reloaded_rule() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let before_removal = generation_of(&engine);

    assert!(engine.remove_rule("rule-1").is_some());
    engine.load_rule(rule(true)).unwrap();
    let after_reload = generation_of(&engine);
    assert_ne!(before_removal, after_reload);

    assert!(!engine.observe_pattern_latency("rule-1", before_removal, Duration::from_millis(500)));
    assert!(engine.get_rule("rule-1").expect("rule tracked").enabled);
    assert_eq!(engine.rule_health("rule-1"), Some(&RuleHealth::Healthy));
}

/// After a breach nothing makes the rule runnable except a reload, which issues a new
/// generation: the reader that matters is `runnable_rules`, not the fields the guard wrote.
#[test]
fn a_breached_rule_is_not_runnable_until_it_is_reloaded() {
    let start = SystemTime::UNIX_EPOCH;
    let mut engine = engine_with_issued_task(start);
    let generation = generation_of(&engine);
    assert!(engine.is_runnable("rule-1", generation));

    assert!(engine.observe_pattern_latency("rule-1", generation, Duration::from_millis(11)));
    assert!(engine.runnable_rules().is_empty());
    assert!(!engine.is_runnable("rule-1", generation));
    assert!(engine.set_rule_enabled("rule-1", true).is_err());
    assert!(
        engine.runnable_rules().is_empty(),
        "a refused enable must leave the rule unrunnable"
    );

    engine.load_rule(rule(true)).unwrap();
    let reloaded = generation_of(&engine);
    assert!(engine.is_runnable("rule-1", reloaded));
    assert!(
        !engine.is_runnable("rule-1", generation),
        "the breached instance's generation stays dead after the reload"
    );
}
