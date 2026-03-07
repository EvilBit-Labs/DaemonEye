# Correlation Tracker Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a `CorrelationTracker` that tracks workflows across local collector processes, with forensic query APIs and broker integration.

**Architecture:** A new `correlation.rs` module in `daemoneye-eventbus` containing `CorrelationTracker` backed by `DashMap` for lock-free concurrent reads on active workflows and a `tokio::sync::RwLock<VecDeque>` for bounded event history. The tracker integrates into `DaemoneyeBroker` via a new `publish_with_correlation()` method. One new dependency: `dashmap` (well-maintained, no unsafe in public API, superior for read-heavy forensic queries).

**Tech Stack:** Rust 2024, tokio async, dashmap, serde serialization, tracing logging, existing `CorrelationMetadata` from `message.rs`

---

## Context for the Implementer

### What Already Exists

- **`CorrelationMetadata`** (`daemoneye-eventbus/src/message.rs:10-218`): Fully implemented with builder pattern, parent/child hierarchy, tags, pattern matching, sequence tracking.
- **`CorrelationFilter`** (`daemoneye-eventbus/src/message.rs:619-795`): Comprehensive filtering by ID, pattern, parent, root, stage, tags, sequence range.
- **`Message`** (`daemoneye-eventbus/src/message.rs:222-442`): Event envelope with `correlation_metadata: CorrelationMetadata` field.
- **`DaemoneyeBroker`** (`daemoneye-eventbus/src/broker.rs`): Has `publish(topic, correlation_id, payload)` and `publish_control_message()`. Routes to subscribers via `subscriber_senders` and `raw_subscriber_senders`.
- **`DaemoneyeEventBus`** (`daemoneye-eventbus/src/broker.rs:911-942`): Thin wrapper around `Arc<DaemoneyeBroker>`.
- **`EventBusError`** (`daemoneye-eventbus/src/error.rs`): Has `Timeout(String)` variant already.
- **`ResultAggregator`** (`daemoneye-eventbus/src/result_aggregation.rs`): Has a basic `CorrelationEntry` struct but no public `CorrelationTracker` type. This is a separate concern; we don't modify it.

### Key Patterns to Follow

- Use `DashMap` for concurrent read-heavy maps (active workflows). Use `tokio::sync::RwLock` for ordered collections (event history `VecDeque`).
- Use `saturating_add` for counters (clippy `arithmetic_side_effects` warning).
- Use `tracing::{debug, info, warn}` for logging.
- Never `.await` inside tracing macros.
- All IDs are `String` (not `Uuid`) to match existing `CorrelationMetadata`.
- `clippy::unwrap_used = "deny"` — never use `.unwrap()`.
- `unsafe_code = "forbid"` — zero unsafe.
- Keep files under 500 lines.

### Workspace Lint Rules

The workspace has extremely strict clippy: `pedantic`, `nursery`, `cargo` all at warn level, plus `unwrap_used = "deny"`, `panic = "deny"`, `str_to_string = "warn"`, `string_add = "warn"`, `shadow_reuse = "warn"`, etc. Run `cargo clippy -p daemoneye-eventbus -- -D warnings` after every implementation step.

### Running Tests

```bash
# Run specific test
NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_name -- --exact

# Run all eventbus tests
NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus

# Clippy check
cargo clippy -p daemoneye-eventbus -- -D warnings
```

---

## Task 1: Create `CorrelationTracker` Core Types

**Files:**

- Modify: `Cargo.toml` (workspace root — add `dashmap` to workspace dependencies)
- Modify: `daemoneye-eventbus/Cargo.toml` (add `dashmap = { workspace = true }`)
- Create: `daemoneye-eventbus/src/correlation.rs`
- Test: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

This task adds the `dashmap` dependency and creates the data types and the basic `CorrelationTracker` struct.

**Step 1: Write the failing test**

Create `daemoneye-eventbus/tests/correlation_tracker_tests.rs`:

```rust
//! Tests for CorrelationTracker workflow tracking

use daemoneye_eventbus::CorrelationMetadata;
use daemoneye_eventbus::correlation::{CorrelationTracker, CorrelationTrackerConfig};

#[tokio::test]
async fn test_correlation_tracker_creation() {
    let config = CorrelationTrackerConfig::default();
    let tracker = CorrelationTracker::new(config);

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 0);
    assert_eq!(stats.total_events_tracked, 0);
    assert_eq!(stats.history_size, 0);
}
```

