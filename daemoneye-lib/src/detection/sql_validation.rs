//! The rule-load SQL validation gate (requirements R1-R4, and R17 for the clause gate).
//!
//! A rule's SQL is parsed once, under the parser's own recursion limit, and then walked with a
//! single `sqlparser` [`Visitor`]. The visitor is what makes the gate total: the derived
//! traversal reaches every AST node, including the ones an ad-hoc recursive matcher used to walk
//! past — CTE bodies, parenthesised expressions, `IN` lists, and function arguments — so a
//! forbidden construct cannot hide in a corner of the tree that no arm happened to name.
//!
//! The gate also refuses any clause the planner has no representation for — `LIMIT`, `ORDER BY`,
//! `GROUP BY`, `HAVING` and the rest. See `unsupported_query_clause` for why that is a
//! rejection rather than something to tolerate.
//!
//! Nothing here executes a rule. Execution is ticket T6's.

use crate::detection::allowlist::is_allowed_sql_function;
use crate::detection::rejection::{SqlPosition, SqlRejection};
use crate::detection_bounds::SQL_PARSER_RECURSION_LIMIT;
use sqlparser::ast::{
    Expr, GroupByExpr, Query, Select, SetExpr, Spanned as _, Statement, TableFactor, Visit as _,
    Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Span;
use std::ops::ControlFlow;

/// Maximum number of projection items in a single `SELECT`.
const MAX_PROJECTION_ITEMS: usize = 50;

/// Maximum number of `JOIN`s across all `FROM` items of a single `SELECT`.
const MAX_JOINS: usize = 4;

/// Validate a detection rule's SQL, rejecting every construct the rule-load gate forbids.
///
/// `max_subquery_depth` is `DetectionConfig::max_subquery_depth`: the number of nesting levels
/// permitted *below* the top-level `SELECT`, so a rule with no subquery has depth 0.
///
/// # Errors
///
/// Returns the [`SqlRejection`] naming the first construct that failed, with the detail an
/// operator needs to find it and a rejection ledger needs to record it.
///
/// # Examples
///
/// ```
/// use daemoneye_lib::detection::sql_validation::validate_detection_sql;
/// assert!(validate_detection_sql("SELECT pid FROM processes", 3).is_ok());
/// assert!(validate_detection_sql("DROP TABLE processes", 3).is_err());
/// ```
pub fn validate_detection_sql(sql: &str, max_subquery_depth: u32) -> Result<(), SqlRejection> {
    let dialect = GenericDialect {};
    let statements = Parser::new(&dialect)
        .with_recursion_limit(SQL_PARSER_RECURSION_LIMIT)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| SqlRejection::ParseFailed {
            message: error.to_string(),
        })?;

    // `first()` rather than a slice pattern: a slice pattern on `&[Statement]` trips
    // `clippy::pattern_type_mismatch`, and the length check has to be explicit anyway.
    let Some(statement) = statements.first().filter(|_only| statements.len() == 1) else {
        return Err(SqlRejection::MultipleStatements {
            count: statements.len(),
        });
    };

    // KTD8: this arm decides only what is *rejected*, so a catch-all is safe and wanted —
    // `Statement` grows variants every minor release and a new one must fail closed.
    #[allow(clippy::wildcard_enum_match_arm)]
    match *statement {
        Statement::Query(_) => {}
        ref other => {
            return Err(SqlRejection::NotASelect {
                statement_kind: leading_keyword(other),
            });
        }
    }

    let mut gate = SqlGate {
        max_subquery_depth,
        query_nesting: 0,
    };
    match statement.visit(&mut gate) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(rejection) => Err(*rejection),
    }
}

/// The leading SQL keyword of a statement, for naming a rejected statement kind to an operator.
///
/// Rendering the statement back to SQL is the only way `sqlparser` exposes a statement's kind as
/// a word; it happens once, on the rejection path.
fn leading_keyword(statement: &Statement) -> String {
    statement
        .to_string()
        .split_whitespace()
        .next()
        .unwrap_or("UNKNOWN")
        .to_uppercase()
}

