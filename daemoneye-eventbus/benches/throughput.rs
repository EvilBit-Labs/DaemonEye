//! Performance benchmarks for daemoneye-eventbus

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent,
    SourceCaps,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn create_test_process_event(pid: u32) -> CollectionEvent {
    let process_event = ProcessEvent {
        pid,
        name: format!("test_process_{}", pid),
        command_line: Some(format!("test_process_{} --arg", pid)),
        executable_path: Some(format!("/usr/bin/test_process_{}", pid)),
        ppid: Some(1000),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    };
    CollectionEvent::Process(process_event)
}

fn create_test_subscription() -> EventSubscription {
    EventSubscription {
        subscriber_id: "benchmark-subscriber".to_string(),
        capabilities: SourceCaps {
            event_types: vec!["process".to_string()],
            collectors: vec!["procmond".to_string()],
            max_priority: 5,
        },
        event_filter: None,
        correlation_filter: None,
        topic_patterns: Some(vec!["events.process.*".to_string()]),
        enable_wildcards: true,
    }
}

fn bench_message_serialization(c: &mut Criterion) {
    c.bench_function("message_serialization", |b| {
        b.iter(|| {
            let event = create_test_process_event(black_box(1234));
            let serialized = bincode::serialize(&event).unwrap();
            black_box(serialized)
        })
    });
}

fn bench_message_deserialization(c: &mut Criterion) {
    // Pre-serialize some data
    let event = create_test_process_event(1234);
    let serialized = bincode::serialize(&event).unwrap();

    c.bench_function("message_deserialization", |b| {
        b.iter(|| {
            let deserialized: CollectionEvent =
                bincode::deserialize(black_box(&serialized)).unwrap();
            black_box(deserialized)
        })
    });
}

fn bench_topic_matching(c: &mut Criterion) {
    use daemoneye_eventbus::TopicPattern;

    let patterns = vec![
        TopicPattern::new("events.process.*".to_string()),
        TopicPattern::new("events.*".to_string()),
        TopicPattern::new("events.process.new".to_string()),
    ];

    let topics = vec![
        "events.process.new",
        "events.process.old",
        "events.network.connections",
        "events.filesystem.create",
        "control.collector.start",
    ];

    c.bench_function("topic_matching", |b| {
        b.iter(|| {
            for pattern in &patterns {
                for topic in &topics {
                    let _ = pattern.matches(black_box(topic));
                }
            }
        })
    });
}

fn bench_event_publishing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("event_publishing", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let socket_path = temp_dir.path().join("bench-publish.sock");

                let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
                    .await
                    .unwrap();
                let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

                let event = create_test_process_event(black_box(1234));
                let result = event_bus
                    .publish(event, "bench-correlation".to_string())
                    .await;
                black_box(result)
            })
        })
    });
}

fn bench_subscription_creation(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("subscription_creation", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let socket_path = temp_dir.path().join("bench-subscription.sock");

                let broker = DaemoneyeBroker::new(socket_path.to_str().unwrap())
                    .await
                    .unwrap();
                let mut event_bus = DaemoneyeEventBus::from_broker(broker).await.unwrap();

                let subscription = create_test_subscription();
                let result = event_bus.subscribe(subscription).await;
                black_box(result)
            })
        })
    });
}

fn bench_statistics_collection(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("statistics_collection", |b| {
        // Setup outside the iteration
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("bench-stats.sock");

        let broker = rt.block_on(async {
            DaemoneyeBroker::new(socket_path.to_str().unwrap())
                .await
                .unwrap()
        });
        let event_bus =
            rt.block_on(async { DaemoneyeEventBus::from_broker(broker).await.unwrap() });

        b.iter(|| {
            rt.block_on(async {
                let stats = event_bus.statistics().await;
                black_box(stats)
            })
        })
    });
}

criterion_group!(
    benches,
    bench_message_serialization,
    bench_message_deserialization,
    bench_topic_matching,
    bench_event_publishing,
    bench_subscription_creation,
    bench_statistics_collection
);
criterion_main!(benches);
