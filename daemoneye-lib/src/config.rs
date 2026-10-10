//! Configuration management with hierarchical overrides using figment.
//!
//! Supports multiple configuration sources with precedence:
//! 1. Command-line flags (highest precedence)
//! 2. Environment variables (`PROCMOND`_*, `DAEMONEYE_AGENT`_*, `DAEMONEYE_CLI`_*)
//! 3. User configuration file (~/.config/daemoneye/config.toml)
//! 4. System configuration file (/etc/daemoneye/config.toml)
//! 5. Embedded defaults (lowest precedence)

#[cfg(not(windows))]
use anyhow::Context;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tracing::{info, warn};
use unidirs::Directories;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

/// Configuration loading and validation errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("Configuration file not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Invalid configuration format: {0}")]
    InvalidFormat(#[from] figment::Error),

    #[error("IO error reading configuration: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Configuration validation failed: {message}")]
    ValidationError { message: String },
}

/// Main configuration structure for `DaemonEye` components.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Config {
    /// Application-specific configuration
    pub app: AppConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Alerting configuration
    pub alerting: AlertingConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
    /// `EventBus` broker configuration
    pub broker: BrokerConfig,
    /// Detection rule-load configuration
    #[serde(default)]
    pub detection: DetectionConfig,
}

/// Application-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    /// Scan interval in milliseconds
    pub scan_interval_ms: u64,
    /// Batch size for process collection
    pub batch_size: usize,
    /// Maximum number of processes to collect per scan
    pub max_processes: Option<usize>,
    /// Enable enhanced metadata collection (requires privileges)
    pub enhanced_metadata: bool,
}

/// Database configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Database file path
    pub path: PathBuf,
    /// Data retention period in days
    pub retention_days: u32,
    /// Maximum database size in MB
    pub max_size_mb: Option<u64>,
    /// Enable database encryption
    pub encryption_enabled: bool,
    /// redb page cache, in MiB. Range [`DatabaseConfig::PAGE_CACHE_MB_MIN`] to
    /// [`DatabaseConfig::PAGE_CACHE_MB_MAX`].
    ///
    /// Bounds the resident memory a scan can accumulate from the event store: redb's own default
    /// is 1 GiB, so left alone, resident memory tracks the bytes a scan touches rather than any
    /// executor setting (T6 U9 measurement). The `_MAX` is redb's default, so the ceiling never
    /// permits more than the unconfigured behaviour; the `_MIN` keeps a few pages of working set
    /// per scan partition. A smaller cache trades memory for disk reads and so for scan latency.
    #[serde(default = "default_page_cache_mb")]
    pub page_cache_mb: usize,
}

/// Serde default for [`DatabaseConfig::page_cache_mb`], so a file written before it existed loads.
const fn default_page_cache_mb() -> usize {
    DatabaseConfig::PAGE_CACHE_MB_DEFAULT
}

impl DatabaseConfig {
    /// Chosen from the sweep in `docs/decisions/2026-10-08-t6-full-retention-memory.md`: the
    /// lowest worst-case peak and smallest latency cost among the sizes that remove the
    /// bucket-count growth. A judgement among noisy options, not a measured optimum.
    pub const PAGE_CACHE_MB_DEFAULT: usize = 32;
    /// A floor that still holds a working set of pages for a scan.
    pub const PAGE_CACHE_MB_MIN: usize = 4;
    /// redb's own default, 1 GiB.
    pub const PAGE_CACHE_MB_MAX: usize = 1024;

    /// The configured cache in bytes, for `redb::Builder::set_cache_size`.
    #[must_use]
    pub const fn page_cache_bytes(&self) -> usize {
        self.page_cache_mb.saturating_mul(1024 * 1024)
    }

    /// Check `page_cache_mb` against its range.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] when out of range; never clamps.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let (min, max) = (Self::PAGE_CACHE_MB_MIN, Self::PAGE_CACHE_MB_MAX);
        if !(min..=max).contains(&self.page_cache_mb) {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "database.page_cache_mb must be between {min} and {max}, got {}",
                    self.page_cache_mb
                ),
            });
        }
        Ok(())
    }
}

/// Alerting configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertingConfig {
    /// Alert sinks configuration
    pub sinks: Vec<AlertSinkConfig>,
    /// Alert deduplication window in seconds
    pub dedup_window_seconds: u64,
    /// Maximum alert rate per minute
    pub max_alerts_per_minute: Option<u32>,
    /// Threshold in seconds for considering an alert as recent
    pub recent_threshold_seconds: u64,
}

/// Detection configuration: the rule-load bounds, the per-cycle match cap, and the sizing of the
/// `DataFusion` executor.
///
/// The Product Contract declares two values tunable, `max_subquery_depth` and
/// `pattern_latency_threshold_ms`. Seven more are tunable because the 100 MiB resident figure is a
/// deployment-sized target, not a fixed invariant: a large busy server may need the ceiling higher
/// and a small SCADA endpoint lower, so the knobs that determine memory are operator-configurable,
/// each defaulting to the constant in [`crate::detection_bounds`] that the default deployment is
/// measured against. There is deliberately **no** memory-target field: nothing in the daemon
/// measures its own resident set in production, so a configured target would be a number nothing
/// checks, a guard that does not guard. The target lives in the measurement record and the
/// operator docs.
///
/// Every field is range-checked by [`DetectionConfig::validate`] and every `_MAX` is chosen so the
/// guarantee its default constant backs still holds at the ceiling:
///
/// - `executor_batch_size`: a `REGEXP` pattern is timed once per batch (ADR-0011), so the batch is
///   the granularity of the latency guard. The ceiling keeps one batch's worst-case matching cost
///   from outgrowing that granularity; the floor stops per-batch fixed cost dominating throughput.
/// - `executor_batch_max_bytes`: bounds a batch's estimated decoded size independently of its row
///   count. The floor is the assumed worst-case row ([`DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES`]),
///   because a smaller bound would exclude every row and turn the rare oversized-row carve-out into
///   a rule that fires on all of them. That floor rests on an **assumption**, documented there.
/// - `executor_memory_pool_bytes`: the `GreedyMemoryPool` the session reserves from. It is not an
///   incidental-bookkeeping allowance. The physical optimizer inserts a `RepartitionExec` whenever
///   the scan supplies fewer partitions than `executor_target_partitions`, and a normal cycle's
///   window sits inside one bucket, so that operator is present on **every** cycle; it reserves
///   batch-sized memory and attempts a spill the disabled disk manager refuses. A pool smaller
///   than that reservation fails the evaluation as a degraded resource error, every cycle. The
///   floor here is a sanity floor, not a working one: the reservation is not yet sized against
///   `executor_batch_size`, so an operator shrinking the pool must also test their own rules.
///   `executor_target_partitions = 1` removes the repartition at the cost of parallelism.
/// - `executor_target_partitions`: partitions multiply per-batch memory, so the ceiling bounds the
///   multiplier.
/// - `posting_cache_max_entries` and `posting_cache_max_postings`: the cache is bounded by count,
///   never bytes, and their product is its worst case, `size_of::<(u64, u32)>()` = 16 bytes per
///   posting (12 of payload, 4 of padding). At the defaults that is 4 MiB; at both ceilings
///   (2,048 x 4,096) it is **128 MiB**, stated in
///   [`crate::detection_bounds::POSTING_CACHE_CEILING_BYTES`] and pinned by a compile-time assert,
///   the same way `REGEX_CACHE_MAX_BYTES` documents the regex cache. An operator configuring both
///   ceilings is choosing a large deployment on purpose. Like that constant, it is a product, not
///   a measured resident bound.
/// - `max_matches_per_rule`: the per-rule per-cycle alert cap.
///
/// Fields absent from a config file take their defaults, so a file written before these existed
/// still loads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DetectionConfig {
    /// Maximum subquery nesting depth accepted in a detection rule.
    ///
    /// A rule nesting subqueries more deeply than this is rejected at load. Valid range is
    /// [`DetectionConfig::MAX_SUBQUERY_DEPTH_MIN`] to
    /// [`DetectionConfig::MAX_SUBQUERY_DEPTH_MAX`]; zero is rejected because it would reject
    /// every rule containing a subquery at all.
    pub max_subquery_depth: u32,
    /// Per-pattern latency threshold in milliseconds.
    ///
    /// Validated and carried into `DetectionEngine` as a `Duration` at construction.
    /// `DetectionEngine::observe_pattern_latency` is the consequence enforced against it: a
    /// pattern execution reported over this budget disables the rule that owns it and marks it
    /// unhealthy. Valid range is [`DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MIN`] to
    /// [`DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX`]. Milliseconds are stored as an
    /// integer so that [`Config`] can keep deriving [`Eq`].
    pub pattern_latency_threshold_ms: u64,
    /// Most alerts one rule may produce in one cycle; the executor stops at one more than this and
    /// degrades the evaluation with the cap as the reason. Range
    /// [`DetectionConfig::MAX_MATCHES_PER_RULE_MIN`] to [`DetectionConfig::MAX_MATCHES_PER_RULE_MAX`].
    pub max_matches_per_rule: u32,
    /// Partitions `DataFusion` may fan a plan over. Range
    /// [`DetectionConfig::EXECUTOR_TARGET_PARTITIONS_MIN`] to
    /// [`DetectionConfig::EXECUTOR_TARGET_PARTITIONS_MAX`].
    pub executor_target_partitions: usize,
    /// Rows per `RecordBatch`, and so the granularity of the latency guard. Range
    /// [`DetectionConfig::EXECUTOR_BATCH_SIZE_MIN`] to [`DetectionConfig::EXECUTOR_BATCH_SIZE_MAX`].
    pub executor_batch_size: usize,
    /// Estimated decoded bytes a scan batch may hold. Range
    /// [`DetectionConfig::EXECUTOR_BATCH_MAX_BYTES_MIN`] (the assumed worst-case row, R28) to
    /// [`DetectionConfig::EXECUTOR_BATCH_MAX_BYTES_MAX`].
    pub executor_batch_max_bytes: usize,
    /// Capacity of the executor's `GreedyMemoryPool`, in bytes. See the type docs: the
    /// `RepartitionExec` every normal cycle plans reserves from this pool. Range
    /// [`DetectionConfig::EXECUTOR_MEMORY_POOL_BYTES_MIN`] to
    /// [`DetectionConfig::EXECUTOR_MEMORY_POOL_BYTES_MAX`].
    pub executor_memory_pool_bytes: usize,
    /// Closed-bucket posting lists the page cache retains. Range
    /// [`DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MIN`] to
    /// [`DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MAX`].
    pub posting_cache_max_entries: usize,
    /// Longest posting list the page cache will retain. Range
    /// [`DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MIN`] to
    /// [`DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MAX`].
    pub posting_cache_max_postings: usize,
}

