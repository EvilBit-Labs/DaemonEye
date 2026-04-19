// This file is an integration test — it compiles as its own binary crate
// under `tests/` (no `#[cfg(test)]` wrapping). A few workspace-level lints
// are standard to relax at the test-file level so we can write expressive,
// easy-to-read assertions without noise. The set below is minimised to
// lints that trip genuinely benign patterns in this file (matches common
// practice across the crate's test files; see PR #178 review).
#![allow(
    clippy::unwrap_used,             // Tests panic on unexpected Err; that's the diagnostic.
    clippy::expect_used,             // Same rationale as unwrap_used.
    clippy::panic,                   // Test-only assertions that panic on bad state.
    clippy::arithmetic_side_effects, // Test iteration counters and expected-value math.
    clippy::as_conversions,          // Small numeric casts in test data.
    clippy::cast_possible_truncation,// Same as as_conversions.
    clippy::cast_sign_loss,          // Same.
    clippy::indexing_slicing,        // Fixed-size test fixtures indexed directly.
    clippy::items_after_statements,  // Test-local const declarations near their use.
    clippy::ignore_without_reason,   // #[ignore] markers used in this file carry rationale in docstrings.
    clippy::shadow_unrelated,        // Test scenarios re-use short binding names (e.g. `stats`) across phases for readability.
    clippy::doc_markdown,             // Integration-test docstrings reference protobuf-style identifiers and plain text labels.
    dead_code                         // Test helpers used in a subset of tests.
)]
//! End-to-end multi-collector coordination tests (END-297 Unit 4).
//!
//! # Scope
//!
//! These tests prove the acceptance criteria R5 (load balancing), R6 (result
//! aggregation), R8 (failover), and R13 (multi-collector integration) from
//! END-297 by exercising the **real broker, task distributor, and result
//! aggregator** through the same code paths production uses.
//!
//! All clients in this file are **in-process** subscribers that represent
//! logical collectors (`procmond-1`, `procmond-2`, ...). The broker itself is
//! constructed with [`DaemoneyeBroker::new`] against a temp-dir socket path
//! because the constructor requires one, but no separate OS processes are
//! spawned and no traffic crosses the `interprocess` transport layer.
//!
//! # Why in-process, not cross-process?
//!
//! Unit 1 of this closure pass (see
//! `docs/plans/2026-04-18-001-feat-close-end-297-message-broker-plan.md`)
//! wired `MessageType::Control` delivery through the *in-process* broker
//! subscription path ([`DaemoneyeBroker::subscribe_raw`]). A latent gap
//! remains at the transport layer: the broker does not currently run a
//! per-connection receive loop that registers subscriptions from remote
//! clients over `interprocess`. `ClientConnectionManager::subscribe_client`
//! exists and is exercised by an in-process unit test in `transport.rs`, but
//! cross-process subscription registration is not wired end-to-end.
//!
//! That gap is explicitly out of scope for END-297 closure. The acceptance
//! criteria are about broker coordination semantics — queue groups, routing
//! strategies, capability filters, failover, correlation — and those live in
//! [`TaskDistributor`], [`ResultAggregator`], and [`DaemoneyeBroker`]. This
//! file exercises every one of them through the production APIs.
//!
//! # Queue group convention
//!
//! "Queue group" in the END-297 ticket maps onto a set of collectors
//! registered under the same `collector_type` with the same
//! `supported_operations`. [`TaskDistributor::distribute_task`] picks exactly
//! one member per task via the configured [`RoutingStrategy`]. Broadcast
//! topics (config reload, wildcard observation) are delivered to *all*
//! matching subscribers by [`DaemoneyeBroker::publish`].

