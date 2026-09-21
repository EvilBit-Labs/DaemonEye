//! Only an authenticated collector can put a descriptor in the catalog (AE5, AE6, R8, R12).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::Arc;

use daemoneye_agent::{CollectorAdmission, CollectorRegistry};
use daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore;
use daemoneye_eventbus::rpc::{
    ColumnDescriptor, ColumnType, PredicateOp, RegistrationRequest, SchemaDescriptor,
    TableDescriptor,
};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::rule_health::RuleHealth;
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::rejection_log::{RegistrationGate, RejectionReason};
use tokio::sync::Mutex;

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
                    column_type: ColumnType::String,
                    nullable: false,
                    supported_ops: vec![PredicateOp::Eq],
                })
                .collect(),
        }],
        conformance_results: Vec::new(),
    }
}

fn request(
    collector_id: &str,
    token: Option<String>,
    descriptor: Option<SchemaDescriptor>,
) -> RegistrationRequest {
    RegistrationRequest {
        collector_id: collector_id.to_owned(),
        collector_type: "procmond".to_owned(),
        hostname: "localhost".to_owned(),
        version: Some("1.0.0".to_owned()),
        pid: Some(1001),
        capabilities: vec![],
        attributes: HashMap::new(),
        heartbeat_interval_ms: Some(10_000),
        descriptor,
        spawn_token: token,
    }
}

type Engine = Arc<Mutex<DetectionEngine>>;

/// A gate over a fresh token store and the one engine it feeds.
///
/// The engine is handed back so assertions read the state the planner actually plans against,
/// rather than a second copy inside the gate — which is the mis-wiring these tests exist to catch.
fn admission(dir: &tempfile::TempDir) -> (Arc<SpawnTokenStore>, Engine, Arc<CollectorAdmission>) {
    let store = Arc::new(SpawnTokenStore::new(dir.path()).unwrap());
    let engine = Arc::new(Mutex::new(DetectionEngine::new()));
    let admission = Arc::new(CollectorAdmission::new(
        Arc::clone(&store),
        Arc::clone(&engine),
    ));
    (store, engine, admission)
}

/// Load a rule that reads exactly `column`, so its health tracks that one reference.
async fn load_rule(engine: &Engine, rule_id: &str, column: &str) {
    engine
        .lock()
        .await
        .load_rule(DetectionRule::new(
            rule_id.to_owned(),
            rule_id.to_owned(),
            format!("Reads processes.{column}"),
            format!("SELECT {column} FROM processes WHERE {column} = 'evil'"),
            "test".to_owned(),
            AlertSeverity::Medium,
        ))
        .unwrap();
}

async fn catalog_is_empty(engine: &Engine) -> bool {
    engine.lock().await.catalog().is_empty()
}

async fn has_column(engine: &Engine, table: &str, column: &str) -> bool {
    engine
        .lock()
        .await
        .catalog()
        .column(table, column)
        .is_some()
}

/// Whether `rule_id` is planned and still validates against the catalog.
///
/// Deliberately "not unhealthy" rather than `RuleHealth::Healthy`: re-planning a rule re-tracks
/// its references, which resets health to `Unknown`, so a rule that just came through a re-plan
/// never reads `Healthy`. What matters here is the pair — a plan exists and nothing invalidated
/// it — which is exactly what the `ReplanOutcome::to_replan()` assertion used to stand for.
async fn is_planned_and_not_unhealthy(engine: &Engine, rule_id: &str) -> bool {
    let guard = engine.lock().await;
    guard.compiled_rule(rule_id).is_some()
        && !matches!(
            guard.rule_health(rule_id),
            Some(&RuleHealth::Unhealthy { .. })
        )
}

async fn is_unhealthy(engine: &Engine, rule_id: &str) -> bool {
    matches!(
        engine.lock().await.rule_health(rule_id),
        Some(&RuleHealth::Unhealthy { .. })
    )
}

fn gates(registry: &CollectorRegistry) -> Vec<RegistrationGate> {
    registry
        .rejection_records()
        .into_iter()
        .filter_map(|record| match record.reason {
            RejectionReason::Registration { gate, .. } => Some(gate),
            RejectionReason::RuleSql { .. }
            | RejectionReason::RuleRegex { .. }
            | RejectionReason::RuleOther { .. } => None,
            _unrecognized => None,
        })
        .collect()
}

