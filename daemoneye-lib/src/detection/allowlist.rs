//! The set of SQL functions a detection rule is permitted to call.
//!
//! This is an **allowlist**: membership is the control, and anything absent is rejected at rule
//! load. It replaced an earlier denylist outright — the two cannot coexist, because a reader who
//! found the denylist first would take it for the authoritative gate and a function omitted from
//! it would look deliberately permitted.
//!
//! The list governs `sqlparser`'s `Expr::Function` call sites only. Several SQL constructs that
//! *look* like function calls parse into dedicated AST nodes instead and therefore never reach
//! this gate: `SUBSTR`/`SUBSTRING` become `Expr::Substring`, and `CAST`, `TRIM`, `POSITION`,
//! `EXTRACT`, `CEIL` and `FLOOR` each have their own variant. Adding those names here would be
//! decoration, so they are deliberately absent.

/// Functions a detection rule may call, lowercase and sorted for bisection and review.
///
/// Membership is derived from what process analysis actually needs:
///
/// - `avg`, `count`, `max`, `min`, `sum` — aggregates over a single result set. A *threshold* on
///   an aggregate is a `HAVING`, and `sql_validation` now refuses `GROUP BY` and `HAVING` outright
///   because the planner can represent neither; these names remain reachable only in a projection
///   or a `WHERE`. Whether they should stay on the list at all is a product call, not this gate's.
/// - `hex`, `unhex` — inspecting `executable_hash` and other binary metadata.
/// - `instr`, `length` — locating and measuring substrings in names, paths and command lines.
/// - `like`, `match`, `regexp` — pattern matching over process fields.
///
/// File, system, evaluation, shell, formatting and randomness functions are absent by design:
/// none has a meaning in process monitoring, and each is a route out of the query sandbox.
///
/// T6's `SessionContext` restriction and T14's injection suite both reference this constant, so
/// it is the single place a function becomes reachable by a rule.
pub const ALLOWED_SQL_FUNCTIONS: &[&str] = &[
    "avg", "count", "hex", "instr", "length", "like", "match", "max", "min", "regexp", "sum",
    "unhex",
];

/// Returns `true` when `name` is on [`ALLOWED_SQL_FUNCTIONS`], compared case-insensitively.
///
/// SQL function names are not case-sensitive, so `COUNT`, `Count` and `count` are one function.
///
/// # Examples
///
/// ```
/// use daemoneye_lib::detection::allowlist::is_allowed_sql_function;
/// assert!(is_allowed_sql_function("COUNT"));
/// assert!(!is_allowed_sql_function("readfile"));
/// ```
#[must_use]
pub fn is_allowed_sql_function(name: &str) -> bool {
    ALLOWED_SQL_FUNCTIONS
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}