impl DetectionConfig {
    /// Smallest accepted subquery nesting depth. Zero would reject every subquery.
    pub const MAX_SUBQUERY_DEPTH_MIN: u32 = 1;
    /// Largest accepted subquery nesting depth, comfortably under
    /// [`crate::detection_bounds::SQL_PARSER_RECURSION_LIMIT`].
    pub const MAX_SUBQUERY_DEPTH_MAX: u32 = 16;
    /// Smallest accepted per-pattern latency threshold. Below 1ms the threshold is finer than the
    /// measurement it would be compared against.
    pub const PATTERN_LATENCY_THRESHOLD_MS_MIN: u64 = 1;
    /// Largest accepted per-pattern latency threshold. One minute is already far past the point
    /// where a pattern should have disabled its rule.
    pub const PATTERN_LATENCY_THRESHOLD_MS_MAX: u64 = 60_000;
    /// Smallest per-rule match cap; zero would make every matching rule degrade.
    pub const MAX_MATCHES_PER_RULE_MIN: u32 = 1;
    /// Largest per-rule match cap.
    pub const MAX_MATCHES_PER_RULE_MAX: u32 = 100_000;
    /// A floor that still scans correctly, just sequentially.
    pub const EXECUTOR_TARGET_PARTITIONS_MIN: usize = 1;
    /// Above the core count of any deployment this is measured against.
    pub const EXECUTOR_TARGET_PARTITIONS_MAX: usize = 32;
    /// Below this, per-batch fixed overhead dominates throughput.
    pub const EXECUTOR_BATCH_SIZE_MIN: usize = 128;
    /// Short of the point where one batch's worst-case pattern-matching cost outgrows the
    /// per-batch granularity the latency guard is built on.
    pub const EXECUTOR_BATCH_SIZE_MAX: usize = 65_536;
    /// The size of one maximally-sized row's estimated decoded encoding, **assumed**, not derived.
    ///
    /// This is the floor R28 enforces on `executor_batch_max_bytes`, and it rests on an
    /// assumption nothing in the workspace enforces: that `command_line` is bounded by the IPC
    /// transport's 1 MiB frame. No length cap on `command_line` exists in the collector, proto,
    /// ingest or store path, and nothing shows the event-bus ingest path shares that frame limit.
    /// The only enforced cap on a row's text is `MAX_EXECUTABLE_PATH_LEN` (4,096) on
    /// `executable_path`. The figure is the measured estimate for a row of a 255-byte name, a
    /// 4,096-byte path, a 64-byte hash and a 1 MiB `command_line`. If `command_line` is ever
    /// capped, or is found uncapped upstream, this number must be revisited: a row larger than it
    /// is excluded from every batch and degrades the evaluation that would have read it.
    pub const ASSUMED_WORST_CASE_ROW_BYTES: usize = 1_053_119;
    /// R28's floor, as a range bound.
    pub const EXECUTOR_BATCH_MAX_BYTES_MIN: usize = Self::ASSUMED_WORST_CASE_ROW_BYTES;
    /// A sanity ceiling rather than an operative one.
    pub const EXECUTOR_BATCH_MAX_BYTES_MAX: usize = 256 * 1024 * 1024;
    /// A sanity floor, not a working one; see the type docs.
    pub const EXECUTOR_MEMORY_POOL_BYTES_MIN: usize = 1024;
    /// One GiB.
    pub const EXECUTOR_MEMORY_POOL_BYTES_MAX: usize = 1024 * 1024 * 1024;
    /// The cache must hold at least one list to be a cache.
    pub const POSTING_CACHE_MAX_ENTRIES_MIN: usize = 1;
    /// With the postings ceiling, bounds the cache's worst case at 128 MiB.
    pub const POSTING_CACHE_MAX_ENTRIES_MAX: usize = 2_048;
    /// A list of at least one posting is cacheable at all.
    pub const POSTING_CACHE_MAX_POSTINGS_MIN: usize = 1;
    /// With the entries ceiling, bounds the cache's worst case at 128 MiB.
    pub const POSTING_CACHE_MAX_POSTINGS_MAX: usize = 4_096;

    /// Check every field against its range, and the cross-field floor R28 sets.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] naming the first field out of range. Values are
    /// rejected, never clamped: an operator who asked for something unsupported should hear so.
    pub fn validate(&self) -> Result<(), ConfigError> {
        check_range(
            "max_subquery_depth",
            &self.max_subquery_depth,
            &Self::MAX_SUBQUERY_DEPTH_MIN,
            &Self::MAX_SUBQUERY_DEPTH_MAX,
        )?;
        check_range(
            "pattern_latency_threshold_ms",
            &self.pattern_latency_threshold_ms,
            &Self::PATTERN_LATENCY_THRESHOLD_MS_MIN,
            &Self::PATTERN_LATENCY_THRESHOLD_MS_MAX,
        )?;
        check_range(
            "max_matches_per_rule",
            &self.max_matches_per_rule,
            &Self::MAX_MATCHES_PER_RULE_MIN,
            &Self::MAX_MATCHES_PER_RULE_MAX,
        )?;
        check_range(
            "executor_target_partitions",
            &self.executor_target_partitions,
            &Self::EXECUTOR_TARGET_PARTITIONS_MIN,
            &Self::EXECUTOR_TARGET_PARTITIONS_MAX,
        )?;
        check_range(
            "executor_batch_size",
            &self.executor_batch_size,
            &Self::EXECUTOR_BATCH_SIZE_MIN,
            &Self::EXECUTOR_BATCH_SIZE_MAX,
        )?;
        self.validate_batch_max_bytes()?;
        check_range(
            "executor_memory_pool_bytes",
            &self.executor_memory_pool_bytes,
            &Self::EXECUTOR_MEMORY_POOL_BYTES_MIN,
            &Self::EXECUTOR_MEMORY_POOL_BYTES_MAX,
        )?;
        check_range(
            "posting_cache_max_entries",
            &self.posting_cache_max_entries,
            &Self::POSTING_CACHE_MAX_ENTRIES_MIN,
            &Self::POSTING_CACHE_MAX_ENTRIES_MAX,
        )?;
        check_range(
            "posting_cache_max_postings",
            &self.posting_cache_max_postings,
            &Self::POSTING_CACHE_MAX_POSTINGS_MIN,
            &Self::POSTING_CACHE_MAX_POSTINGS_MAX,
        )
    }

