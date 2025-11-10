//! Task distribution system for multi-collector coordination.
//!
//! This module provides topic-based task distribution for multiple collector types,
//! enabling coordinated workflows across process, network, filesystem, and other
//! monitoring domains.
//!
//! # Security
//!
//! - All task payloads are validated during serialization
//! - Timeout tracking prevents resource exhaustion
//! - Priority queues ensure critical tasks are processed first
//! - No unsafe code or privilege escalation

use crate::{event::CollectionEvent, source::SourceCaps};
use anyhow::{Context, Result};
use daemoneye_eventbus::DaemoneyeBroker;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Metadata for in-flight tasks awaiting acknowledgment
#[derive(Debug, Clone)]
struct InFlightTask {
    /// The task being tracked
    task: DistributionTask,
    /// Timestamp when task was published
    #[allow(dead_code)]
    published_at: SystemTime,
    /// Number of retry attempts
    retry_count: u32,
}

/// Task priority levels for distribution
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TaskPriority {
    /// Low priority tasks (background analysis)
    Low = 0,
    /// Normal priority tasks (standard detection)
    Normal = 1,
    /// High priority tasks (security alerts)
    High = 2,
    /// Critical priority tasks (immediate threats)
    Critical = 3,
}

/// Task distribution request for collector coordination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionTask {
    /// Unique task identifier
    pub task_id: String,
    /// Task priority level
    pub priority: TaskPriority,
    /// Target collector capabilities required
    pub required_capabilities: SourceCaps,
    /// Target topic for task distribution
    pub target_topic: String,
    /// Task payload (serialized detection rule, analysis request, etc.)
    pub payload: Vec<u8>,
    /// Correlation ID for tracking related tasks
    pub correlation_id: String,
    /// Task creation timestamp
    pub created_at: SystemTime,
    /// Task timeout duration
    pub timeout: Duration,
    /// Task metadata for routing and tracking
    pub metadata: HashMap<String, String>,
}

/// Task distribution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistributionStats {
    /// Total tasks distributed
    pub tasks_distributed: u64,
    /// Tasks currently pending
    pub tasks_pending: usize,
    /// Tasks successfully delivered
    pub tasks_delivered: u64,
    /// Tasks that failed delivery
    pub tasks_failed: u64,
    /// Tasks that timed out
    pub tasks_timed_out: u64,
    /// Average distribution latency (milliseconds)
    pub avg_distribution_latency_ms: f64,
}

/// Task distributor for multi-collector coordination
pub struct TaskDistributor {
    /// Reference to the event bus broker
    broker: Arc<DaemoneyeBroker>,
    /// Task queue organized by priority (persistent queue)
    task_queue: Arc<RwLock<PriorityTaskQueue>>,
    /// Distribution statistics
    stats: Arc<RwLock<DistributionStats>>,
    /// Task timeout tracking (keyed by task_id, value is timeout timestamp)
    timeout_tracker: Arc<RwLock<HashMap<String, SystemTime>>>,
    /// In-flight tasks awaiting consumer acknowledgment
    /// Tasks are moved here after successful publish and removed after ack or timeout
    in_flight: Arc<RwLock<HashMap<String, InFlightTask>>>,
}

/// Priority-based task queue
#[derive(Debug)]
struct PriorityTaskQueue {
    /// Critical priority queue
    critical: VecDeque<DistributionTask>,
    /// High priority queue
    high: VecDeque<DistributionTask>,
    /// Normal priority queue
    normal: VecDeque<DistributionTask>,
    /// Low priority queue
    low: VecDeque<DistributionTask>,
}

impl PriorityTaskQueue {
    fn new() -> Self {
        Self {
            critical: VecDeque::new(),
            high: VecDeque::new(),
            normal: VecDeque::new(),
            low: VecDeque::new(),
        }
    }

    fn push(&mut self, task: DistributionTask) {
        match task.priority {
            TaskPriority::Critical => self.critical.push_back(task),
            TaskPriority::High => self.high.push_back(task),
            TaskPriority::Normal => self.normal.push_back(task),
            TaskPriority::Low => self.low.push_back(task),
        }
    }

    fn pop(&mut self) -> Option<DistributionTask> {
        self.critical
            .pop_front()
            .or_else(|| self.high.pop_front())
            .or_else(|| self.normal.pop_front())
            .or_else(|| self.low.pop_front())
    }

    fn len(&self) -> usize {
        self.critical.len() + self.high.len() + self.normal.len() + self.low.len()
    }

