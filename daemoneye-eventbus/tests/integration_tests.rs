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
    dead_code
)]
//! Integration tests for daemoneye-eventbus

use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent,
    SourceCaps,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tempfile::TempDir;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn test_broker_creation_and_startup() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-broker.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    assert!(broker.start().await.is_ok());
    // No sleep needed: broker.start() is fully synchronous and completes startup before returning.

    let stats = broker.statistics().await;
    assert_eq!(stats.messages_published, 0);
    assert_eq!(stats.active_subscribers, 0);
}

#[tokio::test]
async fn test_event_bus_creation() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-eventbus.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 0);
    assert_eq!(stats.active_subscribers, 0);
}

#[tokio::test]
async fn test_process_event_publishing() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-publish.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Create a process event
    let process_event = ProcessEvent {
        pid: 1234,
        name: "test_process".to_string(),
        command_line: Some("test_process --arg".to_string()),
        executable_path: Some("/usr/bin/test_process".to_string()),
        ppid: Some(1000),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    };

    let collection_event = CollectionEvent::Process(process_event);

    // Publish the event
    let result = event_bus
        .publish(collection_event, "test-correlation-123".to_string())
        .await;
    assert!(result.is_ok());

    // Check statistics
    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 1);
}

#[tokio::test]
async fn test_event_subscription() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-subscription.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Create subscription
    let subscription = EventSubscription {
        subscriber_id: "test-subscriber".to_string(),
        capabilities: SourceCaps {
            event_types: vec!["process".to_string()],
            collectors: vec!["procmond".to_string()],
            max_priority: 5,
        },
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
        include_control: false,
    };

    // Subscribe
    let receiver = event_bus.subscribe(subscription).await.unwrap();
    // Verify receiver is open and ready to receive messages
    assert!(
        !receiver.is_closed(),
        "Receiver should not be closed immediately after subscription"
    );

    // Check statistics
    let stats = event_bus.statistics().await;
    assert_eq!(stats.active_subscribers, 1);
}

#[tokio::test]
async fn test_topic_wildcard_matching() {
    use daemoneye_eventbus::{Topic, TopicPattern};

    // Test exact match
    let pattern = TopicPattern::new("events.process.lifecycle").unwrap();
    let topic1 = Topic::new("events.process.lifecycle").unwrap();
    let topic2 = Topic::new("events.process.metadata").unwrap();
    assert!(pattern.matches(&topic1));
    assert!(!pattern.matches(&topic2));

    // Test wildcard match
    let wildcard_pattern = TopicPattern::new("events.process.+").unwrap();
    assert!(wildcard_pattern.matches(&topic1));
    assert!(wildcard_pattern.matches(&topic2));

    let network_topic = Topic::new("events.network.connections").unwrap();
    assert!(!wildcard_pattern.matches(&network_topic));

    // Test multi-level wildcard
    let multi_pattern = TopicPattern::new("events.#").unwrap();
    assert!(multi_pattern.matches(&topic1));
    assert!(multi_pattern.matches(&network_topic));

    let control_topic = Topic::new("control.collector.status").unwrap();
    assert!(!multi_pattern.matches(&control_topic));
}

#[tokio::test]
async fn test_event_bus_shutdown() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-shutdown.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Shutdown should succeed
    let result = event_bus.shutdown().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_statistics_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-stats.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
        .await
        .unwrap();
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

    // Initial stats
    let initial_stats = event_bus.statistics().await;
    assert_eq!(initial_stats.messages_published, 0);
    assert_eq!(initial_stats.messages_delivered, 0);
    assert_eq!(initial_stats.active_subscribers, 0);
    assert_eq!(initial_stats.active_topics, 0);

    // Publish an event
    let process_event = ProcessEvent {
        pid: 5678,
        name: "stats_test".to_string(),
        command_line: None,
        executable_path: None,
        ppid: None,
        start_time: None,
        metadata: HashMap::new(),
    };

    let collection_event = CollectionEvent::Process(process_event);
    event_bus
        .publish(collection_event, "stats-correlation".to_string())
        .await
        .unwrap();

    // Intentional: allow wall-clock time to advance so uptime_seconds can be validated below.
    sleep(Duration::from_millis(100)).await;

    // Check updated stats
    let updated_stats = event_bus.statistics().await;
    assert_eq!(updated_stats.messages_published, 1);
    // Just verify uptime is a valid value (can be 0 if very fast)
    assert!(
        updated_stats.uptime_seconds < 10,
        "Uptime should be reasonable for a short test"
    );
}

/// Unit 1 / END-297: Verify `EventSubscription::include_control` threads
/// through to the serialized form (postcard round-trip preserves the field).
///
/// This guards against a later `#[serde(default)]` regression that would
/// silently drop the flag on the wire and break control delivery.
#[tokio::test]
async fn test_event_subscription_serialization_preserves_include_control() {
    // Build subscription with include_control=true.
    let sub = EventSubscription {
        subscriber_id: "round-trip".to_string(),
        capabilities: SourceCaps {
            event_types: vec!["control".to_string()],
            collectors: vec![],
            max_priority: 0,
        },
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["control.collector.lifecycle".to_string()]),
        enable_wildcards: true,
        include_control: true,
    };

    let encoded = postcard::to_allocvec(&sub).expect("encode subscription");
    let decoded: EventSubscription = postcard::from_bytes(&encoded).expect("decode subscription");
    assert!(
        decoded.include_control,
        "include_control=true must survive postcard round-trip"
    );

    // And the legacy default path.
    let legacy = EventSubscription {
        include_control: false,
        ..sub.clone()
    };
    let encoded2 = postcard::to_allocvec(&legacy).expect("encode");
    let decoded2: EventSubscription = postcard::from_bytes(&encoded2).expect("decode");
    assert!(!decoded2.include_control);
}

/// Unit 1 / END-297: `EventSubscription::default()` produces a legacy-safe
/// subscription that does NOT opt into Control delivery.
#[test]
fn test_event_subscription_default_does_not_opt_into_control() {
    let sub = EventSubscription::default();
    assert!(
        !sub.include_control,
        "Default EventSubscription must remain legacy (Event-only) to preserve compatibility"
    );
}
