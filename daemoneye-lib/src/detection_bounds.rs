//! Fixed bounds for the detection rule-load pipeline.
//!
//! These values are deliberately **not** configuration fields, except the executor and posting-cache
//! defaults below, which are the *default values* of fields on `DetectionConfig` and are documented
//! there. Each of the others backs a guarantee that only holds if the value cannot be changed at
//! runtime: the regex memory ceiling is a product of
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
///
/// The crate calls this an *approximate* size limit ("Sets the approximate size limit, in bytes,
/// of the compiled regex"). It bounds compilation, and a pattern over it is refused — but the
/// figure is the crate's estimate of a compiled program's size, not a measured allocation. Nothing
/// here can report what a compiled pattern actually costs.
pub const REGEX_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Byte ceiling handed to `regex::RegexBuilder::dfa_size_limit`.
///
/// Unlike [`REGEX_SIZE_LIMIT_BYTES`] this is **never a rejection condition**. It bounds the lazy
/// DFA's runtime cache, which resets and falls back to a slower engine when full rather than
/// failing.
///
/// It does **not** multiply by [`REGEX_CACHE_MAX_ENTRIES`]: the crate describes it as the capacity
/// that *may* be used for a single regex search, so it is a per-cache ceiling rather than a
/// per-resident-pattern reservation.
///
/// It is **not** transient either. `regex-automata` keeps these caches in a pool
/// (`util/pool.rs`, a `Mutex<Vec<T>>` of reusable values) and hands them back for reuse rather
/// than freeing them when a search ends, so they are retained memory. How many are live grows with
/// the number of threads searching concurrently. There is therefore no fixed multiplicand to state
/// here, and no total worth quoting.
///
/// 256 KiB rather than the crate's 2 MiB default keeps any one cache an order of magnitude below
/// the process's 100 MB budget instead of a fiftieth of it.
pub const REGEX_DFA_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Maximum number of compiled patterns held in the least-recently-used regex cache.
///
/// A compiled pattern's real footprint cannot be read back at runtime, so bounding the count is the
/// only lever available: it is what keeps the cache from growing without limit. Making it a tunable
/// would remove that lever, which is why it is a constant rather than configuration.
pub const REGEX_CACHE_MAX_ENTRIES: usize = 64;

/// The product of the two configured limits on cached compiled programs, in bytes.
///
/// **This is not a proven resident bound, and must not be quoted as one.** It is
/// [`REGEX_SIZE_LIMIT_BYTES`] × [`REGEX_CACHE_MAX_ENTRIES`], and the first of those is a limit the
/// crate itself calls approximate. So this bounds how far the compiled-program cache can grow
/// under its own configuration; it is not a measurement, it does not cover the pooled DFA caches
/// described on [`REGEX_DFA_SIZE_LIMIT_BYTES`], and it is not the process's regex footprint.
///
/// It is stated explicitly so the pairing cannot drift apart silently; the assertion below turns
/// drift into a compile error.
pub const REGEX_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

const _: () = assert!(
    REGEX_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES == REGEX_CACHE_MAX_BYTES,
    "regex per-pattern size limit times cache entry count must equal the stated product"
);

const _: () = assert!(
    REGEX_DFA_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES <= REGEX_CACHE_MAX_BYTES,
    "the DFA cache ceiling must also survive multiplication by the cache entry count"
);

// --- Posting-list cache bounds ------------------------------------------------------------------

/// Default number of closed-bucket posting lists the page cache retains (R12).
pub const POSTING_CACHE_MAX_ENTRIES: usize = 256;

/// Default longest posting list, in postings, the page cache will retain (R12).
///
/// A longer list is served live from redb and never cached.
pub const POSTING_CACHE_MAX_POSTINGS: usize = 1024;

const _: () = assert!(
    POSTING_CACHE_MAX_ENTRIES > 0 && POSTING_CACHE_MAX_POSTINGS > 0,
    "the posting cache bounds must be non-zero"
);

/// Worst-case posting bytes the page cache can hold at its **default** bounds.
///
/// **This is a product, not a measurement.** It is [`POSTING_CACHE_MAX_ENTRIES`] ×
/// [`POSTING_CACHE_MAX_POSTINGS`] × `size_of::<(u64, u32)>()` (16 bytes, the padded tuple; 12 of
/// them are payload, so the payload alone is 3 MiB). It excludes the per-entry `Arc` and map
/// overhead, and it bounds the *defaults* only: operator-configured values are bounded by their
/// own ceilings, not by this constant.
///
/// The product currently *equals* this bound rather than fitting inside it, so raising either
/// default is a build break by design: the assertion below is the review gate, not slack.
pub const POSTING_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;

const _: () = assert!(
    POSTING_CACHE_MAX_ENTRIES * POSTING_CACHE_MAX_POSTINGS * size_of::<(u64, u32)>()
        <= POSTING_CACHE_MAX_BYTES,
    "posting cache entry count times list length times posting size must fit the stated product"
);

