//! Conformance vectors: the shared corpus, the agent's reference evaluation, and the driver a
//! collector self-tests with before it registers (R15, R22).
//!
//! # What a pass proves, and what it does not
//!
//! A passing vector proves the collector's evaluation of one operation agrees with
//! [`reference_outcome`] on every corpus case that applies to it. It does **not** prove agreement
//! with the executor that will evaluate the residual half. ADR-0006 makes Apache `DataFusion` that
//! executor and T6 builds it; it does not exist yet, so this reference is the only thing there is
//! to agree with. When T6 lands, the reference's NULL, coercion and collation behaviour has to be
//! re-verified against `DataFusion`, and any divergence found there invalidates every pass recorded
//! under this reference.
//!
//! # Why the corpus lives here
//!
//! R22 puts the result on the registration exchange, so the collector produces it — a self-test,
//! not something the agent runs and not a test-suite artifact. "Agreement" is only meaningful if
//! both sides read the *same* corpus, so the corpus is a shared-library concern: `collector-core`
//! and `procmond` already depend on this crate, and so does the agent.
//!
//! # The outcome alphabet
//!
//! A collector's observable answer at the pushdown boundary is rows, not truth values, so
//! [`ConformanceOutcome`] has three states and not SQL's three-valued logic. FALSE and UNKNOWN
//! both collapse into [`ConformanceOutcome::Excludes`] because a single-predicate plan returns no
//! row either way, and `AND` admits only when every conjunct is TRUE — so the collapse loses
//! nothing a plan could observe. The consequence for the corpus is a real constraint: every NULL
//! case must be one where a divergence *flips admission*, such as `column != 'x'` over a NULL
//! (a naive implementation admits, the reference excludes). A case whose only difference is
//! FALSE-versus-UNKNOWN proves nothing and does not belong in the corpus.

mod corpus;

use crate::detection::regex_cache::RegexCache;
use crate::proto::{ColumnType, PredicateOp, literal};
use std::cmp::Ordering;

pub use corpus::corpus;

/// The divergence a case is there to catch (Q3's four axes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConformanceAxis {
    /// A column or literal carrying SQL NULL.
    Null,
    /// A literal whose kind is not the column's declared type.
    Coercion,
    /// Text comparison: case, Unicode normalisation, and byte-versus-codepoint ordering.
    Collation,
    /// The edges that break implementations: integer extremes, zero, empty string, NaN,
    /// and inclusive-versus-exclusive comparison boundaries.
    Boundary,
}

/// What a single-predicate plan does with the case's one row.
///
/// See the module docs for why FALSE and UNKNOWN share [`Self::Excludes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConformanceOutcome {
    /// The row is returned.
    Admits,
    /// The row is not returned: the predicate was FALSE or UNKNOWN.
    Excludes,
    /// The plan is a static defect and is refused rather than evaluated.
    Refuses,
}

/// One corpus case: a single row, a single predicate, and nothing else.
///
/// The case is stated in terms of a column's *declared type*, never a column name, so one corpus
/// serves every collector. A collector maps each case onto one of its own columns of that type.
///
/// Not `#[non_exhaustive]`: a test needs to derive one case from another, and fabricating a case
/// earns nothing — [`verify_operation`] only ever iterates the shared [`corpus()`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceCase {
    /// The divergence this case exists to catch.
    pub axis: ConformanceAxis,
    /// Declared type of the column the case reads.
    pub column_type: ColumnType,
    /// The operation under test.
    pub op: PredicateOp,
    /// The row's value for that column. `None` is SQL NULL.
    pub observed: Option<literal::Value>,
    /// The predicate's literals. Exactly one for every operation but `IN`.
    pub literals: Vec<literal::Value>,
}

impl ConformanceCase {
    /// Whether the case can only run against a column declared nullable.
    ///
    /// A NULL observed value obviously needs one. So does a NULL literal: against a `NOT NULL`
    /// column that is an acceptance-time plan defect rather than a semantics question, and mixing
    /// the two would make the case fail for the wrong reason.
    #[must_use]
    pub fn requires_nullable(&self) -> bool {
        self.observed.is_none() || self.literals.iter().any(is_null_literal)
    }

    /// Whether this case applies to a column of `column_type` with this `nullable` flag, under
    /// `op`.
    #[must_use]
    pub fn applies_to(&self, column_type: ColumnType, nullable: bool, op: PredicateOp) -> bool {
        self.column_type == column_type && self.op == op && (nullable || !self.requires_nullable())
    }
}

