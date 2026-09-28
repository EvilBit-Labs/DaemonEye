//! Row model, reference predicate evaluator, and rule generator for the planner property test.
//!
//! One evaluator serves both halves of the split: the residual is evaluated by
//! [`eval_sql_predicate`] and the whole predicate is evaluated by the same function over the same
//! rows, so the property is about decomposition rather than about two evaluators agreeing.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm
)]

use std::collections::BTreeMap;

use daemoneye_lib::proto::{Predicate, PredicateOp, PushdownPlan, literal::Value as LiteralValue};
use proptest::prelude::*;
use sqlparser::ast::{BinaryOperator, Expr, SetExpr, Statement, UnaryOperator, Value};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Columns the generated rules address. Every one is declared `INT` in the property catalog.
pub const PROPERTY_COLUMNS: [&str; 4] = ["a", "b", "c", "d"];

/// Range of column values and literals the generator draws from.
const VALUE_RANGE: i64 = 4;

/// One in-memory row: every column the catalog declares, or the subset a projection kept.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    columns: BTreeMap<String, i64>,
}

impl Row {
    /// An empty row, for tests that only need the type.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// The row a collector would return under `projection`. An empty projection means every column.
    #[must_use]
    pub fn project(&self, projection: &[String]) -> Self {
        if projection.is_empty() {
            return self.clone();
        }
        Self {
            columns: self
                .columns
                .iter()
                .filter(|&(name, _value)| projection.contains(name))
                .map(|(name, &value)| (name.clone(), value))
                .collect(),
        }
    }

    /// The value of `column`.
    ///
    /// # Panics
    ///
    /// Panics when the column is absent, which is exactly the failure a projection that does not
    /// cover the residual's columns would produce in production.
    #[must_use]
    pub fn get(&self, column: &str) -> i64 {
        *self
            .columns
            .get(column)
            .unwrap_or_else(|| panic!("projection dropped column `{column}` the residual reads"))
    }
}

/// A small fixed row set spanning the generator's value range in every column.
#[must_use]
pub fn rows_fixture() -> Vec<Row> {
    (0..VALUE_RANGE)
        .flat_map(|first| (0..VALUE_RANGE).map(move |second| (first, second)))
        .map(|(first, second)| Row {
            columns: BTreeMap::from([
                ("a".to_owned(), first),
                ("b".to_owned(), second),
                (
                    "c".to_owned(),
                    first.wrapping_add(second).rem_euclid(VALUE_RANGE),
                ),
                (
                    "d".to_owned(),
                    first.wrapping_mul(second).rem_euclid(VALUE_RANGE),
                ),
            ]),
        })
        .collect()
}

/// A generated predicate tree. `Or` and `Not` are first-class so the generator exercises the case
/// the R14 eligibility test exists for, not just flat conjunctions.
#[derive(Debug, Clone)]
pub enum Pred {
    /// `column op literal`.
    Cmp {
        /// Index into [`PROPERTY_COLUMNS`].
        column: usize,
        /// SQL comparison operator, as written.
        op: &'static str,
        /// Right-hand literal.
        value: i64,
    },
    /// `(left AND right)`.
    And(Box<Self>, Box<Self>),
    /// `(left OR right)`.
    Or(Box<Self>, Box<Self>),
    /// `NOT (inner)`.
    Not(Box<Self>),
}

/// Render a generated predicate as the SQL a rule would carry.
#[must_use]
pub fn render(predicate: &Pred) -> String {
    match *predicate {
        Pred::Cmp { column, op, value } => {
            let name = PROPERTY_COLUMNS[column];
            format!("{name} {op} {value}")
        }
        Pred::And(ref left, ref right) => format!("({} AND {})", render(left), render(right)),
        Pred::Or(ref left, ref right) => format!("({} OR {})", render(left), render(right)),
        Pred::Not(ref inner) => format!("NOT ({})", render(inner)),
    }
}

/// Generator producing genuine `AND`/`OR`/`NOT` nesting, not just top-level conjunctions.
pub fn pred_strategy() -> impl Strategy<Value = Pred> {
    let leaf = (
        0..PROPERTY_COLUMNS.len(),
        prop::sample::select(vec!["=", "!=", "<", "<=", ">", ">="]),
        0..VALUE_RANGE,
    )
        .prop_map(|(column, op, value)| Pred::Cmp { column, op, value });

    leaf.prop_recursive(4, 24, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone())
                .prop_map(|(left, right)| Pred::And(Box::new(left), Box::new(right))),
            (inner.clone(), inner.clone())
                .prop_map(|(left, right)| Pred::Or(Box::new(left), Box::new(right))),
            inner.prop_map(|only| Pred::Not(Box::new(only))),
        ]
    })
}