#[tokio::test]
async fn a_registration_with_no_token_is_refused_by_the_no_token_gate_and_enters_no_catalog() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    // Act
    let error = registry
        .register(request("procmond", None, Some(descriptor(&["name"]))))
        .await
        .expect_err("a tokenless registration must be refused");

    // Assert
    assert!(
        error.to_string().contains("no spawn token presented"),
        "{error}"
    );
    assert_eq!(gates(&registry), [RegistrationGate::NoTokenPresented]);
    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_token_issued_to_another_collector_is_refused_by_the_mismatch_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _procmond = store.issue("procmond").unwrap();
    let _netmond = store.issue("netmond").unwrap();
    let other = store.expected_token("netmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    let error = registry
        .register(request(
            "procmond",
            Some(other),
            Some(descriptor(&["name"])),
        ))
        .await
        .expect_err("another collector's token must be refused");

    assert!(error.to_string().contains("did not match"), "{error}");
    assert_eq!(gates(&registry), [RegistrationGate::TokenMismatch]);
    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_collector_that_was_never_spawned_is_refused_by_the_unknown_collector_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, _engine, admission) = admission(&dir);
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    let error = registry
        .register(request(
            "ghostmond",
            Some("a".repeat(64)),
            Some(descriptor(&["name"])),
        ))
        .await
        .expect_err("an unspawned identity must be refused");

    assert!(
        error.to_string().contains("no spawn token issued"),
        "{error}"
    );
    assert_eq!(gates(&registry), [RegistrationGate::UnknownCollector]);
}

#[tokio::test]
async fn the_correct_token_admits_the_descriptor_and_lookup_can_see_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    let response = registry
        .register(request(
            "procmond",
            Some(token),
            Some(descriptor(&["name"])),
        ))
        .await
        .unwrap();

    assert!(response.accepted);
    assert!(gates(&registry).is_empty());
    assert!(!catalog_is_empty(&engine).await);
    assert!(has_column(&engine, "processes", "name").await);
}

#[tokio::test]
async fn a_descriptor_over_the_identifier_bound_is_refused_before_storage() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));
    let long = "n".repeat(200);

    let error = registry
        .register(request("procmond", Some(token), Some(descriptor(&[&long]))))
        .await
        .expect_err("an oversized identifier must be refused");

    assert!(error.to_string().contains("over the limit"), "{error}");
    assert_eq!(gates(&registry), [RegistrationGate::MalformedRequest]);
    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_descriptor_claiming_another_identity_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));
    let mut claimed = descriptor(&["name"]);
    claimed.collector_id = "netmond".to_owned();

    let error = registry
        .register(request("procmond", Some(token), Some(claimed)))
        .await
        .expect_err("a descriptor claiming another identity must be refused");

    assert!(error.to_string().contains("netmond"), "{error}");
    assert_eq!(gates(&registry), [RegistrationGate::MalformedRequest]);
    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_re_registration_dropping_a_column_leaves_only_the_rule_that_needs_it_unhealthy() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    load_rule(&engine, "needs-cmdline", "cmdline").await;
    load_rule(&engine, "needs-name", "name").await;

    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    admission
        .admit(&request(
            "procmond",
            Some(token),
            Some(descriptor(&["name", "cmdline"])),
        ))
        .await
        .unwrap();
    assert!(is_planned_and_not_unhealthy(&engine, "needs-cmdline").await);
    assert!(is_planned_and_not_unhealthy(&engine, "needs-name").await);

    // Act: the collector restarts and re-registers without `cmdline`.
    let _reissued = store.issue("procmond").unwrap();
    let reissued_token = store.expected_token("procmond").unwrap();
    admission
        .admit(&request(
            "procmond",
            Some(reissued_token),
            Some(descriptor(&["name"])),
        ))
        .await
        .unwrap();

    // Assert: exactly the rule whose column vanished is unhealthy; the other is re-planned.
    assert!(is_unhealthy(&engine, "needs-cmdline").await);
    assert!(is_planned_and_not_unhealthy(&engine, "needs-name").await);
}