/// Every corpus case that applies to one advertised operation on one column.
pub fn cases_for(
    column_type: ColumnType,
    nullable: bool,
    op: PredicateOp,
) -> impl Iterator<Item = &'static ConformanceCase> {
    corpus()
        .iter()
        .filter(move |case| case.applies_to(column_type, nullable, op))
}

/// Run a collector's own evaluation against every applicable case and decide whether the operation
/// passed (R22).
///
/// `probe` evaluates one case with the collector's own machinery. It returns `None` when the
/// collector cannot represent the case's row faithfully — `pid` is a 32-bit field that cannot hold
/// `u64::MAX`, and procmond's `command_line` models the empty string as NULL — in which case the
/// case is skipped rather than counted as a disagreement.
///
/// An operation passes only when at least one case ran and every case that ran agreed. An
/// operation whose cases were all skipped therefore does **not** pass, so a collector cannot earn
/// a pass by being unable to represent anything.
pub fn verify_operation<P>(
    column_type: ColumnType,
    nullable: bool,
    op: PredicateOp,
    mut probe: P,
) -> bool
where
    P: FnMut(&ConformanceCase) -> Option<ConformanceOutcome>,
{
    let mut ran = 0_usize;
    for case in cases_for(column_type, nullable, op) {
        let Some(observed) = probe(case) else {
            continue;
        };
        if observed != reference_outcome(case) {
            return false;
        }
        ran = ran.saturating_add(1);
    }
    ran > 0
}

/// The agent's reference evaluation of one case.
///
/// This is the semantics a pushed predicate is required to have. It is deliberately independent of
/// any collector's implementation — including procmond's `LIKE` translation, which is written
/// separately here so that a self-test comparing the two is not comparing a function with itself.
#[must_use]
pub fn reference_outcome(case: &ConformanceCase) -> ConformanceOutcome {
    if let Some(refusal) = static_defect(case) {
        return refusal;
    }
    let Some(ref observed) = case.observed else {
        // Every operation short-circuits on a NULL column value: the comparison is UNKNOWN, and
        // UNKNOWN does not admit. Dispatching per operation is exactly how `!=` over a NULL
        // wrongly becomes TRUE.
        return ConformanceOutcome::Excludes;
    };
    match case.op {
        PredicateOp::Eq => ordering_outcome(case, observed, Ordering::is_eq),
        PredicateOp::Ne => ordering_outcome(case, observed, Ordering::is_ne),
        PredicateOp::Lt => ordering_outcome(case, observed, Ordering::is_lt),
        PredicateOp::Le => ordering_outcome(case, observed, Ordering::is_le),
        PredicateOp::Gt => ordering_outcome(case, observed, Ordering::is_gt),
        PredicateOp::Ge => ordering_outcome(case, observed, Ordering::is_ge),
        PredicateOp::In => in_outcome(case, observed),
        PredicateOp::Like | PredicateOp::Regexp => pattern_outcome(case, observed),
        // Exhaustive on purpose. `PredicateOp` is `#[non_exhaustive]` only to other crates; here
        // in its own crate a new variant must break this match rather than fall into a wildcard
        // that would read an unknown operation as a comparison it is not.
        PredicateOp::Unspecified => ConformanceOutcome::Refuses,
    }
}

/// A defect the plan carries with no row in hand: a wrong arity, or a literal whose kind is not
/// the column's declared type.
fn static_defect(case: &ConformanceCase) -> Option<ConformanceOutcome> {
    let arity_ok = if case.op == PredicateOp::In {
        !case.literals.is_empty()
    } else {
        case.literals.len() == 1
    };
    if !arity_ok {
        return Some(ConformanceOutcome::Refuses);
    }
    // No coercion, in either direction: a coercion the agent and the collector would perform
    // differently is exactly what silently loses rows.
    if case
        .literals
        .iter()
        .any(|value| !is_null_literal(value) && !matches_column_type(value, case.column_type))
    {
        return Some(ConformanceOutcome::Refuses);
    }
    None
}

/// Whether a literal's kind is the one a column of `column_type` compares against.
const fn matches_column_type(value: &literal::Value, column_type: ColumnType) -> bool {
    match column_type {
        ColumnType::String => matches!(*value, literal::Value::StringValue(_)),
        ColumnType::Int => matches!(*value, literal::Value::IntValue(_)),
        ColumnType::Uint => matches!(*value, literal::Value::UintValue(_)),
        ColumnType::Float => matches!(*value, literal::Value::FloatValue(_)),
        ColumnType::Bool => matches!(*value, literal::Value::BoolValue(_)),
        // Exhaustive for the same reason as the operation match above.
        ColumnType::Unspecified => false,
    }
}

