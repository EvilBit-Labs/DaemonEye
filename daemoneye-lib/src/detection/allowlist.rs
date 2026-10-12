//! The set of SQL functions a detection rule is permitted to call.
//!
//! This is an **allowlist**: membership is the control, and anything absent is rejected at rule
//! load. It replaced an earlier denylist outright — the two cannot coexist, because a reader who
//! found the denylist first would take it for the authoritative gate and a function omitted from
//! it would look deliberately permitted.
//!
//! The list governs `sqlparser`'s `Expr::Function` call sites. Constructs that *look* like
//! function calls but parse into dedicated AST nodes (`SUBSTR`/`SUBSTRING`, `TRIM`, `POSITION`,
//! `EXTRACT`, `CEIL`, `FLOOR`, ...) never reach it; `sql_validation::function_construct` refuses
//! those by variant, and `CAST` has its own gate. Adding their names here would be decoration, so
//! they are deliberately absent.

/// Functions a detection rule may call, lowercase and sorted for bisection and review.
///
/// Membership is derived from what process analysis actually needs:
///
/// - `hex`, `unhex` — inspecting `executable_hash` and other binary metadata.
/// - `instr`, `length` — locating and measuring substrings in names, paths and command lines.
/// - `like`, `match`, `regexp` — pattern matching over process fields.
///
/// Aggregates (`avg`, `count`, `max`, `min`, `sum`) are absent because the planner cannot compute
/// one. A pushdown plan is a predicate, a projection and a residual; an aggregate is none of those,
/// so `build_projection` reduces `count(pid)` to the column `pid` and the rule returns rows where
/// the operator asked for a count. R17 refuses a rule that cannot be lowered, so the aggregate is
/// refused here, at the gate that names the function to the operator. A threshold on an aggregate
/// is a `HAVING`, which `sql_validation` refuses for the same reason.
///
/// File, system, evaluation, shell, formatting and randomness functions are absent by design:
/// none has a meaning in process monitoring, and each is a route out of the query sandbox.
///
/// T6's `SessionContext` restriction and T14's injection suite both reference this constant, so
/// it is the single place a function becomes reachable by a rule.
pub const ALLOWED_SQL_FUNCTIONS: &[&str] =
    &["hex", "instr", "length", "like", "match", "regexp", "unhex"];

/// Returns `true` when `name` is on [`ALLOWED_SQL_FUNCTIONS`], compared case-insensitively.
///
/// SQL function names are not case-sensitive, so `LENGTH`, `Length` and `length` are one function.
///
/// # Examples
///
/// ```
/// use daemoneye_lib::detection::allowlist::is_allowed_sql_function;
/// assert!(is_allowed_sql_function("LENGTH"));
/// assert!(!is_allowed_sql_function("readfile"));
/// ```
#[must_use]
pub fn is_allowed_sql_function(name: &str) -> bool {
    ALLOWED_SQL_FUNCTIONS
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}