    /// R28: a batch byte bound below one worst-case row excludes every row.
    fn validate_batch_max_bytes(&self) -> Result<(), ConfigError> {
        let floor = Self::ASSUMED_WORST_CASE_ROW_BYTES;
        if self.executor_batch_max_bytes < floor {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "detection.executor_batch_max_bytes must be at least {floor}, the assumed \
                     worst-case row size (see DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES), \
                     got {}",
                    self.executor_batch_max_bytes
                ),
            });
        }
        check_range(
            "executor_batch_max_bytes",
            &self.executor_batch_max_bytes,
            &Self::EXECUTOR_BATCH_MAX_BYTES_MIN,
            &Self::EXECUTOR_BATCH_MAX_BYTES_MAX,
        )
    }
}

/// Reject `value` outside `min..=max`, naming the field and both bounds.
fn check_range<T>(field: &str, value: &T, min: &T, max: &T) -> Result<(), ConfigError>
where
    T: PartialOrd + std::fmt::Display,
{
    if *value < *min || *value > *max {
        return Err(ConfigError::ValidationError {
            message: format!("detection.{field} must be between {min} and {max}, got {value}"),
        });
    }
    Ok(())
}

impl Default for DetectionConfig {
    /// Defaults are the values the requirements state, and for the executor the constants in
    /// [`crate::detection_bounds`].
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::config::DetectionConfig;
    /// let cfg = DetectionConfig::default();
    /// assert_eq!(cfg.max_subquery_depth, 3);
    /// assert_eq!(cfg.pattern_latency_threshold_ms, 10);
    /// assert_eq!(cfg.max_matches_per_rule, 1_000);
    /// ```
    fn default() -> Self {
        Self {
            max_subquery_depth: 3,
            pattern_latency_threshold_ms: 10,
            max_matches_per_rule: crate::detection_bounds::MAX_MATCHES_PER_RULE_DEFAULT,
            executor_target_partitions: crate::detection_bounds::EXECUTOR_TARGET_PARTITIONS,
            executor_batch_size: crate::detection_bounds::EXECUTOR_BATCH_SIZE,
            executor_batch_max_bytes: crate::detection_bounds::EXECUTOR_BATCH_MAX_BYTES,
            executor_memory_pool_bytes: crate::detection_bounds::EXECUTOR_MEMORY_POOL_BYTES,
            posting_cache_max_entries: crate::detection_bounds::POSTING_CACHE_MAX_ENTRIES,
            posting_cache_max_postings: crate::detection_bounds::POSTING_CACHE_MAX_POSTINGS,
        }
    }
}

/// Individual alert sink configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertSinkConfig {
    /// Sink type identifier
    pub sink_type: String,
    /// Sink-specific configuration supporting numeric, boolean, array, and table values
    #[serde(default = "default_sink_config")]
    pub config: serde_json::Value,
    /// Enable/disable this sink
    pub enabled: bool,
}

/// Default configuration for alert sinks (empty object)
fn default_sink_config() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Log format (json, human)
    pub format: String,
    /// Log file path (optional, stdout if not specified)
    pub file: Option<PathBuf>,
    /// Enable structured logging
    pub structured: bool,
}

/// Process manager configuration for collector lifecycle management.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessManagerConfig {
    /// Graceful shutdown timeout in seconds
    pub graceful_shutdown_timeout_seconds: u64,
    /// Force shutdown timeout in seconds
    pub force_shutdown_timeout_seconds: u64,
    /// Health check interval in seconds
    pub health_check_interval_seconds: u64,
    /// Enable automatic restart on collector failure
    pub enable_auto_restart: bool,
    /// Maximum restart attempts before giving up
    pub max_restart_attempts: u32,
}

impl Default for ProcessManagerConfig {
    fn default() -> Self {
        Self {
            graceful_shutdown_timeout_seconds: 30,
            force_shutdown_timeout_seconds: 5,
            health_check_interval_seconds: 60,
            enable_auto_restart: false,
            max_restart_attempts: 3,
        }
    }
}

/// `EventBus` broker configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrokerConfig {
    /// Socket path for the embedded broker
    pub socket_path: String,
    /// Enable the embedded broker
    pub enabled: bool,
    /// Broker startup timeout in seconds
    pub startup_timeout_seconds: u64,
    /// Broker shutdown timeout in seconds
    pub shutdown_timeout_seconds: u64,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Message buffer size per connection
    pub message_buffer_size: usize,
    /// Topic hierarchy configuration
    pub topic_hierarchy: TopicHierarchyConfig,
    /// Collector binary paths (`collector_type` -> `binary_path`)
    #[serde(default)]
    pub collector_binaries: std::collections::HashMap<String, PathBuf>,
    /// Process manager configuration
    #[serde(default)]
    pub process_manager: ProcessManagerConfig,
    /// Configuration directory for collector configs
    #[serde(default = "default_config_directory")]
    pub config_directory: PathBuf,
}

// BrokerConfig implementation moved below Default impl

/// Topic hierarchy configuration for the `EventBus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopicHierarchyConfig {
    /// Enable wildcard topic matching
    pub enable_wildcards: bool,
    /// Maximum topic depth allowed
    pub max_topic_depth: usize,
    /// Event topic prefixes
    pub event_topics: EventTopicsConfig,
    /// Control topic prefixes
    pub control_topics: ControlTopicsConfig,
}

/// Event topic configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventTopicsConfig {
    /// Process event topics (events.process.*)
    pub process: String,
    /// Network event topics (events.network.*)
    pub network: String,
    /// Filesystem event topics (events.filesystem.*)
    pub filesystem: String,
    /// Performance event topics (events.performance.*)
    pub performance: String,
}

/// Control topic configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlTopicsConfig {
    /// Collector lifecycle topics (control.collector.*)
    pub collector: String,
    /// Health monitoring topics (control.health.*)
    pub health: String,
}

// Default implementation is now derived

/// Default configuration directory for collector configs.
///
/// Returns system-wide configuration directory paths for system-level components
/// (daemoneye-agent and collectors) using `unidirs::ServiceDirs`. User-level components
/// (daemoneye-cli) should use user-specific directories instead.
///
/// Uses platform-aware system-wide directories via `unidirs::ServiceDirs`:
/// - **Linux**: `/var/lib/evilbitlabs/daemoneye/configs` (via `ServiceDirs`)
/// - **macOS**: `/Library/Application Support/evilbitlabs/daemoneye/configs` (via `ServiceDirs`)
/// - **Windows**: `C:\ProgramData\evilbitlabs\daemoneye\configs` (via `ServiceDirs`)
/// - **Other Unix**: Platform-appropriate system directory (via `ServiceDirs`)
fn default_config_directory() -> PathBuf {
    // Use ServiceDirs for system-wide directories (organization, application)
    let service_dirs = unidirs::ServiceDirs::new("evilbitlabs", "daemoneye");

    // Convert Utf8Path to PathBuf and append "configs"
    service_dirs
        .config_dir()
        .to_path_buf()
        .join("configs")
        .into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            scan_interval_ms: 30000,
            batch_size: 1000,
            max_processes: None,
            enhanced_metadata: false,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/daemoneye/processes.db"),
            retention_days: 30,
            max_size_mb: None,
            encryption_enabled: false,
            page_cache_mb: Self::PAGE_CACHE_MB_DEFAULT,
        }
    }
}

impl Default for AlertingConfig {
    /// Creates the default `AlertingConfig`.
    ///
    /// Defaults:
    /// - `sinks`: empty list
    /// - `dedup_window_seconds`: 300
    /// - `max_alerts_per_minute`: `None`
    /// - `recent_threshold_seconds`: 3600
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::config::AlertingConfig;
    /// let cfg = AlertingConfig::default();
    /// assert!(cfg.sinks.is_empty());
    /// assert_eq!(cfg.dedup_window_seconds, 300);
    /// assert!(cfg.max_alerts_per_minute.is_none());
    /// assert_eq!(cfg.recent_threshold_seconds, 3600);
    /// ```
    fn default() -> Self {
        Self {
            sinks: vec![],
            dedup_window_seconds: 300,
            max_alerts_per_minute: None,
            recent_threshold_seconds: 3600,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: "human".to_owned(),
            file: None,
            structured: false,
        }
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        let socket_path = default_socket_path();

        Self {
            socket_path,
            enabled: true,
            startup_timeout_seconds: 30,
            shutdown_timeout_seconds: 60,
            max_connections: 100,
            message_buffer_size: 1000,
            topic_hierarchy: TopicHierarchyConfig::default(),
            collector_binaries: std::collections::HashMap::new(),
            process_manager: ProcessManagerConfig::default(),
            config_directory: default_config_directory(),
        }
    }
}

