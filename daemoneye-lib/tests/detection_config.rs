//! Detection configuration and fixed-bounds tests (U2).
//!
//! Covers the two tunables the Product Contract declares configurable — subquery nesting depth
//! and the per-pattern latency threshold — and the fixed constants the regex cache's memory proof
//! rests on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use daemoneye_lib::config::{Config, ConfigLoader, DetectionConfig};
use daemoneye_lib::detection_bounds::{
    REGEX_CACHE_MAX_BYTES, REGEX_CACHE_MAX_ENTRIES, REGEX_DFA_SIZE_LIMIT_BYTES,
    REGEX_SIZE_LIMIT_BYTES,
};
use figment::{
    Figment, Jail,
    providers::{Format, Serialized, Toml},
};

/// Builds a `Config` from embedded defaults overlaid with a TOML file, mirroring the layering
/// `ConfigLoader::load` performs. The loader offers no explicit-path API, and redirecting it at a
/// temporary directory would depend on `dirs` honouring `HOME`, which it does not on Windows.
fn config_from_toml(path: &str) -> Result<Config, figment::Error> {
    Figment::from(Serialized::defaults(Config::default()))
        .merge(Toml::file(path))
        .extract()
}

#[test]
fn defaults_match_the_values_the_requirements_state() {
    let detection = DetectionConfig::default();

    assert_eq!(
        detection.max_subquery_depth, 3,
        "R3 fixes the default subquery depth at 3"
    );
    assert_eq!(
        detection.pattern_latency_threshold_ms, 10,
        "R7 fixes the default pattern latency threshold at 10ms"
    );
    assert_eq!(
        Config::default().detection,
        detection,
        "the detection section of a default Config is the DetectionConfig default"
    );
    Config::default()
        .validate()
        .expect("default configuration must validate");
}

#[test]
fn subquery_depth_of_zero_is_rejected_naming_the_field_and_range() {
    let mut config = Config::default();
    config.detection.max_subquery_depth = 0;

    let err = config
        .validate()
        .expect_err("a subquery depth of zero must be rejected");
    let msg = err.to_string();

    assert!(
        msg.contains("detection.max_subquery_depth"),
        "error must name the field: {msg}"
    );
    assert!(
        msg.contains(&DetectionConfig::MAX_SUBQUERY_DEPTH_MIN.to_string())
            && msg.contains(&DetectionConfig::MAX_SUBQUERY_DEPTH_MAX.to_string()),
        "error must state the valid range: {msg}"
    );
}

#[test]
fn subquery_depth_boundaries_are_inclusive() {
    let mut config = Config::default();

    config.detection.max_subquery_depth = DetectionConfig::MAX_SUBQUERY_DEPTH_MIN;
    config.validate().expect("minimum depth must be accepted");

    config.detection.max_subquery_depth = DetectionConfig::MAX_SUBQUERY_DEPTH_MAX;
    config.validate().expect("maximum depth must be accepted");

    config.detection.max_subquery_depth = DetectionConfig::MAX_SUBQUERY_DEPTH_MAX + 1;
    config
        .validate()
        .expect_err("a depth above the maximum must be rejected");
}

#[test]
fn latency_threshold_out_of_range_is_rejected_not_clamped() {
    let mut config = Config::default();
    config.detection.pattern_latency_threshold_ms =
        DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX + 1;

    let err = config
        .validate()
        .expect_err("a latency threshold above the maximum must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("detection.pattern_latency_threshold_ms"),
        "error must name the field: {msg}"
    );
    assert_eq!(
        config.detection.pattern_latency_threshold_ms,
        DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX + 1,
        "validation must reject rather than silently clamp the value"
    );

    config.detection.pattern_latency_threshold_ms = 0;
    config
        .validate()
        .expect_err("a zero latency threshold must be rejected");
}

