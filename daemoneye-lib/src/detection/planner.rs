//! Lowering a rule into a pushed half and a residual half (R11, R13, R14, R15, R16, R17).
//!
//! Planning happens at rule load. Nothing here executes a rule — T6 owns execution — and nothing
//! here issues a task; the plan records what a task would carry.
//!
//! # Why eligibility is decided before capability
//!
//! `eligible_conjuncts` answers R14's question — *is this predicate implied by the whole rule?*
//! — knowing nothing about any collector. Only its output reaches `lower_conjunct` and the R15
//! capability gate. A term under `OR` therefore is never a pushdown candidate in the first place,
//! so no advertised-and-verified operation can rescue it. Reversing the two would be silent: the
//! collector would filter rows the other arm of the disjunction would have matched, and the
//! residual, which only ever sees rows the pushed half admitted, could never recover them.
//!
//! Anything the eligibility test or the lowering declines stays in the residual, which is always
//! correct. Pushing wrongly is not.

use std::collections::BTreeSet;

use sqlparser::ast::{
    BinaryOperator, Expr, Select, SelectItem, SetExpr, Statement, TableFactor, Value, Visit,
    Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::ops::ControlFlow;

use crate::detection::catalog::{SchemaCatalog, UnknownReference};
use crate::detection::regex_cache::{RegexCache, compile_rule_patterns};
use crate::detection_bounds::{PUSHDOWN_TASK_TTL, SQL_PARSER_RECURSION_LIMIT};
use crate::models::rule::{DetectionRule, RuleError};
use crate::proto::{ColumnType, Literal, Predicate, PredicateOp, PushdownPlan, literal};

/// Why a rule could not be lowered into a pushed half plus a residual (R17).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlanError {
    /// The rule's SQL did not parse.
    #[error("rule SQL does not parse: {0}")]
    Parse(String),
    /// The rule is not a single `SELECT`.
    #[error("a detection rule must be a single SELECT statement")]
    NotASelect,
    /// The rule reads no table, so there is no collector to address a plan to.
    #[error("a detection rule must read exactly one table, this one reads none")]
    NoTable,
    /// The rule reads more than one table; a plan is addressed to one owning collector.
    #[error("a detection rule must read exactly one table, this one reads {count}")]
    MultipleTables {
        /// Number of tables the rule reads.
        count: usize,
    },
    /// A table or column the rule names is not in the catalog (R11).
    #[error("{0}")]
    UnknownReference(#[from] UnknownReference),
    /// A `REGEXP` pattern in the rule failed the load-time compilation bounds.
    #[error("{0}")]
    Pattern(#[from] RuleError),
    /// The rule defines a common table expression, which the planner cannot lower.
    #[error(
        "the planner cannot lower a WITH clause, so this rule is refused rather than planned \
         against the catalog table its CTE shadows"
    )]
    CommonTableExpression,
}

/// A rule lowered into the half a collector evaluates and the half the agent keeps (R13).
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRule {
    rule_id: String,
    collector_id: String,
    plan: PushdownPlan,
    residual: Option<String>,
    references: BTreeSet<(String, String)>,
}

impl CompiledRule {
    /// The rule this plan was compiled from.
    #[must_use]
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// The collector that owns the table this plan reads.
    #[must_use]
    pub fn collector_id(&self) -> &str {
        &self.collector_id
    }

    /// The pushed half: a predicate conjunction, a projection, and the task TTL.
    #[must_use]
    pub const fn plan(&self) -> &PushdownPlan {
        &self.plan
    }

    /// The residual half, as the SQL fragment T6 will evaluate over stored rows.
    ///
    /// `None` means every predicate was pushed. It is deliberately distinct from `Some("")`, which
    /// cannot occur: "nothing left to check" and "nothing was pushed" are different facts.
    #[must_use]
    pub fn residual(&self) -> Option<&str> {
        self.residual.as_deref()
    }

    /// Every `(table, column)` reference the rule makes, for [`super::rule_health`] to track.
    #[must_use]
    pub const fn references(&self) -> &BTreeSet<(String, String)> {
        &self.references
    }
}