/// Determines the default socket path.
///
/// Uses `unidirs::ServiceDirs` to access the system-wide data directory for
/// system-level components (daemoneye-agent and collectors). The socket is placed
/// in the data directory rather than runtime directory for persistence across
/// system restarts.
///
/// Platform-specific paths (via `ServiceDirs` `data_dir`, which ignores the organization on Unix):
/// - **Unix** (Linux and macOS alike): `/var/lib/daemoneye/daemoneye-eventbus.sock`
/// - **Windows**: Uses named pipes (`\\.\pipe\daemoneye-eventbus`) which don't
///   require directory paths
///
/// The returned path is not length-checked here, because [`Default`] cannot fail. A path that
/// exceeds the Unix limit is refused at startup by [`BrokerConfig::ensure_socket_directory`]
/// rather than relocated: the socket directory also holds collector spawn tokens, so silently
/// moving it would move credentials somewhere the operator never nominated.
///
/// # Platform Limitations
///
/// - **Unix**: Paths must be ≤ 107 bytes (108 including the trailing NUL)
/// - **Windows**: Named pipes are used (no path length concerns)
fn default_socket_path() -> String {
    #[cfg(unix)]
    {
        // Use ServiceDirs for system-wide data directory
        let service_dirs = unidirs::ServiceDirs::new("evilbitlabs", "daemoneye");
        let data_dir = service_dirs.data_dir();
        data_dir.join("daemoneye-eventbus.sock").to_string()
    }

    #[cfg(windows)]
    {
        // Windows named pipes don't have the same path length restrictions
        r"\\.\pipe\daemoneye-eventbus".to_owned()
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        // For non-Unix, non-Windows platforms, document the limitation
        // In practice, these platforms may need TCP loopback as a transport
        // For now, we use a simple path that may not work on all platforms
        warn!(
            "Unsupported platform detected. Socket path may not function correctly. \
            Consider using TCP loopback (127.0.0.1) as an alternative transport."
        );
        // Use a minimal path that's unlikely to cause issues
        "/tmp/daemoneye-eventbus.sock".to_owned()
    }
}

impl BrokerConfig {
    /// Ensures the directory for the socket path exists with proper permissions.
    ///
    /// This function performs filesystem operations and should be called during broker startup.
    /// On Unix systems, it ensures the directory is owned by the current user and has
    /// restrictive permissions (0700) to prevent unauthorized access.
    ///
    /// # Returns
    ///
    /// Returns the socket path on success.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] when the configured path cannot fit a Unix domain
    /// socket address, or an error if directory creation or permission setting fails.
    pub fn ensure_socket_directory(&self) -> anyhow::Result<std::path::PathBuf> {
        // Refused rather than relocated: this directory also holds collector spawn tokens, so a
        // fallback to a shorter shared path would put credentials somewhere the operator never
        // chose. A `BrokerConfig::default()` never passes through `ConfigLoader::validate_config`,
        // so the same check runs here, on the startup path every caller takes.
        ConfigLoader::validate_socket_path(&self.socket_path)?;

        let socket_path = std::path::Path::new(&self.socket_path);

        #[cfg(windows)]
        {
            // Windows named pipes don't require directory creation
            Ok(socket_path.to_path_buf())
        }

        #[cfg(not(windows))]
        {
            let parent_dir = socket_path.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "Socket path {} has no parent directory.",
                    socket_path.display()
                )
            })?;

            if !parent_dir.exists() {
                // Created owner-only by the create itself, so the directory is never briefly
                // reachable. A directory this process made is ours to tighten.
                #[cfg(unix)]
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent_dir)
                    .with_context(|| {
                        format!(
                            "Failed to create socket directory: {}",
                            parent_dir.display()
                        )
                    })?;
                #[cfg(not(unix))]
                std::fs::create_dir_all(parent_dir).with_context(|| {
                    format!(
                        "Failed to create socket directory: {}",
                        parent_dir.display()
                    )
                })?;
                info!(
                    directory = %parent_dir.display(),
                    "Created socket directory"
                );
            }

            // A directory this process did not create is checked, never corrected. Chmodding one
            // the agent found is how an earlier version came to run `chmod 700` on a shared path,
            // and tightening someone else's directory is not this process's call to make.
            //
            // The check is on the group/other *write* bits, not on every group/other bit. Both
            // attacks here need write on the parent: pre-creating the directory to own what lands
            // in it, or replacing it with a symlink. Read or traverse access leaks directory
            // names, and the credential itself lives in the `spawn-tokens` subdirectory, which its
            // own store creates owner-only and verifies for owner and symlink separately. Refusing
            // on the read bits would also refuse a legitimately shared data directory -- procmond
            // creates it at 0o755 when it starts before the agent -- turning a deployment order
            // into a startup failure for no security gain.
            #[cfg(unix)]
            {
                let metadata = std::fs::symlink_metadata(parent_dir).with_context(|| {
                    format!(
                        "Failed to inspect socket directory: {}",
                        parent_dir.display()
                    )
                })?;
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "socket directory {} is a symlink; refusing to place a socket and collector \
                     spawn tokens behind one",
                    parent_dir.display()
                );
                let mode = metadata.permissions().mode() & 0o777;
                anyhow::ensure!(
                    mode & 0o022 == 0,
                    "socket directory {} is group- or world-writable (mode {mode:#o}); refusing \
                     rather than changing a directory this process did not create",
                    parent_dir.display()
                );
            };

            Ok(socket_path.to_path_buf())
        }
    }

    /// Resolves the binary path for a collector type.
    ///
    /// Searches in the following order:
    /// 1. Configured path in `collector_binaries`
    /// 2. Default installation paths
    /// 3. Development build paths
    ///
    /// # Arguments
    ///
    /// * `collector_type` - The type of collector (e.g., "procmond", "netmond")
    ///
    /// # Returns
    ///
    /// Returns the resolved binary path, or None if not found.
    pub fn resolve_collector_binary(&self, collector_type: &str) -> Option<PathBuf> {
        // 1. Check configured paths first
        if let Some(path) = self.collector_binaries.get(collector_type) {
            if path.exists() {
                return Some(path.clone());
            }
            warn!(
                collector_type,
                configured_path = %path.display(),
                "Configured collector binary not found"
            );
        }

        // 2. Check default installation paths
        let default_paths = [
            PathBuf::from(format!("/usr/local/bin/{collector_type}")),
            PathBuf::from(format!("/usr/bin/{collector_type}")),
        ];

        for path in &default_paths {
            if path.exists() {
                info!(
                    collector_type,
                    resolved_path = %path.display(),
                    "Resolved collector binary from default path"
                );
                return Some(path.clone());
            }
        }

        // 3. Check development build paths
        let dev_paths = [
            PathBuf::from(format!("./target/release/{collector_type}")),
            PathBuf::from(format!("./target/debug/{collector_type}")),
        ];

        for path in &dev_paths {
            if path.exists() {
                info!(
                    collector_type,
                    resolved_path = %path.display(),
                    "Resolved collector binary from development path"
                );
                return Some(path.clone());
            }
        }

        warn!(collector_type, "Failed to resolve collector binary path");
        None
    }
}

impl Default for TopicHierarchyConfig {
    fn default() -> Self {
        Self {
            enable_wildcards: true,
            max_topic_depth: 5,
            event_topics: EventTopicsConfig::default(),
            control_topics: ControlTopicsConfig::default(),
        }
    }
}

impl Default for EventTopicsConfig {
    fn default() -> Self {
        Self {
            process: "events.process".to_owned(),
            network: "events.network".to_owned(),
            filesystem: "events.filesystem".to_owned(),
            performance: "events.performance".to_owned(),
        }
    }
}

impl Default for ControlTopicsConfig {
    fn default() -> Self {
        Self {
            collector: "control.collector".to_owned(),
            health: "control.health".to_owned(),
        }
    }
}

/// Configuration loader with hierarchical override support.
pub struct ConfigLoader {
    component: String,
}

