//! Typed predicate evaluation over one process record.
//!
//! Split out of [`super`] so the evaluator's lifecycle and its comparison semantics stay separately
//! readable. Nothing here holds state: every function decides one predicate, one literal or one
//! projection.

use super::{FieldValue, ProjectedRow, PushdownError, schema};
use daemoneye_eventbus::rpc::{ColumnDescriptor, ColumnType, PredicateOp as DescriptorOp};
use daemoneye_lib::detection::RegexRejection;
use daemoneye_lib::proto::{Literal, Predicate, PredicateOp, ProcessRecord, literal};
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
    observed: &FieldValue,
    predicate: &Predicate,
) -> Result<Option<Ordering>, PushdownError> {
    let value = first_value(predicate)?;
    compare(observed, value, &predicate.column)
}

/// `IN` over three-valued logic: a match wins, otherwise an UNKNOWN comparison makes the whole
/// predicate UNKNOWN rather than false.
pub(super) fn in_holds(
    observed: &FieldValue,
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
    observed: &FieldValue,
    value: &literal::Value,
    column: &str,
) -> Result<Option<Ordering>, PushdownError> {
    // Text is compared ahead of the match below so neither side needs a `ref` binding inside a
    // `&`-pattern, which no spelling of satisfies both `pattern_type_mismatch` and
    // `needless_borrowed_reference`.
    if let (Some(left), Some(right)) = (field_text(observed), literal_text(value)) {
        return Ok(Some(left.cmp(right)));
    }
    match (observed, value) {
        (&FieldValue::Int(left), &literal::Value::IntValue(right)) => Ok(Some(left.cmp(&right))),
        (&FieldValue::Uint(left), &literal::Value::UintValue(right)) => Ok(Some(left.cmp(&right))),
        // `partial_cmp` yields `None` for NaN, which is UNKNOWN in SQL as well.
        (&FieldValue::Float(left), &literal::Value::FloatValue(right)) => {
            Ok(left.partial_cmp(&right))
        }
        (&FieldValue::Bool(left), &literal::Value::BoolValue(right)) => Ok(Some(left.cmp(&right))),
        (_observed, &literal::Value::NullValue(_marker)) => Ok(None),
        (observed_value, literal_value) => Err(PushdownError::LiteralTypeMismatch {
            column: column.to_owned(),
            column_type: field_kind(observed_value),
            literal_kind: literal_kind(literal_value),
        }),
    }
}

/// The text an observed value carries, if it is a text column at all.
pub(super) const fn field_text(value: &FieldValue) -> Option<&str> {
    match *value {
        FieldValue::Str(ref text) => Some(text.as_str()),
        FieldValue::Int(_) | FieldValue::Uint(_) | FieldValue::Float(_) | FieldValue::Bool(_) => {
            None
        }
    }
}