/// Lower `rule` against the current catalog.
///
/// # Errors
///
/// Returns a [`PlanError`] naming what refused the rule: an unparseable statement, a shape the
/// planner cannot address to one collector, a reference the catalog cannot resolve (R11), or a
/// `REGEXP` pattern that did not compile under the fixed bounds.
///
/// A rule whose predicates all land in the residual is **not** an error: its plan carries a
/// projection and no filter, which is a correct plan (R17).
pub fn plan_rule(
    catalog: &SchemaCatalog,
    cache: &RegexCache,
    rule: &DetectionRule,
    max_subquery_depth: u32,
) -> Result<CompiledRule, PlanError> {
    let statement = parse_single_select(&rule.sql_query)?;
    let Statement::Query(ref query) = statement else {
        return Err(PlanError::NotASelect);
    };
    let SetExpr::Select(ref select) = *query.body else {
        return Err(PlanError::NotASelect);
    };

    // The planner resolves the FROM name against the catalog and never reads `with`, so a CTE is
    // not merely unsupported — it silently mis-lowers. `WITH processes AS (SELECT pid FROM
    // processes WHERE pid > 100) SELECT pid FROM processes WHERE pid = 1` planned against the
    // catalog's `processes` and discarded the CTE's own filter entirely, matching more rows than
    // the operator asked for. R17 refuses a rule that cannot be lowered rather than lowering it
    // wrongly. The validation gate still accepts CTEs, which is what keeps its own coverage of
    // constructs hidden inside a CTE body meaningful.
    if query.with.is_some() {
        return Err(PlanError::CommonTableExpression);
    }

    // Every `REGEXP` literal is compiled at load whether or not it ends up pushed, so a pushed
    // pattern has provably compiled and a residual one has too. `compile_rule_patterns` runs the
    // U3 validation gate first, so this call is also where SELECT-only, the function allowlist and
    // the subquery depth limit are enforced.
    let patterns = collect_regex_patterns(select);
    let borrowed: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let _compiled = compile_rule_patterns(cache, rule, &borrowed, max_subquery_depth)?;

    let table = single_table(select)?;
    let owner = catalog
        .owner_of(&table)
        .ok_or_else(|| UnknownReference::Table {
            table: table.clone(),
        })?
        .to_owned();

    let references = resolve_all_references(catalog, &statement, &table)?;

    // R14 first: only top-level conjuncts are candidates, decided without consulting the catalog.
    let conjuncts = select
        .selection
        .as_ref()
        .map(eligible_conjuncts)
        .unwrap_or_default();

    let mut predicates = Vec::new();
    let mut residual_parts = Vec::new();
    for conjunct in conjuncts {
        // R15 second, and only for what R14 admitted.
        match lower_conjunct(catalog, &table, conjunct) {
            Some(predicate) => predicates.push(predicate),
            // Parenthesised on the way out: a conjunct that is itself a disjunction renders
            // without parentheses, and joining two of those with ` AND ` would silently reassociate
            // under SQL's precedence into a different predicate.
            None => residual_parts.push(format!("({conjunct})")),
        }
    }

    let residual = (!residual_parts.is_empty()).then(|| residual_parts.join(" AND "));
    let projection = build_projection(select, &table, &predicates, residual.as_deref())?;

    Ok(CompiledRule {
        rule_id: rule.id.raw().to_owned(),
        collector_id: owner,
        plan: PushdownPlan {
            table,
            predicates,
            projection,
            ttl_ms: ttl_millis(),
        },
        residual,
        references,
    })
}

/// [`PUSHDOWN_TASK_TTL`] in milliseconds (R16).
fn ttl_millis() -> u64 {
    u64::try_from(PUSHDOWN_TASK_TTL.as_millis()).unwrap_or(u64::MAX)
}

/// Parse the rule's SQL into exactly one statement.
fn parse_single_select(sql: &str) -> Result<Statement, PlanError> {
    let dialect = GenericDialect {};
    let mut statements = Parser::new(&dialect)
        .with_recursion_limit(SQL_PARSER_RECURSION_LIMIT)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| PlanError::Parse(error.to_string()))?;
    if statements.len() != 1 {
        return Err(PlanError::NotASelect);
    }
    statements.pop().ok_or(PlanError::NotASelect)
}

/// The single table the rule reads.
fn single_table(select: &Select) -> Result<String, PlanError> {
    let mut names = Vec::new();
    for item in &select.from {
        if !item.joins.is_empty() {
            return Err(PlanError::MultipleTables {
                count: item.joins.len().saturating_add(1),
            });
        }
        if let TableFactor::Table { ref name, .. } = item.relation {
            names.push(name.to_string());
        }
    }
    match names.len() {
        0 => Err(PlanError::NoTable),
        1 => names.pop().ok_or(PlanError::NoTable),
        count => Err(PlanError::MultipleTables { count }),
    }
}

/// Resolve every column the rule names against the catalog, naming the first that fails (R11).
fn resolve_all_references(
    catalog: &SchemaCatalog,
    statement: &Statement,
    table: &str,
) -> Result<BTreeSet<(String, String)>, PlanError> {
    let mut references = BTreeSet::new();
    for column in collect_identifiers(statement) {
        let _descriptor = catalog.resolve_reference(table, &column)?;
        let _inserted = references.insert((table.to_owned(), column));
    }
    Ok(references)
}