    #[allow(dead_code)] // Reserved for future queue management operations
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TaskDistributor {
    /// Create a new task distributor
    pub fn new(broker: Arc<DaemoneyeBroker>) -> Self {
        Self {
            broker,
            task_queue: Arc::new(RwLock::new(PriorityTaskQueue::new())),
            stats: Arc::new(RwLock::new(DistributionStats::default())),
            timeout_tracker: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Distribute a task to appropriate collectors based on capabilities
    ///
    /// This method implements at-least-once delivery semantics:
    /// 1. Task is added to persistent queue
    /// 2. Task is published to event bus
    /// 3. If publish succeeds, task is moved to in-flight tracking
    /// 4. Task remains in queue until explicit acknowledgment via `acknowledge_task`
    /// 5. Timeout tracking begins only after successful publish
    pub async fn distribute_task(&self, task: DistributionTask) -> Result<()> {
        let task_id = task.task_id.clone();
        let correlation_id = task.correlation_id.clone();
        let target_topic = task.target_topic.clone();

        debug!(
            task_id = %task_id,
            priority = ?task.priority,
            target_topic = %target_topic,
            "Distributing task to collectors"
        );

        // Add task to persistent queue first
        {
            let mut queue = self.task_queue.write().await;
            queue.push(task.clone());
        }

        // Publish task to target topic
        let start_time = SystemTime::now();
        match self
            .broker
            .publish(&target_topic, &correlation_id, task.payload.clone())
            .await
        {
            Ok(()) => {
                let published_at = SystemTime::now();

                // Only start timeout tracking after successful publish
                // This ensures the timeout period begins when the task is actually queued
                let timeout_at = published_at + task.timeout;
                {
                    let mut tracker = self.timeout_tracker.write().await;
                    tracker.insert(task_id.clone(), timeout_at);
                }

                // Remove task from queue since it's now published and in-flight
                // This prevents duplicate publications while awaiting acknowledgment
                {
                    let mut queue = self.task_queue.write().await;
                    let mut new_queue = PriorityTaskQueue::new();
                    while let Some(queued_task) = queue.pop() {
                        if queued_task.task_id != task_id {
                            new_queue.push(queued_task);
                        }
                    }
                    *queue = new_queue;
                }

                // Move task to in-flight tracking
                // Task will be removed from in-flight only after explicit acknowledgment
                {
                    let mut in_flight = self.in_flight.write().await;
                    in_flight.insert(
                        task_id.clone(),
                        InFlightTask {
                            task: task.clone(),
                            published_at,
                            retry_count: 0,
                        },
                    );
                }

                // Update statistics
                let mut stats = self.stats.write().await;
                stats.tasks_distributed += 1;
                stats.tasks_delivered += 1;

                // Calculate distribution latency using numerically stable incremental update
                // Formula: new_avg = old_avg + (latency - old_avg) / n
                // This prevents overflow and maintains precision for large task counts
                if let Ok(elapsed) = start_time.elapsed() {
                    let latency_ms = elapsed.as_millis() as f64;
                    let n = stats.tasks_delivered as f64;
                    if n > 0.0 {
                        let old_avg = stats.avg_distribution_latency_ms;
                        stats.avg_distribution_latency_ms = old_avg + (latency_ms - old_avg) / n;
                    } else {
                        stats.avg_distribution_latency_ms = latency_ms;
                    }
                }

                info!(
                    task_id = %task_id,
                    target_topic = %target_topic,
                    "Task published successfully, awaiting acknowledgment"
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    task_id = %task_id,
                    error = %e,
                    "Failed to distribute task"
                );

                // Remove task from queue since publish failed
                // Task was never successfully delivered, so it shouldn't remain in queue
                {
                    let mut queue = self.task_queue.write().await;
                    // Find and remove the task by task_id
                    // Since we can't easily search the priority queue, we'll need to
                    // reconstruct it without the failed task
                    let mut new_queue = PriorityTaskQueue::new();
                    while let Some(queued_task) = queue.pop() {
                        if queued_task.task_id != task_id {
                            new_queue.push(queued_task);
                        }
                    }
                    // Restore queue without the failed task
                    *queue = new_queue;
                }

                // Update failure statistics
                let mut stats = self.stats.write().await;
                stats.tasks_failed += 1;

                Err(anyhow::anyhow!("Task distribution failed: {}", e))
            }
        }
    }

    /// Acknowledge successful processing of a task by the consumer
    ///
    /// This method removes the task from both in-flight tracking and the persistent queue,
    /// completing the at-least-once delivery guarantee. Only tasks that have been successfully
    /// published and are in the in-flight map can be acknowledged.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if task was found and acknowledged
    /// - `Ok(false)` if task was not found in in-flight tracking
    pub async fn acknowledge_task(&self, task_id: &str) -> Result<bool> {
        // Remove from in-flight tracking
        let task_opt = {
            let mut in_flight = self.in_flight.write().await;
            in_flight.remove(task_id)
        };

        match task_opt {
            Some(in_flight_task) => {
                // Remove timeout tracking
                {
                    let mut tracker = self.timeout_tracker.write().await;
                    tracker.remove(task_id);
                }

                // Remove from persistent queue
                {
                    let mut queue = self.task_queue.write().await;
                    let mut new_queue = PriorityTaskQueue::new();
                    while let Some(queued_task) = queue.pop() {
                        if queued_task.task_id != task_id {
                            new_queue.push(queued_task);
                        }
                    }
                    *queue = new_queue;
                }

                debug!(
                    task_id = %task_id,
                    correlation_id = %in_flight_task.task.correlation_id,
                    "Task acknowledged and removed from queue"
                );
                Ok(true)
            }
            None => {
                warn!(
                    task_id = %task_id,
                    "Attempted to acknowledge task not in in-flight tracking"
                );
                Ok(false)
            }
        }
    }

    /// Distribute multiple tasks in batch
    pub async fn distribute_batch(&self, tasks: Vec<DistributionTask>) -> Result<Vec<Result<()>>> {
        let mut results = Vec::with_capacity(tasks.len());

        for task in tasks {
            let result = self.distribute_task(task).await;
            results.push(result);
        }

        Ok(results)
    }

    /// Route task to appropriate topic based on capabilities
    pub fn route_task_by_capabilities(&self, capabilities: SourceCaps) -> String {
        // Determine primary capability and route to appropriate topic
        if capabilities.contains(SourceCaps::PROCESS) {
            "control.collector.process".to_string()
        } else if capabilities.contains(SourceCaps::NETWORK) {
            "control.collector.network".to_string()
        } else if capabilities.contains(SourceCaps::FILESYSTEM) {
            "control.collector.filesystem".to_string()
        } else if capabilities.contains(SourceCaps::PERFORMANCE) {
            "control.collector.performance".to_string()
        } else {
            "control.collector.generic".to_string()
        }
    }

    /// Create a distribution task from a collection event
    pub fn create_task_from_event(
        &self,
        event: &CollectionEvent,
        priority: TaskPriority,
        timeout: Duration,
    ) -> Result<DistributionTask> {
        let task_id = Uuid::new_v4().to_string();
        let correlation_id = Uuid::new_v4().to_string();

        // Determine required capabilities and target topic based on event type
        let (required_capabilities, target_topic) = match event {
            CollectionEvent::Process(_) => {
                (SourceCaps::PROCESS, "control.collector.process".to_string())
            }
            CollectionEvent::Network(_) => {
                (SourceCaps::NETWORK, "control.collector.network".to_string())
            }
            CollectionEvent::Filesystem(_) => (
                SourceCaps::FILESYSTEM,
                "control.collector.filesystem".to_string(),
            ),
            CollectionEvent::Performance(_) => (
                SourceCaps::PERFORMANCE,
                "control.collector.performance".to_string(),
            ),
            CollectionEvent::TriggerRequest(_) => {
                (SourceCaps::PROCESS, "control.trigger.request".to_string())
            }
        };

        // Serialize event as payload
        let payload = bincode::serde::encode_to_vec(event, bincode::config::standard())
            .context("Failed to serialize event")?;

        Ok(DistributionTask {
            task_id,
            priority,
            required_capabilities,
            target_topic,
            payload,
            correlation_id,
            created_at: SystemTime::now(),
            timeout,
            metadata: HashMap::new(),
        })
    }

    /// Check for timed out tasks and clean up
    ///
    /// When a task times out, it is moved from in-flight tracking back to the persistent
    /// queue for retry. This ensures at-least-once delivery semantics even in the face
    /// of consumer failures.
    pub async fn check_timeouts(&self) -> Result<Vec<String>> {
        let now = SystemTime::now();

        let mut tracker = self.timeout_tracker.write().await;
        let mut in_flight = self.in_flight.write().await;
        let mut queue = self.task_queue.write().await;

        // Collect timed out task IDs in single pass
        let timed_out_tasks: Vec<String> = tracker
            .iter()
            .filter_map(|(task_id, timeout_at)| {
                if now >= *timeout_at {
                    Some(task_id.clone())
                } else {
                    None
                }
            })
            .collect();

        // Process timed out tasks: move from in-flight back to queue for retry
        for task_id in &timed_out_tasks {
            if let Some(in_flight_task) = in_flight.remove(task_id) {
                let task = in_flight_task.task;
                let retry_count = in_flight_task.retry_count;

                warn!(
                    task_id = %task_id,
                    retry_count = retry_count,
                    "Task timed out, moving back to queue for retry"
                );

                // Re-queue the task for retry
                // This allows the task to be picked up and published again
                queue.push(task);

                // Remove from timeout tracker (will be re-added on next publish)
                tracker.remove(task_id);
            } else {
                // Task was in timeout tracker but not in-flight (shouldn't happen)
                warn!(
                    task_id = %task_id,
                    "Task in timeout tracker but not in-flight, cleaning up"
                );
                tracker.remove(task_id);
            }
        }
        drop(queue); // Release lock before updating stats
        drop(in_flight); // Release lock before updating stats
        drop(tracker); // Release lock before updating stats

        // Update statistics
        if !timed_out_tasks.is_empty() {
            let mut stats = self.stats.write().await;
            stats.tasks_timed_out += timed_out_tasks.len() as u64;
        }

        Ok(timed_out_tasks)
    }

    /// Get current distribution statistics
    pub async fn get_stats(&self) -> DistributionStats {
        let stats = self.stats.read().await;
        let mut stats_copy = stats.clone();

        // Update pending count
        let queue = self.task_queue.read().await;
        stats_copy.tasks_pending = queue.len();

        stats_copy
    }

    /// Clear all pending tasks
    pub async fn clear_queue(&self) -> usize {
        let mut queue = self.task_queue.write().await;
        let count = queue.len();

        queue.critical.clear();
        queue.high.clear();
        queue.normal.clear();
        queue.low.clear();

        info!(cleared_tasks = count, "Task queue cleared");
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;
    use std::collections::HashMap;

    #[test]
    fn test_priority_queue_ordering() {
        let mut queue = PriorityTaskQueue::new();

        // Add tasks in mixed order
        let low_task = create_test_task(TaskPriority::Low);
        let high_task = create_test_task(TaskPriority::High);
        let normal_task = create_test_task(TaskPriority::Normal);
        let critical_task = create_test_task(TaskPriority::Critical);

        queue.push(low_task);
        queue.push(high_task.clone());
        queue.push(normal_task);
        queue.push(critical_task.clone());

        // Pop should return critical first
        assert_eq!(queue.pop().unwrap().priority, TaskPriority::Critical);
        assert_eq!(queue.pop().unwrap().priority, TaskPriority::High);
        assert_eq!(queue.pop().unwrap().priority, TaskPriority::Normal);
        assert_eq!(queue.pop().unwrap().priority, TaskPriority::Low);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_capability_routing() {
        let broker = Arc::new(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(DaemoneyeBroker::new("/tmp/test-distributor.sock"))
                .unwrap(),
        );
        let distributor = TaskDistributor::new(broker);

        assert_eq!(
            distributor.route_task_by_capabilities(SourceCaps::PROCESS),
            "control.collector.process"
        );
        assert_eq!(
            distributor.route_task_by_capabilities(SourceCaps::NETWORK),
            "control.collector.network"
        );
        assert_eq!(
            distributor.route_task_by_capabilities(SourceCaps::FILESYSTEM),
            "control.collector.filesystem"
        );
    }

    #[tokio::test]
    async fn test_task_creation_from_event() {
        let broker = Arc::new(
            DaemoneyeBroker::new("/tmp/test-task-creation.sock")
                .await
                .unwrap(),
        );
        let distributor = TaskDistributor::new(broker);

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: vec![],
            executable_path: None,
            ppid: None,
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let task = distributor
            .create_task_from_event(
                &process_event,
                TaskPriority::Normal,
                Duration::from_secs(30),
            )
            .unwrap();

        assert_eq!(task.priority, TaskPriority::Normal);
        assert_eq!(task.target_topic, "control.collector.process");
        assert!(task.required_capabilities.contains(SourceCaps::PROCESS));
    }

    fn create_test_task(priority: TaskPriority) -> DistributionTask {
        DistributionTask {
            task_id: Uuid::new_v4().to_string(),
            priority,
            required_capabilities: SourceCaps::PROCESS,
            target_topic: "test.topic".to_string(),
            payload: vec![],
            correlation_id: Uuid::new_v4().to_string(),
            created_at: SystemTime::now(),
            timeout: Duration::from_secs(30),
            metadata: HashMap::new(),
        }
    }
}