/// Evaluate a SQL `WHERE` fragment against one row.
///
/// # Panics
///
/// Panics on a construct the generator never produces, so an unexpected residual shape fails the
/// property test loudly instead of quietly evaluating to `false`.
#[must_use]
pub fn eval_sql_predicate(where_sql: &str, row: &Row) -> bool {
    let dialect = GenericDialect {};
    let sql = format!("SELECT * FROM processes WHERE {where_sql}");
    let statements = Parser::parse_sql(&dialect, &sql)
        .unwrap_or_else(|error| panic!("residual `{where_sql}` does not parse: {error}"));
    let statement = statements.first().expect("expected one statement");
    let Statement::Query(ref query) = *statement else {
        panic!("expected a query");
    };
    let SetExpr::Select(ref select) = *query.body else {
        panic!("expected a select");
    };
    let selection = select.selection.as_ref().expect("expected a WHERE clause");
    eval_expr(selection, row)
}

/// Recursive evaluator over the subset of `Expr` the generator can produce.
fn eval_expr(expr: &Expr, row: &Row) -> bool {
    match *expr {
        Expr::Nested(ref inner) => eval_expr(inner, row),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr: ref operand,
        } => !eval_expr(operand, row),
        Expr::BinaryOp {
            ref left,
            ref op,
            ref right,
        } => eval_binary(left, op, right, row),
        ref other => panic!("evaluator reached an unexpected node: {other:?}"),
    }
}

/// Evaluate one binary node: a logical connective, or a comparison of a column to a literal.
fn eval_binary(left: &Expr, op: &BinaryOperator, right: &Expr, row: &Row) -> bool {
    match *op {
        BinaryOperator::And => return eval_expr(left, row) && eval_expr(right, row),
        BinaryOperator::Or => return eval_expr(left, row) || eval_expr(right, row),
        _ => {}
    }
    let observed = column_value(left, row);
    let expected = literal_value(right);
    match *op {
        BinaryOperator::Eq => observed == expected,
        BinaryOperator::NotEq => observed != expected,
        BinaryOperator::Lt => observed < expected,
        BinaryOperator::LtEq => observed <= expected,
        BinaryOperator::Gt => observed > expected,
        BinaryOperator::GtEq => observed >= expected,
        ref other => panic!("evaluator reached an unexpected operator: {other:?}"),
    }
}

/// The row's value for a bare column reference.
fn column_value(expr: &Expr, row: &Row) -> i64 {
    match *expr {
        Expr::Identifier(ref ident) => row.get(&ident.value),
        ref other => panic!("expected a column reference, got {other:?}"),
    }
}

/// The integer a literal node carries.
fn literal_value(expr: &Expr) -> i64 {
    match *expr {
        Expr::Value(ref value) => match value.value {
            Value::Number(ref digits, _negative) => {
                digits.parse().expect("generated literals are integers")
            }
            ref other => panic!("expected an integer literal, got {other:?}"),
        },
        ref other => panic!("expected a literal, got {other:?}"),
    }
}

/// Whether the collector, evaluating the pushed half, would return this row.
#[must_use]
pub fn pushed_admits(plan: &PushdownPlan, row: &Row) -> bool {
    plan.predicates
        .iter()
        .all(|predicate| predicate_holds(predicate, row))
}

/// Evaluate one pushed predicate the way a conforming collector would.
fn predicate_holds(predicate: &Predicate, row: &Row) -> bool {
    let observed = row.get(&predicate.column);
    let expected = match predicate
        .values
        .first()
        .and_then(|literal| literal.value.clone())
    {
        Some(LiteralValue::IntValue(value)) => value,
        ref other => panic!("expected one integer literal, got {other:?}"),
    };
    match PredicateOp::try_from(predicate.op) {
        Ok(PredicateOp::Eq) => observed == expected,
        Ok(PredicateOp::Ne) => observed != expected,
        Ok(PredicateOp::Lt) => observed < expected,
        Ok(PredicateOp::Le) => observed <= expected,
        Ok(PredicateOp::Gt) => observed > expected,
        Ok(PredicateOp::Ge) => observed >= expected,
        other => panic!("collector model reached an unexpected operation: {other:?}"),
    }
}
