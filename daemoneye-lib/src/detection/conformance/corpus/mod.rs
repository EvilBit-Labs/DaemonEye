//! The conformance corpus itself: the cases both sides read.
//!
//! Every case is stated against a column's *declared type*, never a column name, so one corpus
//! serves every collector. Cases are selected by `(column_type, nullable, op)`, which is what keeps
//! the corpus from demanding semantics a descriptor never claimed: a collector that does not
//! advertise ordering comparisons on text — procmond deliberately does not — simply never runs the
//! collation-ordering cases.
//!
//! Coverage is per operation, across the four axes of [`super::ConformanceAxis`]:
//!
//! - **Null** — a NULL column value under every operation, and a NULL element inside `IN`. Only
//!   cases where a divergence flips admission are here; see the module docs of [`super`].
//! - **Coercion** — a literal of every other kind against each column type. All refuse: there is no
//!   coercion in either direction.
//! - **Collation** — case sensitivity, Unicode normalisation, byte-versus-codepoint ordering, and
//!   `LIKE`'s absent `ESCAPE` clause.
//! - **Boundary** — integer extremes, zero, the empty string, NaN and signed zero, and the
//!   inclusive-versus-exclusive edge of every ordering comparison.

mod numeric;
mod text;

use super::{ConformanceAxis, ConformanceCase};
use crate::proto::{ColumnType, PredicateOp, literal};
use std::sync::OnceLock;

/// The shared corpus, built once.
///
/// Both the agent's reference and every collector's self-test read this same slice; if they read
/// different data, "agreement" would mean nothing.
#[must_use]
pub fn corpus() -> &'static [ConformanceCase] {
    static CORPUS: OnceLock<Vec<ConformanceCase>> = OnceLock::new();
    CORPUS.get_or_init(build_corpus)
}

/// Assemble every case, grouped by the column type it reads.
fn build_corpus() -> Vec<ConformanceCase> {
    let mut cases = Vec::new();
    cases.extend(numeric::int_cases());
    cases.extend(numeric::uint_cases());
    cases.extend(numeric::float_cases());
    cases.extend(text::bool_cases());
    cases.extend(text::string_cases());
    cases
}

/// One case.
pub(super) const fn case(
    axis: ConformanceAxis,
    column_type: ColumnType,
    op: PredicateOp,
    observed: Option<literal::Value>,
    literals: Vec<literal::Value>,
) -> ConformanceCase {
    ConformanceCase {
        axis,
        column_type,
        op,
        observed,
        literals,
    }
}

/// A NULL column value under every operation named, which must never admit.
pub(super) fn null_column_cases(
    column_type: ColumnType,
    ops: &[PredicateOp],
    literal: &literal::Value,
) -> Vec<ConformanceCase> {
    ops.iter()
        .map(|op| {
            case(
                ConformanceAxis::Null,
                column_type,
                *op,
                None,
                vec![literal.clone()],
            )
        })
        .collect()
}

/// Every literal kind that is *not* `column_type`, for every operation named. All refuse: the
/// coercion axis is generated per operation so no advertised operation is left without it.
pub(super) fn coercion_cases(
    column_type: ColumnType,
    ops: &[PredicateOp],
    observed: &literal::Value,
) -> Vec<ConformanceCase> {
    ops.iter()
        .flat_map(|op| coercion_cases_for_op(column_type, *op, observed))
        .collect()
}

/// Every literal kind that is *not* `column_type`, each of which must refuse.
fn coercion_cases_for_op(
    column_type: ColumnType,
    op: PredicateOp,
    observed: &literal::Value,
) -> Vec<ConformanceCase> {
    let candidates = [
        literal::Value::StringValue("42".to_owned()),
        literal::Value::IntValue(42),
        literal::Value::UintValue(42),
        literal::Value::FloatValue(42.0),
        literal::Value::BoolValue(true),
    ];
    candidates
        .into_iter()
        .filter(|candidate| !super::matches_column_type(candidate, column_type))
        .map(|candidate| {
            case(
                ConformanceAxis::Coercion,
                column_type,
                op,
                Some(observed.clone()),
                vec![candidate],
            )
        })
        .collect()
}

/// The ordering comparisons a scalar column can advertise, `IN` excluded.
pub(super) const ORDERING_OPS: [PredicateOp; 6] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
];

/// Every operation a scalar column can advertise.
pub(super) const SCALAR_OPS: [PredicateOp; 7] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::In,
];

/// Every operation a text column can advertise, ordering included: a collector may advertise it
/// even though procmond does not.
pub(super) const ALL_TEXT_OPS: [PredicateOp; 9] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::In,
    PredicateOp::Like,
    PredicateOp::Regexp,
];
