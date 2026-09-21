//! Structured reasons a detection rule's SQL was rejected at load time.
//!
//! Every variant carries named fields rather than a pre-formatted string so that the rejection
//! ledger can record *what* failed and *where* as separate values, and so a caller can name the
//! specific offending reference back to the operator without re-parsing an error message.

use std::fmt;
use thiserror::Error;

/// Where in the rule's SQL a rejected construct sits.
///
/// `sqlparser` records a span for some AST nodes and leaves it empty for others. When no span is
/// available this is [`SqlPosition::Unknown`]: no offset is invented, because a fabricated
/// position would send an operator to the wrong part of their rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SqlPosition {
    /// `sqlparser` recorded a span; `line` and `column` both start at 1.
    Known {
        /// One-based line within the rule's SQL text.
        line: u64,
        /// One-based column within that line.
        column: u64,
    },
    /// `sqlparser` left the construct's span empty, so no real source position is recoverable.
    Unknown,
}

impl fmt::Display for SqlPosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Known { line, column } => write!(formatter, "line {line}, column {column}"),
            Self::Unknown => formatter.write_str("position unavailable"),
        }
    }
}

/// A single reason a rule's SQL failed the rule-load validation gate.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SqlRejection {
    /// The SQL did not parse. This includes tripping the parser's own recursion limit, which
    /// fires before any AST exists and therefore ahead of every semantic check below.
    #[error("SQL failed to parse: {message}")]
    ParseFailed {
        /// The parser's own diagnostic, verbatim.
        message: String,
    },

    /// The input held more than one statement.
    #[error("only a single SQL statement is allowed, found {count}")]
    MultipleStatements {
        /// How many statements the input actually held.
        count: usize,
    },

    /// The single statement was not a `SELECT`.
    #[error("only SELECT statements are allowed, found {statement_kind}")]
    NotASelect {
        /// The leading keyword of the offending statement, for example `DROP` or `INSERT`.
        statement_kind: String,
    },

    /// A function was called that is not on the allowlist.
    #[error("function `{function}` is not on the detection function allowlist ({position})")]
    FunctionNotAllowed {
        /// The function name exactly as it appeared in the rule.
        function: String,
        /// Where the call sits in the rule's SQL.
        position: SqlPosition,
    },

    /// Subqueries were nested deeper than the configured maximum.
    #[error("subquery nesting depth {depth} exceeds the configured maximum of {max_depth}")]
    SubqueryTooDeep {
        /// The deepest nesting observed, counted below the top-level `SELECT`.
        depth: u32,
        /// The configured ceiling, `DetectionConfig::max_subquery_depth`.
        max_depth: u32,
    },

    /// A structural bound on the `SELECT` was exceeded.
    #[error("{construct} count {found} exceeds the maximum of {limit}")]
    TooManyOf {
        /// What was counted, for example `JOIN` or `SELECT column`.
        construct: &'static str,
        /// How many the rule actually used.
        found: usize,
        /// The ceiling.
        limit: usize,
    },

    /// A `FROM` item was neither a plain table nor a subquery.
    #[error("unsupported FROM item: {construct}")]
    UnsupportedFromItem {
        /// The kind of `FROM` item, for example `UNNEST` or `PIVOT`.
        construct: &'static str,
    },

    /// A `SELECT` had no `FROM` clause, so it names no table to collect from.
    #[error("SELECT statement must have a FROM clause")]
    MissingFrom,
}