/// Convert a `sqlparser` span into a reportable position.
///
/// `sqlparser` uses line 0 to mean "no span recorded" and never reports a real line 0, so an
/// empty span becomes [`SqlPosition::Unknown`] rather than a fabricated offset.
const fn span_start(span: Span) -> SqlPosition {
    if span.start.line == 0 {
        SqlPosition::Unknown
    } else {
        SqlPosition::Known {
            line: span.start.line,
            column: span.start.column,
        }
    }
}

/// Walks a parsed rule and breaks on the first forbidden construct.
///
/// Depth is measured by counting `Query` nodes rather than by matching subquery-shaped `Expr`
/// variants: a subquery also appears in a CTE, in a set-operation body and as a bare function
/// argument, so variant matching would undercount exactly the cases worth catching.
struct SqlGate {
    /// The configured ceiling on nesting below the top-level `SELECT`.
    max_subquery_depth: u32,
    /// How many `Query` nodes are currently open, the outermost included.
    query_nesting: u32,
}

impl Visitor for SqlGate {
    /// Boxed on `sqlparser`'s own advice: the break value rides the recursive traversal's stack.
    type Break = Box<SqlRejection>;

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        self.query_nesting = self.query_nesting.saturating_add(1);

        // The top-level SELECT is itself a `Query`, so subquery depth is one less than the number
        // of open `Query` nodes: a rule with no subquery has depth 0.
        let depth = self.query_nesting.saturating_sub(1);
        if depth > self.max_subquery_depth {
            return ControlFlow::Break(Box::new(SqlRejection::SubqueryTooDeep {
                depth,
                max_depth: self.max_subquery_depth,
            }));
        }

        match validate_query_structure(query) {
            Ok(()) => ControlFlow::Continue(()),
            Err(rejection) => ControlFlow::Break(Box::new(rejection)),
        }
    }

    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        self.query_nesting = self.query_nesting.saturating_sub(1);
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<Self::Break> {
        match check_table_factor(table_factor) {
            Ok(()) => ControlFlow::Continue(()),
            Err(rejection) => ControlFlow::Break(Box::new(rejection)),
        }
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        let Expr::Function(ref function) = *expr else {
            return ControlFlow::Continue(());
        };
        let name = function.name.to_string();
        if is_allowed_sql_function(&name) {
            return ControlFlow::Continue(());
        }
        ControlFlow::Break(Box::new(SqlRejection::FunctionNotAllowed {
            function: name,
            position: span_start(function.name.span()),
        }))
    }
}

/// Require a query body to be a `SELECT` and check the structural bounds on it.
///
/// KTD8: this match decides what is *permitted*, so it stays exhaustive. A `SetExpr` variant
/// added by a future `sqlparser` release must break the build rather than slip through a
/// wildcard unexamined.
fn validate_query_structure(query: &Query) -> Result<(), SqlRejection> {
    if let Some(clause) = unsupported_query_clause(query) {
        return Err(SqlRejection::UnsupportedClause { clause });
    }
    match *query.body {
        SetExpr::Select(ref select) => validate_select_structure(select),
        SetExpr::Query(_) => Err(not_a_select("parenthesised subquery body")),
        SetExpr::SetOperation { .. } => Err(not_a_select("set operation")),
        SetExpr::Values(_) => Err(not_a_select("VALUES")),
        SetExpr::Insert(_) => Err(not_a_select("INSERT")),
        SetExpr::Update(_) => Err(not_a_select("UPDATE")),
        SetExpr::Delete(_) => Err(not_a_select("DELETE")),
        SetExpr::Table(_) => Err(not_a_select("TABLE")),
        SetExpr::Merge(_) => Err(not_a_select("MERGE")),
    }
}

fn not_a_select(statement_kind: &str) -> SqlRejection {
    SqlRejection::NotASelect {
        statement_kind: statement_kind.to_owned(),
    }
}

