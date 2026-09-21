//! Rule-load regex compilation and caching (requirements R5, R6, R7).
//!
//! Nothing here executes a detection rule. Every test exercises the load-time path: compile a
//! pattern under the fixed bounds, or reject it before it is ever cached.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A rejection assertion has to name the gate that fired and fail loudly on any other, which is
// exactly a wildcard arm ending in `panic!`. Enumerating the sibling variants instead would make
// every new rejection variant silently widen these assertions.
#![allow(clippy::wildcard_enum_match_arm)]

use daemoneye_lib::detection::{RegexCache, RegexConstruct, RegexRejection, compile_rule_patterns};
use daemoneye_lib::detection_bounds::{REGEX_CACHE_MAX_ENTRIES, REGEX_SIZE_LIMIT_BYTES};
use daemoneye_lib::models::alert::AlertSeverity;
use daemoneye_lib::models::rule::{DetectionRule, RuleError};

/// A pattern whose compiled program is far past [`REGEX_SIZE_LIMIT_BYTES`].
///
/// Bounded repetition of Unicode classes is the cheapest way to blow the ceiling: the compiler
/// unrolls the repetition, so the program grows with the repeat count times the class size.
const OVERSIZED_PATTERN: &str = r"(?:\p{L}\p{N}\p{S}\p{P}){4000}";

fn rule_with_sql(sql: &str) -> DetectionRule {
    DetectionRule::new(
        "regex-cache-test",
        "Regex cache test",
        "Fixture rule for the rule-load regex path",
        sql,
        "test",
        AlertSeverity::Low,
    )
}

fn reject(cache: &RegexCache, pattern: &str) -> RegexRejection {
    cache
        .get_or_compile(pattern)
        .expect_err("expected the pattern to be rejected at load")
}

/// Covers AE9.
///
/// What this proves: two distinct patterns each resolve to their own compiled program, and each
/// matches only its own input, through the public API.
///
/// What it does NOT prove: it does not exhibit an actual pair of strings colliding under a named
/// 64-bit hash. It does not need to. The cache key is the full pattern string, so a hash collision
/// cannot arise in the first place — there is no hash of the pattern anywhere in the lookup path.
/// A test built on a real collision pair would only prove the key is not *that* hash; this proves
/// the stronger property that distinct patterns never share an entry.
#[test]
fn distinct_patterns_each_get_their_own_compiled_program() {
    let cache = RegexCache::new();

    let first = cache.get_or_compile("^alpha$").unwrap();
    let second = cache.get_or_compile("^beta$").unwrap();

    assert!(first.is_match("alpha"));
    assert!(!first.is_match("beta"));
    assert!(second.is_match("beta"));
    assert!(!second.is_match("alpha"));
    assert_eq!(cache.len(), 2);
    assert!(cache.is_cached("^alpha$"));
    assert!(cache.is_cached("^beta$"));
}

#[test]
fn pattern_exceeding_size_limit_is_rejected_and_never_cached() {
    let cache = RegexCache::new();

    let rejection = reject(&cache, OVERSIZED_PATTERN);

    match rejection {
        RegexRejection::CompiledTooBig {
            ref pattern,
            size_limit_bytes,
        } => {
            assert_eq!(pattern, OVERSIZED_PATTERN);
            assert_eq!(size_limit_bytes, REGEX_SIZE_LIMIT_BYTES);
        }
        other => panic!("the wrong gate fired: {other:?}"),
    }
    assert!(!cache.is_cached(OVERSIZED_PATTERN));
    assert_eq!(cache.len(), 0);
}

#[test]
fn backreference_is_rejected_with_the_construct_named() {
    let cache = RegexCache::new();

    for pattern in [r"(a)\1", r"(?<word>a)\k<word>"] {
        match reject(&cache, pattern) {
            RegexRejection::UnsupportedConstruct {
                pattern: ref rejected,
                construct,
            } => {
                assert_eq!(rejected, pattern);
                assert_eq!(construct, RegexConstruct::Backreference);
            }
            other => panic!("the wrong gate fired for {pattern}: {other:?}"),
        }
    }
}

#[test]
fn lookaround_is_rejected_with_the_construct_named() {
    let cache = RegexCache::new();

    let cases = [
        (r"foo(?=bar)", RegexConstruct::Lookahead),
        (r"foo(?!bar)", RegexConstruct::Lookahead),
        (r"(?<=foo)bar", RegexConstruct::Lookbehind),
        (r"(?<!foo)bar", RegexConstruct::Lookbehind),
    ];

    for (pattern, expected) in cases {
        match reject(&cache, pattern) {
            RegexRejection::UnsupportedConstruct {
                pattern: ref rejected,
                construct,
            } => {
                assert_eq!(rejected, pattern);
                assert_eq!(construct, expected);
            }
            other => panic!("the wrong gate fired for {pattern}: {other:?}"),
        }
    }
}

