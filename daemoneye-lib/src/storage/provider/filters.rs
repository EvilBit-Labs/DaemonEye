//! Which predicates the provider consumes, and what it takes from them (R10, R12).
//!
//! One parser serves both [`classify`] (for `supports_filters_pushdown`) and
//! [`PushedFilters::from_filters`] (for `scan`), so a predicate is reported as consumed exactly
//! when `scan` acts on it. Whatever this module cannot parse is `Unsupported`, which is always
//! safe: `DataFusion` keeps it in a `FilterExec`.

use crate::detection_bounds::MAX_IN_VALUES;
use crate::storage::read::IndexTerm;
use datafusion::arrow::datatypes::Schema;
use datafusion::logical_expr::{BinaryExpr, Expr, Operator};
use datafusion::scalar::ScalarValue;

/// A predicate the scan can use, and what it narrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Pushable {
    /// A `collection_time` comparison, as a half-open millisecond window.
    Time { start_ms: u64, end_ms: u64 },
    /// An equality or `IN` on an indexed column: any one of these terms may match.
    Terms(Vec<IndexTerm>),
}

/// Classify one filter, or `None` when the scan cannot use it.
pub(super) fn classify(expr: &Expr, schema: &Schema) -> Option<Pushable> {
    if let Expr::BinaryExpr(ref binary) = *expr {
        return classify_binary(binary, schema);
    }
    if let Expr::InList(ref list) = *expr {
        if list.negated || list.list.is_empty() || list.list.len() > MAX_IN_VALUES {
            return None;
        }
        let column = list.expr.try_as_col()?;
        has_column(schema, &column.name)?;
        return list
            .list
            .iter()
            .map(|item| item.as_literal().and_then(|v| term_for(&column.name, v)))
            .collect::<Option<Vec<_>>>()
            .map(Pushable::Terms);
    }
    None
}

fn classify_binary(binary: &BinaryExpr, schema: &Schema) -> Option<Pushable> {
    let (name, op, value) =
        if let (Some(c), Some(v)) = (binary.left.try_as_col(), binary.right.as_literal()) {
            (c.name.as_str(), binary.op, v)
        } else if let (Some(v), Some(c)) = (binary.left.as_literal(), binary.right.try_as_col()) {
            (c.name.as_str(), binary.op.swap()?, v)
        } else {
            return None;
        };
    has_column(schema, name)?;
    if name == "collection_time" {
        let (start_ms, end_ms) = time_window(op, int_of(value)?)?;
        return Some(Pushable::Time { start_ms, end_ms });
    }
    if op != Operator::Eq {
        return None;
    }
    term_for(name, value).map(|term| Pushable::Terms(vec![term]))
}

fn has_column(schema: &Schema, name: &str) -> Option<()> {
    schema.index_of(name).ok().map(|_index| ())
}

/// `[start, end)` in milliseconds for `collection_time <op> t`; negative times clamp to zero.
fn time_window(op: Operator, t: i64) -> Option<(u64, u64)> {
    let clamp = |x: i64| u64::try_from(x.max(0)).unwrap_or(0);
    let next = t.saturating_add(1);
    if op == Operator::GtEq {
        return Some((clamp(t), u64::MAX));
    }
    if op == Operator::Gt {
        return Some((clamp(next), u64::MAX));
    }
    if op == Operator::Lt {
        return Some((0, clamp(t)));
    }
    if op == Operator::LtEq {
        return Some((0, clamp(next)));
    }
    if op == Operator::Eq {
        return Some((clamp(t), clamp(next)));
    }
    None
}

fn int_of(value: &ScalarValue) -> Option<i64> {
    if let ScalarValue::Int64(Some(v)) = *value {
        return Some(v);
    }
    if let ScalarValue::UInt64(Some(v)) = *value {
        return i64::try_from(v).ok();
    }
    None
}

const fn str_of(value: &ScalarValue) -> Option<&str> {
    if let ScalarValue::Utf8(Some(ref s))
    | ScalarValue::LargeUtf8(Some(ref s))
    | ScalarValue::Utf8View(Some(ref s)) = *value
    {
        return Some(s.as_str());
    }
    None
}

fn term_for(column: &str, value: &ScalarValue) -> Option<IndexTerm> {
    let as_u32 = || int_of(value).and_then(|v| u32::try_from(v).ok());
    match column {
        "pid" => as_u32().map(IndexTerm::Pid),
        "ppid" => as_u32().map(IndexTerm::Ppid),
        "name" => str_of(value).map(IndexTerm::name),
        "executable_hash" => str_of(value).and_then(IndexTerm::exe_hash),
        _ => None,
    }
}

/// Everything `scan` takes from its filters: a time window and index term sets to intersect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PushedFilters {
    start_ms: u64,
    end_ms: u64,
    term_sets: Vec<Vec<IndexTerm>>,
}

impl PushedFilters {
    /// Fold every consumable filter; the rest are ignored (and re-checked above the scan).
    pub(super) fn from_filters(filters: &[Expr], schema: &Schema) -> Self {
        filters.iter().filter_map(|f| classify(f, schema)).fold(
            Self {
                start_ms: 0,
                end_ms: u64::MAX,
                term_sets: Vec::new(),
            },
            |mut acc, pushable| {
                match pushable {
                    Pushable::Time { start_ms, end_ms } => {
                        acc.start_ms = acc.start_ms.max(start_ms);
                        acc.end_ms = acc.end_ms.min(end_ms);
                    }
                    Pushable::Terms(terms) => acc.term_sets.push(terms),
                }
                acc
            },
        )
    }

    pub(super) const fn start_ms(&self) -> u64 {
        self.start_ms
    }

    pub(super) const fn end_ms(&self) -> u64 {
        self.end_ms
    }

    pub(super) const fn term_set_count(&self) -> usize {
        self.term_sets.len()
    }

    /// Term sets to intersect; within a set, any term may match.
    pub(super) fn term_sets(&self) -> &[Vec<IndexTerm>] {
        &self.term_sets
    }
}