#[test]
fn toml_round_trips_and_is_bounds_checked() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "valid.toml",
            "[detection]\nmax_subquery_depth = 7\npattern_latency_threshold_ms = 25\n",
        )?;
        let config = config_from_toml("valid.toml").expect("valid TOML must extract");
        assert_eq!(config.detection.max_subquery_depth, 7);
        assert_eq!(config.detection.pattern_latency_threshold_ms, 25);
        config.validate().expect("in-range TOML must validate");

        jail.create_file(
            "invalid.toml",
            "[detection]\nmax_subquery_depth = 0\npattern_latency_threshold_ms = 10\n",
        )?;
        let out_of_range = config_from_toml("invalid.toml").expect("out-of-range TOML extracts");
        let err = out_of_range
            .validate()
            .expect_err("out-of-range TOML must fail validation");
        assert!(
            err.to_string().contains("detection.max_subquery_depth"),
            "error must name the field: {err}"
        );
        Ok(())
    });
}

#[test]
fn environment_overrides_reach_both_fields_through_the_documented_prefix() {
    Jail::expect_with(|jail| {
        jail.set_env("DAEMONEYE_AGENT_DETECTION__MAX_SUBQUERY_DEPTH", "5");
        jail.set_env(
            "DAEMONEYE_AGENT_DETECTION__PATTERN_LATENCY_THRESHOLD_MS",
            "42",
        );

        let config = ConfigLoader::new("daemoneye-agent")
            .load()
            .expect("environment overrides must load");
        assert_eq!(config.detection.max_subquery_depth, 5);
        assert_eq!(config.detection.pattern_latency_threshold_ms, 42);
        Ok(())
    });
}

#[test]
fn environment_overrides_are_bounds_checked_too() {
    Jail::expect_with(|jail| {
        jail.set_env("DAEMONEYE_AGENT_DETECTION__MAX_SUBQUERY_DEPTH", "0");

        let err = ConfigLoader::new("daemoneye-agent")
            .load()
            .expect_err("an out-of-range environment override must be rejected");
        assert!(
            err.to_string().contains("detection.max_subquery_depth"),
            "error must name the field: {err}"
        );
        Ok(())
    });
}

#[test]
fn regex_bounds_satisfy_the_stated_memory_arithmetic() {
    const COMPILED_CEILING: usize = REGEX_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES;
    const DFA_CEILING: usize = REGEX_DFA_SIZE_LIMIT_BYTES * REGEX_CACHE_MAX_ENTRIES;
    const SIXTEEN_MIB: usize = 16 * 1024 * 1024;

    assert_eq!(
        REGEX_SIZE_LIMIT_BYTES, 262_144,
        "R5 fixes the per-pattern size limit at 256 KiB"
    );
    assert_eq!(
        REGEX_CACHE_MAX_ENTRIES, 64,
        "R6 fixes the cache at 64 entries"
    );
    assert_eq!(
        COMPILED_CEILING, REGEX_CACHE_MAX_BYTES,
        "size limit times entry count must equal the stated ceiling"
    );
    assert_eq!(
        REGEX_CACHE_MAX_BYTES, SIXTEEN_MIB,
        "the stated ceiling is 16 MiB"
    );
    assert_eq!(
        DFA_CEILING, SIXTEEN_MIB,
        "the DFA cache ceiling must also survive multiplication by the entry count"
    );
}

// --- Executor and memory-shaping fields (U8, KTD14, R28) ----------------------------------------

use daemoneye_lib::detection_bounds::{
    EXECUTOR_BATCH_MAX_BYTES, EXECUTOR_BATCH_SIZE, EXECUTOR_MEMORY_POOL_BYTES,
    EXECUTOR_TARGET_PARTITIONS, POSTING_CACHE_MAX_ENTRIES, POSTING_CACHE_MAX_POSTINGS,
};

#[test]
fn the_new_fields_default_to_the_detection_bounds_constants() {
    let detection = DetectionConfig::default();

    assert_eq!(detection.max_matches_per_rule, 1_000);
    assert_eq!(
        detection.executor_target_partitions,
        EXECUTOR_TARGET_PARTITIONS
    );
    assert_eq!(detection.executor_batch_size, EXECUTOR_BATCH_SIZE);
    assert_eq!(detection.executor_batch_max_bytes, EXECUTOR_BATCH_MAX_BYTES);
    assert_eq!(
        detection.executor_memory_pool_bytes,
        EXECUTOR_MEMORY_POOL_BYTES
    );
    assert_eq!(
        detection.posting_cache_max_entries,
        POSTING_CACHE_MAX_ENTRIES
    );
    assert_eq!(
        detection.posting_cache_max_postings,
        POSTING_CACHE_MAX_POSTINGS
    );
}

