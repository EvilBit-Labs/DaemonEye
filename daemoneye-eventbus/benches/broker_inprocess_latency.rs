#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::str_to_string,
    clippy::unseparated_literal_suffix
)]
//! End-to-end in-process broker latency benchmark (R14 AC4 gate).
//!
//! Unlike `throughput.rs` (publish-only), `ipc_performance.rs` (socket), and
//! procmond's `eventbus_benchmarks.rs` (WAL connector), this measures the
//! `daemoneye-eventbus` **in-process broker delivery path**: an event published
//! through `DaemoneyeEventBus` until an in-process subscriber receives it. This
//! is the path the agent runs (the broker constructs with `transport_server:
//! None`, fanning out to in-process tokio mpsc subscribers).
//!
//! It exists so the R14 AC4 no-regression gate has a concrete pre-migration
//! baseline to compare against before the legacy crossbeam
//! `HighPerformanceEventBus` is removed (R14 AC7). Record a baseline with:
//!
//! ```bash
//! cargo bench --package daemoneye-eventbus --bench broker_inprocess_latency \
//!     -- --save-baseline pre-migration
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent,
    SourceCaps,
};
use std::collections::HashMap;
use std::hint::black_box;
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
        subscriber_id: "broker-latency-subscriber".to_string(),
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
    }
}

/// Publish-to-receive latency through the in-process broker path.
fn bench_broker_inprocess_publish_to_receive(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    // The broker constructs with no transport server bound, so delivery stays
    // entirely in-process (tokio mpsc fan-out) — no socket I/O on this path.
    let socket_path = temp_dir.path().join("bench-broker-e2e.sock");

    let broker = rt.block_on(async {
        DaemoneyeBroker::new(socket_path.to_str().unwrap())
            .await
            .unwrap()
    });
    let mut event_bus =
        rt.block_on(async { DaemoneyeEventBus::from_broker(broker).await.unwrap() });
    let mut receiver = rt.block_on(async {
        event_bus
            .subscribe(create_test_subscription())
            .await
            .unwrap()
    });

    c.bench_function("broker_inprocess_publish_to_receive", |b| {
        b.iter(|| {
            rt.block_on(async {
                event_bus
                    .publish(
                        create_test_process_event(black_box(1234)),
                        "bench-e2e".to_string(),
                    )
                    .await
                    .unwrap();
                black_box(receiver.recv().await)
            })
        });
    });
}

criterion_group!(benches, bench_broker_inprocess_publish_to_receive);
criterion_main!(benches);