impl ConfigLoader {
    /// Create a new configuration loader for the specified component.
    pub fn new(component: &str) -> Self {
        Self {
            component: component.to_owned(),
        }
    }

    /// Load configuration with hierarchical overrides using figment.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let mut figment = Figment::new()
            // Start with embedded defaults
            .merge(Serialized::defaults(Config::default()));

        // System configuration file (optional, platform-specific)
        #[cfg(unix)]
        {
            let system_config_path = std::path::Path::new("/etc/daemoneye/config.toml");
            if system_config_path.exists() {
                figment = figment.merge(Toml::file(system_config_path));
            }
        }

        #[cfg(windows)]
        {
            // Windows: ProgramData provides machine-wide configuration
            if let Ok(program_data) = std::env::var("PROGRAMDATA") {
                let system_config_path = std::path::Path::new(&program_data)
                    .join("DaemonEye")
                    .join("config.toml");
                if system_config_path.exists() {
                    figment = figment.merge(Toml::file(&system_config_path));
                }
            }
        }

        // Other platforms rely solely on user-scoped configuration (loaded below).

        // User configuration file (optional)
        match Self::user_config_path() {
            Ok(user_config_path) => {
                if user_config_path.exists() {
                    figment = figment.merge(Toml::file(&user_config_path));
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Skipping user-scoped configuration because the directory could not be determined"
                );
            }
        }

        // Environment variables with component prefix
        figment = figment.merge(
            Env::prefixed(&format!(
                "{}_",
                self.component.replace('-', "_").to_uppercase()
            ))
            .split("__"),
        );

        let config = figment.extract()?;

        // Validate final configuration
        Self::validate_config(&config)?;

        Ok(config)
    }

    /// Load configuration synchronously (for CLI usage).
    pub fn load_blocking(&self) -> Result<Config, ConfigError> {
        // Use the same figment-based loading for consistency
        self.load()
    }

    /// Get the user configuration file path using platform-aware directory lookup.
    ///
    /// Priority:
    /// 1. Platform-specific config directory (via `dirs::config_dir()`)
    /// 2. HOME environment variable (if available)
    /// 3. Returns an error if no user configuration directory can be determined
    fn user_config_path() -> Result<PathBuf, ConfigError> {
        // Try platform-aware config directory first
        if let Some(config_dir) = dirs::config_dir() {
            return Ok(config_dir.join("daemoneye").join("config.toml"));
        }

        // Fallback to HOME environment variable
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home)
                .join(".config")
                .join("daemoneye")
                .join("config.toml"));
        }

        Err(ConfigError::ValidationError {
            message: "Unable to determine a user configuration directory. Ensure HOME is set or provide an explicit configuration path.".to_owned(),
        })
    }

    /// Validate the final configuration.
    ///
    /// Delegates to [`Config::validate`], which is the public seam callers outside this module
    /// (including integration tests) use.
    fn validate_config(config: &Config) -> Result<(), ConfigError> {
        config.validate()
    }
}

impl Config {
    /// Validate numeric ranges, path safety, and OS socket-path limits.
    ///
    /// Applied automatically by [`ConfigLoader::load`] after the TOML and environment layers have
    /// been merged, so a value out of range is rejected from either source rather than clamped.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // --- Numeric range validation ---

        const SCAN_INTERVAL_MIN: u64 = 100;
        const SCAN_INTERVAL_MAX: u64 = 3_600_000;
        const BATCH_SIZE_MIN: usize = 1;
        const BATCH_SIZE_MAX: usize = 10_000;
        const RETENTION_DAYS_MIN: u32 = 1;
        const RETENTION_DAYS_MAX: u32 = 3_650;

        let config = self;

        if config.app.scan_interval_ms < SCAN_INTERVAL_MIN
            || config.app.scan_interval_ms > SCAN_INTERVAL_MAX
        {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "scan_interval_ms must be between {SCAN_INTERVAL_MIN} and {SCAN_INTERVAL_MAX}, got {}",
                    config.app.scan_interval_ms
                ),
            });
        }

        if config.app.batch_size < BATCH_SIZE_MIN || config.app.batch_size > BATCH_SIZE_MAX {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "batch_size must be between {BATCH_SIZE_MIN} and {BATCH_SIZE_MAX}, got {}",
                    config.app.batch_size
                ),
            });
        }

        if config.database.retention_days < RETENTION_DAYS_MIN
            || config.database.retention_days > RETENTION_DAYS_MAX
        {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "retention_days must be between {RETENTION_DAYS_MIN} and {RETENTION_DAYS_MAX}, got {}",
                    config.database.retention_days
                ),
            });
        }

        // --- Detection bounds validation ---

        config.detection.validate()?;
        config.database.validate()?;

        // --- Path traversal validation ---

        ConfigLoader::validate_path_no_traversal(&config.database.path, "database.path")?;
        ConfigLoader::validate_path_no_traversal(
            &config.broker.config_directory,
            "broker.config_directory",
        )?;

        if let Some(ref log_file) = config.logging.file {
            ConfigLoader::validate_path_no_traversal(log_file, "logging.file")?;
        }

        for (collector_type, binary_path) in &config.broker.collector_binaries {
            ConfigLoader::validate_path_no_traversal(
                binary_path,
                &format!("broker.collector_binaries[{collector_type}]"),
            )?;
        }

        // --- Socket path validation ---

        ConfigLoader::validate_socket_path(&config.broker.socket_path)?;

        Ok(())
    }
}

impl ConfigLoader {
    /// Validate that a path does not contain `..` components (directory traversal).
    fn validate_path_no_traversal(path: &std::path::Path, field: &str) -> Result<(), ConfigError> {
        use std::path::Component;
        for component in path.components() {
            if component == Component::ParentDir {
                return Err(ConfigError::ValidationError {
                    message: format!(
                        "Path field '{field}' must not contain '..' (directory traversal): {}",
                        path.display()
                    ),
                });
            }
        }
        Ok(())
    }

