//! Bounded compilation and caching of rule `REGEXP` patterns (requirements R5, R6, R7).
//!
//! A pattern is compiled once, under a fixed per-pattern byte ceiling, and kept in a
//! least-recently-used cache holding a fixed number of entries. Neither number is configurable:
//! the cache's memory bound is the product of the two, and a compiled pattern's real footprint
//! cannot be read back at runtime, so a count against a hard per-pattern limit is the only
//! provable ceiling available. `detection_bounds` asserts the arithmetic at compile time.
//!
//! Nothing here executes a rule. Compilation happens at rule load, after AST validation, and a
//! pattern that fails is rejected exactly as any other invalid construct is.
//!
//! # Latency threshold (R7)
//!
//! `DetectionConfig::pattern_latency_threshold_ms` is the per-pattern budget, and a pattern that
//! exceeds it disables the rule owning it. Observing that latency requires executing a pattern
//! against real rows, which is ticket T6's work; this module compiles and caches only, and holds
//! no timing state.

use crate::detection::rejection::{RegexConstruct, RegexRejection};
use crate::detection_bounds::{
    REGEX_CACHE_MAX_ENTRIES, REGEX_DFA_SIZE_LIMIT_BYTES, REGEX_SIZE_LIMIT_BYTES,
};
use crate::models::rule::{DetectionRule, RuleError};
use lru::LruCache;
use regex::{Regex, RegexBuilder};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// [`REGEX_CACHE_MAX_ENTRIES`] as the non-zero type `LruCache::new` requires.
///
/// A `const` `match` rather than `NonZeroUsize::new(..).unwrap_or(..)` so that a zero constant is
/// a compile error here as well as at the assertion below, never a silently shrunken cache.
const CACHE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(REGEX_CACHE_MAX_ENTRIES) {
    Some(capacity) => capacity,
    None => NonZeroUsize::MIN,
};

const _: () = assert!(
    REGEX_CACHE_MAX_ENTRIES > 0,
    "the regex cache capacity must be non-zero"
);

/// A compiled pattern as the cache hands it out.
///
/// Shared rather than owned so a caller may hold one across calls — a collector resolves a
/// predicate's pattern once per batch and matches it against every record without returning to the
/// cache.
pub type CompiledPattern = Arc<Regex>;

/// Observable counters for the regex cache.
///
/// These exist because "the pattern was not recompiled" and "no compilation was attempted" are
/// not observable from a return value: a cache hit and a fresh compile produce equal `Regex`
/// values, and a skipped compile produces nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegexCacheStats {
    /// How many lookups were served from a resident entry.
    pub hits: u64,
    /// How many compilations were *attempted*, including those that ended in a rejection.
    pub compiles: u64,
}

/// A bounded, least-recently-used cache of compiled detection patterns.
///
/// Entries are keyed by the **full pattern string**, never by a hash of it, so two distinct
/// patterns cannot share an entry however their hashes relate.
#[derive(Debug)]
pub struct RegexCache {
    entries: Mutex<LruCache<String, Arc<Regex>>>,
    hits: AtomicU64,
    compiles: AtomicU64,
}

impl Default for RegexCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RegexCache {
    /// Create an empty cache holding at most [`REGEX_CACHE_MAX_ENTRIES`] compiled patterns.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::detection::RegexCache;
    /// let cache = RegexCache::new();
    /// assert_eq!(cache.len(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(LruCache::new(CACHE_CAPACITY)),
            hits: AtomicU64::new(0),
            compiles: AtomicU64::new(0),
        }
    }

    /// Return the compiled form of `pattern`, compiling it under the fixed bounds on a miss.
    ///
    /// A hit promotes the entry to most-recently-used. A miss compiles, inserts, and evicts the
    /// least-recently-used entry if the cache was full. A rejected pattern never enters the cache.
    ///
    /// Two threads missing on the same pattern at once may each compile it; the second insert
    /// replaces the first and both callers hold an equivalent program, so the only cost is the
    /// duplicate work. Holding the lock across compilation would serialise every rule load behind
    /// the slowest pattern, which is the worse trade.
    ///
    /// # Errors
    ///
    /// Returns the [`RegexRejection`] naming why the pattern cannot be compiled.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::detection::RegexCache;
    /// let cache = RegexCache::new();
    /// let compiled = cache.get_or_compile("^alpha$")?;
    /// assert!(compiled.is_match("alpha"));
    /// # Ok::<(), daemoneye_lib::detection::RegexRejection>(())
    /// ```
    pub fn get_or_compile(&self, pattern: &str) -> Result<Arc<Regex>, RegexRejection> {
        // The guard is bound and dropped inside this block: holding it across the compile below
        // would serialise every rule load behind the slowest pattern.
        let resident = self.lock_entries().get(pattern).map(Arc::clone);
        if let Some(compiled) = resident {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(compiled);
        }

        self.compiles.fetch_add(1, Ordering::Relaxed);
        let compiled = Arc::new(compile_bounded(pattern)?);
        self.lock_entries()
            .put(pattern.to_owned(), Arc::clone(&compiled));
        Ok(compiled)
    }

    /// Whether `pattern` is currently resident, without disturbing the eviction order.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::detection::RegexCache;
    /// let cache = RegexCache::new();
    /// assert!(!cache.is_cached("^alpha$"));
    /// ```
    pub fn is_cached(&self, pattern: &str) -> bool {
        self.lock_entries().peek(pattern).is_some()
    }

    /// How many compiled patterns are resident.
    pub fn len(&self) -> usize {
        self.lock_entries().len()
    }

    /// Whether the cache holds no compiled patterns.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A snapshot of the hit and compile counters.
    pub fn stats(&self) -> RegexCacheStats {
        RegexCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            compiles: self.compiles.load(Ordering::Relaxed),
        }
    }

    /// Take the entry lock, recovering the map if a previous holder panicked.
    ///
    /// A poisoned lock means some other caller unwound; the cache itself is still a consistent
    /// map of patterns to programs, and refusing every subsequent rule load over it would turn
    /// one panic into a permanent outage.
    fn lock_entries(&self) -> MutexGuard<'_, LruCache<String, Arc<Regex>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Validate a rule's SQL, then compile the patterns it uses — in that order.