/// Check the structural bounds on one `SELECT`.
///
/// Expression contents are not inspected here — the visitor reaches every expression in the tree
/// on its own, so duplicating that walk would only create a second place for it to drift.
fn validate_select_structure(select: &Select) -> Result<(), SqlRejection> {
    if select.from.is_empty() {
        return Err(SqlRejection::MissingFrom);
    }

    if select.projection.len() > MAX_PROJECTION_ITEMS {
        return Err(SqlRejection::TooManyOf {
            construct: "SELECT column",
            found: select.projection.len(),
            limit: MAX_PROJECTION_ITEMS,
        });
    }

    let join_count = select
        .from
        .iter()
        .map(|item| item.joins.len())
        .sum::<usize>();
    if join_count > MAX_JOINS {
        return Err(SqlRejection::TooManyOf {
            construct: "JOIN",
            found: join_count,
            limit: MAX_JOINS,
        });
    }

    if let Some(clause) = unsupported_select_clause(select) {
        return Err(SqlRejection::UnsupportedClause { clause });
    }

    Ok(())
}

/// The first clause on a `Query` that the planner has no representation for, if any.
///
/// `planner::plan_rule` reads a rule's `body` and nothing else off the `Query`. Every other field
/// here would be parsed, accepted and then dropped, so the compiled rule would match a different
/// set of rows than the operator wrote — `LIMIT 1` compiling to "every match" is the plainest
/// case. Refusing at load is R17: a rule that cannot be lowered into tasks plus a residual is
/// rejected on first load.
///
/// KTD8: the destructuring names every field rather than ending in `..`, so a field added by a
/// future `sqlparser` release breaks the build instead of joining the silently-dropped set.
fn unsupported_query_clause(query: &Query) -> Option<&'static str> {
    let Query {
        // Not a clause the planner drops: a CTE body is walked by this visitor like any other
        // query, and a rule whose `FROM` names a CTE fails to resolve against the catalog.
        with: _,
        // Read by the planner.
        body: _,
        ref order_by,
        ref limit_clause,
        ref fetch,
        ref locks,
        ref for_clause,
        ref settings,
        ref format_clause,
        ref pipe_operators,
    } = *query;

    order_by
        .is_some()
        .then_some("ORDER BY")
        .or_else(|| limit_clause.is_some().then_some("LIMIT"))
        .or_else(|| fetch.is_some().then_some("FETCH"))
        .or_else(|| (!locks.is_empty()).then_some("FOR UPDATE/SHARE"))
        .or_else(|| for_clause.is_some().then_some("FOR XML/JSON"))
        .or_else(|| settings.is_some().then_some("SETTINGS"))
        .or_else(|| format_clause.is_some().then_some("FORMAT"))
        .or_else(|| (!pipe_operators.is_empty()).then_some("a pipe operator"))
}

/// The first clause on a `Select` that the planner has no representation for, if any.
///
/// The planner reads `projection`, `from` and `selection`. Everything else either filters rows
/// (`HAVING`, `QUALIFY`, `PREWHERE`), reshapes them (`GROUP BY`, `DISTINCT`), orders them or caps
/// them (`TOP`) without the plan recording it. See [`unsupported_query_clause`] for the reasoning
/// and for why this destructuring is exhaustive.
fn unsupported_select_clause(select: &Select) -> Option<&'static str> {
    let Select {
        // Positional metadata, not a clause.
        select_token: _,
        ref optimizer_hints,
        ref distinct,
        ref select_modifiers,
        ref top,
        // Only meaningful alongside `top`, which is rejected above.
        top_before_distinct: _,
        // Read by the planner.
        projection: _,
        ref exclude,
        ref into,
        // Read by the planner.
        from: _,
        ref lateral_views,
        ref prewhere,
        // Read by the planner.
        selection: _,
        ref connect_by,
        ref group_by,
        ref cluster_by,
        ref distribute_by,
        ref sort_by,
        ref having,
        ref named_window,
        ref qualify,
        // Only a spelling difference in where `QUALIFY` and `WINDOW` sit; both are rejected.
        window_before_qualify: _,
        ref value_table_mode,
        // `FROM`-first spelling of the same `SELECT`; the planner reads the fields, not the order.
        flavor: _,
    } = *select;

    (!optimizer_hints.is_empty())
        .then_some("an optimizer hint")
        .or_else(|| distinct.is_some().then_some("DISTINCT"))
        .or_else(|| select_modifiers.is_some().then_some("a SELECT modifier"))
        .or_else(|| top.is_some().then_some("TOP"))
        .or_else(|| exclude.is_some().then_some("EXCLUDE"))
        .or_else(|| into.is_some().then_some("INTO"))
        .or_else(|| (!lateral_views.is_empty()).then_some("LATERAL VIEW"))
        .or_else(|| prewhere.is_some().then_some("PREWHERE"))
        .or_else(|| (!connect_by.is_empty()).then_some("CONNECT BY"))
        .or_else(|| group_by_clause(group_by))
        .or_else(|| (!cluster_by.is_empty()).then_some("CLUSTER BY"))
        .or_else(|| (!distribute_by.is_empty()).then_some("DISTRIBUTE BY"))
        .or_else(|| (!sort_by.is_empty()).then_some("SORT BY"))
        .or_else(|| having.is_some().then_some("HAVING"))
        .or_else(|| (!named_window.is_empty()).then_some("WINDOW"))
        .or_else(|| qualify.is_some().then_some("QUALIFY"))
        .or_else(|| {
            value_table_mode
                .is_some()
                .then_some("SELECT AS VALUE/STRUCT")
        })
}

