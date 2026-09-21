//! The rule-load SQL validation gate (requirements R1-R4).
//!
//! A rule's SQL is parsed once, under the parser's own recursion limit, and then walked with a
//! single `sqlparser` [`Visitor`]. The visitor is what makes the gate total: the derived
//! traversal reaches every AST node, including the ones an ad-hoc recursive matcher used to walk
//! past — CTE bodies, parenthesised expressions, `IN` lists, and function arguments — so a
//! forbidden construct cannot hide in a corner of the tree that no arm happened to name.
//!
//! Nothing here executes a rule. Execution is ticket T6's.

use crate::detection::allowlist::is_allowed_sql_function;
use crate::detection::rejection::{SqlPosition, SqlRejection};
use crate::detection_bounds::SQL_PARSER_RECURSION_LIMIT;
use sqlparser::ast::{
    Expr, Query, Select, SetExpr, Spanned as _, Statement, TableFactor, Visit as _, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Span;
use std::ops::ControlFlow;

/// Maximum number of projection items in a single `SELECT`.
const MAX_PROJECTION_ITEMS: usize = 50;

/// Maximum number of `JOIN`s across all `FROM` items of a single `SELECT`.
const MAX_JOINS: usize = 4;

/// Maximum number of explicit `GROUP BY` expressions in a single `SELECT`.
const MAX_GROUP_BY_EXPRESSIONS: usize = 10;

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

    match select.group_by {
        sqlparser::ast::GroupByExpr::All(_) => Ok(()),
        sqlparser::ast::GroupByExpr::Expressions(ref expressions, _) => {
            if expressions.len() > MAX_GROUP_BY_EXPRESSIONS {
                return Err(SqlRejection::TooManyOf {
                    construct: "GROUP BY column",
                    found: expressions.len(),
                    limit: MAX_GROUP_BY_EXPRESSIONS,
                });
            }
            Ok(())
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