/// Whether a literal is the SQL NULL marker.
const fn is_null_literal(value: &literal::Value) -> bool {
    matches!(*value, literal::Value::NullValue(_marker))
}

/// A comparison operation over the case's single literal.
fn ordering_outcome<F>(
    case: &ConformanceCase,
    observed: &literal::Value,
    holds: F,
) -> ConformanceOutcome
where
    F: Fn(Ordering) -> bool,
{
    let Some(literal) = case.literals.first() else {
        return ConformanceOutcome::Refuses;
    };
    // `None` is UNKNOWN, which excludes exactly as FALSE does.
    let Some(ordering) = compare(observed, literal) else {
        return ConformanceOutcome::Excludes;
    };
    if holds(ordering) {
        return ConformanceOutcome::Admits;
    }
    ConformanceOutcome::Excludes
}

/// `IN` under three-valued logic: a match wins outright, and otherwise an UNKNOWN comparison
/// leaves the whole predicate UNKNOWN rather than FALSE.
fn in_outcome(case: &ConformanceCase, observed: &literal::Value) -> ConformanceOutcome {
    for literal in &case.literals {
        if compare(observed, literal) == Some(Ordering::Equal) {
            return ConformanceOutcome::Admits;
        }
    }
    ConformanceOutcome::Excludes
}

/// `LIKE` and `REGEXP`, both through this crate's bounded pattern compiler.
///
/// A pattern the compiler refuses yields [`ConformanceOutcome::Refuses`]: a pattern the agent
/// cannot compile is a plan the agent would not issue, which is the same answer.
fn pattern_outcome(case: &ConformanceCase, observed: &literal::Value) -> ConformanceOutcome {
    let (Some(text), Some(pattern)) = (
        string_of(observed),
        case.literals.first().and_then(string_of),
    ) else {
        return ConformanceOutcome::Refuses;
    };
    let source = if case.op == PredicateOp::Like {
        like_to_regex(pattern)
    } else {
        pattern.to_owned()
    };
    let cache = RegexCache::new();
    let Ok(compiled) = cache.get_or_compile(&source) else {
        return ConformanceOutcome::Refuses;
    };
    if compiled.is_match(text) {
        return ConformanceOutcome::Admits;
    }
    ConformanceOutcome::Excludes
}

/// The text a value carries, if it is text at all.
const fn string_of(value: &literal::Value) -> Option<&str> {
    match *value {
        literal::Value::StringValue(ref text) => Some(text.as_str()),
        literal::Value::IntValue(_)
        | literal::Value::UintValue(_)
        | literal::Value::FloatValue(_)
        | literal::Value::BoolValue(_)
        | literal::Value::NullValue(_) => None,
    }
}

/// Same-kind comparison. A NULL literal is UNKNOWN, and NaN is UNKNOWN as it is in SQL.
fn compare(observed: &literal::Value, value: &literal::Value) -> Option<Ordering> {
    // Text is compared ahead of the match below so neither side needs a `ref` binding inside a
    // `&`-pattern, which no spelling of satisfies both `pattern_type_mismatch` and
    // `needless_borrowed_reference`.
    if let (Some(left), Some(right)) = (string_of(observed), string_of(value)) {
        return Some(left.as_bytes().cmp(right.as_bytes()));
    }
    match (observed, value) {
        (&literal::Value::IntValue(left), &literal::Value::IntValue(right)) => {
            Some(left.cmp(&right))
        }
        (&literal::Value::UintValue(left), &literal::Value::UintValue(right)) => {
            Some(left.cmp(&right))
        }
        (&literal::Value::FloatValue(left), &literal::Value::FloatValue(right)) => {
            left.partial_cmp(&right)
        }
        (&literal::Value::BoolValue(left), &literal::Value::BoolValue(right)) => {
            Some(left.cmp(&right))
        }
        // A NULL literal, and any cross-kind pair `static_defect` did not already refuse.
        (_observed, _literal) => None,
    }
}

/// Translates a SQL `LIKE` pattern into an anchored regular expression.
///
/// `%` and `_` are the only wildcards, everything else is literal, and there is **no** `ESCAPE`
/// clause: a backslash in a `LIKE` pattern matches a backslash. ASCII punctuation is escaped so a
/// pattern cannot smuggle regular-expression syntax in through `LIKE`.
fn like_to_regex(pattern: &str) -> String {
    let mut translated = String::from("(?s)^");
    for character in pattern.chars() {
        match character {
            '%' => translated.push_str(".*"),
            '_' => translated.push('.'),
            literal_character => {
                if literal_character.is_ascii_punctuation() {
                    translated.push('\\');
                }
                translated.push(literal_character);
            }
        }
    }
    translated.push('$');
    translated
}