#[tokio::test]
async fn a_refused_descriptor_triggers_no_replanning() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    load_rule(&engine, "needs-name", "name").await;
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let long = "n".repeat(200);

    let _error = admission
        .admit(&request(
            "procmond",
            Some(token),
            Some(descriptor(&[&long])),
        ))
        .await
        .expect_err("an oversized identifier must be refused");

    assert_eq!(
        engine.lock().await.deferred_rule_ids(),
        ["needs-name"],
        "a refused descriptor must leave the rule deferred, not planned or judged"
    );
    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_registration_carrying_no_descriptor_still_authenticates_but_widens_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();

    admission
        .admit(&request("procmond", Some(token), None))
        .await
        .expect("a descriptorless registration still authenticates");

    assert!(catalog_is_empty(&engine).await);
}

#[tokio::test]
async fn a_duplicate_registration_is_refused_without_moving_the_catalog() {
    // Arrange: a collector already registered with one descriptor.
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));
    let _accepted = registry
        .register(request(
            "procmond",
            Some(token),
            Some(descriptor(&["name", "cmdline"])),
        ))
        .await
        .unwrap();

    // Act: a second registration for the same identity, carrying a narrower descriptor.
    let _reissued = store.issue("procmond").unwrap();
    let reissued_token = store.expected_token("procmond").unwrap();
    let error = registry
        .register(request(
            "procmond",
            Some(reissued_token),
            Some(descriptor(&["name"])),
        ))
        .await
        .expect_err("a duplicate registration is refused");

    // Assert: refused, and the catalog still holds what the accepted registration put there.
    assert!(error.to_string().contains("already registered"), "{error}");
    assert_eq!(gates(&registry), [RegistrationGate::AlreadyRegistered]);
    assert!(
        has_column(&engine, "processes", "cmdline").await,
        "a refused registration must not have narrowed the catalog"
    );
}

#[tokio::test]
async fn a_restarted_collector_re_registers_after_deregistration_and_replans_rules() {
    // Covers AE6 through the public registration path, including the deregistration a real
    // collector restart implies.
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    load_rule(&engine, "needs-cmdline", "cmdline").await;
    load_rule(&engine, "needs-name", "name").await;
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let _accepted = registry
        .register(request(
            "procmond",
            Some(token),
            Some(descriptor(&["name", "cmdline"])),
        ))
        .await
        .unwrap();
    assert!(is_planned_and_not_unhealthy(&engine, "needs-cmdline").await);
    assert!(is_planned_and_not_unhealthy(&engine, "needs-name").await);

    // Act: the collector is reaped and restarts without `cmdline`.
    registry
        .deregister(daemoneye_eventbus::rpc::DeregistrationRequest {
            collector_id: "procmond".to_owned(),
            reason: None,
            force: false,
        })
        .await
        .unwrap();
    let _reissued = store.issue("procmond").unwrap();
    let reissued_token = store.expected_token("procmond").unwrap();
    let _re_accepted = registry
        .register(request(
            "procmond",
            Some(reissued_token),
            Some(descriptor(&["name"])),
        ))
        .await
        .unwrap();

    // Assert: exactly the rule that filtered on the dropped column is unhealthy.
    assert!(is_unhealthy(&engine, "needs-cmdline").await);
    assert!(is_planned_and_not_unhealthy(&engine, "needs-name").await);
    assert!(!has_column(&engine, "processes", "cmdline").await);
}

#[tokio::test]
async fn a_rule_deferred_against_an_empty_catalog_is_planned_once_a_collector_registers() {
    // R18 defers a rule loaded before any collector advertised a schema; R12 makes the first
    // registration re-validate and re-plan it. Both only hold end to end if the *real* admission
    // path reaches the planner's engine, which is what this exercises.
    let dir = tempfile::tempdir().unwrap();
    let (store, engine, admission) = admission(&dir);
    let registry = CollectorRegistry::with_admission(Arc::clone(&admission));

    load_rule(&engine, "needs-name", "name").await;
    assert_eq!(
        engine.lock().await.deferred_rule_ids(),
        ["needs-name"],
        "a rule loaded against an empty catalog waits for the first collector"
    );

    // Act: a collector registers through the authenticated registration path.
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let _accepted = registry
        .register(request(
            "procmond",
            Some(token),
            Some(descriptor(&["name"])),
        ))
        .await
        .unwrap();

    // Assert
    let guard = engine.lock().await;
    let drained = guard.deferred_rule_ids().is_empty();
    let planned = guard.compiled_rule("needs-name").is_some();
    drop(guard);
    assert!(drained, "the registration must drain the deferred queue");
    assert!(
        planned,
        "the registration must reach the planner and plan the deferred rule"
    );
}