    /// Validate a socket path for null bytes, directory traversal, and OS length limits.
    ///
    /// Unix domain socket paths are limited to 108 bytes on Linux and macOS (the `sun_path`
    /// field in `sockaddr_un` is 108 bytes including the NUL terminator).
    fn validate_socket_path(socket_path: &str) -> Result<(), ConfigError> {
        // 108-byte sun_path limit minus 1 for the NUL terminator.
        const SOCKET_PATH_MAX_LEN: usize = 107;

        if socket_path.contains('\0') {
            return Err(ConfigError::ValidationError {
                message: "broker.socket_path must not contain null bytes".to_owned(),
            });
        }

        // Reject directory-traversal components.
        for component in std::path::Path::new(socket_path).components() {
            if component == std::path::Component::ParentDir {
                return Err(ConfigError::ValidationError {
                    message: format!(
                        "broker.socket_path must not contain '..' (directory traversal): {socket_path}"
                    ),
                });
            }
        }

        if socket_path.len() > SOCKET_PATH_MAX_LEN {
            return Err(ConfigError::ValidationError {
                message: format!(
                    "broker.socket_path must not exceed {SOCKET_PATH_MAX_LEN} bytes (OS sun_path limit), got {} bytes: {socket_path}",
                    socket_path.len()
                ),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    // use std::env; // Removed due to unsafe_code = "forbid"

    #[tokio::test]
    async fn test_config_loader_default() {
        let loader = ConfigLoader::new("procmond");
        let config = loader.load().expect("Failed to load config in test");

        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.app.batch_size, 1000);
        assert_eq!(config.database.retention_days, 30);
    }

    #[tokio::test]
    async fn test_config_loader_figment_defaults() {
        let loader = ConfigLoader::new("procmond");
        let config = loader.load().expect("Failed to load config in test");

        // Test with figment-loaded defaults
        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 0;

        let _loader = ConfigLoader::new("procmond");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = Config::default();
        let _loader = ConfigLoader::new("procmond");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_ok());
    }

    // --- Numeric range validation tests ---

    #[test]
    fn test_validate_detection_subquery_depth_zero() {
        let mut config = Config::default();
        config.detection.max_subquery_depth = 0;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("detection.max_subquery_depth"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_detection_latency_threshold_above_maximum() {
        let mut config = Config::default();
        config.detection.pattern_latency_threshold_ms =
            DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX + 1;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("detection.pattern_latency_threshold_ms"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_detection_defaults_are_in_range() {
        let config = Config::default();
        assert_eq!(config.detection.max_subquery_depth, 3);
        assert_eq!(config.detection.pattern_latency_threshold_ms, 10);
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_scan_interval_below_minimum() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 99;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("scan_interval_ms"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_scan_interval_at_minimum() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 100;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_scan_interval_above_maximum() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 3_600_001;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("scan_interval_ms"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_scan_interval_at_maximum() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 3_600_000;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_batch_size_below_minimum() {
        let mut config = Config::default();
        config.app.batch_size = 0;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("batch_size"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_batch_size_above_maximum() {
        let mut config = Config::default();
        config.app.batch_size = 10_001;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("batch_size"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_batch_size_at_boundaries() {
        let mut config = Config::default();
        config.app.batch_size = 1;
        assert!(ConfigLoader::validate_config(&config).is_ok());
        config.app.batch_size = 10_000;
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_retention_days_below_minimum() {
        let mut config = Config::default();
        config.database.retention_days = 0;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("retention_days"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_retention_days_above_maximum() {
        let mut config = Config::default();
        config.database.retention_days = 3_651;
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("retention_days"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_retention_days_at_boundaries() {
        let mut config = Config::default();
        config.database.retention_days = 1;
        assert!(ConfigLoader::validate_config(&config).is_ok());
        config.database.retention_days = 3_650;
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    // --- Path traversal validation tests ---

    #[test]
    fn test_validate_database_path_traversal() {
        let mut config = Config::default();
        config.database.path = PathBuf::from("/var/lib/../etc/daemoneye/processes.db");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("database.path"),
            "error should mention field: {msg}"
        );
        assert!(msg.contains(".."), "error should mention '..': {msg}");
    }

    #[test]
    fn test_validate_database_path_valid() {
        let mut config = Config::default();
        config.database.path = PathBuf::from("/var/lib/daemoneye/processes.db");
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_logging_file_traversal() {
        let mut config = Config::default();
        config.logging.file = Some(PathBuf::from("/var/log/../../../etc/passwd"));
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("logging.file"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_logging_file_none_is_ok() {
        let mut config = Config::default();
        config.logging.file = None;
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_logging_file_valid() {
        let mut config = Config::default();
        config.logging.file = Some(PathBuf::from("/var/log/daemoneye/agent.log"));
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_broker_config_directory_traversal() {
        let mut config = Config::default();
        config.broker.config_directory = PathBuf::from("/etc/daemoneye/../../../tmp/evil");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("broker.config_directory"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_collector_binary_path_traversal() {
        let mut config = Config::default();
        config.broker.collector_binaries.insert(
            "procmond".to_owned(),
            PathBuf::from("/usr/bin/../../../etc/passwd"),
        );
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("broker.collector_binaries[procmond]"),
            "error should mention field: {msg}"
        );
    }

    #[test]
    fn test_validate_collector_binary_path_valid() {
        let mut config = Config::default();
        config
            .broker
            .collector_binaries
            .insert("procmond".to_owned(), PathBuf::from("/usr/bin/procmond"));
        assert!(ConfigLoader::validate_config(&config).is_ok());
    }

    // --- Socket path validation tests ---

    #[test]
    fn test_validate_socket_path_null_byte() {
        let result = ConfigLoader::validate_socket_path("/tmp/test\0.sock");
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("null bytes"),
            "error should mention null bytes: {msg}"
        );
    }

    #[test]
    fn test_validate_socket_path_too_long() {
        // 108 bytes total — one over the 107-byte limit.
        let mut long_path = String::from("/tmp/");
        long_path.push_str(&"a".repeat(103));
        assert_eq!(
            long_path.len(),
            108,
            "path must be 108 bytes to exceed the 107-byte limit"
        );
        let result = ConfigLoader::validate_socket_path(&long_path);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("107"),
            "error should mention max length: {msg}"
        );
    }

    #[test]
    fn test_validate_socket_path_exactly_at_limit() {
        // 107 bytes exactly — should pass (the OS limit is 108 bytes including NUL).
        let mut path = String::from("/tmp/");
        path.push_str(&"a".repeat(102));
        assert_eq!(path.len(), 107, "path must be exactly 107 bytes");
        let result = ConfigLoader::validate_socket_path(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_socket_path_traversal() {
        let result = ConfigLoader::validate_socket_path("/tmp/../etc/evil.sock");
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("directory traversal"),
            "error should mention directory traversal: {msg}"
        );
    }

    #[test]
    fn test_validate_socket_path_valid() {
        let result = ConfigLoader::validate_socket_path("/tmp/daemoneye-eventbus.sock");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_broker_socket_path_via_config() {
        let mut config = Config::default();
        config.broker.socket_path = "/tmp/test\0malicious.sock".to_owned();
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        let msg = format!("{}", result.expect_err("expected validation error"));
        assert!(
            msg.contains("null bytes"),
            "error should mention null bytes: {msg}"
        );
    }

    #[test]
    fn test_config_loader_creation() {
        let loader = ConfigLoader::new("test-component");
        assert_eq!(loader.component, "test-component");
    }

    #[test]
    fn test_config_loader_load_blocking() {
        let loader = ConfigLoader::new("procmond");
        let config = loader
            .load_blocking()
            .expect("Failed to load config in test");
        assert_eq!(config.app.scan_interval_ms, 30000);
    }

    #[test]
    fn test_config_loader_handles_missing_files() {
        // This test verifies that configuration loading works even when
        // system and user config files don't exist (should fall back to defaults)
        let loader = ConfigLoader::new("test-component");
        let config = loader
            .load()
            .expect("Failed to load config with missing files");

        // Should get default values when no config files exist
        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.app.batch_size, 1000);
    }

    #[test]
    fn test_config_figment_serialization() {
        let config = Config::default();
        let figment = Figment::new().merge(Serialized::defaults(config.clone()));
        let extracted: Config = figment.extract().expect("Failed to extract config in test");
        assert_eq!(config.app.scan_interval_ms, extracted.app.scan_interval_ms);
    }

    #[test]
    fn test_config_error_display() {
        let errors = vec![
            ConfigError::ValidationError {
                message: "test error".to_owned(),
            },
            ConfigError::IoError(std::io::Error::other("test error")),
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_app_config_creation() {
        let app_config = AppConfig::default();
        assert_eq!(app_config.scan_interval_ms, 30000);
        assert_eq!(app_config.batch_size, 1000);
    }

    #[test]
    fn test_database_config_creation() {
        let db_config = DatabaseConfig::default();
        assert_eq!(
            db_config.path,
            std::path::PathBuf::from("/var/lib/daemoneye/processes.db")
        );
        assert_eq!(db_config.retention_days, 30);
    }

    #[test]
    fn test_logging_config_creation() {
        let logging_config = LoggingConfig::default();
        assert_eq!(logging_config.level, "info");
        assert_eq!(logging_config.format, "human");
    }

    #[test]
    fn test_alerting_config_creation() {
        let alerting_config = AlertingConfig::default();
        assert!(alerting_config.sinks.is_empty());
        assert_eq!(alerting_config.recent_threshold_seconds, 3600);
    }

    #[test]
    fn test_config_toml_serialization() {
        let config = Config::default();
        let toml = toml::to_string(&config).expect("Failed to serialize config to TOML in test");
        let deserialized: Config =
            toml::from_str(&toml).expect("Failed to deserialize config from TOML in test");
        assert_eq!(
            config.app.scan_interval_ms,
            deserialized.app.scan_interval_ms
        );
        assert_eq!(
            config.alerting.recent_threshold_seconds,
            deserialized.alerting.recent_threshold_seconds
        );
    }

    #[test]
    fn test_alerting_config_recent_threshold() {
        let mut config = AlertingConfig::default();
        assert_eq!(config.recent_threshold_seconds, 3600);

        // Test custom threshold
        config.recent_threshold_seconds = 1800;
        assert_eq!(config.recent_threshold_seconds, 1800);
    }

    #[test]
    fn test_alert_sink_config_with_different_value_types() {
        use serde_json::json;

        // Test with string values
        let sink_with_strings = AlertSinkConfig {
            sink_type: "webhook".to_owned(),
            config: json!({
                "url": "https://example.com/webhook",
                "method": "POST"
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_strings
                .config
                .get("url")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com/webhook")
        );

        // Test with numeric values
        let sink_with_numbers = AlertSinkConfig {
            sink_type: "syslog".to_owned(),
            config: json!({
                "port": 514,
                "timeout_ms": 5000,
                "retry_count": 3
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_numbers
                .config
                .get("port")
                .and_then(serde_json::Value::as_u64),
            Some(514)
        );
        assert_eq!(
            sink_with_numbers
                .config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_u64),
            Some(5000)
        );
        assert_eq!(
            sink_with_numbers
                .config
                .get("retry_count")
                .and_then(serde_json::Value::as_u64),
            Some(3)
        );

        // Test with boolean values
        let sink_with_bools = AlertSinkConfig {
            sink_type: "custom".to_owned(),
            config: json!({
                "use_tls": true,
                "verify_ssl": false
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_bools
                .config
                .get("use_tls")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            sink_with_bools
                .config
                .get("verify_ssl")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );

        // Test with array values
        let sink_with_arrays = AlertSinkConfig {
            sink_type: "multi".to_owned(),
            config: json!({
                "endpoints": ["http://endpoint1.com", "http://endpoint2.com"],
                "priorities": [1, 2, 3]
            }),
            enabled: true,
        };
        assert!(
            sink_with_arrays
                .config
                .get("endpoints")
                .is_some_and(serde_json::Value::is_array)
        );
        assert_eq!(
            sink_with_arrays
                .config
                .get("endpoints")
                .and_then(serde_json::Value::as_array)
                .map(std::vec::Vec::len),
            Some(2)
        );
    }

    #[test]
    fn test_broker_config_creation() {
        let broker_config = BrokerConfig::default();
        if cfg!(windows) {
            assert_eq!(broker_config.socket_path, r"\\.\pipe\daemoneye-eventbus");
        } else {
            assert!(
                broker_config
                    .socket_path
                    .ends_with("daemoneye-eventbus.sock")
            );
        }
        assert!(broker_config.enabled);
        assert_eq!(broker_config.startup_timeout_seconds, 30);
        assert_eq!(broker_config.shutdown_timeout_seconds, 60);
        assert_eq!(broker_config.max_connections, 100);
        assert_eq!(broker_config.message_buffer_size, 1000);
    }

    #[test]
    fn test_topic_hierarchy_config_creation() {
        let topic_config = TopicHierarchyConfig::default();
        assert!(topic_config.enable_wildcards);
        assert_eq!(topic_config.max_topic_depth, 5);
        assert_eq!(topic_config.event_topics.process, "events.process");
        assert_eq!(topic_config.control_topics.health, "control.health");
    }

    #[test]
    fn test_config_toml_with_complex_sink_config() {
        let toml_str = r#"
[app]
scan_interval_ms = 30000
batch_size = 1000
enhanced_metadata = false

[database]
path = "/tmp/test.db"
retention_days = 30
encryption_enabled = false

[logging]
level = "info"
format = "human"
structured = false

[alerting]
dedup_window_seconds = 300
recent_threshold_seconds = 3600

[broker]
socket_path = "/tmp/test-broker.sock"
enabled = true
startup_timeout_seconds = 30
shutdown_timeout_seconds = 60
max_connections = 100
message_buffer_size = 1000

[broker.topic_hierarchy]
enable_wildcards = true
max_topic_depth = 5

[broker.topic_hierarchy.event_topics]
process = "events.process"
network = "events.network"
filesystem = "events.filesystem"
performance = "events.performance"

[broker.topic_hierarchy.control_topics]
collector = "control.collector"
health = "control.health"

[[alerting.sinks]]
sink_type = "webhook"
enabled = true
[alerting.sinks.config]
url = "https://example.com/webhook"
timeout_ms = 5000
retry_count = 3

[[alerting.sinks]]
sink_type = "syslog"
enabled = true
[alerting.sinks.config]
facility = "daemon"
port = 514
use_tls = false
"#;

        let config: Config =
            toml::from_str(toml_str).expect("Failed to parse TOML with complex sink config");

        assert_eq!(config.alerting.sinks.len(), 2);
        // A file written before `page_cache_mb` existed still loads, at the default.
        assert_eq!(
            config.database.page_cache_mb,
            DatabaseConfig::PAGE_CACHE_MB_DEFAULT
        );

        // Verify broker configuration
        assert_eq!(config.broker.socket_path, "/tmp/test-broker.sock");
        assert!(config.broker.enabled);
        assert_eq!(config.broker.startup_timeout_seconds, 30);
        assert_eq!(config.broker.shutdown_timeout_seconds, 60);
        assert_eq!(config.broker.max_connections, 100);
        assert_eq!(config.broker.message_buffer_size, 1000);

        // Verify topic hierarchy configuration
        assert!(config.broker.topic_hierarchy.enable_wildcards);
        assert_eq!(config.broker.topic_hierarchy.max_topic_depth, 5);
        assert_eq!(
            config.broker.topic_hierarchy.event_topics.process,
            "events.process"
        );
        assert_eq!(
            config.broker.topic_hierarchy.control_topics.health,
            "control.health"
        );

        // Verify webhook sink
        let webhook_sink = config
            .alerting
            .sinks
            .first()
            .expect("expected first alert sink");
        assert_eq!(webhook_sink.sink_type, "webhook");
        assert!(webhook_sink.enabled);
        assert_eq!(
            webhook_sink
                .config
                .get("url")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com/webhook")
        );
        assert_eq!(
            webhook_sink
                .config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_i64),
            Some(5000)
        );
        assert_eq!(
            webhook_sink
                .config
                .get("retry_count")
                .and_then(serde_json::Value::as_i64),
            Some(3)
        );

        // Verify syslog sink
        let syslog_sink = config
            .alerting
            .sinks
            .get(1)
            .expect("expected second alert sink");
        assert_eq!(syslog_sink.sink_type, "syslog");
        assert!(syslog_sink.enabled);
        assert_eq!(
            syslog_sink
                .config
                .get("facility")
                .and_then(serde_json::Value::as_str),
            Some("daemon")
        );
        assert_eq!(
            syslog_sink
                .config
                .get("port")
                .and_then(serde_json::Value::as_i64),
            Some(514)
        );
        assert_eq!(
            syslog_sink
                .config
                .get("use_tls")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    // ============================================================================
    // ConfigLoader Environment Variable Tests
    // ============================================================================

    #[test]
    fn test_config_loader_with_different_components() {
        // Test that different components create unique loaders
        let procmond_loader = ConfigLoader::new("procmond");
        let agent_loader = ConfigLoader::new("daemoneye-agent");
        let cli_loader = ConfigLoader::new("daemoneye-cli");

        assert_eq!(procmond_loader.component, "procmond");
        assert_eq!(agent_loader.component, "daemoneye-agent");
        assert_eq!(cli_loader.component, "daemoneye-cli");
    }

    // ============================================================================
    // Validation Error Tests
    // ============================================================================

    #[test]
    fn test_config_validation_batch_size_zero() {
        let mut config = Config::default();
        config.app.batch_size = 0;

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());

        let err = result.expect_err("validation should fail for zero batch_size");
        assert!(err.to_string().contains("batch_size"));
    }

    #[test]
    fn test_config_validation_retention_days_zero() {
        let mut config = Config::default();
        config.database.retention_days = 0;

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());

        let err = result.expect_err("validation should fail for zero retention_days");
        assert!(err.to_string().contains("retention_days"));
    }

    #[test]
    fn test_config_validation_all_invalid_combined() {
        // Test that the first validation failure is caught
        let mut config = Config::default();
        config.app.scan_interval_ms = 0;
        config.app.batch_size = 0;
        config.database.retention_days = 0;

        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
        // First validation (scan_interval_ms) should be caught
        let err = result.expect_err("validation should fail for invalid config");
        assert!(err.to_string().contains("scan_interval_ms"));
    }

    // ============================================================================
    // Default Value Fallback Tests
    // ============================================================================

    #[test]
    fn test_default_sink_config_function() {
        let result = default_sink_config();
        assert!(result.is_object());
        assert!(
            result
                .as_object()
                .expect("default sink config should be an object")
                .is_empty()
        );
    }

    #[test]
    fn test_alert_sink_config_default_config_field() {
        // Test that AlertSinkConfig uses default_sink_config when not specified
        let toml_str = r#"
sink_type = "test"
enabled = true
"#;

        let sink: AlertSinkConfig =
            toml::from_str(toml_str).expect("Failed to parse AlertSinkConfig without config field");

        assert_eq!(sink.sink_type, "test");
        assert!(sink.enabled);
        assert!(sink.config.is_object());
        assert!(
            sink.config
                .as_object()
                .expect("sink config should be an object")
                .is_empty()
        );
    }

    // ============================================================================
    // BrokerConfig Tests
    // ============================================================================

    #[test]
    fn test_broker_config_resolve_collector_binary_configured() {
        use std::collections::HashMap;

        let mut binaries = HashMap::new();
        // Point to a path that doesn't exist
        binaries.insert(
            "procmond".to_owned(),
            PathBuf::from("/nonexistent/procmond"),
        );

        let config = BrokerConfig {
            collector_binaries: binaries,
            ..Default::default()
        };

        // Should return None because the configured path doesn't exist
        let result = config.resolve_collector_binary("procmond");
        assert!(result.is_none());
    }

    #[test]
    fn test_broker_config_resolve_collector_binary_not_configured() {
        let config = BrokerConfig::default();

        // Should return None because nothing is configured and defaults don't exist
        let result = config.resolve_collector_binary("nonexistent-collector");
        assert!(result.is_none());
    }

    #[test]
    fn test_process_manager_config_default() {
        let config = ProcessManagerConfig::default();

        assert_eq!(config.graceful_shutdown_timeout_seconds, 30);
        assert_eq!(config.force_shutdown_timeout_seconds, 5);
        assert_eq!(config.health_check_interval_seconds, 60);
        assert!(!config.enable_auto_restart);
        assert_eq!(config.max_restart_attempts, 3);
    }

    // ============================================================================
    // Topic Configuration Tests
    // ============================================================================

    #[test]
    fn test_event_topics_config_default() {
        let config = EventTopicsConfig::default();

        assert_eq!(config.process, "events.process");
        assert_eq!(config.network, "events.network");
        assert_eq!(config.filesystem, "events.filesystem");
        assert_eq!(config.performance, "events.performance");
    }

    #[test]
    fn test_control_topics_config_default() {
        let config = ControlTopicsConfig::default();

        assert_eq!(config.collector, "control.collector");
        assert_eq!(config.health, "control.health");
    }

    // ============================================================================
    // ConfigError Tests
    // ============================================================================

    #[test]
    fn test_config_error_file_not_found_display() {
        let err = ConfigError::FileNotFound {
            path: PathBuf::from("/nonexistent/config.toml"),
        };
        let msg = err.to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("/nonexistent/config.toml"));
    }

    #[test]
    fn test_config_error_validation_display() {
        let err = ConfigError::ValidationError {
            message: "scan_interval_ms must be greater than 0".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("validation failed"));
        assert!(msg.contains("scan_interval_ms"));
    }

    // ============================================================================
    // Config Equality Tests
    // ============================================================================

    #[test]
    fn test_config_equality() {
        let config1 = Config::default();
        let config2 = Config::default();

        assert_eq!(config1, config2);

        let mut config3 = Config::default();
        config3.app.scan_interval_ms = 60000;

        assert_ne!(config1, config3);
    }

    #[test]
    fn test_app_config_equality() {
        let config1 = AppConfig::default();
        let config2 = AppConfig::default();

        assert_eq!(config1, config2);

        let config3 = AppConfig {
            scan_interval_ms: 60000,
            ..Default::default()
        };

        assert_ne!(config1, config3);
    }

    // ============================================================================
    // Config Clone Tests
    // ============================================================================

    #[test]
    fn test_config_clone() {
        let original = Config::default();
        let cloned = original.clone();

        assert_eq!(original, cloned);
        assert_eq!(original.app.scan_interval_ms, cloned.app.scan_interval_ms);
    }

    #[test]
    fn test_broker_config_clone() {
        let original = BrokerConfig::default();
        let cloned = original.clone();

        assert_eq!(original.socket_path, cloned.socket_path);
        assert_eq!(original.enabled, cloned.enabled);
    }

    // ============================================================================
    // Socket Path Tests
    // ============================================================================

    #[test]
    fn test_default_socket_path_is_valid() {
        let config = BrokerConfig::default();

        // On Windows, should be a named pipe path
        // On Unix, should be a socket path
        #[cfg(windows)]
        {
            assert!(config.socket_path.starts_with(r"\\.\pipe\"));
        }

        #[cfg(unix)]
        {
            assert!(
                std::path::Path::new(&config.socket_path)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sock"))
                    || config.socket_path.contains("daemoneye"),
                "Socket path should be a valid Unix socket: {}",
                config.socket_path
            );
        }
    }

    /// The default must fit the platform limit on its own, now that no fallback rewrites it.
    #[cfg(unix)]
    #[test]
    fn test_default_socket_path_fits_unix_limit() {
        let config = BrokerConfig::default();

        assert!(
            ConfigLoader::validate_socket_path(&config.socket_path).is_ok(),
            "default socket path must be usable without a fallback: {}",
            config.socket_path
        );
    }

    /// An over-long path is refused, not relocated to a shorter shared directory.
    #[cfg(unix)]
    #[test]
    fn test_ensure_socket_directory_refuses_overlong_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let long_component = "a".repeat(107);
        let socket_path = temp_dir
            .path()
            .join(long_component)
            .join("broker.sock")
            .to_string_lossy()
            .to_string();
        let config = BrokerConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };

        let error = config
            .ensure_socket_directory()
            .expect_err("over-long socket path must be refused");

        let message = error.to_string();
        assert!(
            matches!(
                error.downcast_ref::<ConfigError>(),
                Some(ConfigError::ValidationError { .. })
            ),
            "expected a validation error, got: {message}"
        );
        assert!(
            message.contains("107"),
            "message must name the limit: {message}"
        );
        assert!(
            message.contains(&socket_path),
            "message must name the offending path: {message}"
        );
        assert!(
            !std::path::Path::new(&socket_path)
                .parent()
                .is_some_and(std::path::Path::exists),
            "refusal must not create the socket directory"
        );
    }

    // ============================================================================
    // Serialization Round-Trip Tests
    // ============================================================================

    #[test]
    fn test_config_json_round_trip() {
        let original = Config::default();
        let json =
            serde_json::to_string(&original).expect("Failed to serialize config to JSON in test");
        let deserialized: Config =
            serde_json::from_str(&json).expect("Failed to deserialize config from JSON in test");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_alerting_config_json_round_trip() {
        use serde_json::json;

        let original = AlertingConfig {
            sinks: vec![AlertSinkConfig {
                sink_type: "webhook".to_owned(),
                config: json!({
                    "url": "https://example.com",
                    "timeout_ms": 5000
                }),
                enabled: true,
            }],
            dedup_window_seconds: 600,
            max_alerts_per_minute: Some(100),
            recent_threshold_seconds: 7200,
        };

        let json = serde_json::to_string(&original)
            .expect("Failed to serialize AlertingConfig to JSON in test");
        let deserialized: AlertingConfig = serde_json::from_str(&json)
            .expect("Failed to deserialize AlertingConfig from JSON in test");

        assert_eq!(original, deserialized);
    }

    // ============================================================================
    // Environment Variable Prefix Tests
    // ============================================================================

    #[test]
    fn test_config_loader_component_prefix_normalization() {
        // Test that component names with hyphens are normalized for env vars
        let loader = ConfigLoader::new("daemoneye-agent");

        // The component should be stored as-is
        assert_eq!(loader.component, "daemoneye-agent");

        // When loading, it should be normalized to DAEMONEYE_AGENT_ prefix
        // This is implicitly tested through the load() method
    }

    #[test]
    fn page_cache_mb_is_range_checked_at_both_ends() {
        let with = |page_cache_mb| DatabaseConfig {
            page_cache_mb,
            ..DatabaseConfig::default()
        };
        assert!(DatabaseConfig::default().validate().is_ok());
        assert!(with(DatabaseConfig::PAGE_CACHE_MB_MIN).validate().is_ok());
        assert!(with(DatabaseConfig::PAGE_CACHE_MB_MAX).validate().is_ok());
        assert!(
            with(DatabaseConfig::PAGE_CACHE_MB_MIN.saturating_sub(1))
                .validate()
                .is_err()
        );
        assert!(
            with(DatabaseConfig::PAGE_CACHE_MB_MAX.saturating_add(1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn page_cache_bytes_converts_mib() {
        let config = DatabaseConfig {
            page_cache_mb: 3,
            ..DatabaseConfig::default()
        };
        assert_eq!(config.page_cache_bytes(), 3 * 1024 * 1024);
    }

    #[test]
    fn config_validate_rejects_an_out_of_range_page_cache() {
        let mut config = Config::default();
        config.database.page_cache_mb = 0;
        assert!(config.validate().is_err());
    }
}