/// The R14 eligibility test: the top-level conjuncts of a `WHERE` clause.
///
/// A conjunct of the top-level `AND` chain qualifies, because matching `A AND B` implies
/// satisfying `A`. Everything else — a disjunction, a negation, a conjunct nested inside either —
/// is returned whole and unopened, so no term beneath it can become a candidate. The function
/// consults no catalog: eligibility is a property of the rule alone.
// KTD8: the catch-all *declines to open* the node, which is the conservative answer — the whole
// expression becomes one residual term. A `sqlparser` release adding an `Expr` variant therefore
// makes that variant less pushable, never more.
#[allow(clippy::wildcard_enum_match_arm)]
fn eligible_conjuncts(where_clause: &Expr) -> Vec<&Expr> {
    match *where_clause {
        Expr::Nested(ref inner) => eligible_conjuncts(inner),
        Expr::BinaryOp {
            ref left,
            op: BinaryOperator::And,
            ref right,
        } => {
            let mut conjuncts = eligible_conjuncts(left);
            conjuncts.extend(eligible_conjuncts(right));
            conjuncts
        }
        _ => vec![where_clause],
    }
}

/// Lower one eligible conjunct into a pushed predicate, or decline it into the residual.
///
/// Declines whenever the shape is not `column op literal`, the literal does not match the column's
/// declared type, or the R15 gate says the owning collector has not both advertised *and*
/// conformance-passed that operation on that column.
fn lower_conjunct(catalog: &SchemaCatalog, table: &str, conjunct: &Expr) -> Option<Predicate> {
    let (column, op, value_exprs) = predicate_shape(conjunct)?;
    let descriptor = catalog.resolve_reference(table, &column).ok()?;
    let column_type = ColumnType::try_from(descriptor.column_type).ok()?;
    let values = value_exprs
        .into_iter()
        .map(|expr| lower_literal(expr, column_type))
        .collect::<Option<Vec<Literal>>>()?;
    if values.is_empty() || !catalog.is_pushable(table, &column, op) {
        return None;
    }
    Some(Predicate {
        column,
        op: i32::from(op),
        values,
    })
}

/// The `column op literal` shape a pushed predicate must have, if this conjunct has one.
///
/// KTD8: every arm here decides what is *permitted* to be pushed, so the operator match stays
/// exhaustive over the shapes the planner understands and the catch-all only ever declines. A
/// negated `LIKE`, `IN` or `REGEXP` is declined rather than inverted: the De Morgan rewrite is a
/// minefield around `IN` and nullable columns, and the residual is always correct.
#[allow(clippy::wildcard_enum_match_arm)]
fn predicate_shape(conjunct: &Expr) -> Option<(String, PredicateOp, Vec<&Expr>)> {
    match *conjunct {
        Expr::Nested(ref inner) => predicate_shape(inner),
        Expr::BinaryOp {
            ref left,
            ref op,
            ref right,
        } => {
            let column = identifier_name(left)?;
            let operation = comparison_op(op)?;
            Some((column, operation, vec![right]))
        }
        Expr::Like {
            negated: false,
            any: false,
            ref expr,
            ref pattern,
            escape_char: None,
        } => Some((identifier_name(expr)?, PredicateOp::Like, vec![pattern])),
        Expr::RLike {
            negated: false,
            ref expr,
            ref pattern,
            regexp: true,
        } => Some((identifier_name(expr)?, PredicateOp::Regexp, vec![pattern])),
        Expr::InList {
            ref expr,
            ref list,
            negated: false,
        } => Some((
            identifier_name(expr)?,
            PredicateOp::In,
            list.iter().collect(),
        )),
        _ => None,
    }
}

/// The comparison operations the planner can express as a [`PredicateOp`].
///
/// KTD8: the catch-all declines, never permits, so a `BinaryOperator` variant added by a future
/// `sqlparser` release stays in the residual instead of being pushed under a guessed meaning.
#[allow(clippy::wildcard_enum_match_arm)]
const fn comparison_op(op: &BinaryOperator) -> Option<PredicateOp> {
    match *op {
        BinaryOperator::Eq => Some(PredicateOp::Eq),
        BinaryOperator::NotEq => Some(PredicateOp::Ne),
        BinaryOperator::Lt => Some(PredicateOp::Lt),
        BinaryOperator::LtEq => Some(PredicateOp::Le),
        BinaryOperator::Gt => Some(PredicateOp::Gt),
        BinaryOperator::GtEq => Some(PredicateOp::Ge),
        _ => None,
    }
}

/// The bare column name a node refers to, if it is a column reference at all.
#[allow(clippy::wildcard_enum_match_arm)]
fn identifier_name(expr: &Expr) -> Option<String> {
    match *expr {
        Expr::Identifier(ref ident) => Some(ident.value.clone()),
        Expr::CompoundIdentifier(ref parts) => parts.last().map(|ident| ident.value.clone()),
        _ => None,
    }
}

