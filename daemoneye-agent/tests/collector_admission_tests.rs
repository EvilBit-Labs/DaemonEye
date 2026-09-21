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
use daemoneye_lib::rejection_log::{RegistrationGate, RejectionReason};

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

fn admission(dir: &tempfile::TempDir) -> (Arc<SpawnTokenStore>, Arc<CollectorAdmission>) {
    let store = Arc::new(SpawnTokenStore::new(dir.path()).unwrap());
    let admission = Arc::new(CollectorAdmission::new(Arc::clone(&store)));
    (store, admission)
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
    let (store, admission) = admission(&dir);
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
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_token_issued_to_another_collector_is_refused_by_the_mismatch_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
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
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_collector_that_was_never_spawned_is_refused_by_the_unknown_collector_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, admission) = admission(&dir);
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
    let (store, admission) = admission(&dir);
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
    assert!(!admission.catalog_is_empty().await);
    assert!(admission.has_column("processes", "name").await);
}

#[tokio::test]
async fn a_descriptor_over_the_identifier_bound_is_refused_before_storage() {
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
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
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_descriptor_claiming_another_identity_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
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
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_re_registration_dropping_a_column_leaves_only_the_rule_that_needs_it_unhealthy() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
    admission
        .track_rule("needs-cmdline", [("processes", "cmdline")])
        .await;
    admission
        .track_rule("needs-name", [("processes", "name")])
        .await;

    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    let first = admission
        .admit(&request(
            "procmond",
            Some(token),
            Some(descriptor(&["name", "cmdline"])),
        ))
        .await
        .unwrap();
    assert_eq!(first.to_replan().len(), 2);

    // Act: the collector restarts and re-registers without `cmdline`.
    let _reissued = store.issue("procmond").unwrap();
    let reissued_token = store.expected_token("procmond").unwrap();
    let outcome = admission
        .admit(&request(
            "procmond",
            Some(reissued_token),
            Some(descriptor(&["name"])),
        ))
        .await
        .unwrap();

    // Assert
    assert_eq!(outcome.newly_unhealthy(), ["needs-cmdline"]);
    assert_eq!(outcome.to_replan(), ["needs-name"]);
    assert_eq!(
        admission.unhealthy_rules().await,
        [String::from("needs-cmdline")]
    );
}

#[tokio::test]
async fn a_refused_descriptor_triggers_no_replanning() {
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
    admission
        .track_rule("needs-name", [("processes", "name")])
        .await;
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

    assert!(admission.unhealthy_rules().await.is_empty());
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_registration_carrying_no_descriptor_still_authenticates_but_widens_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();

    let outcome = admission
        .admit(&request("procmond", Some(token), None))
        .await
        .unwrap();

    assert!(outcome.to_replan().is_empty());
    assert!(admission.catalog_is_empty().await);
}

#[tokio::test]
async fn a_duplicate_registration_is_refused_without_moving_the_catalog() {
    // Arrange: a collector already registered with one descriptor.
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
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
        admission.has_column("processes", "cmdline").await,
        "a refused registration must not have narrowed the catalog"
    );
}

#[tokio::test]
async fn a_restarted_collector_re_registers_after_deregistration_and_replans_rules() {
    // Covers AE6 through the public registration path, including the deregistration a real
    // collector restart implies.
    let dir = tempfile::tempdir().unwrap();
    let (store, admission) = admission(&dir);
    admission
        .track_rule("needs-cmdline", [("processes", "cmdline")])
        .await;
    admission
        .track_rule("needs-name", [("processes", "name")])
        .await;
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
    assert!(admission.unhealthy_rules().await.is_empty());

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
    assert_eq!(
        admission.unhealthy_rules().await,
        [String::from("needs-cmdline")]
    );
    assert!(!admission.has_column("processes", "cmdline").await);
}
