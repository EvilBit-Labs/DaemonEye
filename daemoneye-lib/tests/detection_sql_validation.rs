//! Rule-load SQL validation gate (T5 / U3, requirements R1-R4).
//!
//! Every rejection case asserts on the discriminating field of [`SqlRejection`], never on the
//! outer [`RuleError`] variant: five gates share one outer variant, so a variant-only assertion
//! could not tell which of them fired. See
//! `docs/solutions/best-practices/assert-which-gate-fired-when-error-variants-collide-2026-09-17.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use daemoneye_lib::detection::allowlist::{ALLOWED_SQL_FUNCTIONS, is_allowed_sql_function};
use daemoneye_lib::detection::{SqlPosition, SqlRejection};
use daemoneye_lib::models::alert::AlertSeverity;
use daemoneye_lib::models::rule::{DetectionRule, RuleError};
use proptest::prelude::{Just, ProptestConfig, Strategy as _, prop};
use proptest::{prop_assert, prop_assert_eq, proptest};

/// The default `DetectionConfig::max_subquery_depth`, mirrored so the boundary tests read plainly.
const DEFAULT_MAX_SUBQUERY_DEPTH: u32 = 3;

fn rule_with(sql: &str) -> DetectionRule {
    DetectionRule::new(
        "u3-test",
        "U3 test rule",
        "Rule-load validation fixture",
        sql,
        "test",
        AlertSeverity::Low,
    )
}

/// Validate `sql` at the default depth and return the rejection, panicking if it was accepted.
fn reject(sql: &str) -> SqlRejection {
    match rule_with(sql).validate_sql() {
        Ok(()) => panic!("expected {sql} to be rejected, but it loaded"),
        Err(RuleError::SqlRejected(rejection)) => rejection,
        Err(other) => panic!("expected an SQL rejection, got {other:?}"),
    }
}

fn accept(sql: &str) {
    if let Err(err) = rule_with(sql).validate_sql() {
        panic!("expected {sql} to load, but it was rejected: {err}");
    }
}

/// Build `levels` nested `IN (SELECT ...)` subqueries below one top-level SELECT.
fn nested_subqueries(levels: u32) -> String {
    let mut sql = String::from("SELECT pid FROM processes");
    for _level in 0..levels {
        sql = format!("SELECT pid FROM processes WHERE pid IN ({sql})");
    }
    sql
}

// --- R2: function allowlist -------------------------------------------------------------------

#[test]
fn a_function_outside_the_allowlist_is_rejected_and_named() {
    let rejection = reject("SELECT readfile('/etc/passwd') FROM processes");
    let SqlRejection::FunctionNotAllowed { function, position } = rejection else {
        panic!("expected the allowlist gate to fire, got {rejection:?}");
    };
    assert_eq!(function, "readfile");
    assert!(
        matches!(position, SqlPosition::Known { .. }),
        "the rejection must carry a real source position, got {position:?}"
    );
}

#[test]
fn every_function_the_old_denylist_banned_is_still_rejected_by_name() {
    // The denylist this allowlist replaces (KTD4). Each must now fail membership, not a denial.
    for banned in [
        "load_extension",
        "readfile",
        "writefile",
        "eval",
        "exec",
        "system",
        "shell",
        "glob",
        "replace",
        "abs",
        "random",
        "randomblob",
        "quote",
        "printf",
        "char",
        "unicode",
        "soundex",
        "difference",
    ] {
        let sql = format!("SELECT {banned}(name) FROM processes");
        let rejection = reject(&sql);
        let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
            panic!("expected the allowlist gate to fire for {banned}, got {rejection:?}");
        };
        assert_eq!(function.to_lowercase(), banned);
    }
}

#[test]
fn an_allowlisted_aggregate_loads() {
    accept("SELECT count(pid), min(pid), max(pid), sum(pid), avg(pid) FROM processes");
}

#[test]
fn the_allowlist_is_lowercase_and_sorted_and_matches_case_insensitively() {
    let mut sorted = ALLOWED_SQL_FUNCTIONS.to_vec();
    sorted.sort_unstable();
    assert_eq!(ALLOWED_SQL_FUNCTIONS, sorted.as_slice());
    for entry in ALLOWED_SQL_FUNCTIONS {
        assert_eq!(*entry, entry.to_lowercase());
        assert!(is_allowed_sql_function(&entry.to_uppercase()));
    }
    assert!(!is_allowed_sql_function("readfile"));
}