/// One `(name, min, max, setter)` row per validated field, so a boundary is checked identically
/// for every field and a failure names which one.
type Setter = fn(&mut DetectionConfig, u64);

fn wide(value: usize) -> u64 {
    u64::try_from(value).unwrap()
}

fn bounded_fields() -> Vec<(&'static str, u64, u64, Setter)> {
    vec![
        (
            "max_matches_per_rule",
            u64::from(DetectionConfig::MAX_MATCHES_PER_RULE_MIN),
            u64::from(DetectionConfig::MAX_MATCHES_PER_RULE_MAX),
            |c, v| c.max_matches_per_rule = u32::try_from(v).unwrap(),
        ),
        (
            "executor_target_partitions",
            wide(DetectionConfig::EXECUTOR_TARGET_PARTITIONS_MIN),
            wide(DetectionConfig::EXECUTOR_TARGET_PARTITIONS_MAX),
            |c, v| c.executor_target_partitions = usize::try_from(v).unwrap(),
        ),
        (
            "executor_batch_size",
            wide(DetectionConfig::EXECUTOR_BATCH_SIZE_MIN),
            wide(DetectionConfig::EXECUTOR_BATCH_SIZE_MAX),
            |c, v| c.executor_batch_size = usize::try_from(v).unwrap(),
        ),
        (
            "executor_batch_max_bytes",
            wide(DetectionConfig::EXECUTOR_BATCH_MAX_BYTES_MIN),
            wide(DetectionConfig::EXECUTOR_BATCH_MAX_BYTES_MAX),
            |c, v| c.executor_batch_max_bytes = usize::try_from(v).unwrap(),
        ),
        (
            "executor_memory_pool_bytes",
            wide(DetectionConfig::EXECUTOR_MEMORY_POOL_BYTES_MIN),
            wide(DetectionConfig::EXECUTOR_MEMORY_POOL_BYTES_MAX),
            |c, v| c.executor_memory_pool_bytes = usize::try_from(v).unwrap(),
        ),
        (
            "posting_cache_max_entries",
            wide(DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MIN),
            wide(DetectionConfig::POSTING_CACHE_MAX_ENTRIES_MAX),
            |c, v| c.posting_cache_max_entries = usize::try_from(v).unwrap(),
        ),
        (
            "posting_cache_max_postings",
            wide(DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MIN),
            wide(DetectionConfig::POSTING_CACHE_MAX_POSTINGS_MAX),
            |c, v| c.posting_cache_max_postings = usize::try_from(v).unwrap(),
        ),
    ]
}

fn validate_with(set: Setter, value: u64) -> Result<(), String> {
    let mut config = Config::default();
    set(&mut config.detection, value);
    config.validate().map_err(|e| e.to_string())
}

#[test]
fn every_new_field_accepts_min_and_max_and_rejects_one_beyond_either() {
    for (name, min, max, set) in bounded_fields() {
        assert!(validate_with(set, min).is_ok(), "{name} must accept MIN");
        assert!(validate_with(set, max).is_ok(), "{name} must accept MAX");

        let below_message = validate_with(set, min - 1).expect_err(name);
        assert!(
            below_message.contains(&format!("detection.{name}")),
            "{name} below MIN must be rejected naming the field: {below_message}"
        );

        let above_message = validate_with(set, max + 1).expect_err(name);
        assert!(
            above_message.contains(&format!("detection.{name}")),
            "{name} above MAX must be rejected naming the field: {above_message}"
        );
    }
}

/// R28: a batch byte bound below one maximally-sized row would exclude every row, so the carve-out
/// for a rare oversized row would fire on all of them.
#[test]
fn a_batch_byte_bound_below_the_assumed_worst_case_row_is_rejected_naming_the_field() {
    let floor = DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES;

    let mut at = Config::default();
    at.detection.executor_batch_max_bytes = floor;
    at.validate()
        .expect("a bound exactly at the floor is accepted");

    let mut below = Config::default();
    below.detection.executor_batch_max_bytes = floor - 1;
    let message = below
        .validate()
        .expect_err("one byte below the floor")
        .to_string();
    assert!(
        message.contains("detection.executor_batch_max_bytes"),
        "{message}"
    );
    assert!(message.contains("worst-case row"), "{message}");
}