use daemoneye_eventbus::{
    AggregationConfig, CollectionEvent, CollectorCapability, CollectorResult, DaemoneyeBroker,
    ProcessEvent, ResultAggregator, RoutingStrategy, TaskDistributor, TaskRequest, collector,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Queue-group name used throughout these tests, mirroring the ticket's
/// `local-procmond-workers` example via the canonical `procmond`
/// collector_type.
const QUEUE_GROUP_TYPE: &str = "procmond";
const QUEUE_GROUP_OPERATION: &str = "enumerate_processes";
const MEMBER_1: &str = "procmond-1";
const MEMBER_2: &str = "procmond-2";

/// Wall-clock budget for any single `recv` attempt. Keeps watchdog timeouts
/// small so a regression hangs fast.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Fixture helpers (kept inline — no `tests/common/mod.rs` convention in this
// crate today).
// ---------------------------------------------------------------------------

/// Bring up a fresh broker backed by a temp-dir socket path. The broker is
/// not `start()`-ed because the in-process subscription paths do not need the
/// transport server. Returns the broker and the `TempDir` guard so the
/// caller can keep the temp dir alive for the full test.
async fn spawn_broker(test_name: &str) -> (Arc<DaemoneyeBroker>, TempDir) {
    let temp_dir = TempDir::new().expect("create temp dir");
    let socket_path = temp_dir.path().join(format!("{test_name}.sock"));
    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .expect("construct broker"),
    );
    (broker, temp_dir)
}

/// Build a `CollectorCapability` for a queue-group member. All members share
/// `collector_type` and `supported_operations` so the distributor routes
/// between them.
fn queue_group_capability(collector_id: &str, max_concurrent: u32) -> CollectorCapability {
    CollectorCapability {
        collector_id: collector_id.to_owned(),
        collector_type: QUEUE_GROUP_TYPE.to_owned(),
        supported_operations: vec![QUEUE_GROUP_OPERATION.to_owned()],
        max_concurrent_tasks: max_concurrent,
        priority_levels: vec![1, 2, 3, 4, 5],
        metadata: HashMap::new(),
    }
}

/// Build a minimal `TaskRequest` targeting the queue group's operation.
fn make_task(task_id: &str, priority: u8) -> TaskRequest {
    let now = SystemTime::now();
    TaskRequest {
        task_id: task_id.to_owned(),
        operation: QUEUE_GROUP_OPERATION.to_owned(),
        priority,
        payload: Vec::new(),
        timeout_ms: 30_000,
        metadata: HashMap::new(),
        correlation_id: Some(format!("corr-{task_id}")),
        created_at: now,
        deadline: now + Duration::from_secs(30),
    }
}

/// Register a logical collector on the distributor *and* subscribe its raw
/// task-topic receiver on the broker. Returns the receiver so the test can
/// assert which tasks each member received.
async fn join_queue_group(
    broker: &Arc<DaemoneyeBroker>,
    distributor: &TaskDistributor,
    collector_id: &str,
    max_concurrent: u32,
) -> mpsc::UnboundedReceiver<daemoneye_eventbus::Message> {
    let task_topic = collector::task_topic(QUEUE_GROUP_TYPE, collector_id);
    let subscriber_id = Uuid::new_v4();
    let rx = broker
        .subscribe_raw(&task_topic, subscriber_id)
        .await
        .expect("subscribe to per-collector task topic");
    distributor
        .register_collector(queue_group_capability(collector_id, max_concurrent))
        .await
        .expect("register collector with distributor");
    rx
}

/// Drain pending messages from a receiver without blocking. Returns the
/// drained messages and leaves the receiver open.
fn drain_now(
    rx: &mut mpsc::UnboundedReceiver<daemoneye_eventbus::Message>,
) -> Vec<daemoneye_eventbus::Message> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