#[test]
fn a_function_hidden_inside_a_cte_is_rejected() {
    let rejection = reject("WITH t AS (SELECT eval('x') AS v FROM processes) SELECT v FROM t");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire, got {rejection:?}");
    };
    assert_eq!(function.to_lowercase(), "eval");
}

#[test]
fn a_function_hidden_inside_a_parenthesised_in_list_is_rejected() {
    let rejection = reject("SELECT pid FROM processes WHERE (name) IN (printf('%s', name))");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire, got {rejection:?}");
    };
    assert_eq!(function.to_lowercase(), "printf");
}

#[test]
fn a_table_valued_function_call_in_from_is_rejected_and_named() {
    // A function name in table position is not an `Expr::Function`, so the expression walk alone
    // would never see it. No collector serves a table-valued function.
    let rejection = reject("SELECT * FROM readfile('/etc/passwd')");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire on the FROM item, got {rejection:?}");
    };
    assert_eq!(function.to_lowercase(), "readfile");
}

#[test]
fn a_plain_table_and_a_derived_table_in_from_still_load() {
    accept("SELECT pid FROM processes");
    accept("SELECT pid FROM (SELECT pid FROM processes) AS inner_processes");
}

// --- R3: subquery depth -----------------------------------------------------------------------

#[test]
fn a_subquery_nested_to_the_configured_maximum_loads() {
    accept(&nested_subqueries(DEFAULT_MAX_SUBQUERY_DEPTH));
}

#[test]
fn a_subquery_nested_one_past_the_configured_maximum_is_rejected() {
    let rejection = reject(&nested_subqueries(DEFAULT_MAX_SUBQUERY_DEPTH + 1));
    let SqlRejection::SubqueryTooDeep { depth, max_depth } = rejection else {
        panic!("expected the depth gate to fire, got {rejection:?}");
    };
    assert_eq!(max_depth, DEFAULT_MAX_SUBQUERY_DEPTH);
    assert_eq!(depth, DEFAULT_MAX_SUBQUERY_DEPTH + 1);
}

#[test]
fn depth_counts_a_subquery_reached_through_a_cte() {
    // Three levels below the top-level SELECT, the outermost reached only through a CTE body.
    let inner = nested_subqueries(DEFAULT_MAX_SUBQUERY_DEPTH);
    let sql = format!("WITH t AS ({inner}) SELECT pid FROM t");
    let rejection = reject(&sql);
    let SqlRejection::SubqueryTooDeep { depth, max_depth } = rejection else {
        panic!("expected the depth gate to fire through the CTE, got {rejection:?}");
    };
    assert_eq!(max_depth, DEFAULT_MAX_SUBQUERY_DEPTH);
    assert_eq!(depth, DEFAULT_MAX_SUBQUERY_DEPTH + 1);
}

#[test]
fn depth_counts_a_subquery_appearing_as_a_bare_function_argument() {
    // The subquery is an argument to an allowlisted function, not under a WHERE clause.
    let inner = nested_subqueries(DEFAULT_MAX_SUBQUERY_DEPTH);
    let sql = format!("SELECT length(({inner})) FROM processes");
    let rejection = reject(&sql);
    let SqlRejection::SubqueryTooDeep { depth, max_depth } = rejection else {
        panic!("expected the depth gate to fire through the function argument, got {rejection:?}");
    };
    assert_eq!(max_depth, DEFAULT_MAX_SUBQUERY_DEPTH);
    assert_eq!(depth, DEFAULT_MAX_SUBQUERY_DEPTH + 1);
}

#[test]
fn the_depth_limit_is_configurable() {
    let rule = rule_with(&nested_subqueries(2));
    assert!(rule.validate_sql_with_depth(2).is_ok());
    let err = rule.validate_sql_with_depth(1).unwrap_err();
    let RuleError::SqlRejected(SqlRejection::SubqueryTooDeep { depth, max_depth }) = err else {
        panic!("expected the depth gate to fire at the tightened limit, got {err:?}");
    };
    assert_eq!((depth, max_depth), (2, 1));
}

// --- R1: single SELECT statement --------------------------------------------------------------