**Step 2: Run test to verify it fails**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_correlation_tracker_creation -- --exact 2>&1 | head -30`

Expected: FAIL — module `correlation` not found.

**Step 3: Add `dashmap` dependency and write minimal implementation**

First, add `dashmap` to the workspace root `Cargo.toml` (in the `[workspace.dependencies]` section, alphabetically near `crossbeam`):

```toml
dashmap = "6.1.0"
```

Then add to `daemoneye-eventbus/Cargo.toml` in the `[dependencies]` section:

```toml
dashmap = { workspace = true }
```

Create `daemoneye-eventbus/src/correlation.rs`:

```rust
//! Correlation tracking for multi-collector workflow coordination
//!
//! Tracks workflows across local collector processes, provides forensic
//! query APIs for tag-based search and timeline reconstruction.
//!
//! # Security
//!
//! - No unsafe code
//! - All inputs validated at trust boundaries
//! - Bounded resources with configurable limits

use crate::message::CorrelationMetadata;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Configuration for the correlation tracker
#[derive(Debug, Clone)]
pub struct CorrelationTrackerConfig {
    /// Maximum number of events retained in history
    pub max_history_size: usize,
    /// Maximum number of active workflows before oldest are evicted
    pub max_active_workflows: usize,
    /// Duration after which inactive workflows are cleaned up
    pub workflow_timeout: Duration,
}

impl Default for CorrelationTrackerConfig {
    fn default() -> Self {
        Self {
            max_history_size: 10_000,
            max_active_workflows: 1_000,
            workflow_timeout: Duration::from_secs(300),
        }
    }
}

/// State of a tracked workflow
#[derive(Debug, Clone)]
pub struct WorkflowState {
    /// Correlation ID for this workflow
    pub correlation_id: String,
    /// When workflow tracking started
    pub started_at: Instant,
    /// Stages observed in this workflow
    pub stages: HashMap<String, StageInfo>,
    /// Process/collector IDs that participated
    pub participating_sources: HashSet<String>,
    /// Total events tracked for this workflow
    pub total_events: usize,
    /// Last activity timestamp
    pub last_activity: Instant,
}

/// Information about a single workflow stage
#[derive(Debug, Clone)]
pub struct StageInfo {
    /// Stage name
    pub stage_name: String,
    /// When this stage was first seen
    pub started_at: Instant,
    /// When this stage completed (if applicable)
    pub completed_at: Option<Instant>,
    /// Number of events in this stage
    pub events_count: usize,
}

/// A correlated event stored in history
#[derive(Debug, Clone)]
pub struct CorrelatedEvent {
    /// Correlation ID this event belongs to
    pub correlation_id: String,
    /// Root correlation ID for the workflow
    pub root_correlation_id: String,
    /// Topic the event was published on
    pub topic: String,
    /// Correlation metadata snapshot
    pub metadata: CorrelationMetadata,
    /// When the event was received by the tracker
    pub received_at: Instant,
    /// Wall-clock timestamp for serialization
    pub wall_clock: SystemTime,
}

/// Event in a reconstructed timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// Workflow stage
    pub stage: Option<String>,
    /// Source process/collector
    pub source: Option<String>,
    /// Topic
    pub topic: String,
    /// Correlation ID
    pub correlation_id: String,
    /// Wall-clock timestamp
    pub timestamp: SystemTime,
}

/// Reconstructed workflow timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTimeline {
    /// Root correlation ID
    pub root_correlation_id: String,
    /// Ordered events in the timeline
    pub events: Vec<TimelineEvent>,
}

/// Statistics for the correlation tracker
#[derive(Debug, Clone, Default)]
pub struct CorrelationTrackerStats {
    /// Number of active workflows being tracked
    pub active_workflows: usize,
    /// Total events tracked since creation
    pub total_events_tracked: u64,
    /// Current event history size
    pub history_size: usize,
    /// Workflows that timed out
    pub workflows_timed_out: u64,
}

