//! Fixed bounds for the detection rule-load pipeline.
//!
//! These values are deliberately **not** configuration fields. Each one backs a guarantee that
//! only holds if the value cannot be changed at runtime: the regex memory ceiling is a product of
//! two fixed numbers, and the descriptor bounds are the only thing standing between a hostile rule
//! and unbounded allocation during validation.
//!
//! The module is compiled unconditionally (it is not behind the `detection-engine` feature) so
//! that `collector-core` and `procmond` can compile regexes under the same bounds the agent
//! applies at rule load.

use std::time::Duration;

// --- Regex compilation and cache bounds -------------------------------------------------------

/// Byte ceiling handed to `regex::RegexBuilder::size_limit` when compiling a `REGEXP` pattern.
///
/// 256 KiB holds roughly five times what a Unicode-aware `\w` needs (the `regex` crate's own
/// doctest has `\w` failing at 45 KB), so legitimate patterns are unaffected. Exceeding it is a
/// hard compile failure, which is why it doubles as a rejection condition at rule load.
pub const REGEX_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Byte ceiling handed to `regex::RegexBuilder::dfa_size_limit`.
///
/// Unlike [`REGEX_SIZE_LIMIT_BYTES`] this is **never a rejection condition**. It bounds the lazy
/// DFA's runtime cache, which resets and falls back to a slower engine when full rather than
/// failing. It is set equal to the compile ceiling so that it survives the same multiplication:
/// [`REGEX_CACHE_MAX_ENTRIES`] cached patterns cost at most 16 MiB of DFA cache on top of the
/// 16 MiB of compiled programs, keeping total regex residency near 32 MiB against the process's
/// 100 MB budget. The `regex` crate's own 2 MiB default would have permitted 128 MiB here.
pub const REGEX_DFA_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Maximum number of compiled patterns held in the least-recently-used regex cache.
///
/// A compiled pattern's real footprint cannot be read back at runtime, so a fixed entry count
/// against the fixed per-pattern ceiling of [`REGEX_SIZE_LIMIT_BYTES`] is the only provable memory
/// bound available. Making this a tunable would dissolve that proof, which is why it is a constant.
pub const REGEX_CACHE_MAX_ENTRIES: usize = 64;

/// The proven worst-case memory held by the regex cache's compiled programs, in bytes.
///
/// This is the product [`REGEX_SIZE_LIMIT_BYTES`] × [`REGEX_CACHE_MAX_ENTRIES`] stated explicitly
/// so the pairing cannot drift apart silently; the assertion below turns drift into a compile
/// error.
pub const REGEX_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

const _: () = assert!(
    REGEX_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES == REGEX_CACHE_MAX_BYTES,
    "regex per-pattern size limit times cache entry count must equal the stated memory ceiling"
);

const _: () = assert!(
    REGEX_DFA_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES <= REGEX_CACHE_MAX_BYTES,
    "the DFA cache ceiling must also survive multiplication by the cache entry count"
);

// --- Parser bounds ----------------------------------------------------------------------------

/// Recursion limit handed to `sqlparser::Parser::with_recursion_limit`.
///
/// Matches the crate's own default. It is a stack-exhaustion guard on arbitrary nesting, not the
/// semantic subquery limit — that is `DetectionConfig::max_subquery_depth`, which is far smaller
/// and is enforced against the parsed AST rather than during parsing.
pub const SQL_PARSER_RECURSION_LIMIT: usize = 50;

// --- Pushdown task lifetime -------------------------------------------------------------------

/// Lifetime of a pushdown collection task before a collector may discard it.
///
/// Five minutes is long enough that a brief agent restart does not drop live tasks, and short
/// enough that a collector orphaned by an agent crash stops collecting within one scrape window
/// rather than running unattended.
pub const PUSHDOWN_TASK_TTL: Duration = Duration::from_mins(5);

/// Interval at which the agent renews a pushdown task whose rule is still enabled.
///
/// One third of [`PUSHDOWN_TASK_TTL`], so two consecutive renewals can be lost to transient IPC
/// failure before a task expires.
pub const PUSHDOWN_TASK_RENEWAL_INTERVAL: Duration = Duration::from_secs(100);

const _: () = assert!(
    PUSHDOWN_TASK_RENEWAL_INTERVAL.as_secs() * 2 < PUSHDOWN_TASK_TTL.as_secs(),
    "renewal must happen early enough that two lost renewals do not expire a task"
);