/// The text a literal carries, if it is a text literal at all.
// KTD8: the wildcard arm yields `None`, which routes the comparison into the mismatch refusal
// below. `literal::Value` is `#[non_exhaustive]`, and a kind this build cannot name is never text.
#[allow(clippy::wildcard_enum_match_arm)]
pub(super) const fn literal_text(value: &literal::Value) -> Option<&str> {
    match *value {
        literal::Value::StringValue(ref text) => Some(text.as_str()),
        ref _non_text => None,
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

/// The predicate's single literal as text, for a pattern operation.
// KTD8: the wildcard arm refuses. `literal::Value` is `#[non_exhaustive]`, and a literal kind this
// build cannot name is never a pattern.
#[allow(clippy::wildcard_enum_match_arm)]
pub(super) fn first_string(predicate: &Predicate) -> Result<&str, PushdownError> {
    match *first_value(predicate)? {
        literal::Value::StringValue(ref text) => Ok(text.as_str()),
        ref other => Err(PushdownError::LiteralTypeMismatch {
            column: predicate.column.clone(),
            column_type: ColumnType::String.as_wire_name(),
            literal_kind: literal_kind(other),
        }),
    }
}

/// Names the column whose pattern the bounded compiler refused.
pub(super) fn pattern_rejected(predicate: &Predicate, rejection: RegexRejection) -> PushdownError {
    PushdownError::PatternRejected {
        column: predicate.column.clone(),
        rejection,
    }
}

/// The regular-expression source a pattern predicate compiles to.
///
/// `LIKE` is translated rather than given a second matcher: one bounded compiler, one cache.
// KTD8: the wildcard arm refuses. `PredicateOp` is `#[non_exhaustive]`; an operation newer than
// this build must not be silently treated as `REGEXP`.
#[allow(clippy::wildcard_enum_match_arm)]
pub(super) fn pattern_source(predicate: &Predicate) -> Result<String, PushdownError> {
    let source = first_string(predicate)?;
    match predicate.op() {
        PredicateOp::Like => Ok(like_to_regex(source)),
        PredicateOp::Regexp => Ok(source.to_owned()),
        _unsupported => Err(PushdownError::unusable_operation(&predicate.column)),
    }
}

/// Refuses a literal whose kind does not match the column's declared type, and a NULL literal
/// against a column declared `NOT NULL`.
///
/// Both are static defects of the plan: neither needs a row to detect, and neither can ever match.
pub(super) fn check_literal(
    column: &ColumnDescriptor,
    value: &Literal,
) -> Result<(), PushdownError> {
    let Some(ref inner) = value.value else {
        return Err(PushdownError::LiteralTypeMismatch {
            column: column.name.clone(),
            column_type: column.column_type.as_wire_name(),
            literal_kind: "unset",
        });
    };
    if matches!(*inner, literal::Value::NullValue(_marker)) {
        if column.nullable {
            return Ok(());
        }
        return Err(PushdownError::NullLiteralOnNonNullable {
            column: column.name.clone(),
        });
    }
    if literal_matches_type(inner, column.column_type) {
        return Ok(());
    }
    Err(PushdownError::LiteralTypeMismatch {
        column: column.name.clone(),
        column_type: column.column_type.as_wire_name(),
        literal_kind: literal_kind(inner),
    })
}

/// Whether a literal's kind is the one a column of `column_type` compares against.
///
/// `ColumnType::Unspecified`, and any type newer than this build, match nothing: a literal whose
/// column type cannot be named is never comparable.
pub(super) const fn literal_matches_type(value: &literal::Value, column_type: ColumnType) -> bool {
    match column_type {
        ColumnType::String => matches!(*value, literal::Value::StringValue(_)),
        ColumnType::Int => matches!(*value, literal::Value::IntValue(_)),
        ColumnType::Uint => matches!(*value, literal::Value::UintValue(_)),
        ColumnType::Float => matches!(*value, literal::Value::FloatValue(_)),
        ColumnType::Bool => matches!(*value, literal::Value::BoolValue(_)),
        ColumnType::Unspecified => false,
        _unrecognized => false,
    }
}

/// Wire name of a literal's kind, for a refusal message.
pub(super) const fn literal_kind(value: &literal::Value) -> &'static str {
    match *value {
        literal::Value::StringValue(_) => "string",
        literal::Value::IntValue(_) => "int",
        literal::Value::UintValue(_) => "uint",
        literal::Value::FloatValue(_) => "float",
        literal::Value::BoolValue(_) => "bool",
        literal::Value::NullValue(_) => "null",
        // Description only. A literal kind this build cannot name is refused by
        // `literal_matches_type`, which never returns true for one.
        ref _unrecognized => "unrecognized",
    }
}

/// Wire name of an observed value's kind, for a refusal message.
pub(super) const fn field_kind(value: &FieldValue) -> &'static str {
    match *value {
        FieldValue::Int(_) => ColumnType::Int.as_wire_name(),
        FieldValue::Uint(_) => ColumnType::Uint.as_wire_name(),
        FieldValue::Float(_) => ColumnType::Float.as_wire_name(),
        FieldValue::Bool(_) => ColumnType::Bool.as_wire_name(),
        FieldValue::Str(_) => ColumnType::String.as_wire_name(),
    }
}

/// Maps a wire operation onto the descriptor's vocabulary. `None` is never pushable.
pub(super) const fn descriptor_op(op: PredicateOp) -> Option<DescriptorOp> {
    match op {
        PredicateOp::Eq => Some(DescriptorOp::Eq),
        PredicateOp::Ne => Some(DescriptorOp::Ne),
        PredicateOp::Lt => Some(DescriptorOp::Lt),
        PredicateOp::Le => Some(DescriptorOp::Le),
        PredicateOp::Gt => Some(DescriptorOp::Gt),
        PredicateOp::Ge => Some(DescriptorOp::Ge),
        PredicateOp::In => Some(DescriptorOp::In),
        PredicateOp::Like => Some(DescriptorOp::Like),
        PredicateOp::Regexp => Some(DescriptorOp::Regexp),
        PredicateOp::Unspecified => None,
        _unrecognized => None,
    }
}

/// Translates a SQL `LIKE` pattern into an anchored regular expression.
///
/// `%` and `_` are the only wildcards; everything else is literal, and ASCII punctuation is
/// escaped so a pattern cannot smuggle regex syntax through `LIKE`. There is no `ESCAPE` clause,
/// so a backslash in a `LIKE` pattern matches a backslash.
pub(super) fn like_to_regex(pattern: &str) -> String {
    let mut translated = String::from("(?s)^");
    for character in pattern.chars() {
        match character {
            '%' => translated.push_str(".*"),
            '_' => translated.push('.'),
            literal_character => {
                if literal_character.is_ascii_punctuation() {
                    translated.push('\\');
                }
                translated.push(literal_character);
            }
        }
    }
    translated.push('$');
    translated
}