/// Tracks correlation workflows across local collector processes
pub struct CorrelationTracker {
    /// Active workflows indexed by correlation ID (lock-free concurrent reads)
    active_workflows: DashMap<String, WorkflowState>,
    /// Event history for forensic analysis (bounded, ordered)
    event_history: RwLock<VecDeque<CorrelatedEvent>>,
    /// Configuration
    config: CorrelationTrackerConfig,
    /// Statistics
    stats: RwLock<CorrelationTrackerStats>,
}

impl CorrelationTracker {
    /// Create a new correlation tracker with the given configuration
    pub fn new(config: CorrelationTrackerConfig) -> Self {
        Self {
            active_workflows: DashMap::new(),
            event_history: RwLock::new(VecDeque::new()),
            config,
            stats: RwLock::new(CorrelationTrackerStats::default()),
        }
    }

    /// Get current tracker statistics
    pub async fn stats(&self) -> CorrelationTrackerStats {
        self.stats.read().await.clone()
    }
}
```

Then add the module to `daemoneye-eventbus/src/lib.rs`:

Add after line 73 (`pub mod transport;`):

```rust
pub mod correlation;
```

Add to the re-exports section (after the existing re-export blocks, around line 117):

```rust
pub use correlation::{
    CorrelatedEvent, CorrelationTracker, CorrelationTrackerConfig, CorrelationTrackerStats,
    StageInfo, TimelineEvent, WorkflowState, WorkflowTimeline,
};
```

**Step 4: Run test to verify it passes**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_correlation_tracker_creation -- --exact`

Expected: PASS

**Step 5: Run clippy**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings`

Expected: No errors. Fix any warnings before proceeding.

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock daemoneye-eventbus/Cargo.toml daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/src/lib.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): add CorrelationTracker core types

Add dashmap workspace dependency and correlation.rs module with
CorrelationTracker (DashMap-backed), WorkflowState, StageInfo,
CorrelatedEvent, and WorkflowTimeline types.
Initial creation test passes."
```

---

## Task 2: Implement `track_event()` Method

**Files:**

- Modify: `daemoneye-eventbus/src/correlation.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing tests**

Append to `correlation_tracker_tests.rs`:

```rust
#[tokio::test]
async fn test_track_single_event() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata =
        CorrelationMetadata::new("workflow-1".to_owned()).with_stage("collection".to_owned());

    tracker
        .track_event("events.process.new", &metadata)
        .await
        .expect("track_event should succeed");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 1);
    assert_eq!(stats.total_events_tracked, 1);
    assert_eq!(stats.history_size, 1);
}

#[tokio::test]
async fn test_track_multiple_events_same_workflow() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata1 = CorrelationMetadata::new("workflow-1".to_owned())
        .with_stage("collection".to_owned())
        .with_sequence(0);

    let metadata2 = CorrelationMetadata::new("workflow-1".to_owned())
        .with_stage("analysis".to_owned())
        .with_sequence(1);

    tracker
        .track_event("events.process.new", &metadata1)
        .await
        .expect("first track should succeed");
    tracker
        .track_event("events.process.analyzed", &metadata2)
        .await
        .expect("second track should succeed");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 1, "same workflow, one entry");
    assert_eq!(stats.total_events_tracked, 2);
    assert_eq!(stats.history_size, 2);
}

#[tokio::test]
async fn test_track_events_different_workflows() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let meta_a = CorrelationMetadata::new("workflow-a".to_owned());
    let meta_b = CorrelationMetadata::new("workflow-b".to_owned());

    tracker
        .track_event("events.process.new", &meta_a)
        .await
        .expect("a");
    tracker
        .track_event("events.network.new", &meta_b)
        .await
        .expect("b");

    let stats = tracker.stats().await;
    assert_eq!(stats.active_workflows, 2);
    assert_eq!(stats.total_events_tracked, 2);
}
```

**Step 2: Run tests to verify they fail**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests 2>&1 | tail -20`

Expected: FAIL — `track_event` method not found.

**Step 3: Implement `track_event()`**

Add to `CorrelationTracker` impl block in `correlation.rs`:

