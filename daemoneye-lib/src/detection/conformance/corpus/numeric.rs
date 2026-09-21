//! Numeric corpus cases: signed, unsigned and floating-point columns.
//!
//! Split out of [`super`] only for file size; the cases are part of the one shared corpus.

use super::super::{ConformanceAxis, ConformanceCase};
use super::{ORDERING_OPS, SCALAR_OPS, case, coercion_cases, null_column_cases};
use crate::proto::{ColumnType, PredicateOp, literal};

/// Signed-integer columns: the extremes, zero, the strict/inclusive edges, and NULL.
pub(super) fn int_cases() -> Vec<ConformanceCase> {
    use ConformanceAxis::{Boundary, Null};
    use PredicateOp::{Eq, Ge, Gt, In, Le, Lt, Ne};

    let value = literal::Value::IntValue;
    let mut cases = vec![
        // Extremes compare as themselves rather than wrapping.
        case(
            Boundary,
            ColumnType::Int,
            Eq,
            Some(value(i64::MAX)),
            vec![value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Eq,
            Some(value(i64::MIN)),
            vec![value(i64::MIN)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Eq,
            Some(value(0)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Ne,
            Some(value(i64::MIN)),
            vec![value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Lt,
            Some(value(i64::MIN)),
            vec![value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Gt,
            Some(value(i64::MAX)),
            vec![value(i64::MIN)],
        ),
        // The inclusive/exclusive edge, at the extremes where an off-by-one shows.
        case(
            Boundary,
            ColumnType::Int,
            Lt,
            Some(value(i64::MIN)),
            vec![value(i64::MIN)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Le,
            Some(value(i64::MIN)),
            vec![value(i64::MIN)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Gt,
            Some(value(i64::MAX)),
            vec![value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Ge,
            Some(value(i64::MAX)),
            vec![value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Lt,
            Some(value(-1)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            Ge,
            Some(value(0)),
            vec![value(-1)],
        ),
        // `IN` spanning the range, and `IN` with a NULL sibling: a match still wins.
        case(
            Boundary,
            ColumnType::Int,
            In,
            Some(value(0)),
            vec![value(i64::MIN), value(0), value(i64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Int,
            In,
            Some(value(7)),
            vec![value(1), value(2)],
        ),
        case(
            Null,
            ColumnType::Int,
            In,
            Some(value(7)),
            vec![value(7), literal::Value::NullValue(true)],
        ),
        case(
            Null,
            ColumnType::Int,
            In,
            Some(value(9)),
            vec![value(7), literal::Value::NullValue(true)],
        ),
    ];
    cases.extend(null_column_cases(ColumnType::Int, &ORDERING_OPS, &value(7)));
    cases.extend(null_column_cases(ColumnType::Int, &[In], &value(7)));
    cases.extend(coercion_cases(ColumnType::Int, &SCALAR_OPS, &value(42)));
    cases
}

/// Unsigned columns: zero and `u64::MAX`, where a signed round-trip would wrap.
pub(super) fn uint_cases() -> Vec<ConformanceCase> {
    use ConformanceAxis::{Boundary, Null};
    use PredicateOp::{Eq, Ge, Gt, In, Le, Lt, Ne};

    let value = literal::Value::UintValue;
    let mut cases = vec![
        case(
            Boundary,
            ColumnType::Uint,
            Eq,
            Some(value(u64::MAX)),
            vec![value(u64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Eq,
            Some(value(0)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Ne,
            Some(value(0)),
            vec![value(u64::MAX)],
        ),
        // `u64::MAX` read as a signed value would be -1, and this would flip.
        case(
            Boundary,
            ColumnType::Uint,
            Gt,
            Some(value(u64::MAX)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Lt,
            Some(value(0)),
            vec![value(u64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Lt,
            Some(value(0)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Le,
            Some(value(0)),
            vec![value(0)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Gt,
            Some(value(u64::MAX)),
            vec![value(u64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Ge,
            Some(value(u64::MAX)),
            vec![value(u64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            In,
            Some(value(0)),
            vec![value(0), value(u64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            In,
            Some(value(3)),
            vec![value(1), value(2)],
        ),
        // Mid-range ordering, so a column too narrow to hold `u64::MAX` — `pid` is 32 bits — still
        // has a runnable case for every ordering operation rather than none at all.
        case(
            Boundary,
            ColumnType::Uint,
            Gt,
            Some(value(7)),
            vec![value(3)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Gt,
            Some(value(3)),
            vec![value(7)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Ge,
            Some(value(7)),
            vec![value(7)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Ge,
            Some(value(3)),
            vec![value(7)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Lt,
            Some(value(3)),
            vec![value(7)],
        ),
        case(
            Boundary,
            ColumnType::Uint,
            Le,
            Some(value(7)),
            vec![value(7)],
        ),
        case(
            Null,
            ColumnType::Uint,
            In,
            Some(value(3)),
            vec![value(3), literal::Value::NullValue(true)],
        ),
    ];
    cases.extend(null_column_cases(
        ColumnType::Uint,
        &ORDERING_OPS,
        &value(7),
    ));
    cases.extend(null_column_cases(ColumnType::Uint, &[In], &value(7)));
    cases.extend(coercion_cases(ColumnType::Uint, &SCALAR_OPS, &value(42)));
    cases
}

/// Float columns: NaN is UNKNOWN under every comparison, and signed zero compares equal.
pub(super) fn float_cases() -> Vec<ConformanceCase> {
    use ConformanceAxis::Boundary;
    use PredicateOp::{Eq, Ge, Gt, In, Le, Lt, Ne};

    let value = literal::Value::FloatValue;
    let mut cases = vec![
        // NaN is UNKNOWN, not FALSE and not TRUE: `= NaN` and `!= NaN` both exclude.
        case(
            Boundary,
            ColumnType::Float,
            Eq,
            Some(value(f64::NAN)),
            vec![value(f64::NAN)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Ne,
            Some(value(f64::NAN)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Lt,
            Some(value(f64::NAN)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Ge,
            Some(value(f64::NAN)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            In,
            Some(value(f64::NAN)),
            vec![value(f64::NAN)],
        ),
        // IEEE signed zero: -0.0 == 0.0.
        case(
            Boundary,
            ColumnType::Float,
            Eq,
            Some(value(-0.0)),
            vec![value(0.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Eq,
            Some(value(f64::MAX)),
            vec![value(f64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Gt,
            Some(value(f64::INFINITY)),
            vec![value(f64::MAX)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Lt,
            Some(value(f64::NEG_INFINITY)),
            vec![value(f64::MIN)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Gt,
            Some(value(1.0)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Ge,
            Some(value(1.0)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            Le,
            Some(value(1.0)),
            vec![value(1.0)],
        ),
        case(
            Boundary,
            ColumnType::Float,
            In,
            Some(value(1.5)),
            vec![value(1.5), value(2.5)],
        ),
    ];
    cases.extend(null_column_cases(
        ColumnType::Float,
        &ORDERING_OPS,
        &value(1.0),
    ));
    cases.extend(null_column_cases(ColumnType::Float, &[In], &value(1.0)));
    cases.extend(coercion_cases(ColumnType::Float, &SCALAR_OPS, &value(42.0)));
    cases
}
