//! Durable-record behaviour for rule-load and registration rejections (U5, R4/R8).
//!
//! The store is agent-side and in-memory: `audit_ledger` is procmond's to write
//! (`AGENTS.md`), so nothing here opens it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::{
    detection::{DetectionEngine, SqlRejection},
    detection_bounds::MAX_REJECTION_RECORDS,
    models::{AlertSeverity, DetectionRule},
    rejection_log::{RegistrationGate, RejectionLog, RejectionReason},
};

fn rule(id: &str, sql: &str) -> DetectionRule {
    DetectionRule::new(
        id.to_owned(),
        "Test Rule".to_owned(),
        "Test detection rule".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::Medium,
    )
}

#[test]
fn rejected_rule_produces_one_record_naming_rule_and_construct() {
    // Arrange
    let mut engine = DetectionEngine::new();

    // Act
    let result = engine.load_rule(rule("bad-rule", "DROP TABLE processes"));

    // Assert
    assert!(result.is_err());
    let records = engine.rejection_log().records();
    assert_eq!(records.len(), 1);
    assert!(
        matches!(
            records[0].reason,
            RejectionReason::RuleSql {
                rejection: SqlRejection::NotASelect { .. },
                ..
            }
        ),
        "the record must carry the structured rejection, not a flattened string"
    );
    let rendered = records[0].reason.to_string();
    assert!(
        rendered.contains("bad-rule"),
        "record must name the rule: {rendered}"
    );
    assert!(
        rendered.contains("DROP"),
        "record must name the offending construct: {rendered}"
    );
}

#[test]
fn successful_load_writes_no_rejection_record() {
    // Arrange
    let mut engine = DetectionEngine::new();

    // Act
    engine
        .load_rule(rule(
            "good-rule",
            "SELECT name FROM processes WHERE name = 'x'",
        ))
        .expect("valid rule loads");

    // Assert
    assert!(engine.rejection_log().records().is_empty());
}

#[test]
fn registration_rejection_record_cannot_carry_the_presented_token() {
    // Arrange
    const TOKEN: &str = "ZZsupersecretspawntokenZZ";
    let mut log = RejectionLog::new();

    // Act: the recording API takes a gate, never the request that held TOKEN.
    let record = log.record(RejectionReason::registration(
        "collector-alpha",
        RegistrationGate::TokenMismatch,
    ));

    // Assert: no rendering of the record can reproduce the token.
    let surfaces = [
        record.reason.to_string(),
        format!("{record:?}"),
        format!("{log:?}"),
        record.payload_hash.clone(),
        record.entry_hash.clone(),
    ];
    for surface in &surfaces {
        // Message carries no secret material: CodeQL rust/cleartext-logging.
        assert!(
            !surface.contains(TOKEN),
            "a record surface reproduced the presented token"
        );
    }
    assert!(record.reason.to_string().contains("collector-alpha"));
}

#[test]
fn consecutive_rejections_form_a_chain_that_verifies() {
    // Arrange
    let mut log = RejectionLog::new();

    // Act
    for id in ["a", "b", "c"] {
        log.record(RejectionReason::registration(
            id,
            RegistrationGate::NoTokenPresented,
        ));
    }

    // Assert
    assert_eq!(log.records().len(), 3);
    assert_eq!(log.records()[0].previous_hash, None);
    assert_eq!(
        log.records()[1].previous_hash.as_deref(),
        Some(log.records()[0].entry_hash.as_str())
    );
    log.verify_integrity().expect("an untouched chain verifies");
}

#[test]
fn the_log_stops_growing_at_the_cap_and_drops_the_oldest_first() {
    // Arrange: registration is reachable over IPC, so a collector retrying with a stale token is
    // the shape this bound exists for.
    const OVERFLOW: usize = 5;

    let mut log = RejectionLog::new();
    let evicted = u64::try_from(OVERFLOW).unwrap();
    let last_sequence = u64::try_from(MAX_REJECTION_RECORDS + OVERFLOW - 1).unwrap();

    // Act
    for index in 0..MAX_REJECTION_RECORDS + OVERFLOW {
        log.record(RejectionReason::registration(
            &format!("collector-{index}"),
            RegistrationGate::TokenMismatch,
        ));
    }

    // Assert: the window is full, not longer, and it is the *newest* records that survived.
    assert_eq!(log.records().len(), MAX_REJECTION_RECORDS);
    assert_eq!(
        log.records()[0].sequence,
        evicted,
        "the oldest retained record must be the first one not evicted"
    );
    assert_eq!(
        log.records()[MAX_REJECTION_RECORDS - 1].sequence,
        last_sequence,
        "sequence numbers must keep counting past the cap rather than repeating"
    );
    assert!(
        log.records()
            .iter()
            .all(|record| !matches!(record.reason, RejectionReason::RuleSql { .. })),
        "only registration rejections were written"
    );
}

#[test]
fn the_retained_window_still_verifies_after_eviction() {
    // Arrange
    let mut log = RejectionLog::new();

    // Act: write one more than the cap, so the chain head has been evicted.
    for index in 0..=MAX_REJECTION_RECORDS {
        log.record(RejectionReason::registration(
            &format!("collector-{index}"),
            RegistrationGate::NoTokenPresented,
        ));
    }

    // Assert: the oldest retained record names a predecessor that is gone, and that is not a
    // discontinuity — every link that *is* still checkable holds.
    assert!(log.records()[0].previous_hash.is_some());
    assert_ne!(log.records()[0].sequence, 0);
    log.verify_integrity()
        .expect("an evicted prefix is not a chain break");
}