```rust
/// Track an event in the correlation workflow
///
/// Updates the workflow state for the given correlation ID and appends
/// the event to the bounded history.
///
/// # Security
/// - Validates correlation ID length
/// - Enforces max_active_workflows limit (evicts oldest)
/// - Enforces max_history_size (drops oldest events)
pub async fn track_event(
    &self,
    topic: &str,
    metadata: &CorrelationMetadata,
) -> crate::error::Result<()> {
    // Validate correlation ID length
    const MAX_ID_LENGTH: usize = 256;
    if metadata.correlation_id.len() > MAX_ID_LENGTH {
        return Err(crate::error::EventBusError::broker(
            "Correlation ID exceeds maximum length",
        ));
    }

    let now = Instant::now();

    // Update active workflow using DashMap (lock-free reads)
    {
        // Evict oldest workflow if at capacity
        if self.active_workflows.len() >= self.config.max_active_workflows
            && !self.active_workflows.contains_key(&metadata.correlation_id)
        {
            if let Some(oldest_key) = self
                .active_workflows
                .iter()
                .min_by_key(|entry| entry.value().last_activity)
                .map(|entry| entry.key().clone())
            {
                self.active_workflows.remove(&oldest_key);
                debug!("Evicted oldest workflow: {}", oldest_key);
            }
        }

        let mut workflow = self
            .active_workflows
            .entry(metadata.correlation_id.clone())
            .or_insert_with(|| WorkflowState {
                correlation_id: metadata.correlation_id.clone(),
                started_at: now,
                stages: HashMap::new(),
                participating_sources: HashSet::new(),
                total_events: 0,
                last_activity: now,
            });

        workflow.total_events = workflow.total_events.saturating_add(1);
        workflow.last_activity = now;

        // Track stage if present
        if let Some(stage_name) = &metadata.workflow_stage {
            let stage = workflow
                .stages
                .entry(stage_name.clone())
                .or_insert_with(|| StageInfo {
                    stage_name: stage_name.clone(),
                    started_at: now,
                    completed_at: None,
                    events_count: 0,
                });
            stage.events_count = stage.events_count.saturating_add(1);
        }

        // Track source process from tags
        if let Some(source) = metadata.correlation_tags.get("source_process") {
            workflow.participating_sources.insert(source.clone());
        }
    }

    // Append to event history
    {
        let mut history = self.event_history.write().await;

        // Enforce history size limit
        while history.len() >= self.config.max_history_size {
            history.pop_front();
        }

        history.push_back(CorrelatedEvent {
            correlation_id: metadata.correlation_id.clone(),
            root_correlation_id: metadata.root_correlation_id.clone(),
            topic: topic.to_owned(),
            metadata: metadata.clone(),
            received_at: now,
            wall_clock: SystemTime::now(),
        });
    }

    // Update stats
    {
        let history = self.event_history.read().await;
        let mut stats = self.stats.write().await;
        stats.total_events_tracked = stats.total_events_tracked.saturating_add(1);
        stats.active_workflows = self.active_workflows.len();
        stats.history_size = history.len();
    }

    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 4 tests PASS.

**Step 5: Run clippy**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings`

Expected: Clean.

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): implement CorrelationTracker.track_event()

Tracks workflow state per correlation ID, maintains bounded event
history, validates input lengths, and evicts oldest workflows when
at capacity."
```

---

## Task 3: Implement History Bounding and `get_workflow_state()`

**Files:**

- Modify: `daemoneye-eventbus/src/correlation.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing tests**

Append to `correlation_tracker_tests.rs`:

