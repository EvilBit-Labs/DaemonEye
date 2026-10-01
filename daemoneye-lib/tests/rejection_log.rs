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
    crypto::Blake3Hasher,
    detection::{DetectionEngine, DetectionEngineError, SqlRejection},
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

/// Covers AE1's audit half: the record `load_rule` writes when the allowlist gate refuses a rule.
/// `readfile` rather than an aggregate, which T6 may re-admit.
#[test]
fn a_disallowed_function_is_named_in_exactly_one_rejection_record() {
    // Arrange
    let mut engine = DetectionEngine::new();

    // Act
    let result = engine.load_rule(rule(
        "ae1-rule",
        "SELECT readfile('/etc/passwd') FROM processes",
    ));

    // Assert
    assert!(result.is_err());
    let records = engine.rejection_log().records();
    assert_eq!(
        records.len(),
        1,
        "one refusal must write exactly one record"
    );
    let RejectionReason::RuleSql {
        rejection: SqlRejection::FunctionNotAllowed { ref function, .. },
        ..
    } = records[0].reason
    else {
        // Five gates share the outer error variant, so only the rejection identifies this one.
        panic!(
            "expected the allowlist gate in the record, got {:?}",
            records[0].reason
        );
    };
    assert_eq!(function, "readfile");
    let rendered = records[0].reason.to_string();
    assert!(
        rendered.contains("ae1-rule"),
        "record must name the rule: {rendered}"
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

// --- U4: injection vectors are rejected and audited (R7, KTD5) --------------------------------

/// Which gate a vector's rejection must satisfy, and the field that names it.
///
/// A dedicated enum rather than a boxed closure: a closure capturing its expected literal cannot
/// coerce to a `fn` pointer, so a `const` table of `(rule_id, sql, expected)` needs a plain value
/// here instead.
#[derive(Debug, Clone, Copy)]
enum Gate {
    NotASelect(&'static str),
    MultipleStatements(usize),
    FunctionNotAllowed(&'static str),
}

impl Gate {
    /// Whether `rejection` trips this gate, naming both the variant and its discriminating field.
    fn matches(self, rejection: &SqlRejection) -> bool {
        match self {
            Self::NotASelect(expected) => matches!(
                *rejection,
                SqlRejection::NotASelect { ref statement_kind } if statement_kind == expected
            ),
            Self::MultipleStatements(expected) => matches!(
                *rejection,
                SqlRejection::MultipleStatements { count } if count == expected
            ),
            Self::FunctionNotAllowed(expected) => matches!(
                *rejection,
                SqlRejection::FunctionNotAllowed { ref function, .. }
                    if function.to_lowercase() == expected
            ),
        }
    }
}

/// The nine basic injection vectors, each paired with the gate its refusal must trip.
const INJECTION_VECTORS: &[(&str, &str, Gate)] = &[
    ("u4-drop", "DROP TABLE processes", Gate::NotASelect("DROP")),
    (
        "u4-insert",
        "INSERT INTO processes (pid) VALUES (1)",
        Gate::NotASelect("INSERT"),
    ),
    (
        "u4-update",
        "UPDATE processes SET name = 'x'",
        Gate::NotASelect("UPDATE"),
    ),
    (
        "u4-delete",
        "DELETE FROM processes",
        Gate::NotASelect("DELETE"),
    ),
    (
        "u4-multi-statement",
        "SELECT pid FROM processes; DROP TABLE processes",
        Gate::MultipleStatements(2),
    ),
    (
        "u4-union",
        "SELECT pid FROM processes UNION SELECT pid FROM processes",
        Gate::NotASelect("set operation"),
    ),
    (
        "u4-union-in-cte",
        "WITH t AS (SELECT pid FROM processes UNION SELECT pid FROM processes) SELECT pid FROM t",
        Gate::NotASelect("set operation"),
    ),
    (
        "u4-readfile",
        "SELECT * FROM readfile('/etc/passwd')",
        Gate::FunctionNotAllowed("readfile"),
    ),
    (
        "u4-load-extension",
        "SELECT load_extension('evil') FROM processes",
        Gate::FunctionNotAllowed("load_extension"),
    ),
];

/// Ties each injection vector to the single rejection record its refusal must write.
///
/// One `DetectionEngine::new()` for the whole table (KTD5): no collector is registered, so every
/// vector meets the validation gate in `load_rule` and never reaches the planner.
#[test]
fn injection_vectors_are_rejected_and_each_writes_one_bound_record() {
    // Arrange
    let mut engine = DetectionEngine::new();

    // Act + Assert, one vector at a time.
    for &(rule_id, sql, gate) in INJECTION_VECTORS {
        let before = engine.rejection_log().records().len();

        let result = engine.load_rule(rule(rule_id, sql));

        assert!(
            matches!(result, Err(DetectionEngineError::SqlValidationError(_))),
            "{rule_id} must be refused at the validation gate, got {result:?}"
        );
        assert!(
            engine.get_rule(rule_id).is_none(),
            "{rule_id} must not be loaded after refusal"
        );

        let records = engine.rejection_log().records();
        assert_eq!(
            records.len(),
            before + 1,
            "{rule_id}'s refusal must write exactly one record"
        );
        let record = &records[records.len() - 1];

        let RejectionReason::RuleSql {
            rule_id: ref recorded_id,
            ref rejection,
        } = record.reason
        else {
            panic!("{rule_id} must record RuleSql, got {:?}", record.reason);
        };
        assert_eq!(
            recorded_id.as_str(),
            rule_id,
            "record must name the refused rule"
        );
        assert!(
            gate.matches(rejection),
            "{rule_id} expected {gate:?} to match, got {rejection:?}"
        );

        // `verify_integrity` recomputes `entry_hash` from the stored `payload_hash` and never
        // re-derives it from `reason`, so chain acceptance alone would not catch a `payload_hash`
        // that drifted from the audited rule id and gate. Pin the binding directly.
        let rendered = record.reason.to_string();
        assert_eq!(
            record.payload_hash,
            Blake3Hasher::hash_string(&rendered),
            "{rule_id}'s payload_hash must be the BLAKE3 hash of the rendered reason"
        );
    }

    let records = engine.rejection_log().records();
    assert_eq!(records.len(), INJECTION_VECTORS.len());
    engine
        .rejection_log()
        .verify_integrity()
        .expect("a chain of nine rejections must verify");
}