/// Lower a literal node into the typed [`Literal`] the column's declared type calls for.
///
/// A literal that does not match the declared type is declined rather than coerced: a coercion
/// the collector would perform differently is exactly the mismatch that loses rows.
#[allow(clippy::wildcard_enum_match_arm)]
fn lower_literal(expr: &Expr, column_type: ColumnType) -> Option<Literal> {
    let Expr::Value(ref spanned) = *expr else {
        return None;
    };
    let value = match (&spanned.value, column_type) {
        (&Value::Number(ref digits, _negative), ColumnType::Int) => {
            literal::Value::IntValue(digits.parse().ok()?)
        }
        (&Value::Number(ref digits, _negative), ColumnType::Uint) => {
            literal::Value::UintValue(digits.parse().ok()?)
        }
        (&Value::Number(ref digits, _negative), ColumnType::Float) => {
            literal::Value::FloatValue(digits.parse().ok()?)
        }
        (
            &Value::SingleQuotedString(ref text) | &Value::DoubleQuotedString(ref text),
            ColumnType::String,
        ) => literal::Value::StringValue(text.clone()),
        (&Value::Boolean(flag), ColumnType::Bool) => literal::Value::BoolValue(flag),
        _ => return None,
    };
    Some(Literal { value: Some(value) })
}

/// The columns the collector must return: the rule's own projection, plus every column the
/// residual reads, plus the pushed columns.
///
/// Missing a residual column here would leave T6 unable to evaluate the residual at all, which is
/// the quiet way a correct-looking split still loses rows. An empty list means every declared
/// column, which is what a wildcard projection asks for.
// KTD8 note on the wildcard arm below: it *widens* the projection to every declared column, which
// can only return more data, never less. A future `SelectItem` variant falling into it is safe in
// a way a narrowing catch-all would not be.
#[allow(clippy::wildcard_enum_match_arm)]
fn build_projection(
    select: &Select,
    table: &str,
    predicates: &[Predicate],
    residual: Option<&str>,
) -> Result<Vec<String>, PlanError> {
    let mut columns = BTreeSet::new();
    for item in &select.projection {
        match *item {
            SelectItem::UnnamedExpr(ref expr) | SelectItem::ExprWithAlias { ref expr, .. } => {
                columns.extend(collect_identifiers(expr));
            }
            // Anything else — a wildcard, or a future variant — asks for every declared column.
            // Widening is safe here in a way narrowing would not be.
            _ => return Ok(Vec::new()),
        }
    }
    if let Some(fragment) = residual {
        let parsed = parse_single_select(&format!("SELECT 1 FROM {table} WHERE {fragment}"))?;
        columns.extend(collect_identifiers(&parsed));
    }
    for predicate in predicates {
        let _inserted = columns.insert(predicate.column.clone());
    }
    Ok(columns.into_iter().collect())
}

/// Every bare column name appearing anywhere in a node.
fn collect_identifiers<N: Visit>(node: &N) -> BTreeSet<String> {
    let mut collector = IdentifierCollector {
        names: BTreeSet::new(),
    };
    let ControlFlow::Continue(()) = node.visit(&mut collector) else {
        return collector.names;
    };
    collector.names
}

/// Every `REGEXP` pattern literal in the rule, so each is compiled at load.
fn collect_regex_patterns(select: &Select) -> Vec<String> {
    let mut collector = PatternCollector {
        patterns: Vec::new(),
    };
    let ControlFlow::Continue(()) = select.visit(&mut collector) else {
        return collector.patterns;
    };
    collector.patterns
}

/// Visitor gathering bare column names. Uses the derived traversal so a reference cannot hide in a
/// corner of the tree no arm happens to name — the same reason `sql_validation` uses one.
struct IdentifierCollector {
    names: BTreeSet<String>,
}

impl Visitor for IdentifierCollector {
    type Break = ();

    #[allow(clippy::wildcard_enum_match_arm)]
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        match *expr {
            Expr::Identifier(ref ident) => {
                let _inserted = self.names.insert(ident.value.clone());
            }
            Expr::CompoundIdentifier(ref parts) => {
                if let Some(last) = parts.last() {
                    let _inserted = self.names.insert(last.value.clone());
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// Visitor gathering `REGEXP` pattern literals.
struct PatternCollector {
    patterns: Vec<String>,
}

impl Visitor for PatternCollector {
    type Break = ();

    #[allow(clippy::wildcard_enum_match_arm)]
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::RLike {
            ref pattern,
            regexp: true,
            ..
        } = *expr
            && let Expr::Value(ref spanned) = **pattern
            && let Value::SingleQuotedString(ref text) = spanned.value
        {
            self.patterns.push(text.clone());
        }
        ControlFlow::Continue(())
    }
}