```rust
#[tokio::test]
async fn test_event_history_bounded() {
    let config = CorrelationTrackerConfig {
        max_history_size: 3,
        max_active_workflows: 100,
        workflow_timeout: Duration::from_secs(300),
    };
    let tracker = CorrelationTracker::new(config);

    for i in 0..5 {
        let meta = CorrelationMetadata::new(format!("wf-{i}"));
        tracker
            .track_event("events.process.new", &meta)
            .await
            .expect("track should succeed");
    }

    let stats = tracker.stats().await;
    assert_eq!(
        stats.history_size, 3,
        "history should be bounded to max_history_size"
    );
    assert_eq!(
        stats.total_events_tracked, 5,
        "total should count all events"
    );
}

#[tokio::test]
async fn test_get_workflow_state() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let metadata =
        CorrelationMetadata::new("wf-lookup".to_owned()).with_stage("collection".to_owned());

    tracker
        .track_event("events.process.new", &metadata)
        .await
        .expect("track");

    let state = tracker.get_workflow_state("wf-lookup").await;
    assert!(state.is_some());
    let state = state.expect("should exist");
    assert_eq!(state.correlation_id, "wf-lookup");
    assert_eq!(state.total_events, 1);
    assert!(state.stages.contains_key("collection"));

    let missing = tracker.get_workflow_state("nonexistent").await;
    assert!(missing.is_none());
}
```

Add `use std::time::Duration;` to the top of the test file if not already present.

**Step 2: Run tests to verify they fail**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_get_workflow_state -- --exact 2>&1 | tail -10`

Expected: FAIL — `get_workflow_state` not found.

**Step 3: Implement `get_workflow_state()`**

Add to `CorrelationTracker` impl in `correlation.rs`:

```rust
/// Get the current state of a workflow by correlation ID
pub async fn get_workflow_state(&self, correlation_id: &str) -> Option<WorkflowState> {
    self.active_workflows
        .get(correlation_id)
        .map(|entry| entry.value().clone())
}
```

**Step 4: Run tests**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 6 tests PASS.

**Step 5: Run clippy**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings`

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): add get_workflow_state() and history bounding tests"
```

---

## Task 4: Implement Forensic Query APIs

**Files:**

- Modify: `daemoneye-eventbus/src/correlation.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing tests**

Append to `correlation_tracker_tests.rs`:

```rust
#[tokio::test]
async fn test_find_events_by_tag() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let meta1 = CorrelationMetadata::new("inc-001".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-12345".to_owned())
        .with_tag("severity".to_owned(), "critical".to_owned());

    let meta2 = CorrelationMetadata::new("inc-002".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-12345".to_owned())
        .with_tag("severity".to_owned(), "low".to_owned());

    let meta3 = CorrelationMetadata::new("inc-003".to_owned())
        .with_tag("investigation_id".to_owned(), "INC-99999".to_owned());

    tracker
        .track_event("events.process.new", &meta1)
        .await
        .expect("1");
    tracker
        .track_event("events.process.new", &meta2)
        .await
        .expect("2");
    tracker
        .track_event("events.network.new", &meta3)
        .await
        .expect("3");

    let results = tracker
        .find_events_by_tag("investigation_id", "INC-12345")
        .await;
    assert_eq!(results.len(), 2);

    let results = tracker.find_events_by_tag("severity", "critical").await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].correlation_id, "inc-001");

    let results = tracker.find_events_by_tag("nonexistent", "value").await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_get_workflow_timeline() {
    let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

    let root_id = "root-timeline".to_owned();

    let meta1 = CorrelationMetadata::new(root_id.clone())
        .with_stage("enumeration".to_owned())
        .with_sequence(0);

    let child_meta = meta1
        .create_child("child-1".to_owned())
        .with_stage("analysis".to_owned())
        .with_sequence(1);

    tracker
        .track_event("events.process.new", &meta1)
        .await
        .expect("1");
    tracker
        .track_event("events.process.analyzed", &child_meta)
        .await
        .expect("2");

    let timeline = tracker.get_workflow_timeline(&root_id).await;
    assert_eq!(timeline.root_correlation_id, root_id);
    assert_eq!(timeline.events.len(), 2);
    assert_eq!(timeline.events[0].stage, Some("enumeration".to_owned()));
    assert_eq!(timeline.events[1].stage, Some("analysis".to_owned()));
}
```

**Step 2: Run tests to verify they fail**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_find_events_by_tag -- --exact 2>&1 | tail -10`

Expected: FAIL — method not found.

**Step 3: Implement forensic query methods**

Add to `CorrelationTracker` impl in `correlation.rs`:

```rust
/// Find events by a specific correlation tag (forensic query)
///
/// Searches the event history for events whose correlation metadata
/// contains the given tag key-value pair.
pub async fn find_events_by_tag(&self, tag_key: &str, tag_value: &str) -> Vec<CorrelatedEvent> {
    let history = self.event_history.read().await;

    history
        .iter()
        .filter(|event| event.metadata.has_tag(tag_key, tag_value))
        .cloned()
        .collect()
}

