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

/// Covers AE1's gate half: the rejection and the named function at the gate `load_rule` calls. The
/// record `load_rule` writes is covered by `rejection_log.rs`'s
/// `a_disallowed_function_is_named_in_exactly_one_rejection_record`.
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

/// An aggregate is refused by the allowlist, because no plan can compute one.
///
/// `build_projection` collects bare identifiers, so admitting `SELECT count(pid)` would plan the
/// projection `["pid"]` — rows where the operator asked for a count. R17 refuses it.
///
/// One statement per name: the visitor breaks on the first disallowed function, so a single
/// statement naming all five would only prove the first.
#[test]
fn an_aggregate_is_rejected_because_no_plan_can_compute_one() {
    for aggregate in ["avg", "count", "max", "min", "sum"] {
        let sql = format!("SELECT {aggregate}(pid) FROM processes");
        let rejection = reject(&sql);
        let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
            panic!("expected the allowlist gate to fire for {aggregate}, got {rejection:?}");
        };
        assert_eq!(function.to_lowercase(), aggregate);
    }

    // A projected aggregate beside a pushable predicate: the shape that mis-plans most quietly.
    let rejection = reject("SELECT count(pid) FROM processes WHERE pid > 1");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire, got {rejection:?}");
    };
    assert_eq!(function.to_lowercase(), "count");
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
fn a_table_valued_function_call_nested_in_a_derived_table_is_rejected_and_named() {
    // `TableFactor::Derived` carries `Ok(())` itself, but its `subquery` field is not skipped by
    // the derived `Visit` impl, so the visitor still descends into it and reaches the nested
    // table-valued call. This pins that nothing changed to make that so.
    let rejection = reject("SELECT * FROM (SELECT * FROM readfile('/etc/passwd')) AS t");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire on the nested FROM item, got {rejection:?}");
    };
    assert_eq!(function.to_lowercase(), "readfile");
}

#[test]
fn a_table_valued_function_call_nested_in_a_cte_is_rejected_and_named() {
    let rejection = reject("WITH t AS (SELECT * FROM readfile('/etc/passwd')) SELECT * FROM t");
    let SqlRejection::FunctionNotAllowed { ref function, .. } = rejection else {
        panic!("expected the allowlist gate to fire inside the CTE, got {rejection:?}");
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

/// Function-like syntax that parses to its own `Expr` variant is refused at load, by name.
///
/// `SUBSTR`/`SUBSTRING`, `TRIM`, `POSITION`, `EXTRACT`, `CEIL` and `FLOOR` never reach the
/// allowlist as `Expr::Function`, and the executor registers no implementation behind them, so
/// without this gate a rule using one would load and then fail on every cycle. The rejection is
/// the same one an unlisted function gets, naming the construct.
///
/// `CAST` is not here: R27 refuses it by its own gate
/// (`a_cast_is_refused_at_load_naming_the_construct_seen`).
#[test]
fn parser_level_function_syntax_is_refused_at_load() {
    for (sql, construct) in [
        ("SELECT substr(name, 1, 3) FROM processes", "SUBSTRING"),
        (
            "SELECT SUBSTRING(name FROM 1 FOR 3) FROM processes",
            "SUBSTRING",
        ),
        ("SELECT trim(name) FROM processes", "TRIM"),
        ("SELECT position('a' IN name) FROM processes", "POSITION"),
        ("SELECT ceil(cpu_usage) FROM processes", "CEIL"),
        ("SELECT floor(cpu_usage) FROM processes", "FLOOR"),
    ] {
        let error = rule_with(sql).validate_sql().unwrap_err().to_string();
        assert!(
            error.contains(&format!("`{construct}`")),
            "{sql} must be refused naming {construct}, got: {error}"
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

// --- R27: casts are refused at load, by construct ---------------------------------------------

/// Each cast spelling parses to its own AST node and never reaches the function allowlist, so the
/// assertion is on `CastNotAllowed` and its `construct`: a parse error or an allowlist hit would
/// also refuse the rule and prove nothing about this gate.
#[test]
fn a_cast_is_refused_at_load_naming_the_construct_seen() {
    for (sql, construct) in [
        (
            "SELECT pid FROM processes WHERE CAST(name AS INT) = 1",
            "CAST",
        ),
        (
            "SELECT pid FROM processes WHERE TRY_CAST(name AS INT) = 1",
            "TRY_CAST",
        ),
        (
            "SELECT pid FROM processes WHERE SAFE_CAST(name AS INT) = 1",
            "SAFE_CAST",
        ),
        ("SELECT pid FROM processes WHERE name::INT = 1", "::"),
        (
            "SELECT pid FROM processes WHERE start_time > DATE '2020-01-01'",
            "typed string literal",
        ),
    ] {
        let rejection = reject(sql);
        let SqlRejection::CastNotAllowed { construct: named } = rejection else {
            panic!("expected the cast gate to fire for {sql}, got {rejection:?}");
        };
        assert_eq!(named, construct, "wrong construct named for {sql}");
    }
}

#[test]
fn a_cast_nested_in_a_subquery_or_projection_is_still_refused() {
    for sql in [
        "SELECT CAST(pid AS TEXT) FROM processes",
        "SELECT pid FROM processes WHERE pid IN (SELECT pid FROM processes WHERE name::TEXT = 'x')",
    ] {
        assert!(
            matches!(reject(sql), SqlRejection::CastNotAllowed { .. }),
            "{sql}"
        );
    }
}

/// The control: a rule with no cast is not caught by the new gate.
#[test]
fn a_rule_with_no_cast_still_loads() {
    accept("SELECT pid FROM processes WHERE pid = 1 AND name = 'bash'");
}

/// The operators that spell a regular-expression match outside `REGEXP` are refused at load,
/// naming the operator: they would reach `DataFusion`'s own kernel, which neither bounds the
/// program through the `RegexCache` nor times it for the latency guard.
#[test]
fn regex_operators_outside_regexp_are_refused_naming_the_operator() {
    for (sql, operator) in [
        ("SELECT pid FROM processes WHERE name ~ '^b'", "~"),
        ("SELECT pid FROM processes WHERE name ~* '^b'", "~*"),
        ("SELECT pid FROM processes WHERE name !~ '^b'", "!~"),
        ("SELECT pid FROM processes WHERE name !~* '^b'", "!~*"),
        (
            "SELECT pid FROM processes WHERE name SIMILAR TO 'b%'",
            "SIMILAR TO",
        ),
    ] {
        let rejection = reject(sql);
        assert!(
            matches!(rejection, SqlRejection::OperatorNotAllowed { operator: seen, .. } if seen == operator),
            "{sql} must be refused naming {operator}, got {rejection:?}"
        );
    }
    // The bounded path is untouched, in all three of its spellings.
    accept("SELECT pid FROM processes WHERE name REGEXP '^b'");
    accept("SELECT pid FROM processes WHERE name RLIKE '^b'");
    accept("SELECT pid FROM processes WHERE regexp(name, '^b')");
}
