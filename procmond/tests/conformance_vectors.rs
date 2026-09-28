//! procmond's conformance self-test: every advertised operation is verified against the shared
//! corpus before registration, and the results ride the descriptor (R15, R22).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use daemoneye_eventbus::rpc::{ConformanceResult, PredicateOp};
use procmond::pushdown_eval::PushdownEvaluator;
use procmond::pushdown_eval::conformance::{descriptor_with_conformance, self_test};

const COLLECTOR_ID: &str = "procmond-conformance";

fn results() -> Vec<ConformanceResult> {
    self_test(&PushdownEvaluator::new(COLLECTOR_ID))
}

#[test]
fn every_advertised_operation_passes_its_vector() {
    // Arrange + Act
    let results = results();

    // Assert
    let failures: Vec<&ConformanceResult> =
        results.iter().filter(|result| !result.passed).collect();
    assert!(
        failures.is_empty(),
        "procmond disagrees with the reference on: {failures:?}"
    );
}

#[test]
fn a_result_is_reported_for_every_advertised_operation() {
    // Arrange
    let evaluator = PushdownEvaluator::new(COLLECTOR_ID);
    let advertised: usize = evaluator
        .descriptor()
        .tables
        .iter()
        .flat_map(|table| table.columns.iter())
        .map(|column| column.supported_ops.len())
        .sum();

    // Act
    let results = self_test(&evaluator);

    // Assert
    assert_eq!(
        results.len(),
        advertised,
        "R22 wants one result per advertised operation"
    );
}

#[test]
fn the_registration_descriptor_carries_the_results() {
    // Act
    let descriptor = descriptor_with_conformance(COLLECTOR_ID);

    // Assert
    assert_eq!(descriptor.collector_id, COLLECTOR_ID);
    assert!(!descriptor.conformance_results.is_empty());
    assert!(
        descriptor
            .conformance_results
            .iter()
            .any(|result| result.column == "name" && result.op == PredicateOp::Like)
    );
}