/// Reconstruct a workflow timeline by root correlation ID
///
/// Returns all events that share the given root correlation ID,
/// ordered by the time they were received.
pub async fn get_workflow_timeline(&self, root_correlation_id: &str) -> WorkflowTimeline {
    let history = self.event_history.read().await;

    let events: Vec<TimelineEvent> = history
        .iter()
        .filter(|event| event.root_correlation_id == root_correlation_id)
        .map(|event| TimelineEvent {
            stage: event.metadata.workflow_stage.clone(),
            source: event
                .metadata
                .correlation_tags
                .get("source_process")
                .cloned(),
            topic: event.topic.clone(),
            correlation_id: event.correlation_id.clone(),
            timestamp: event.wall_clock,
        })
        .collect();

    WorkflowTimeline {
        root_correlation_id: root_correlation_id.to_owned(),
        events,
    }
}
```

**Step 4: Run tests**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 8 tests PASS.

**Step 5: Run clippy**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings`

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): add forensic query APIs to CorrelationTracker

Implement find_events_by_tag() for tag-based forensic search and
get_workflow_timeline() for timeline reconstruction by root
correlation ID."
```

---

## Task 5: Implement Workflow Cleanup

**Files:**

- Modify: `daemoneye-eventbus/src/correlation.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing test**

Append to `correlation_tracker_tests.rs`:

```rust
#[tokio::test]
async fn test_cleanup_expired_workflows() {
    let config = CorrelationTrackerConfig {
        max_history_size: 10_000,
        max_active_workflows: 100,
        // Very short timeout for testing
        workflow_timeout: Duration::from_millis(50),
    };
    let tracker = CorrelationTracker::new(config);

    let meta = CorrelationMetadata::new("expiring-wf".to_owned());
    tracker
        .track_event("events.process.new", &meta)
        .await
        .expect("track");

    assert_eq!(tracker.stats().await.active_workflows, 1);

    // Wait for timeout
    tokio::time::sleep(Duration::from_millis(100)).await;

    let cleaned = tracker.cleanup_expired_workflows().await;
    assert_eq!(cleaned, 1);
    assert_eq!(tracker.stats().await.active_workflows, 0);
}
```

**Step 2: Run test to verify it fails**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_cleanup_expired -- --exact 2>&1 | tail -10`

Expected: FAIL — method not found.

**Step 3: Implement `cleanup_expired_workflows()`**

Add to `CorrelationTracker` impl in `correlation.rs`:

```rust
/// Remove workflows that have been inactive longer than the configured timeout
///
/// Returns the number of workflows removed.
pub async fn cleanup_expired_workflows(&self) -> usize {
    let now = Instant::now();
    let timeout = self.config.workflow_timeout;

    let before = self.active_workflows.len();

    self.active_workflows.retain(|_id, state| {
        let elapsed = now.duration_since(state.last_activity);
        elapsed < timeout
    });

    let removed = before.saturating_sub(self.active_workflows.len());

    if removed > 0 {
        debug!("Cleaned up {} expired workflows", removed);
        let mut stats = self.stats.write().await;
        stats.active_workflows = self.active_workflows.len();
        stats.workflows_timed_out = stats.workflows_timed_out.saturating_add(removed as u64);
    }

    removed
}
```

**Step 4: Run tests**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 9 tests PASS.

**Step 5: Run clippy**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings`

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): add workflow cleanup for expired correlations"
```

---

## Task 6: Integrate `CorrelationTracker` into `DaemoneyeBroker`

**Files:**

- Modify: `daemoneye-eventbus/src/broker.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing test**

Append to `correlation_tracker_tests.rs`:

```rust
use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, ProcessEvent,
    SourceCaps,
};
use tempfile::tempdir;

#[tokio::test]
async fn test_broker_publish_with_correlation_tracks_event() {
    let temp_dir = tempdir().expect("tempdir");
    let socket_path = temp_dir.path().join("tracker-test.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
        .await
        .expect("broker");

    let metadata = CorrelationMetadata::new("broker-track-wf".to_owned())
        .with_stage("collection".to_owned())
        .with_tag("source_process".to_owned(), "procmond".to_owned());

    let event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "test".to_owned(),
        command_line: None,
        executable_path: None,
        ppid: None,
        start_time: None,
        metadata: std::collections::HashMap::new(),
    });

    let payload = postcard::to_allocvec(&event).expect("serialize");

    broker
        .publish_with_correlation("events.process.new", &metadata, payload)
        .await
        .expect("publish_with_correlation should succeed");

    let tracker_stats = broker.correlation_tracker().stats().await;
    assert_eq!(tracker_stats.total_events_tracked, 1);
    assert_eq!(tracker_stats.active_workflows, 1);
}
```

Add `use daemoneye_eventbus::correlation::CorrelationTrackerConfig;` is already imported. Also add at the top of the file:

```rust
use std::collections::HashMap;
```

**Step 2: Run test to verify it fails**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests test_broker_publish_with_correlation -- --exact 2>&1 | tail -15`

Expected: FAIL — `publish_with_correlation` and `correlation_tracker` not found on `DaemoneyeBroker`.

**Step 3: Add `CorrelationTracker` to broker**

In `daemoneye-eventbus/src/broker.rs`:

1. Add import at top (after existing `use` statements):

```rust
use crate::correlation::{CorrelationTracker, CorrelationTrackerConfig};
```

2. Add field to `DaemoneyeBroker` struct (after `queue_manager` field, around line 57):

```rust
    /// Correlation tracker for multi-collector workflow tracking
    correlation_tracker: Arc<CorrelationTracker>,
```

3. Initialize in `new_with_config()` (before `let broker = Self {`, around line 104):

```rust
        let correlation_tracker = Arc::new(CorrelationTracker::new(CorrelationTrackerConfig::default()));
```

4. Add to the `Self { ... }` struct literal (after `queue_manager,`):

```rust
            correlation_tracker,
```

5. Add accessor and publish method to `impl DaemoneyeBroker` block (after the existing `publish()` method, around line 611):

```rust
/// Get a reference to the correlation tracker
pub fn correlation_tracker(&self) -> &CorrelationTracker {
    &self.correlation_tracker
}

/// Publish a message with full correlation metadata tracking
///
/// Like `publish()`, but accepts `CorrelationMetadata` directly and
/// records the event in the correlation tracker for workflow tracking
/// and forensic queries.
pub async fn publish_with_correlation(
    &self,
    topic: &str,
    metadata: &crate::message::CorrelationMetadata,
    payload: Vec<u8>,
) -> Result<()> {
    // Track the event in the correlation tracker
    self.correlation_tracker
        .track_event(topic, metadata)
        .await?;

    // Delegate to existing publish with correlation ID
    self.publish(topic, &metadata.correlation_id, payload).await
}
```

**Step 4: Run tests**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 10 tests PASS.

**Step 5: Run clippy and full test suite**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings && NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus`

Expected: All existing tests still pass, no clippy warnings.

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/broker.rs daemoneye-eventbus/src/correlation.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): integrate CorrelationTracker into DaemoneyeBroker