/// Name `GROUP BY` when it is actually present.
///
/// A `SELECT` with no `GROUP BY` still carries `Expressions` — with both lists empty — so the
/// absent state has to be recognised by contents rather than by variant.
fn group_by_clause(group_by: &GroupByExpr) -> Option<&'static str> {
    match *group_by {
        GroupByExpr::All(_) => Some("GROUP BY ALL"),
        GroupByExpr::Expressions(ref expressions, ref modifiers) => {
            (!expressions.is_empty() || !modifiers.is_empty()).then_some("GROUP BY")
        }
    }
}

/// Require a `FROM` item to be a plain table or a subquery.
///
/// A function name in table position is not an `Expr::Function`, so the expression walk never
/// sees it — without this check `SELECT * FROM readfile('/etc/passwd')` would load. No collector
/// serves a table-valued function, so every call-carrying form is rejected outright rather than
/// checked against the allowlist.
///
/// KTD8: this match decides what is *permitted*, so it stays exhaustive.
fn check_table_factor(table_factor: &TableFactor) -> Result<(), SqlRejection> {
    match *table_factor {
        // A plain table is fine; the same syntax with an argument list is a table-valued call.
        TableFactor::Table {
            ref name,
            args: ref arguments,
            ..
        } => {
            if arguments.is_none() {
                return Ok(());
            }
            Err(SqlRejection::FunctionNotAllowed {
                function: name.to_string(),
                position: span_start(name.span()),
            })
        }
        // A subquery or a parenthesised join carries no call of its own; their contents are
        // reached by the visitor and counted by the depth gate.
        TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => Ok(()),
        TableFactor::Function { ref name, .. } => Err(SqlRejection::FunctionNotAllowed {
            function: name.to_string(),
            position: span_start(name.span()),
        }),
        TableFactor::TableFunction { .. } => Err(unsupported_from("table function")),
        TableFactor::UNNEST { .. } => Err(unsupported_from("UNNEST")),
        TableFactor::JsonTable { .. } => Err(unsupported_from("JSON_TABLE")),
        TableFactor::OpenJsonTable { .. } => Err(unsupported_from("OPENJSON")),
        TableFactor::Pivot { .. } => Err(unsupported_from("PIVOT")),
        TableFactor::Unpivot { .. } => Err(unsupported_from("UNPIVOT")),
        TableFactor::MatchRecognize { .. } => Err(unsupported_from("MATCH_RECOGNIZE")),
        TableFactor::XmlTable { .. } => Err(unsupported_from("XMLTABLE")),
        TableFactor::SemanticView { .. } => Err(unsupported_from("SEMANTIC_VIEW")),
    }
}

const fn unsupported_from(construct: &'static str) -> SqlRejection {
    SqlRejection::UnsupportedFromItem { construct }
}
