//! Text and boolean corpus cases, where collation does its work.
//!
//! Split out of [`super`] only for file size; the cases are part of the one shared corpus.

use super::super::{ConformanceAxis, ConformanceCase};
use super::{ALL_TEXT_OPS, case, coercion_cases, null_column_cases};
use crate::proto::{ColumnType, PredicateOp, literal};

/// Boolean columns: only equality, and NULL is not FALSE.
pub(super) fn bool_cases() -> Vec<ConformanceCase> {
    use ConformanceAxis::Boundary;
    use PredicateOp::{Eq, Ne};

    let value = literal::Value::BoolValue;
    let mut cases = vec![
        case(
            Boundary,
            ColumnType::Bool,
            Eq,
            Some(value(true)),
            vec![value(true)],
        ),
        case(
            Boundary,
            ColumnType::Bool,
            Eq,
            Some(value(false)),
            vec![value(false)],
        ),
        case(
            Boundary,
            ColumnType::Bool,
            Eq,
            Some(value(true)),
            vec![value(false)],
        ),
        case(
            Boundary,
            ColumnType::Bool,
            Ne,
            Some(value(true)),
            vec![value(false)],
        ),
        case(
            Boundary,
            ColumnType::Bool,
            Ne,
            Some(value(false)),
            vec![value(false)],
        ),
    ];
    cases.extend(null_column_cases(
        ColumnType::Bool,
        &[Eq, Ne],
        &value(false),
    ));
    cases.extend(coercion_cases(ColumnType::Bool, &[Eq, Ne], &value(true)));
    cases
}

/// Text columns: collation is the whole point, plus the empty string and `LIKE`'s anchoring.
pub(super) fn string_cases() -> Vec<ConformanceCase> {
    use ConformanceAxis::{Boundary, Collation, Null};
    use PredicateOp::{Eq, Ge, Gt, In, Le, Like, Lt, Ne, Regexp};

    let value = |text: &str| literal::Value::StringValue(text.to_owned());
    let mut cases = vec![
        // Case sensitivity. A case-insensitive default collation is the commonest divergence.
        case(
            Collation,
            ColumnType::String,
            Eq,
            Some(value("ABC")),
            vec![value("abc")],
        ),
        case(
            Collation,
            ColumnType::String,
            Ne,
            Some(value("ABC")),
            vec![value("abc")],
        ),
        case(
            Collation,
            ColumnType::String,
            Like,
            Some(value("ABC")),
            vec![value("abc%")],
        ),
        case(
            Collation,
            ColumnType::String,
            Regexp,
            Some(value("ABC")),
            vec![value("abc")],
        ),
        case(
            Collation,
            ColumnType::String,
            In,
            Some(value("ABC")),
            vec![value("abc"), value("aBc")],
        ),
        // No Unicode case folding and no expansion: `ß` is not `SS`.
        case(
            Collation,
            ColumnType::String,
            Eq,
            Some(value("Stra\u{df}e")),
            vec![value("STRASSE")],
        ),
        // No Unicode normalisation: decomposed `e` + combining acute is not precomposed `é`.
        case(
            Collation,
            ColumnType::String,
            Eq,
            Some(value("e\u{301}")),
            vec![value("\u{e9}")],
        ),
        // Ordering, for a collector that advertises it: byte order, so `Z` (0x5A) precedes `a`
        // (0x61) and any non-ASCII codepoint follows every ASCII one.
        case(
            Collation,
            ColumnType::String,
            Lt,
            Some(value("Z")),
            vec![value("a")],
        ),
        case(
            Collation,
            ColumnType::String,
            Gt,
            Some(value("\u{e9}")),
            vec![value("z")],
        ),
        case(
            Collation,
            ColumnType::String,
            Ge,
            Some(value("abc")),
            vec![value("abc")],
        ),
        case(
            Collation,
            ColumnType::String,
            Lt,
            Some(value("abc")),
            vec![value("abcd")],
        ),
        case(
            Collation,
            ColumnType::String,
            Le,
            Some(value("Z")),
            vec![value("a")],
        ),
        // The empty string sorts before everything, and the inclusive edge holds.
        case(
            Boundary,
            ColumnType::String,
            Lt,
            Some(value("")),
            vec![value("a")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Le,
            Some(value("a")),
            vec![value("a")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Gt,
            Some(value("a")),
            vec![value("")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Ge,
            Some(value("")),
            vec![value("")],
        ),
        // `LIKE` has no ESCAPE clause: a backslash is a literal backslash, so `a\_b` matches
        // `a\Xb` and does not match `a_b`.
        case(
            Collation,
            ColumnType::String,
            Like,
            Some(value("a_b")),
            vec![value("a\\_b")],
        ),
        case(
            Collation,
            ColumnType::String,
            Like,
            Some(value("a\\zb")),
            vec![value("a\\_b")],
        ),
        // The empty string is a value, never a synonym for NULL.
        case(
            Boundary,
            ColumnType::String,
            Eq,
            Some(value("")),
            vec![value("")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Eq,
            Some(value("")),
            vec![value("x")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Ne,
            Some(value("")),
            vec![value("x")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("")),
            vec![value("%")],
        ),
        // `LIKE` is anchored and `REGEXP` is not; conflating them is a silent divergence.
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("abc")),
            vec![value("abc")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("abc")),
            vec![value("b")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("abc")),
            vec![value("a_c")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Regexp,
            Some(value("abc")),
            vec![value("b")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Regexp,
            Some(value("abc")),
            vec![value("^b")],
        ),
        // `%` and `_` are wildcards only in `LIKE`; everything else is literal, so a pattern
        // cannot smuggle regular-expression syntax in.
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("a.c")),
            vec![value("a.c")],
        ),
        case(
            Boundary,
            ColumnType::String,
            Like,
            Some(value("abc")),
            vec![value("a.c")],
        ),
        case(
            Boundary,
            ColumnType::String,
            In,
            Some(value("b")),
            vec![value("a"), value("b")],
        ),
        case(
            Boundary,
            ColumnType::String,
            In,
            Some(value("c")),
            vec![value("a"), value("b")],
        ),
        case(
            Null,
            ColumnType::String,
            In,
            Some(value("a")),
            vec![value("a"), literal::Value::NullValue(true)],
        ),
        case(
            Null,
            ColumnType::String,
            In,
            Some(value("z")),
            vec![value("a"), literal::Value::NullValue(true)],
        ),
    ];
    cases.extend(null_column_cases(
        ColumnType::String,
        &ALL_TEXT_OPS,
        &value("abc"),
    ));
    cases.extend(coercion_cases(
        ColumnType::String,
        &ALL_TEXT_OPS,
        &value("42"),
    ));
    cases
}