/// Worst case of the posting cache at **both** configurable ceilings, in bytes (128 MiB).
///
/// [`crate::config::DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MAX`] x
/// [`crate::config::DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MAX`] x
/// `size_of::<(u64, u32)>()` (16 bytes, the padded tuple). An earlier draft of the plan said 96
/// MiB by counting 12 payload bytes per posting; the real element is 16. A product, not a
/// measurement, and it excludes the per-entry `Arc` and map overhead. An operator configuring both
/// ceilings is choosing a large deployment on purpose.
pub const POSTING_CACHE_CEILING_BYTES: usize = 128 * 1024 * 1024;

const _: () = assert!(
    crate::config::DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MAX
        * crate::config::DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MAX
        * size_of::<(u64, u32)>()
        == POSTING_CACHE_CEILING_BYTES,
    "the posting cache's configured ceilings must multiply out to the stated worst case"
);

// --- Executor defaults --------------------------------------------------------------------------

/// Default cap on alerts one rule may raise in one cycle (KTD14).
pub const MAX_MATCHES_PER_RULE_DEFAULT: u32 = 1_000;

/// Default byte capacity of the executor's `GreedyMemoryPool` (R6, KTD3).
///
/// **A ceiling on what `DataFusion` reserves, not on what the process holds.** The pool only sees
/// allocations an operator registers with it. A filter or projection reserves nothing; the
/// `RepartitionExec` the optimizer inserts to reach `EXECUTOR_TARGET_PARTITIONS` over a
/// one-partition scan does, and is what exhausts a too-small pool. The store's row decode is
/// bounded separately by R9. So this number is a default for an operator-configurable field, not
/// a resident-set bound, and nothing here measures one.
///
/// A greedy pool, not a fair one: a fair pool divides memory across concurrent spillable operators,
/// and this plan has none. Spilling is disabled outright, so exhausting the pool is a resource error
/// that becomes a degraded reason, never a temp file.
pub const EXECUTOR_MEMORY_POOL_BYTES: usize = 32 * 1024 * 1024;

/// Default number of rows per `RecordBatch` the executor's session asks for (KTD14).
///
/// Also the granularity of the pattern-latency guard: a `REGEXP` pattern is timed once per batch,
/// so a larger batch trades measurement precision for throughput. The configurable field is
/// validated against a ceiling chosen to keep a single batch's worst-case matching cost from
/// outgrowing that granularity.
pub const EXECUTOR_BATCH_SIZE: usize = 8192;

/// Default number of partitions `DataFusion` may fan a plan out over (KTD14).
///
/// Fixed rather than derived from the host's core count so a rule behaves the same on every
/// deployment; partitions multiply per-batch memory, so this is also a memory-shaping knob.
pub const EXECUTOR_TARGET_PARTITIONS: usize = 4;

/// Default ceiling on the estimated decoded bytes in one scan `RecordBatch` (R9, KTD2).
///
/// Bounds a batch independently of [`EXECUTOR_BATCH_SIZE`]: a batch closes on whichever bound is
/// hit first, so rows with large `command_line` or `executable_path` values yield short batches
/// instead of an 8192-row one. A single row whose estimated size alone exceeds this is excluded
/// and its evaluation is degraded, so it must stay above one maximally-sized row (R28's floor).
///
/// **The floor rests on an assumption, not an enforced cap.** `MAX_EXECUTABLE_PATH_LEN` (4096)
/// caps `executable_path` where it is authenticated, but `command_line` has no length cap
/// anywhere in the collector, proto, ingest or store path. The nearest bounds are the 1 MiB frame of
/// the IPC transport (`IpcConfig::max_frame_bytes`, which the event-bus ingest path is not shown to
/// share) and the host's argument-length limit. The worst row assumed is a
/// 255-byte name, a 4096-byte path, a 64-byte hash and a 1 MiB `command_line`: measured at
/// 1,053,119 estimated bytes (1,057,632 allocated by Arrow), so 4 MiB leaves about 4x headroom.
/// R28's validation of this field in U8 inherits that assumption and must be revisited if
/// `command_line` ever gains, or loses, an enforced cap.
///
/// It bounds the *estimate*; Arrow's string builders may allocate up to 2x that for a batch of
/// large rows. The estimate over-counts typical rows, so 8192 ordinary rows (about 2 MiB) still
/// close on the row bound.
pub const EXECUTOR_BATCH_MAX_BYTES: usize = 4 * 1024 * 1024;

const _: () = assert!(
    EXECUTOR_MEMORY_POOL_BYTES > 0
        && EXECUTOR_BATCH_SIZE > 0
        && EXECUTOR_TARGET_PARTITIONS > 0
        && EXECUTOR_BATCH_MAX_BYTES > 0,
    "the executor defaults must be non-zero"
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

// --- Collector registration bounds --------------------------------------------------------------

/// Maximum byte length of a hostname carried on a collector registration.
///
/// A hostname is not an identifier drawn from a fixed catalog, so [`MAX_IDENTIFIER_LENGTH`] does
/// not apply to it: `collector-core` sends whatever the host reports, and any RFC 1123 name is
/// legitimate. 253 is the DNS maximum for a fully-qualified name (the 255-byte wire form of a name
/// less the length byte and the root label), so this refuses only what could not be a real
/// hostname. The bound still exists because validation runs before authentication — but nothing
/// bounded by it is retained: a rejection record holds only the collector id, truncated to
/// [`MAX_IDENTIFIER_LENGTH`] bytes.
pub const MAX_HOSTNAME_LENGTH: usize = 253;
