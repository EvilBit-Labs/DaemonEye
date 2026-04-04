#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::use_debug,
    clippy::dbg_macro,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_lossless,
    clippy::pattern_type_mismatch,
    clippy::non_ascii_literal,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::let_underscore_must_use,
    clippy::must_use_candidate,
    clippy::missing_const_for_fn,
    clippy::used_underscore_binding,
    clippy::redundant_clone,
    clippy::explicit_iter_loop,
    clippy::integer_division,
    clippy::modulo_arithmetic,
    clippy::unseparated_literal_suffix,
    clippy::doc_markdown,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::float_cmp,
    clippy::unreadable_literal,
    clippy::semicolon_outside_block,
    clippy::semicolon_inside_block,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::redundant_type_annotations,
    clippy::significant_drop_tightening,
    clippy::redundant_else,
    clippy::match_same_arms,
    clippy::ignore_without_reason,
    dead_code,
    unused_imports
)]
//! Integration tests for Collector with DaemoneyeEventBus.

use collector_core::event_bus::EventBus;
use collector_core::{Collector, CollectorConfig, DaemoneyeEventBus, event_bus::EventBusConfig};
use std::time::Duration;

#[cfg(unix)]
#[tokio::test]
async fn test_collector_with_daemoneye_eventbus() {
    let config = CollectorConfig::default()
        .with_max_event_sources(1)
        .with_event_buffer_size(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("collector-integration.sock");
    let collector = Collector::configure_daemoneye_eventbus(config, socket_path.to_str().unwrap())
        .expect("Failed to create collector with DaemoneyeEventBus");

    // This test just verifies that the collector can be created with DaemoneyeEventBus
    // without errors. The actual event processing would require more complex setup.
    assert_eq!(collector.source_count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_broker_access() {
    let config = EventBusConfig::default();
    let temp_dir2 = tempfile::tempdir().unwrap();
    let socket_path2 = temp_dir2.path().join("broker-access-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path2.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    // Test broker access
    let broker = event_bus.broker();
    let broker_stats = broker.statistics().await;

    assert_eq!(broker_stats.messages_published, 0);
    assert_eq!(broker_stats.active_subscribers, 0);

    event_bus.start().await.expect("Failed to start EventBus");
    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[cfg(unix)]
#[tokio::test]
async fn test_eventbus_statistics_conversion() {
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("stats-conversion-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus.start().await.expect("Failed to start EventBus");

    // Get statistics through the EventBus trait
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");

    // Verify statistics structure
    assert_eq!(stats.events_published, 0);
    assert_eq!(stats.events_delivered, 0);
    assert_eq!(stats.active_subscribers, 0);
    assert!(stats.uptime >= Duration::from_secs(0));

    event_bus.shutdown().await.expect("Failed to shutdown");
}
