//! Typed predicate evaluation over one process record.
//!
//! Split out of [`super`] so the evaluator's lifecycle and its comparison semantics stay separately
//! readable. Nothing here holds state: every function decides one predicate, one literal or one
//! projection.

use super::schema::FieldRef;
use super::{ProjectedRow, PushdownError, schema};
use collector_core::pushdown::{literal_kind, literal_text};
use daemoneye_lib::detection::RegexRejection;
use daemoneye_lib::proto::{Predicate, ProcessRecord, literal};
use std::cmp::Ordering;

/// The projected row for one record: exactly the named columns, never a superset.
pub(super) fn project(
    record: &ProcessRecord,
    projection: &[String],
) -> Result<ProjectedRow, PushdownError> {
    let mut row = ProjectedRow::new();
    for name in projection {
        let value = schema::field_value(record, name)?;
        row.insert(name.clone(), value);
    }
    Ok(row)
}

/// Compares the observed value with the predicate's single literal. `None` is UNKNOWN.
pub(super) fn compare_first(
    observed: FieldRef<'_>,
    predicate: &Predicate,
) -> Result<Option<Ordering>, PushdownError> {
    let value = first_value(predicate)?;
    compare(observed, value, &predicate.column)
}

/// `IN` over three-valued logic: a match wins, otherwise an UNKNOWN comparison makes the whole
/// predicate UNKNOWN rather than false.
pub(super) fn in_holds(
    observed: FieldRef<'_>,
    predicate: &Predicate,
) -> Result<Option<bool>, PushdownError> {
    if predicate.values.is_empty() {
        return Err(PushdownError::bad_arity(predicate));
    }
    let mut unknown = false;
    for literal in &predicate.values {
        let value = literal
            .value
            .as_ref()
            .ok_or_else(|| PushdownError::bad_arity(predicate))?;
        match compare(observed, value, &predicate.column)? {
            Some(Ordering::Equal) => return Ok(Some(true)),
            Some(_unequal) => {}
            None => unknown = true,
        }
    }
    Ok(if unknown { None } else { Some(false) })
}

/// Same-kind comparison. A cross-kind literal is a refusal, and a NULL literal is UNKNOWN.
pub(super) fn compare(
    observed: FieldRef<'_>,
    value: &literal::Value,
    column: &str,
) -> Result<Option<Ordering>, PushdownError> {
    // Text is compared ahead of the match below so neither side needs a `ref` binding inside a
    // `&`-pattern, which no spelling of satisfies both `pattern_type_mismatch` and
    // `needless_borrowed_reference`.
    if let (Some(left), Some(right)) = (observed.text(), literal_text(value)) {
        return Ok(Some(left.cmp(right)));
    }
    match (observed, value) {
        (FieldRef::Int(left), &literal::Value::IntValue(right)) => Ok(Some(left.cmp(&right))),
        (FieldRef::Uint(left), &literal::Value::UintValue(right)) => Ok(Some(left.cmp(&right))),
        // `partial_cmp` yields `None` for NaN, which is UNKNOWN in SQL as well.
        (FieldRef::Float(left), &literal::Value::FloatValue(right)) => Ok(left.partial_cmp(&right)),
        (FieldRef::Bool(left), &literal::Value::BoolValue(right)) => Ok(Some(left.cmp(&right))),
        (_observed, &literal::Value::NullValue(_marker)) => Ok(None),
        (observed_value, literal_value) => Err(PushdownError::LiteralTypeMismatch {
            column: column.to_owned(),
            column_type: observed_value.kind(),
            literal_kind: literal_kind(literal_value),
        }),
    }
}

/// The predicate's single literal value.
pub(super) fn first_value(predicate: &Predicate) -> Result<&literal::Value, PushdownError> {
    predicate
        .values
        .first()
        .and_then(|literal| literal.value.as_ref())
        .ok_or_else(|| PushdownError::bad_arity(predicate))
}

/// Names the column whose pattern the bounded compiler refused.
pub(super) fn pattern_rejected(predicate: &Predicate, rejection: RegexRejection) -> PushdownError {
    PushdownError::PatternRejected {
        column: predicate.column.clone(),
        rejection,
    }
}
