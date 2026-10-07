//! Plan derivation: a compiled rule and a cycle window become a `DataFusion` `DataFrame`
//! (requirements R2, R3, R5; KTD5).
//!
//! The rule's own SQL never reaches the engine. The pushed half is rebuilt as typed expressions
//! from the `PushdownPlan` protos; the residual is re-parsed from the string
//! `CompiledRule::residual` returns (nothing upstream retains an AST), its `REGEXP` infix is
//! rewritten to the `regexp` function, and `DataFusion` plans the result against the provider's
//! schema through the locked-down session, so a function outside the allowlist fails there.
//!
//! The filter order is window, pushed, residual, then the projection, then a limit of
//! `cap + 1`. The extra row is how the executor tells "exactly `cap` matches" from "more than
//! `cap`, so degrade with the cap as the reason".

use std::mem;
use std::ops::ControlFlow;
use std::sync::Arc;

use datafusion::datasource::TableProvider;
use datafusion::error::DataFusionError;
use datafusion::execution::context::SessionState;
use datafusion::logical_expr::Expr;
use datafusion::prelude::{DataFrame, SessionContext, col, lit};
use datafusion::scalar::ScalarValue;
use sqlparser::ast::{
    Expr as SqlExpr, FunctionArg, FunctionArgExpr, FunctionArgumentList, FunctionArguments, Ident,
    ObjectName, SetExpr, Statement, UnaryOperator, Value, VisitMut, VisitorMut,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::detection::planner::CompiledRule;
use crate::detection_bounds::SQL_PARSER_RECURSION_LIMIT;
use crate::proto::{Literal, Predicate, PredicateOp, PushdownPlan, literal};

/// The time column every catalog table carries, in `Int64` milliseconds.
const TIME_COLUMN: &str = "collection_time";

/// The regular-expression UDF's registered name.
const REGEXP_FUNCTION: &str = "regexp";

/// The half-open interval of `collection_time` a cycle evaluates (R3).
///
/// A row stored at exactly `after_ms` belongs to the previous cycle; a row at exactly
/// `through_ms` belongs to this one. Consecutive windows `(a, b]` and `(b, c]` therefore tile the
/// timeline, so a row that matched in one cycle is not seen by the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleWindow {
    /// The previous cycle's high-water mark, exclusive.
    pub after_ms: u64,
    /// This cycle's high-water mark, inclusive.
    pub through_ms: u64,
}

/// Why a plan could not be derived.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeriveError {
    /// A predicate's operation was unset or not one this build knows.
    ///
    /// Never read as equality: a garbage operation silently becoming `=` would widen or narrow a
    /// rule's matches without a trace.
    #[error("predicate on `{column}` has an unspecified or unknown operation")]
    UnspecifiedOp {
        /// The predicate's column.
        column: String,
    },
    /// A predicate's values did not fit its operation.
    #[error("predicate on `{column}` is malformed: {reason}")]
    BadPredicate {
        /// The predicate's column.
        column: String,
        /// What was wrong with it.
        reason: &'static str,
    },
    /// The residual fragment was not a single parseable expression.
    #[error("the residual does not parse as one expression: {message}")]
    ResidualParse {
        /// The parser's diagnostic.
        message: String,
    },
    /// `DataFusion` refused to plan the derived expressions, for example a function that is not
    /// registered in the session.
    #[error("{0}")]
    Plan(#[from] DataFusionError),
}

/// Derive the `DataFrame` for one rule over one cycle window.
///
/// `cap` is `DetectionConfig::max_matches_per_rule`; the plan fetches `cap + 1` rows.
///
/// # Errors
///
/// [`DeriveError`] when a pushed predicate is malformed, the residual does not parse, or the
/// session cannot plan it.
pub fn derive(
    ctx: &SessionContext,
    provider: Arc<dyn TableProvider>,
    compiled: &CompiledRule,
    window: CycleWindow,
    cap: u32,
) -> Result<DataFrame, DeriveError> {
    derive_from_parts(
        ctx,
        provider,
        compiled.plan(),
        compiled.residual(),
        window,
        cap,
    )
}

