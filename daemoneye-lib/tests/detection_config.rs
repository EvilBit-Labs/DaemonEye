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