///
/// R5 requires AST validation to complete before any compilation runs. The ordering is structural
/// here: validation is the first statement and `?` returns before the cache is touched at all.
///
/// `patterns` are the pattern literals the caller lowered out of the rule. Extracting them from
/// the AST belongs to the pushdown planner, not to this module.
///
/// # Errors
///
/// Returns [`RuleError::SqlRejected`] if the rule's SQL fails the load gate, or
/// [`RuleError::RegexRejected`] for the first pattern that cannot be compiled.
///
/// # Examples
///
/// ```
/// use daemoneye_lib::detection::{RegexCache, compile_rule_patterns};
/// use daemoneye_lib::models::alert::AlertSeverity;
/// use daemoneye_lib::models::rule::DetectionRule;
///
/// let rule = DetectionRule::new(
///     "r1",
///     "Example",
///     "Example rule",
///     "SELECT pid FROM processes",
///     "example",
///     AlertSeverity::Low,
/// );
/// let cache = RegexCache::new();
/// let compiled = compile_rule_patterns(&cache, &rule, &["^alpha$"], 3)?;
/// assert_eq!(compiled.len(), 1);
/// # Ok::<(), daemoneye_lib::models::rule::RuleError>(())
/// ```
pub fn compile_rule_patterns(
    cache: &RegexCache,
    rule: &DetectionRule,
    patterns: &[&str],
    max_subquery_depth: u32,
) -> Result<Vec<Arc<Regex>>, RuleError> {
    rule.validate_sql_with_depth(max_subquery_depth)?;

    patterns
        .iter()
        .map(|pattern| {
            cache
                .get_or_compile(pattern)
                .map_err(RuleError::RegexRejected)
        })
        .collect()
}

/// Compile one pattern under both fixed bounds, classifying any failure.
fn compile_bounded(pattern: &str) -> Result<Regex, RegexRejection> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT_BYTES)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT_BYTES)
        .build()
        .map_err(|error| classify_failure(pattern, &error))
}

/// Turn a `regex` build failure into a structured rejection.
///
/// The classifier only ever runs on a pattern that has *already* failed to compile. Scanning
/// ahead of compilation would make the scanner's precision load-bearing forever: a false positive
/// on `(?<name>...)` or `\\1` would reject a rule the engine compiles perfectly well. Running it
/// after the fact makes that class of bug unreachable.
// KTD8: `regex::Error` is `#[non_exhaustive]` and this arm decides only how a failure is
// *described*, never whether it is rejected. A new variant must fall through to the general
// syntax rejection rather than fail to compile this match.
#[allow(clippy::wildcard_enum_match_arm)]
fn classify_failure(pattern: &str, error: &regex::Error) -> RegexRejection {
    match *error {
        regex::Error::CompiledTooBig(_) => RegexRejection::CompiledTooBig {
            pattern: pattern.to_owned(),
            size_limit_bytes: REGEX_SIZE_LIMIT_BYTES,
        },
        regex::Error::Syntax(ref message) => classify_construct(pattern).map_or_else(
            || RegexRejection::InvalidSyntax {
                pattern: pattern.to_owned(),
                message: message.clone(),
            },
            |construct| RegexRejection::UnsupportedConstruct {
                pattern: pattern.to_owned(),
                construct,
            },
        ),
        ref other => RegexRejection::InvalidSyntax {
            pattern: pattern.to_owned(),
            message: other.to_string(),
        },
    }
}

/// Name the unsupported construct in a pattern that failed to compile, if there is one.
///
/// The walk tracks escaping and character-class membership, because the naive spellings are all
/// ambiguous: `(?<name>` is a named capture while `(?<=` is lookbehind, `\\1` is an escaped
/// backslash while `\1` is a backreference, and inside `[...]` none of these glyphs are syntax.
fn classify_construct(pattern: &str) -> Option<RegexConstruct> {
    let mut characters = pattern.chars().peekable();
    let mut in_class = false;

    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = characters.next();
                if !in_class && matches!(escaped, Some('1'..='9' | 'k')) {
                    return Some(RegexConstruct::Backreference);
                }
            }
            ']' if in_class => in_class = false,
            _ if in_class => {}
            '[' => in_class = true,
            '(' if characters.peek() == Some(&'?') => {
                let _question = characters.next();
                match characters.peek().copied() {
                    Some('=' | '!') => return Some(RegexConstruct::Lookahead),
                    Some('<') => {
                        let _angle = characters.next();
                        if matches!(characters.peek().copied(), Some('=' | '!')) {
                            return Some(RegexConstruct::Lookbehind);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    None
}