/// [`derive()`] over a plan and residual taken apart.
///
/// Exists so the executor-side guarantees can be driven without a rule that passed the load gate:
/// a residual calling a disallowed function must fail here too, not only at load.
///
/// # Errors
///
/// As [`derive()`].
pub fn derive_from_parts(
    ctx: &SessionContext,
    provider: Arc<dyn TableProvider>,
    plan: &PushdownPlan,
    residual: Option<&str>,
    window: CycleWindow,
    cap: u32,
) -> Result<DataFrame, DeriveError> {
    let state = ctx.state();
    let mut frame = ctx.read_table(provider)?.filter(window_expr(window))?;
    for predicate in &plan.predicates {
        frame = frame.filter(predicate_to_expr(&state, predicate)?)?;
    }
    if let Some(fragment) = residual {
        let rewritten = rewrite_residual(fragment)?;
        let expr = state.create_logical_expr(&rewritten, frame.schema())?;
        frame = frame.filter(expr)?;
    }
    if !plan.projection.is_empty() {
        let columns: Vec<&str> = plan.projection.iter().map(String::as_str).collect();
        frame = frame.select_columns(&columns)?;
    }
    let fetch = usize::try_from(cap).unwrap_or(usize::MAX).saturating_add(1);
    Ok(frame.limit(0, Some(fetch))?)
}

/// `collection_time > after AND collection_time <= through`.
///
/// Bounds beyond `i64::MAX` saturate: the column is `Int64`, so no stored time exceeds it.
fn window_expr(window: CycleWindow) -> Expr {
    let as_i64 = |ms: u64| i64::try_from(ms).unwrap_or(i64::MAX);
    col(TIME_COLUMN)
        .gt(lit(as_i64(window.after_ms)))
        .and(col(TIME_COLUMN).lt_eq(lit(as_i64(window.through_ms))))
}

/// Build the typed expression for one pushed predicate.
///
/// Literals keep the type the planner lowered them to (`Int64`, `UInt64`, `Float64`, `Utf8`,
/// `Boolean`), which is the type of the column they were checked against, so the comparison needs
/// no cast and the provider sees a bare `column op literal`.
///
/// # Errors
///
/// [`DeriveError::UnspecifiedOp`] for an unset or unknown operation, and
/// [`DeriveError::BadPredicate`] for the wrong number or kind of values, or when the session has
/// no `regexp` function to plan a `REGEXP` predicate with.
pub fn predicate_to_expr(state: &SessionState, predicate: &Predicate) -> Result<Expr, DeriveError> {
    let column = predicate.column.as_str();
    let op = PredicateOp::try_from(predicate.op)
        .ok()
        .filter(|op| *op != PredicateOp::Unspecified)
        .ok_or_else(|| DeriveError::UnspecifiedOp {
            column: column.to_owned(),
        })?;
    let bad = |reason| DeriveError::BadPredicate {
        column: column.to_owned(),
        reason,
    };
    let values = predicate
        .values
        .iter()
        .map(|value| scalar_of(value).ok_or_else(|| bad("a literal carries no value")))
        .collect::<Result<Vec<ScalarValue>, DeriveError>>()?;

    if op == PredicateOp::In {
        if values.is_empty() {
            return Err(bad("IN needs at least one value"));
        }
        return Ok(col(column).in_list(values.into_iter().map(lit).collect(), false));
    }
    let [value] = <[ScalarValue; 1]>::try_from(values)
        .map_err(|_wrong_arity| bad("this operation takes exactly one value"))?;
    let operand = lit(value);
    Ok(match op {
        PredicateOp::Eq => col(column).eq(operand),
        PredicateOp::Ne => col(column).not_eq(operand),
        PredicateOp::Lt => col(column).lt(operand),
        PredicateOp::Le => col(column).lt_eq(operand),
        PredicateOp::Gt => col(column).gt(operand),
        PredicateOp::Ge => col(column).gt_eq(operand),
        PredicateOp::Like => col(column).like(operand),
        PredicateOp::Regexp => state
            .scalar_functions()
            .get(REGEXP_FUNCTION)
            .ok_or_else(|| bad("the session has no regexp function"))?
            .call(vec![col(column), operand]),
        PredicateOp::Unspecified | PredicateOp::In => {
            return Err(DeriveError::UnspecifiedOp {
                column: column.to_owned(),
            });
        }
    })
}