/// Near misses for the construct classifier: every one of these is legal `regex` syntax and must
/// load. A naive substring scan for `(?<`, `(?=` or `\1` rejects all of them.
#[test]
fn constructs_that_merely_resemble_lookaround_or_backreferences_are_accepted() {
    let cache = RegexCache::new();

    for pattern in [
        r"(?<name>alpha)",  // named capture group, not lookbehind
        r"(?P<name>alpha)", // the other named-capture spelling
        r"(?:alpha)",       // non-capturing group
        r"(?i)alpha",       // inline flag
        r"alpha\\1",        // escaped backslash then a literal 1, not a backreference
        r"[(?=]",           // the lookahead glyphs as members of a character class
        r"[\w(?<]",         // and again inside a class alongside a class escape
    ] {
        assert!(
            cache.get_or_compile(pattern).is_ok(),
            "legitimate pattern was rejected: {pattern}"
        );
    }
}

#[test]
fn unicode_word_class_compiles_within_the_size_limit() {
    let cache = RegexCache::new();

    let compiled = cache
        .get_or_compile(r"\w+")
        .expect("a bare Unicode \\w must fit inside the 256 KiB ceiling");

    assert!(compiled.is_match("h\u{e9}llo"));
}

#[test]
fn filling_past_capacity_evicts_the_least_recently_used_entry() {
    let cache = RegexCache::new();

    for index in 0..REGEX_CACHE_MAX_ENTRIES {
        cache.get_or_compile(&format!("^entry{index}$")).unwrap();
    }
    assert_eq!(cache.len(), REGEX_CACHE_MAX_ENTRIES);

    cache.get_or_compile("^overflow$").unwrap();

    assert_eq!(cache.len(), REGEX_CACHE_MAX_ENTRIES);
    assert!(!cache.is_cached("^entry0$"), "the oldest entry survived");
    assert!(cache.is_cached("^entry1$"));
    assert!(cache.is_cached("^overflow$"));
}

#[test]
fn re_touching_the_oldest_entry_spares_it_and_evicts_the_next_oldest() {
    let cache = RegexCache::new();

    for index in 0..REGEX_CACHE_MAX_ENTRIES {
        cache.get_or_compile(&format!("^entry{index}$")).unwrap();
    }

    cache.get_or_compile("^entry0$").unwrap();
    cache.get_or_compile("^overflow$").unwrap();

    assert!(
        cache.is_cached("^entry0$"),
        "the re-touched entry was evicted"
    );
    assert!(
        !cache.is_cached("^entry1$"),
        "the next-oldest entry survived"
    );
    assert_eq!(cache.len(), REGEX_CACHE_MAX_ENTRIES);
}

#[test]
fn a_cache_hit_does_not_recompile() {
    let cache = RegexCache::new();

    cache.get_or_compile("^hit-me$").unwrap();
    let after_first = cache.stats();
    assert_eq!(after_first.compiles, 1);
    assert_eq!(after_first.hits, 0);

    cache.get_or_compile("^hit-me$").unwrap();

    let after_second = cache.stats();
    assert_eq!(
        after_second.compiles, 1,
        "a cache hit recompiled the pattern"
    );
    assert_eq!(after_second.hits, 1);
}

#[test]
fn sql_rejection_means_no_pattern_is_ever_compiled() {
    let cache = RegexCache::new();
    let rule = rule_with_sql("DROP TABLE processes");

    let error = compile_rule_patterns(&cache, &rule, &["^alpha$"], 3)
        .expect_err("a non-SELECT rule must not load");

    assert!(
        matches!(error, RuleError::SqlRejected(_)),
        "expected the SQL gate to fire, got {error:?}"
    );
    assert_eq!(
        cache.stats().compiles,
        0,
        "a pattern was compiled although AST validation had already failed"
    );
    assert_eq!(cache.len(), 0);
}

#[test]
fn a_valid_rule_compiles_its_patterns_after_validation() {
    let cache = RegexCache::new();
    let rule = rule_with_sql("SELECT pid FROM processes WHERE regexp(name, '^alpha$')");

    let compiled = compile_rule_patterns(&cache, &rule, &["^alpha$"], 3)
        .expect("a valid rule with a valid pattern must load");

    assert_eq!(compiled.len(), 1);
    assert!(compiled.first().unwrap().is_match("alpha"));
    assert_eq!(cache.stats().compiles, 1);
}

#[test]
fn a_rejected_pattern_fails_rule_load_after_the_sql_gate_passes() {
    let cache = RegexCache::new();
    let rule = rule_with_sql("SELECT pid FROM processes WHERE regexp(name, 'x')");

    let error = compile_rule_patterns(&cache, &rule, &[r"foo(?=bar)"], 3)
        .expect_err("a lookahead pattern must fail rule load");

    assert!(
        matches!(
            error,
            RuleError::RegexRejected(RegexRejection::UnsupportedConstruct {
                construct: RegexConstruct::Lookahead,
                ..
            })
        ),
        "expected the lookahead gate to fire, got {error:?}"
    );
}