// The parser's structural recursion limit fires before an AST exists; R3's semantic depth limit
// sits on top of it. A recursion limit at or below the deepest configurable nesting would turn a
// rule the operator is allowed to write into a parse error, so the ordering is an invariant rather
// than a coincidence of the two defaults.
// SAFETY: widening a u32 constant to usize. usize is at least 32 bits on every target this
// workspace supports, so the conversion is lossless; `usize::try_from` is not const.
#[allow(clippy::as_conversions)]
const _: () = assert!(
    SQL_PARSER_RECURSION_LIMIT > crate::config::DetectionConfig::MAX_SUBQUERY_DEPTH_MAX as usize,
    "the parser recursion limit must exceed the deepest configurable subquery nesting"
);

// --- Collection descriptor bounds ---------------------------------------------------------------

/// Maximum number of tables a single collection descriptor may name.
///
/// The catalog a rule can reach is a small, fixed set of process and connection tables; 32 leaves
/// generous headroom for future collectors while keeping a malformed descriptor from forcing a
/// large allocation during validation.
pub const MAX_TABLES_PER_DESCRIPTOR: usize = 32;

/// Maximum number of columns a descriptor may declare for one table.
///
/// The widest table in the catalog carries well under this; 128 bounds the per-table work without
/// constraining any real schema.
pub const MAX_COLUMNS_PER_TABLE: usize = 128;

/// Maximum number of pushdown operations a descriptor may declare for one column.
///
/// Operations are drawn from a small comparison and matching vocabulary, so 16 is comfortably
/// above any legitimate descriptor and well below a count worth allocating for blindly.
pub const MAX_OPERATIONS_PER_COLUMN: usize = 16;

/// Maximum number of conformance-vector results one registration may carry (R22).
///
/// One result per advertised operation is the shape R22 asks for, so the ceiling is exactly what
/// a maximally-sized descriptor could advertise. A collector sending more is malformed, and the
/// bound is checked before any of them is stored.
pub const MAX_CONFORMANCE_RESULTS: usize =
    MAX_TABLES_PER_DESCRIPTOR * MAX_COLUMNS_PER_TABLE * MAX_OPERATIONS_PER_COLUMN;

/// Maximum byte length of a table, column, or operation identifier in a descriptor.
///
/// Identifiers come from a fixed catalog whose longest entry is far shorter; 128 bytes rejects
/// padded or adversarial identifiers early, before they reach any lookup or log line.
pub const MAX_IDENTIFIER_LENGTH: usize = 128;

// --- Pushdown plan shape bounds ---------------------------------------------------------------

/// Maximum number of predicates one pushed plan may carry.
///
/// A plan's predicates are evaluated per record across a scrape of 10,000+ processes, and the
/// agent — the *lower*-privileged component — is what pushes them to the elevated collector. A
/// conjunction is a lowered `WHERE` clause over a table of at most [`MAX_COLUMNS_PER_TABLE`]
/// columns, so 64 is far above any rule an operator would write and far below a count worth
/// multiplying by a scrape.
pub const MAX_PREDICATES_PER_PLAN: usize = 64;

/// Maximum number of columns one pushed plan may project.
///
/// A projection names columns of a single table, so the table's own column ceiling is the natural
/// bound: a plan asking for more than the widest table declares is malformed by construction.
pub const MAX_PROJECTION_COLUMNS: usize = MAX_COLUMNS_PER_TABLE;

/// Maximum number of literals a single `IN` predicate may carry.
///
/// `IN` is the one operation whose arity is unbounded by shape, and it is scanned per record. 256
/// covers a realistic allow- or deny-list while keeping the per-record work proportional to the
/// plan rather than to whatever the sender chose to send.
pub const MAX_IN_VALUES: usize = 256;

// --- Agent rejection log bounds -----------------------------------------------------------------

/// Maximum number of rejection records the agent's in-memory chain retains.
///
/// Collector registration is reachable over IPC and every refusal writes a record, so a collector
/// retrying with a stale or rotated token would otherwise grow the chain without limit against the
/// agent's 100 MB resident budget. 1024 is wide enough that an operator still sees the whole of a
/// realistic rule-load or registration failure burst, and the retained window stays a few megabytes
/// even where records carry long rendered detail.
///
/// Every identifier a record retains is itself truncated to [`MAX_IDENTIFIER_LENGTH`] *bytes* at
/// the point it is recorded, so the retained identities are bounded by the product of two fixed
/// numbers — 1024 × 128 bytes, or 128 KiB — rather than by the count alone. It is still not a byte ceiling for the whole chain: a rule-load rejection's
/// rendered reason can quote a pattern or a SQL fragment, and those are bounded by their own
/// load-time gates rather than by this constant.
pub const MAX_REJECTION_RECORDS: usize = 1024;

const _: () = assert!(
    MAX_REJECTION_RECORDS > 0,
    "the rejection log must retain at least one record"
);
