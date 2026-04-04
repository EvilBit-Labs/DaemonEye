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
//! Simple test to verify DaemoneyeEventBus basic functionality.

use collector_core::{
    DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent},
    event_bus::{EventBus, EventBusConfig, EventSubscription},
    source::SourceCaps,
};
use std::time::SystemTime;

#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_creation_and_startup() {
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("simple-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    // Start the event bus
    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Get initial statistics
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");

    assert_eq!(stats.events_published, 0);
    assert_eq!(stats.active_subscribers, 0);

    // Shutdown
    event_bus
        .shutdown()
        .await
        .expect("Failed to shutdown EventBus");
}

#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_subscription_only() {
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir for socket");
    let socket_path = temp_dir.path().join("simple-sub-test.sock");
    let socket_path_str = socket_path
        .to_str()
        .expect("socket path contains invalid UTF-8")
        .to_owned();
    let mut event_bus = DaemoneyeEventBus::new(config, &socket_path_str)
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "simple-test-subscriber".to_string(),
        capabilities: SourceCaps::PROCESS,
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
        topic_filter: None,
    };

    let _receiver = event_bus
        .subscribe(subscription)
        .await
        .expect("Failed to subscribe");

    // Check statistics
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");

    // Should have one subscriber now
    assert!(stats.active_subscribers > 0);

    event_bus.shutdown().await.expect("Failed to shutdown");
}

#[cfg(unix)]
#[tokio::test]
async fn test_daemoneye_eventbus_publish_only() {
    let config = EventBusConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("simple-pub-test.sock");
    let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
        .await
        .expect("Failed to create DaemoneyeEventBus");

    event_bus
        .start()
        .await
        .expect("Failed to start DaemoneyeEventBus");

    // Create a simple process event
    let process_event = CollectionEvent::Process(ProcessEvent {
        pid: 1111,
        ppid: None,
        name: "simple_test".to_string(),
        executable_path: None,
        command_line: vec![],
        start_time: None,
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: None,
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    // Publish the event (should succeed even without subscribers)
    event_bus
        .publish(
            process_event,
            collector_core::event_bus::CorrelationMetadata::new(
                "simple-test-correlation".to_string(),
            ),
        )
        .await
        .expect("Failed to publish event");

    // Check statistics
    let stats = event_bus
        .get_statistics()
        .await
        .expect("Failed to get statistics");

    // Should have published one event
    assert!(stats.events_published > 0);

    event_bus.shutdown().await.expect("Failed to shutdown");
}