/// The floor is an assumption, not a derived bound; the default must clear it with room to spare
/// or the default configuration would sit on the edge of the very carve-out it guards.
#[test]
fn the_default_batch_byte_bound_clears_the_assumed_floor() {
    assert!(
        DetectionConfig::default().executor_batch_max_bytes
            >= DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES
    );
}

/// A config file written before these fields existed must still load, filling the new fields from
/// the defaults rather than failing on a missing key.
#[test]
fn a_toml_section_naming_only_the_old_fields_still_loads() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "old.toml",
            "[detection]\nmax_subquery_depth = 2\npattern_latency_threshold_ms = 20\n",
        )?;
        let config = config_from_toml("old.toml")?;
        assert_eq!(config.detection.max_subquery_depth, 2);
        assert_eq!(config.detection.max_matches_per_rule, 1_000);
        config.validate().expect("defaults fill the rest");
        Ok(())
    });
}

// --- Configured values reach the three constructors the earlier units built (U8 rewiring) ---------

use std::sync::Arc;

use daemoneye_lib::detection::RegexCache;
use daemoneye_lib::detection::execution::session::{
    ExecutorRuntime, LatencySink, session_state_from_config,
};
use daemoneye_lib::storage::postings_cache::PostingsCache;
use daemoneye_lib::storage::provider::ScanLimits;

fn non_default_config() -> DetectionConfig {
    DetectionConfig {
        executor_target_partitions: 2,
        executor_batch_size: 256,
        executor_batch_max_bytes: DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES + 1,
        executor_memory_pool_bytes: 4096,
        posting_cache_max_entries: 1,
        posting_cache_max_postings: 2,
        ..DetectionConfig::default()
    }
}

#[test]
fn scan_limits_are_built_from_the_configured_values() {
    let limits = ScanLimits::from(&non_default_config());

    assert_eq!(limits.target_partitions, 2);
    assert_eq!(limits.batch_size, 256);
    assert_eq!(
        limits.batch_max_bytes,
        DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES + 1
    );
    assert_eq!(
        ScanLimits::from(&DetectionConfig::default()),
        ScanLimits::default(),
        "the default configuration is the default scan limits"
    );
}

#[test]
fn the_session_runtime_pool_and_state_take_the_configured_values() {
    let config = non_default_config();
    let runtime = ExecutorRuntime::from_config(&config).unwrap().env();
    assert_eq!(
        datafusion_pool_size(&runtime),
        4096,
        "the pool is the configured size"
    );

    let state = session_state_from_config(
        runtime,
        Arc::new(LatencySink::default()),
        Arc::new(RegexCache::new()),
        &config,
    )
    .unwrap();
    assert_eq!(state.config().target_partitions(), 2);
    assert_eq!(state.config().batch_size(), 256);
}

fn datafusion_pool_size(runtime: &datafusion::execution::runtime_env::RuntimeEnv) -> usize {
    let datafusion::execution::memory_pool::MemoryLimit::Finite(bytes) =
        runtime.memory_pool.memory_limit()
    else {
        panic!("expected a finite pool");
    };
    bytes
}

#[test]
fn the_postings_cache_takes_the_configured_bounds() {
    let cache = PostingsCache::from_config(&non_default_config());
    let key = |term| (daemoneye_lib::storage::read::IndexKind::Pid, 1, term, 0);

    let long: Result<_, ()> = cache.get_or_load(key(1), 10, || Ok(vec![(1, 1), (2, 2), (3, 3)]));
    assert_eq!(long.unwrap().len(), 3);
    assert_eq!(
        cache.bypassed_long(),
        1,
        "a list over 2 postings is not retained"
    );

    let short: Result<_, ()> = cache.get_or_load(key(2), 10, || Ok(vec![(1, 1), (2, 2)]));
    assert_eq!(short.unwrap().len(), 2);
    let _other: Result<_, ()> = cache.get_or_load(key(3), 10, || Ok(vec![(1, 1)]));
    assert_eq!(cache.len(), 1, "a cache of one entry holds one list");
}