Add correlation_tracker field to DaemoneyeBroker, expose via
correlation_tracker() accessor, and add publish_with_correlation()
method that tracks events before routing."
```

---

## Task 7: Add `publish_with_correlation` to `EventBus` Trait (Optional Extension)

**Files:**

- Modify: `daemoneye-eventbus/src/lib.rs`
- Modify: `daemoneye-eventbus/src/broker.rs`
- Modify: `daemoneye-eventbus/tests/correlation_tracker_tests.rs`

**Step 1: Write the failing test**

Append to `correlation_tracker_tests.rs`:

```rust
#[tokio::test]
async fn test_eventbus_publish_with_correlation_metadata() {
    let temp_dir = tempdir().expect("tempdir");
    let socket_path = temp_dir.path().join("eventbus-corr-test.sock");

    let broker = DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
        .await
        .expect("broker");
    let mut event_bus = DaemoneyeEventBus::from_broker(broker)
        .await
        .expect("eventbus");

    let metadata =
        CorrelationMetadata::new("eventbus-wf".to_owned()).with_stage("detection".to_owned());

    let event = CollectionEvent::Process(ProcessEvent {
        pid: 5678,
        name: "suspicious".to_owned(),
        command_line: Some("./malware --exec".to_owned()),
        executable_path: Some("/tmp/malware".to_owned()),
        ppid: Some(1),
        start_time: None,
        metadata: HashMap::new(),
    });

    event_bus
        .publish_with_metadata(event, metadata)
        .await
        .expect("publish_with_metadata should succeed");

    let stats = event_bus.statistics().await;
    assert_eq!(stats.messages_published, 1);
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — `publish_with_metadata` not found.

**Step 3: Add `publish_with_metadata` to `DaemoneyeEventBus`**

In `daemoneye-eventbus/src/broker.rs`, add method to `impl DaemoneyeEventBus` block (after `broker()`, around line 941):

```rust
/// Publish an event with full correlation metadata for workflow tracking
pub async fn publish_with_metadata(
    &mut self,
    event: CollectionEvent,
    metadata: crate::message::CorrelationMetadata,
) -> Result<()> {
    let topic = match &event {
        CollectionEvent::Process(_) => "events.process.new",
        CollectionEvent::Network(_) => "events.network.new",
        CollectionEvent::Filesystem(_) => "events.filesystem.new",
        CollectionEvent::Performance(_) => "events.performance.new",
        CollectionEvent::TriggerRequest(_) => "control.trigger.request",
    };

    let payload =
        postcard::to_allocvec(&event).map_err(|e| EventBusError::serialization(e.to_string()))?;

    self.broker
        .publish_with_correlation(topic, &metadata, payload)
        .await
}
```

**Step 4: Run tests**

Run: `NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus --test correlation_tracker_tests`

Expected: All 11 tests PASS.

**Step 5: Run clippy and full test suite**

Run: `cargo clippy -p daemoneye-eventbus -- -D warnings && NO_COLOR=1 TERM=dumb cargo test -p daemoneye-eventbus`

Expected: Clean.

**Step 6: Commit**

```bash
git add daemoneye-eventbus/src/broker.rs daemoneye-eventbus/tests/correlation_tracker_tests.rs
git commit -s -m "feat(eventbus): add publish_with_metadata to DaemoneyeEventBus

Convenience method that accepts CorrelationMetadata directly,
tracks the event, and publishes through the broker."
```

---

## Task 8: Final Verification and Full Test Suite

**Files:**

- No changes — verification only.

**Step 1: Run full workspace build**

Run: `cargo build --workspace`

Expected: Clean build.

**Step 2: Run full workspace tests**

Run: `NO_COLOR=1 TERM=dumb cargo test --workspace`

Expected: All tests pass, including existing `correlation_metadata_tests.rs`.

**Step 3: Run clippy on full workspace**

Run: `cargo clippy --workspace -- -D warnings`

Expected: Zero warnings.

**Step 4: Run format check**

Run: `cargo fmt --all --check`

Expected: Clean (or run `cargo fmt --all` if needed).

**Step 5: Verify file sizes**

Run: `wc -l daemoneye-eventbus/src/correlation.rs`

Expected: Under 300 lines.

**Step 6: No commit — this is verification only**

---

## Acceptance Criteria Checklist

After all tasks complete, verify:

- [ ] `CorrelationTracker` tracks workflows across local processes
- [ ] `track_event()` updates workflow state and bounded history
- [ ] `get_workflow_state()` returns workflow by correlation ID
- [ ] `find_events_by_tag()` searches history by tag key-value
- [ ] `get_workflow_timeline()` reconstructs timeline by root ID
- [ ] `cleanup_expired_workflows()` removes stale entries
- [ ] `publish_with_correlation()` on `DaemoneyeBroker` tracks then routes
- [ ] `publish_with_metadata()` on `DaemoneyeEventBus` for convenience
- [ ] Event history bounded to configurable `max_history_size`
- [ ] Active workflows bounded to `max_active_workflows`
- [ ] Zero `unsafe` code
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] All existing tests continue to pass
- [ ] New tests cover all added functionality