/// The scalar a proto literal carries, or `None` when the oneof is unset.
fn scalar_of(value: &Literal) -> Option<ScalarValue> {
    Some(match value.value {
        Some(literal::Value::StringValue(ref text)) => ScalarValue::Utf8(Some(text.clone())),
        Some(literal::Value::IntValue(number)) => ScalarValue::Int64(Some(number)),
        Some(literal::Value::UintValue(number)) => ScalarValue::UInt64(Some(number)),
        Some(literal::Value::FloatValue(number)) => ScalarValue::Float64(Some(number)),
        Some(literal::Value::BoolValue(flag)) => ScalarValue::Boolean(Some(flag)),
        Some(literal::Value::NullValue(_marker)) => ScalarValue::Null,
        None => return None,
    })
}

/// Rewrite a residual fragment's `REGEXP`/`RLIKE` infix into the `regexp(column, pattern)`
/// function and render it back to text.
///
/// The fragment is parsed inside `SELECT 1 FROM t WHERE <fragment>` under the parser's recursion
/// limit, so trailing statements, clauses or junk are refused rather than ignored. A negated
/// operator becomes `NOT regexp(..)`. Everything else, parentheses included, is rendered as
/// parsed.
///
/// # Errors
///
/// [`DeriveError::ResidualParse`] when the fragment is not exactly one expression.
pub fn rewrite_residual(fragment: &str) -> Result<String, DeriveError> {
    let mut selection = parse_selection(fragment)?;
    let _flow = selection.visit(&mut RegexpRewriter);
    Ok(selection.to_string())
}

/// The `WHERE` expression of `SELECT 1 FROM t WHERE <fragment>`, refusing anything but one query
/// with a bare selection.
fn parse_selection(fragment: &str) -> Result<SqlExpr, DeriveError> {
    let parse_error = |message: &str| DeriveError::ResidualParse {
        message: message.to_owned(),
    };
    let sql = format!("SELECT 1 FROM t WHERE {fragment}");
    let parsed = Parser::new(&GenericDialect {})
        .with_recursion_limit(SQL_PARSER_RECURSION_LIMIT)
        .try_with_sql(&sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| parse_error(&error.to_string()))?;
    let mut statements = parsed.into_iter();
    let (Some(Statement::Query(query)), None) = (statements.next(), statements.next()) else {
        return Err(parse_error("not exactly one query"));
    };
    if query.order_by.is_some() || query.limit_clause.is_some() {
        return Err(parse_error("a clause trails the expression"));
    }
    let SetExpr::Select(select) = *query.body else {
        return Err(parse_error("not a select"));
    };
    select.selection.ok_or_else(|| parse_error("no expression"))
}

/// Replaces every `RLike` node with a call to the `regexp` function, innermost first.
struct RegexpRewriter;

impl VisitorMut for RegexpRewriter {
    type Break = ();

    fn post_visit_expr(&mut self, expr: &mut SqlExpr) -> ControlFlow<Self::Break> {
        if let SqlExpr::RLike {
            negated,
            expr: ref mut operand,
            ref mut pattern,
            regexp: _,
        } = *expr
        {
            let placeholder = || SqlExpr::Value(Value::Null.with_empty_span());
            let call = regexp_call(
                mem::replace(&mut **operand, placeholder()),
                mem::replace(&mut **pattern, placeholder()),
            );
            *expr = if negated {
                SqlExpr::UnaryOp {
                    op: UnaryOperator::Not,
                    expr: Box::new(call),
                }
            } else {
                call
            };
        }
        ControlFlow::Continue(())
    }
}

/// `regexp(operand, pattern)`.
fn regexp_call(operand: SqlExpr, pattern: SqlExpr) -> SqlExpr {
    let argument = |expr| FunctionArg::Unnamed(FunctionArgExpr::Expr(expr));
    SqlExpr::Function(sqlparser::ast::Function {
        name: ObjectName::from(vec![Ident::new(REGEXP_FUNCTION)]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: vec![argument(operand), argument(pattern)],
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}