#[test]
fn a_non_select_statement_is_rejected_and_named() {
    let rejection = reject("DROP TABLE processes");
    let SqlRejection::NotASelect { ref statement_kind } = rejection else {
        panic!("expected the SELECT-only gate to fire, got {rejection:?}");
    };
    assert_eq!(statement_kind.to_uppercase(), "DROP");
}

#[test]
fn a_set_operation_is_rejected_at_the_top_level_and_inside_a_cte() {
    for sql in [
        "SELECT pid FROM processes UNION SELECT pid FROM processes",
        "WITH t AS (SELECT pid FROM processes UNION SELECT pid FROM processes) SELECT pid FROM t",
    ] {
        let rejection = reject(sql);
        let SqlRejection::NotASelect { ref statement_kind } = rejection else {
            panic!("expected the SELECT-only gate to fire for {sql}, got {rejection:?}");
        };
        assert_eq!(statement_kind, "set operation");
    }
}

#[test]
fn a_multi_statement_input_is_rejected_with_the_statement_count() {
    let rejection = reject("SELECT pid FROM processes; SELECT name FROM processes");
    let SqlRejection::MultipleStatements { count } = rejection else {
        panic!("expected the single-statement gate to fire, got {rejection:?}");
    };
    assert_eq!(count, 2);
}