/// Wait for `target` messages across `N` receivers, budgeted by `RECV_TIMEOUT`.
/// Returns the per-receiver counts. Polls each receiver with `try_recv()` and
/// drains all immediately-available messages; if no receiver has anything
/// ready, sleeps briefly before retrying so the test stays responsive
/// without busy-looping. Delivery order across receivers is not guaranteed,
/// so this helper is order-independent by design.
async fn collect_until<const N: usize>(
    receivers: &mut [&mut mpsc::UnboundedReceiver<daemoneye_eventbus::Message>; N],
    target: usize,
) -> [usize; N] {
    let mut counts = [0_usize; N];

    let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let total: usize = counts.iter().sum();
        if total >= target {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        // Try each receiver non-blockingly first.
        let mut made_progress = false;
        for (idx, rx) in receivers.iter_mut().enumerate() {
            while let Ok(_msg) = rx.try_recv() {
                counts[idx] = counts[idx].saturating_add(1);
                made_progress = true;
            }
        }

        if !made_progress {
            // Nothing was immediately available — yield so the broker can
            // finish delivering in-flight messages. A short sleep keeps the
            // test responsive without busy-looping.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    counts
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Load balancing across a queue group: 100 tasks distributed round-robin
/// between two members should reach both; neither member should starve and
/// neither should monopolize the group.
#[tokio::test]
async fn load_balancing_across_queue_group_members() {
    let (broker, _temp) = spawn_broker("load-balancing").await;
    let distributor = TaskDistributor::new(Arc::clone(&broker))
        .await
        .expect("construct distributor");
    distributor
        .set_routing_strategy(RoutingStrategy::RoundRobin)
        .await;

    // Two queue-group members with enough capacity to absorb the full 100-task
    // burst so nothing gets queued — load-balancing assertion is on the
    // distribution, not the queue.
    let mut rx1 = join_queue_group(&broker, &distributor, MEMBER_1, 200).await;
    let mut rx2 = join_queue_group(&broker, &distributor, MEMBER_2, 200).await;

    const TASK_COUNT: usize = 100;
    for i in 0..TASK_COUNT {
        let task = make_task(&format!("lb-task-{i}"), 3);
        let selected = distributor
            .distribute_task(task)
            .await
            .expect("distribute task");
        assert!(
            selected == MEMBER_1 || selected == MEMBER_2,
            "unexpected collector selection: {selected}"
        );
    }

    let counts = collect_until(&mut [&mut rx1, &mut rx2], TASK_COUNT).await;
    let received_1 = counts[0];
    let received_2 = counts[1];
    let total_received = received_1.saturating_add(received_2);

    assert_eq!(
        total_received, TASK_COUNT,
        "every distributed task should be delivered to exactly one member",
    );
    assert!(
        received_1 > 0 && received_2 > 0,
        "both members must receive at least one task (got {received_1}/{received_2})",
    );
    // Plan acceptance: neither member gets > 95 of 100.
    assert!(
        received_1 < 95 && received_2 < 95,
        "neither member should monopolize the queue group \
         (got {received_1}/{received_2})",
    );
}

/// Broadcast: when the agent publishes a single config reload on a shared
/// broadcast topic, every member subscribed to that topic receives it. The
/// broker's fan-out delivers one copy per subscriber — this is the opposite
/// semantic to load balancing.
#[tokio::test]
async fn broadcast_config_reload_reaches_all_members() {
    let (broker, _temp) = spawn_broker("broadcast-config").await;

    // Shared broadcast topic — "config" for the whole procmond family.
    let config_topic = format!("{}.{}", collector::CONFIG, QUEUE_GROUP_TYPE);

    let sub1_id = Uuid::new_v4();
    let sub2_id = Uuid::new_v4();
    let mut rx1 = broker
        .subscribe_raw(&config_topic, sub1_id)
        .await
        .expect("member 1 subscribes to config topic");
    let mut rx2 = broker
        .subscribe_raw(&config_topic, sub2_id)
        .await
        .expect("member 2 subscribes to config topic");

    let correlation_id = "config-reload-1";
    let payload = b"config-version=2".to_vec();
    broker
        .publish(&config_topic, correlation_id, payload.clone())
        .await
        .expect("publish config reload");

    let msg1 = timeout(RECV_TIMEOUT, rx1.recv())
        .await
        .expect("member 1 receives config reload before timeout")
        .expect("member 1 channel open");
    let msg2 = timeout(RECV_TIMEOUT, rx2.recv())
        .await
        .expect("member 2 receives config reload before timeout")
        .expect("member 2 channel open");

    assert_eq!(msg1.topic, config_topic);
    assert_eq!(msg2.topic, config_topic);
    assert_eq!(msg1.payload, payload);
    assert_eq!(msg2.payload, payload);
    assert_eq!(msg1.correlation_metadata.correlation_id, correlation_id);
    assert_eq!(msg2.correlation_metadata.correlation_id, correlation_id);
}

/// Failover: after one queue-group member is deregistered mid-run, every
/// subsequent task routes to the surviving member; no task is silently
/// dropped.
#[tokio::test]
async fn failover_redirects_tasks_after_member_deregisters() {
    let (broker, _temp) = spawn_broker("failover").await;
    let distributor = TaskDistributor::new(Arc::clone(&broker))
        .await
        .expect("construct distributor");
    distributor
        .set_routing_strategy(RoutingStrategy::RoundRobin)
        .await;

    let mut rx1 = join_queue_group(&broker, &distributor, MEMBER_1, 200).await;
    let mut rx2 = join_queue_group(&broker, &distributor, MEMBER_2, 200).await;

    // Phase 1: distribute an initial batch while both members are healthy.
    const PHASE_1_COUNT: usize = 10;
    for i in 0..PHASE_1_COUNT {
        let task = make_task(&format!("phase1-{i}"), 2);
        distributor.distribute_task(task).await.expect("phase 1");
    }

    // Give the broker a moment to fan everything out.
    let phase1_counts = collect_until(&mut [&mut rx1, &mut rx2], PHASE_1_COUNT).await;
    let phase1_total = phase1_counts[0].saturating_add(phase1_counts[1]);
    assert_eq!(
        phase1_total, PHASE_1_COUNT,
        "phase 1 tasks should all be delivered while both members are healthy",
    );

    // Fail member 1 — simulates a procmond crash. There is no kill primitive
    // for an in-process subscriber, so the canonical way for a queue group to
    // drop a member is `deregister_collector`, which is exactly the
    // code path the production restart-supervisor would take.
    distributor
        .deregister_collector(MEMBER_1)
        .await
        .expect("deregister member 1");

    // Phase 2: every new task must route to member 2. Member 1's raw
    // receiver is still open (broker doesn't force-close subscriptions on
    // collector deregistration — that is a distributor-level state change),
    // but the distributor will no longer route to it.
    const PHASE_2_COUNT: usize = 15;
    for i in 0..PHASE_2_COUNT {
        let task = make_task(&format!("phase2-{i}"), 2);
        let selected = distributor
            .distribute_task(task)
            .await
            .expect("phase 2 distribute");
        assert_eq!(
            selected, MEMBER_2,
            "after failover, all tasks must route to the surviving member",
        );
    }

    // Drain phase-2 traffic and confirm member 1 received nothing new while
    // member 2 received the full phase-2 burst.
    let rx1_phase2 = drain_now(&mut rx1);
    assert!(
        rx1_phase2.is_empty(),
        "failed-over member 1 should receive zero phase-2 tasks (got {})",
        rx1_phase2.len(),
    );
    let phase2_counts = collect_until(&mut [&mut rx2], PHASE_2_COUNT).await;
    assert_eq!(
        phase2_counts[0], PHASE_2_COUNT,
        "member 2 should receive every phase-2 task ({PHASE_2_COUNT} expected)",
    );

    // No tasks lost overall.
    let total_delivered = phase1_total.saturating_add(phase2_counts[0]);
    assert_eq!(
        total_delivered,
        PHASE_1_COUNT.saturating_add(PHASE_2_COUNT),
        "no tasks should be lost across the failover boundary",
    );
}

/// Result aggregation: both queue-group members publish results tagged with
/// the same correlation ID. The aggregator collects them, preserves the
/// correlation ID, and the deduplication cache is not bypassed when
/// sequence numbers differ.
#[tokio::test]
async fn result_aggregation_preserves_correlation_ids() {
    let (broker, _temp) = spawn_broker("result-aggregation").await;
    let aggregator = ResultAggregator::new(Arc::clone(&broker), AggregationConfig::default())
        .await
        .expect("construct aggregator");

    let correlation_id = "multi-collector-corr";
    let make_result = |collector_id: &str, pid: u32, sequence: u64| -> CollectorResult {
        let mut metadata = HashMap::new();
        metadata.insert("correlation_id".to_owned(), correlation_id.to_owned());
        CollectorResult {
            collector_id: collector_id.to_owned(),
            collector_type: QUEUE_GROUP_TYPE.to_owned(),
            event: CollectionEvent::Process(ProcessEvent {
                pid,
                name: format!("proc-{collector_id}"),
                command_line: Some(format!("run --from {collector_id}")),
                executable_path: Some(format!("/bin/{collector_id}")),
                ppid: Some(1),
                start_time: Some(SystemTime::now()),
                metadata,
            }),
            timestamp: SystemTime::now(),
            sequence,
        }
    };

    // Each member contributes two results under the same correlation ID.
    aggregator
        .collect_result(make_result(MEMBER_1, 1001, 1))
        .await
        .expect("collect member 1 result 1");
    aggregator
        .collect_result(make_result(MEMBER_1, 1002, 2))
        .await
        .expect("collect member 1 result 2");
    aggregator
        .collect_result(make_result(MEMBER_2, 2001, 1))
        .await
        .expect("collect member 2 result 1");
    aggregator
        .collect_result(make_result(MEMBER_2, 2002, 2))
        .await
        .expect("collect member 2 result 2");

    let stats = aggregator.get_stats().await;
    assert_eq!(
        stats.results_collected, 4,
        "every unique result across both members should be collected",
    );
    assert_eq!(
        stats.results_deduplicated, 0,
        "results with distinct (pid, sequence) tuples should not be deduplicated",
    );
    assert_eq!(
        stats.results_pending, 4,
        "pending count should equal results collected before any aggregation emits",
    );

    // Re-submitting an identical result should be caught by the
    // deduplication cache rather than double-counted.
    aggregator
        .collect_result(make_result(MEMBER_1, 1001, 1))
        .await
        .expect("collect duplicate");

    let stats = aggregator.get_stats().await;
    assert_eq!(
        stats.results_collected, 4,
        "duplicate result must not increment results_collected",
    );
    assert_eq!(
        stats.results_deduplicated, 1,
        "duplicate result must be counted as deduplicated",
    );
}

/// Wildcard observer: a third subscriber listens on `control.collector.task.#`
/// (multi-level wildcard covering both `collector_type` and `collector_id`
/// segments) and observes every task distribution **without** participating
/// in the queue group. Queue-group routing is unaffected.
#[tokio::test]
async fn wildcard_observer_sees_all_task_broadcasts() {
    let (broker, _temp) = spawn_broker("wildcard-observer").await;
    let distributor = TaskDistributor::new(Arc::clone(&broker))
        .await
        .expect("construct distributor");
    distributor
        .set_routing_strategy(RoutingStrategy::RoundRobin)
        .await;

    let mut rx1 = join_queue_group(&broker, &distributor, MEMBER_1, 50).await;
    let mut rx2 = join_queue_group(&broker, &distributor, MEMBER_2, 50).await;

    // Observer subscribes to the multi-level wildcard. `#` matches zero or
    // more segments; for every `control.collector.task.procmond.<id>` topic
    // this pattern matches on the trailing segments.
    let observer_id = Uuid::new_v4();
    let observer_pattern = format!("{}.#", collector::TASK);
    let mut observer_rx = broker
        .subscribe_raw(&observer_pattern, observer_id)
        .await
        .expect("observer subscribes to wildcard pattern");

    const TASK_COUNT: usize = 12;
    let mut published_ids = HashSet::new();
    for i in 0..TASK_COUNT {
        let task_id = format!("wild-{i}");
        published_ids.insert(task_id.clone());
        distributor
            .distribute_task(make_task(&task_id, 2))
            .await
            .expect("distribute wildcard-observed task");
    }

    // Queue group still gets balanced routing.
    let qg_counts = collect_until(&mut [&mut rx1, &mut rx2], TASK_COUNT).await;
    assert_eq!(
        qg_counts[0].saturating_add(qg_counts[1]),
        TASK_COUNT,
        "queue-group delivery should be unaffected by the wildcard observer",
    );

    // Observer receives one copy per task — the broker fans the wildcard
    // subscriber in alongside the targeted member, not instead of it.
    let mut observed_ids = HashSet::new();
    let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
    while observed_ids.len() < TASK_COUNT && tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(50), observer_rx.recv()).await {
            Ok(Some(msg)) => {
                assert!(
                    msg.topic.starts_with(collector::TASK),
                    "observed topic must be under the task prefix, got {}",
                    msg.topic,
                );
                let task: TaskRequest =
                    postcard::from_bytes(&msg.payload).expect("payload decodes as TaskRequest");
                observed_ids.insert(task.task_id);
            }
            Ok(None) => panic!("observer channel closed unexpectedly"),
            Err(_elapsed) => {
                // no message this tick — loop until deadline
            }
        }
    }

    assert_eq!(
        observed_ids, published_ids,
        "wildcard observer should see every published task exactly once",
    );
}
