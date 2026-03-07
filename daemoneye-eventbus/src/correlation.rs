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
use tracing::debug;

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

    /// Get the current state of a workflow by correlation ID
    pub async fn get_workflow_state(&self, correlation_id: &str) -> Option<WorkflowState> {
        self.active_workflows
            .get(correlation_id)
            .map(|entry| entry.value().clone())
    }

    /// Track an event in the correlation workflow
    ///
    /// Updates the workflow state for the given correlation ID and appends
    /// the event to the bounded history.
    ///
    /// # Security
    /// - Validates correlation ID length
    /// - Enforces `max_active_workflows` limit (evicts oldest)
    /// - Enforces `max_history_size` (drops oldest events)
    pub async fn track_event(
        &self,
        topic: &str,
        metadata: &CorrelationMetadata,
    ) -> crate::error::Result<()> {
        const MAX_ID_LENGTH: usize = 256;
        const MAX_TOPIC_LENGTH: usize = 512;
        const MAX_STAGE_LENGTH: usize = 256;
        const MAX_TAG_COUNT: usize = 64;
        const MAX_TAG_KEY_LENGTH: usize = 128;
        const MAX_TAG_VALUE_LENGTH: usize = 1024;

        if metadata.correlation_id.len() > MAX_ID_LENGTH {
            return Err(crate::error::EventBusError::broker(
                "Correlation ID exceeds maximum length",
            ));
        }
        if metadata.root_correlation_id.len() > MAX_ID_LENGTH {
            return Err(crate::error::EventBusError::broker(
                "Root correlation ID exceeds maximum length",
            ));
        }
        if topic.len() > MAX_TOPIC_LENGTH {
            return Err(crate::error::EventBusError::broker(
                "Topic exceeds maximum length",
            ));
        }
        if let Some(stage) = &metadata.workflow_stage
            && stage.len() > MAX_STAGE_LENGTH
        {
            return Err(crate::error::EventBusError::broker(
                "Workflow stage name exceeds maximum length",
            ));
        }
        if metadata.correlation_tags.len() > MAX_TAG_COUNT {
            return Err(crate::error::EventBusError::broker(
                "Too many correlation tags",
            ));
        }
        for (key, value) in &metadata.correlation_tags {
            if key.len() > MAX_TAG_KEY_LENGTH || value.len() > MAX_TAG_VALUE_LENGTH {
                return Err(crate::error::EventBusError::broker(
                    "Correlation tag key or value exceeds maximum length",
                ));
            }
        }

        let now = Instant::now();

        // Update active workflow using DashMap (lock-free reads)
        {
            // Evict oldest workflow if at capacity
            if self.active_workflows.len() >= self.config.max_active_workflows
                && !self.active_workflows.contains_key(&metadata.correlation_id)
                && let Some(oldest_key) = self
                    .active_workflows
                    .iter()
                    .min_by_key(|entry| entry.value().last_activity)
                    .map(|entry| entry.key().clone())
            {
                self.active_workflows.remove(&oldest_key);
                debug!("Evicted oldest workflow: {oldest_key}");
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

            // Enforce history size limit; skip storage entirely if configured to zero
            if self.config.max_history_size == 0 {
                // History disabled — nothing to store
            } else {
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

    /// Remove workflows that have been inactive longer than the configured timeout
    ///
    /// Returns the number of workflows removed.
    #[allow(clippy::as_conversions)] // usize to u64 is safe: usize <= u64 on all supported platforms
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
            debug!("Cleaned up {removed} expired workflows");
            let mut stats = self.stats.write().await;
            stats.active_workflows = self.active_workflows.len();
            stats.workflows_timed_out = stats.workflows_timed_out.saturating_add(removed as u64);
        }

        removed
    }

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
}