#[test]
fn input_nested_past_the_parser_recursion_limit_fails_as_a_parse_error() {
    // Deep parenthesisation, not deep subqueries: this must trip the parser's own structural
    // limit before an AST exists, so the semantic depth gate never sees it.
    let depth = 60;
    let sql = format!(
        "SELECT pid FROM processes WHERE {}pid{} = 1",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    let rejection = reject(&sql);
    let SqlRejection::ParseFailed { ref message } = rejection else {
        panic!("expected a parse failure ahead of validation, got {rejection:?}");
    };
    assert!(
        message.to_lowercase().contains("recursion"),
        "expected the parser recursion limit to be named, got: {message}"
    );
}

// --- Property: no non-allowlisted identifier reaches a lowering path --------------------------

/// The allowlist only governs constructs that reach the gate as `Expr::Function`.
///
/// `SUBSTR`/`SUBSTRING`, `CAST`, `TRIM`, `POSITION` and `EXTRACT` are parsed by `sqlparser` into
/// their own dedicated `Expr` variants, so they never consult the allowlist and listing them there
/// would be decoration. That is a property of the parser, not of this crate, so it is pinned here:
/// if a future `sqlparser` release reclassifies any of them as an ordinary function call, they
/// would start being rejected as unlisted, silently breaking rules that use them. This test fails
/// at that moment instead.
#[test]
fn parser_level_constructs_do_not_reach_the_function_allowlist() {
    for sql in [
        "SELECT substr(name, 1, 3) FROM processes",
        "SELECT SUBSTRING(name FROM 1 FOR 3) FROM processes",
        "SELECT cast(pid AS TEXT) FROM processes",
        "SELECT trim(name) FROM processes",
    ] {
        assert!(
            rule_with(sql).validate_sql().is_ok(),
            "{sql} must load: it is a parser-level construct, not a function call the allowlist \
             governs. A sqlparser change reclassifying it as Expr::Function would land here."
        );
    }
}

/// An ordinary function call absent from the allowlist is rejected, membership being the control.
///
/// `lower` is the contrast to the test above: it does parse as `Expr::Function`, the old denylist
/// never named it, and under R2 it is now rejected because it is not listed rather than allowed
/// because it was never banned.
#[test]
fn an_unlisted_ordinary_function_is_rejected_by_name() {
    let err = rule_with("SELECT lower(name) FROM processes")
        .validate_sql()
        .unwrap_err();
    let RuleError::SqlRejected(SqlRejection::FunctionNotAllowed { function, .. }) = err else {
        panic!("expected the allowlist gate to fire, got {err:?}");
    };
    assert_eq!(function.to_lowercase(), "lower");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any identifier in function position that is not on the allowlist is rejected by name.
    ///
    /// Names are generated with a `zz` prefix so that none can collide with an SQL keyword: a
    /// keyword would fail to parse, and a parse failure here would mask the gate under test
    /// rather than exercise it.
    #[test]
    fn any_non_allowlisted_identifier_in_function_position_is_rejected(
        name in prop::string::string_regex("zz[a-z0-9_]{0,10}").unwrap().prop_flat_map(Just)
    ) {
        prop_assert!(!is_allowed_sql_function(&name));
        let sql = format!("SELECT {name}(pid) FROM processes");
        let err = rule_with(&sql).validate_sql().unwrap_err();
        let RuleError::SqlRejected(SqlRejection::FunctionNotAllowed { function, .. }) = err else {
            panic!("expected the allowlist gate to fire for {name}, got {err:?}");
        };
        prop_assert_eq!(function.to_lowercase(), name);
    }
}

// --- R17: a clause the planner cannot lower is refused, never silently dropped ----------------

/// Assert that `sql` is refused by the clause gate specifically, naming `clause`.
fn assert_clause_refused(sql: &str, clause: &str) {
    let rejection = reject(sql);
    let SqlRejection::UnsupportedClause { clause: named, .. } = rejection else {
        panic!("expected the clause gate to fire for {sql}, got {rejection:?}");
    };
    assert_eq!(named, clause, "wrong clause named for {sql}");
}

#[test]
fn a_clause_the_planner_cannot_lower_is_refused_by_name() {
    // Each of these loaded clean before the clause gate existed, with the compiled rule carrying
    // no record of the clause — `LIMIT 1` meaning "one matching process" became "every match".
    for (sql, clause) in [
        ("SELECT pid FROM processes WHERE pid = 1 LIMIT 1", "LIMIT"),
        (
            "SELECT pid FROM processes WHERE pid = 1 ORDER BY pid",
            "ORDER BY",
        ),
        ("SELECT DISTINCT pid FROM processes", "DISTINCT"),
        ("SELECT count(pid) FROM processes GROUP BY name", "GROUP BY"),
        ("SELECT pid FROM processes GROUP BY ALL", "GROUP BY ALL"),
        (
            "SELECT count(pid) FROM processes GROUP BY name HAVING count(pid) > 1",
            "GROUP BY",
        ),
        (
            "SELECT pid FROM processes WHERE pid = 1 QUALIFY pid > 0",
            "QUALIFY",
        ),
        ("SELECT TOP 1 pid FROM processes", "TOP"),
        (
            "SELECT pid FROM processes WHERE pid = 1 FETCH FIRST 1 ROW ONLY",
            "FETCH",
        ),
        ("SELECT pid FROM processes PREWHERE pid = 1", "PREWHERE"),
        (
            "SELECT pid FROM processes WHERE pid = 1 SORT BY pid",
            "SORT BY",
        ),
        (
            "SELECT pid FROM processes WHERE pid = 1 CLUSTER BY pid",
            "CLUSTER BY",
        ),
        (
            "SELECT pid FROM processes WHERE pid = 1 FOR UPDATE",
            "FOR UPDATE/SHARE",
        ),
        (
            "SELECT pid FROM processes WHERE pid = 1 SETTINGS a = 1",
            "SETTINGS",
        ),
        (
            "SELECT pid FROM processes WHERE pid = 1 |> WHERE pid = 2",
            "a pipe operator",
        ),
    ] {
        assert_clause_refused(sql, clause);
    }
}

#[test]
fn a_bare_having_without_a_group_by_is_still_refused() {
    // `HAVING` carries its own filter, which the planner reads no more than it reads `GROUP BY`.
    assert_clause_refused(
        "SELECT count(pid) FROM processes HAVING count(pid) > 1",
        "HAVING",
    );
}

#[test]
fn the_clause_gate_reaches_a_subquery() {
    // A subquery's clauses are as invisible to the planner as the top level's.
    assert_clause_refused(
        "SELECT pid FROM processes WHERE pid IN (SELECT pid FROM processes LIMIT 1)",
        "LIMIT",
    );
}

#[test]
fn an_ordinary_rule_with_no_extra_clause_still_loads() {
    // `GroupByExpr::Expressions` with empty lists is the *absent* GROUP BY, carried by every
    // plain SELECT: gating on the variant rather than its contents would refuse every rule.
    accept("SELECT pid FROM processes WHERE pid = 1");
    accept("SELECT p.name, p.pid FROM processes p WHERE p.name LIKE '%test%'");
}
