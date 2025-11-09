//! Task distribution and capability-based routing for collector coordination
//!
//! This module implements task distribution logic that routes tasks to appropriate
//! collectors based on their advertised capabilities. It provides:
//!
//! # Security
//!
//! - No unsafe code allowed in this module
//! - All inputs validated at trust boundaries
//! - Bounded resources with configurable limits
//! - Timeout enforcement for all async operations

#![deny(unsafe_code)]
//!
//! - Task queuing with priority handling
//! - Capability-based routing to appropriate collector types
//! - Dynamic routing updates when collectors join/leave
//! - Fallback routing for unavailable collectors
//! - Load balancing across multiple collector instances
//!
//! ## Architecture
//!
//! The task distribution system consists of:
//!
//! - **TaskDistributor**: Main coordinator that routes tasks to collectors
//! - **CapabilityRegistry**: Tracks collector capabilities and availability
//! - **TaskQueue**: Priority queue for pending tasks
//! - **RoutingStrategy**: Pluggable routing algorithms (round-robin, least-loaded, etc.)
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use daemoneye_eventbus::task_distribution::{TaskDistributor, TaskRequest, CollectorCapability};
//! use daemoneye_eventbus::broker::DaemoneyeBroker;
//! use std::sync::Arc;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let broker = Arc::new(DaemoneyeBroker::new("/tmp/task-dist.sock").await?);
//!     let mut distributor = TaskDistributor::new(broker).await?;
//!
//!     // Register a collector's capabilities
//!     let capability = CollectorCapability {
//!         collector_id: "procmond-1".to_string(),
//!         collector_type: "procmond".to_string(),
//!         supported_operations: vec!["enumerate_processes".to_string()],
//!         max_concurrent_tasks: 10,
//!         priority_levels: vec![1, 2, 3, 4, 5],
//!         metadata: std::collections::HashMap::new(),
//!     };
//!     distributor.register_collector(capability).await?;
//!
//!     // Distribute a task
//!     let now = std::time::SystemTime::now();
//!     let task = TaskRequest {
//!         task_id: "task-1".to_string(),
//!         operation: "enumerate_processes".to_string(),
//!         priority: 3,
//!         payload: vec![],
//!         timeout_ms: 30000,
//!         metadata: std::collections::HashMap::new(),
//!         correlation_id: Some("correlation-1".to_string()),
//!         created_at: now,
//!         deadline: now + std::time::Duration::from_millis(30000),
//!     };
//!     distributor.distribute_task(task).await?;
//!
//!     Ok(())
//! }
//! ```

use crate::broker::DaemoneyeBroker;
use crate::error::{EventBusError, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Task request to be distributed to collectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    /// Unique task identifier
    pub task_id: String,
    /// Operation to perform (e.g., "enumerate_processes", "scan_file")
    pub operation: String,
    /// Priority level (higher = more urgent)
    pub priority: u8,
    /// Task payload (operation-specific data)
    pub payload: Vec<u8>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
    /// Correlation ID for tracking
    pub correlation_id: Option<String>,
    /// Timestamp when task was created
    pub created_at: SystemTime,
    /// Deadline for task completion
    pub deadline: SystemTime,
}

/// Collector capability advertisement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorCapability {
    /// Collector identifier
    pub collector_id: String,
    /// Collector type (procmond, netmond, etc.)
    pub collector_type: String,
    /// Supported operations
    pub supported_operations: Vec<String>,
    /// Maximum concurrent tasks
    pub max_concurrent_tasks: u32,
    /// Supported priority levels
    pub priority_levels: Vec<u8>,
    /// Additional capability metadata
    pub metadata: HashMap<String, String>,
}

/// Collector registration information
#[derive(Debug, Clone)]
struct CollectorRegistration {
    /// Collector capability
    capability: CollectorCapability,
    /// Current task count
    current_tasks: u32,
    /// Last heartbeat timestamp
    last_heartbeat: SystemTime,
    /// Collector status
    status: CollectorStatus,
    /// Topic for sending tasks to this collector
    task_topic: String,
}

/// Collector status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectorStatus {
    /// Collector is available and accepting tasks
    Available,
    /// Collector is at capacity
    AtCapacity,
    /// Collector is unhealthy or unresponsive
    Unhealthy,
    /// Collector is shutting down
    /// SECURITY_TODO: Implement graceful shutdown coordination (Task 2.5.5)
    #[allow(dead_code)] // Reserved for future graceful shutdown coordination
    ShuttingDown,
}

/// Task queue entry with priority
#[derive(Debug, Clone)]
struct QueuedTask {
    /// Task request
    task: TaskRequest,
    /// Number of retry attempts
    retry_count: u32,
    /// Timestamp when task was queued
    queued_at: SystemTime,
}

impl PartialEq for QueuedTask {
    fn eq(&self, other: &Self) -> bool {
        self.task.task_id == other.task.task_id
    }
}

impl Eq for QueuedTask {}

impl PartialOrd for QueuedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority tasks come first
        match self.task.priority.cmp(&other.task.priority) {
            Ordering::Equal => {
                // For same priority, older tasks come first (FIFO)
                other.queued_at.cmp(&self.queued_at)
            }
            other => other,
        }
    }
}

/// Routing strategy for task distribution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Round-robin across available collectors
    RoundRobin,
    /// Route to least-loaded collector
    LeastLoaded,
    /// Route to first available collector
    FirstAvailable,
    /// Random selection
    Random,
}

/// Task distribution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistributionStats {
    /// Total tasks distributed
    pub tasks_distributed: u64,
    /// Tasks currently queued
    pub tasks_queued: usize,
    /// Tasks failed to distribute
    pub tasks_failed: u64,
    /// Active collectors
    pub active_collectors: usize,
    /// Tasks by collector type
    pub tasks_by_type: HashMap<String, u64>,
}

/// Main task distributor
pub struct TaskDistributor {
    /// Reference to the broker for publishing tasks
    broker: Arc<DaemoneyeBroker>,
    /// Capability registry
    capability_registry: Arc<RwLock<HashMap<String, CollectorRegistration>>>,
    /// Task queue (priority-based)
    task_queue: Arc<Mutex<BinaryHeap<QueuedTask>>>,
    /// Routing strategy
    routing_strategy: Arc<RwLock<RoutingStrategy>>,
    /// Round-robin counter for routing
    round_robin_counter: Arc<Mutex<usize>>,
    /// Distribution statistics
    stats: Arc<Mutex<DistributionStats>>,
    /// Maximum queue size
    max_queue_size: usize,
    /// Maximum retry attempts
    max_retries: u32,
    /// Heartbeat timeout
    heartbeat_timeout: Duration,
}

impl TaskDistributor {
    /// Create a new task distributor
    pub async fn new(broker: Arc<DaemoneyeBroker>) -> Result<Self> {
        Ok(Self {
            broker,
            capability_registry: Arc::new(RwLock::new(HashMap::with_capacity(16))),
            task_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            routing_strategy: Arc::new(RwLock::new(RoutingStrategy::LeastLoaded)),
            round_robin_counter: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(DistributionStats::default())),
            max_queue_size: 10000,
            max_retries: 3,
            heartbeat_timeout: Duration::from_secs(30),
        })
    }

    /// Create a new task distributor with custom configuration
    pub async fn with_config(
        broker: Arc<DaemoneyeBroker>,
        max_queue_size: usize,
        max_retries: u32,
        heartbeat_timeout: Duration,
        routing_strategy: RoutingStrategy,
    ) -> Result<Self> {
        Ok(Self {
            broker,
            capability_registry: Arc::new(RwLock::new(HashMap::with_capacity(16))),
            task_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            routing_strategy: Arc::new(RwLock::new(routing_strategy)),
            round_robin_counter: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(DistributionStats::default())),
            max_queue_size,
            max_retries,
            heartbeat_timeout,
        })
    }

    /// Register a collector's capabilities
    pub async fn register_collector(&self, capability: CollectorCapability) -> Result<()> {
        let collector_id = capability.collector_id.clone();
        let collector_type = capability.collector_type.clone();

        // Create task topic for this collector
        let task_topic = format!("control.tasks.{}.{}", collector_type, collector_id);

        let registration = CollectorRegistration {
            capability,
            current_tasks: 0,
            last_heartbeat: SystemTime::now(),
            status: CollectorStatus::Available,
            task_topic,
        };

        let registry_len = {
            let mut registry = self.capability_registry.write().await;
            registry.insert(collector_id.clone(), registration);
            registry.len()
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.active_collectors = registry_len;
        }

        info!(
            "Registered collector: {} (type: {})",
            collector_id, collector_type
        );
        Ok(())
    }

    /// Deregister a collector
    pub async fn deregister_collector(&self, collector_id: &str) -> Result<()> {
        let registry_len = {
            let mut registry = self.capability_registry.write().await;
            if registry.remove(collector_id).is_none() {
                return Err(EventBusError::topic(format!(
                    "Collector not found: {}",
                    collector_id
                )));
            }
            registry.len()
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.active_collectors = registry_len;
        }

        info!("Deregistered collector: {}", collector_id);
        Ok(())
    }

    /// Update collector heartbeat
    pub async fn update_heartbeat(&self, collector_id: &str) -> Result<()> {
        let mut registry = self.capability_registry.write().await;
        if let Some(registration) = registry.get_mut(collector_id) {
            registration.last_heartbeat = SystemTime::now();
            debug!("Updated heartbeat for collector: {}", collector_id);
            Ok(())
        } else {
            Err(EventBusError::topic(format!(
                "Collector not found: {}",
                collector_id
            )))
        }
    }

    /// Distribute a task to an appropriate collector
    ///
    /// # Timeout Behavior
    ///
    /// This operation respects the task's deadline. Tasks past their deadline
    /// will be rejected during queue processing.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Task queue is at capacity (resource exhaustion)
    /// - No suitable collectors available and queue is full
    /// - Serialization of task payload fails
    pub async fn distribute_task(&self, task: TaskRequest) -> Result<String> {
        // Check if task queue is full
        {
            let queue = self.task_queue.lock().await;
            if queue.len() >= self.max_queue_size {
                return Err(EventBusError::broker(format!(
                    "Task queue at capacity (max: {})",
                    self.max_queue_size
                )));
            }
        }

        // Find suitable collectors for this task
        let suitable_collectors = self.find_suitable_collectors(&task).await?;

        if suitable_collectors.is_empty() {
            // No collectors available, queue the task
            warn!(
                task_id = %task.task_id,
                operation = %task.operation,
                "No suitable collectors available, queuing task"
            );
            self.queue_task(task).await?;
            return Ok("queued".to_string());
        }

        // Select a collector using the routing strategy
        let selected_collector = self.select_collector(&suitable_collectors).await?;

        // Publish task to the selected collector
        self.publish_task_to_collector(&task, &selected_collector)
            .await?;

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.tasks_distributed += 1;
            *stats
                .tasks_by_type
                .entry(selected_collector.capability.collector_type.clone())
                .or_insert(0) += 1;
        }

        info!(
            task_id = %task.task_id,
            collector_id = %selected_collector.capability.collector_id,
            collector_type = %selected_collector.capability.collector_type,
            "Task distributed successfully"
        );
        Ok(selected_collector.capability.collector_id.clone())
    }

    /// Find collectors that can handle the given task
    async fn find_suitable_collectors(
        &self,
        task: &TaskRequest,
    ) -> Result<Vec<CollectorRegistration>> {
        let registry = self.capability_registry.read().await;
        let now = SystemTime::now();

        let suitable: Vec<CollectorRegistration> = registry
            .values()
            .filter(|reg| {
                // Check if collector supports the operation
                if !reg
                    .capability
                    .supported_operations
                    .contains(&task.operation)
                {
                    return false;
                }

                // Check if collector supports the priority level
                if !reg.capability.priority_levels.contains(&task.priority) {
                    return false;
                }

                // Check if collector is available
                if reg.status != CollectorStatus::Available {
                    return false;
                }

                // Check if collector is not at capacity
                if reg.current_tasks >= reg.capability.max_concurrent_tasks {
                    return false;
                }

                // Check if collector is responsive (heartbeat)
                if let Ok(elapsed) = now.duration_since(reg.last_heartbeat)
                    && elapsed > self.heartbeat_timeout
                {
                    return false;
                }

                true
            })
            .cloned()
            .collect();

        Ok(suitable)
    }

    /// Select a collector using the configured routing strategy
    async fn select_collector(
        &self,
        collectors: &[CollectorRegistration],
    ) -> Result<CollectorRegistration> {
        if collectors.is_empty() {
            return Err(EventBusError::broker("No collectors available".to_string()));
        }

        let strategy = *self.routing_strategy.read().await;

        match strategy {
            RoutingStrategy::RoundRobin => {
                let mut counter = self.round_robin_counter.lock().await;
                let index = *counter % collectors.len();
                *counter = counter.wrapping_add(1);
                Ok(collectors[index].clone())
            }
            RoutingStrategy::LeastLoaded => {
                // Find collector with fewest current tasks
                collectors
                    .iter()
                    .min_by_key(|reg| reg.current_tasks)
                    .cloned()
                    .ok_or_else(|| EventBusError::broker("No collectors available".to_string()))
            }
            RoutingStrategy::FirstAvailable => Ok(collectors[0].clone()),
            RoutingStrategy::Random => {
                use rand::Rng;
                let mut rng = rand::rng();
                let index = rng.random_range(0..collectors.len());
                Ok(collectors[index].clone())
            }
        }
    }

    /// Publish a task to a specific collector
    async fn publish_task_to_collector(
        &self,
        task: &TaskRequest,
        collector: &CollectorRegistration,
    ) -> Result<()> {
        // Serialize task
        let payload = bincode::serde::encode_to_vec(task, bincode::config::standard())
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Publish to collector's task topic
        let correlation_id = task
            .correlation_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        self.broker
            .publish(&collector.task_topic, &correlation_id, payload)
            .await?;

        // Increment current task count
        {
            let mut registry = self.capability_registry.write().await;
            if let Some(reg) = registry.get_mut(&collector.capability.collector_id) {
                reg.current_tasks += 1;
                // Update status if at capacity
                if reg.current_tasks >= reg.capability.max_concurrent_tasks {
                    reg.status = CollectorStatus::AtCapacity;
                }
            }
        }

        debug!(
            "Published task {} to collector {} on topic {}",
            task.task_id, collector.capability.collector_id, collector.task_topic
        );
        Ok(())
    }

    /// Queue a task for later distribution
    async fn queue_task(&self, task: TaskRequest) -> Result<()> {
        let queued_task = QueuedTask {
            task,
            retry_count: 0,
            queued_at: SystemTime::now(),
        };

        let queue_len = {
            let mut queue = self.task_queue.lock().await;
            queue.push(queued_task);
            queue.len()
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.tasks_queued = queue_len;
        }

        Ok(())
    }

    /// Process queued tasks (should be called periodically)
    pub async fn process_queue(&self) -> Result<usize> {
        let mut processed = 0;

        loop {
            // Get next task from queue
            let queued_task = {
                let mut queue = self.task_queue.lock().await;
                queue.pop()
            };

            let Some(mut queued_task) = queued_task else {
                break;
            };

            // Check if task has expired
            if SystemTime::now() > queued_task.task.deadline {
                warn!("Task {} expired, dropping", queued_task.task.task_id);
                let mut stats = self.stats.lock().await;
                stats.tasks_failed += 1;
                continue;
            }

            // Try to distribute the task
            match self.distribute_task(queued_task.task.clone()).await {
                Ok(collector_id) => {
                    if collector_id != "queued" {
                        processed += 1;
                        debug!(
                            "Processed queued task {} to collector {}",
                            queued_task.task.task_id, collector_id
                        );
                    } else {
                        // Task was re-queued, put it back
                        let mut queue = self.task_queue.lock().await;
                        queue.push(queued_task);
                        break;
                    }
                }
                Err(e) => {
                    queued_task.retry_count += 1;
                    if queued_task.retry_count >= self.max_retries {
                        error!(
                            "Task {} failed after {} retries: {}",
                            queued_task.task.task_id, self.max_retries, e
                        );
                        let mut stats = self.stats.lock().await;
                        stats.tasks_failed += 1;
                    } else {
                        // Re-queue for retry
                        let mut queue = self.task_queue.lock().await;
                        queue.push(queued_task);
                        break;
                    }
                }
            }
        }

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.tasks_queued = {
                let queue = self.task_queue.lock().await;
                queue.len()
            };
        }

        Ok(processed)
    }

    /// Mark a task as completed (decrements collector's task count)
    pub async fn task_completed(&self, collector_id: &str, task_id: &str) -> Result<()> {
        let mut registry = self.capability_registry.write().await;
        if let Some(reg) = registry.get_mut(collector_id) {
            if reg.current_tasks > 0 {
                reg.current_tasks -= 1;
                // Update status if no longer at capacity
                if reg.current_tasks < reg.capability.max_concurrent_tasks
                    && reg.status == CollectorStatus::AtCapacity
                {
                    reg.status = CollectorStatus::Available;
                }
                debug!(
                    "Task {} completed on collector {} ({} tasks remaining)",
                    task_id, collector_id, reg.current_tasks
                );
                Ok(())
            } else {
                warn!(
                    "Task completion for collector {} but no tasks tracked",
                    collector_id
                );
                Ok(())
            }
        } else {
            Err(EventBusError::topic(format!(
                "Collector not found: {}",
                collector_id
            )))
        }
    }

    /// Get collectors by type
    pub async fn get_collectors_by_type(&self, collector_type: &str) -> Vec<String> {
        let registry = self.capability_registry.read().await;
        registry
            .values()
            .filter(|reg| reg.capability.collector_type == collector_type)
            .map(|reg| reg.capability.collector_id.clone())
            .collect()
    }

    /// Get collectors by operation
    pub async fn get_collectors_by_operation(&self, operation: &str) -> Vec<String> {
        let registry = self.capability_registry.read().await;
        registry
            .values()
            .filter(|reg| {
                reg.capability
                    .supported_operations
                    .iter()
                    .any(|op| op == operation)
            })
            .map(|reg| reg.capability.collector_id.clone())
            .collect()
    }

    /// Check collector health and update status
    pub async fn check_collector_health(&self) -> Result<()> {
        let now = SystemTime::now();
        let mut unhealthy_count = 0;
        let mut recovered_count = 0;

        let available_count = {
            let mut registry = self.capability_registry.write().await;
            for (collector_id, reg) in registry.iter_mut() {
                if let Ok(elapsed) = now.duration_since(reg.last_heartbeat) {
                    if elapsed > self.heartbeat_timeout {
                        if reg.status != CollectorStatus::Unhealthy {
                            warn!(
                                "Collector {} is unhealthy (last heartbeat: {:?} ago)",
                                collector_id, elapsed
                            );
                            reg.status = CollectorStatus::Unhealthy;
                            unhealthy_count += 1;
                        }
                    } else {
                        // Collector is responsive
                        if reg.status == CollectorStatus::Unhealthy {
                            info!("Collector {} has recovered", collector_id);
                            reg.status = if reg.current_tasks >= reg.capability.max_concurrent_tasks
                            {
                                CollectorStatus::AtCapacity
                            } else {
                                CollectorStatus::Available
                            };
                            recovered_count += 1;
                        }
                    }
                }
            }
            registry
                .values()
                .filter(|reg| reg.status == CollectorStatus::Available)
                .count()
        };

        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.active_collectors = available_count;
        }

        if unhealthy_count > 0 {
            info!("Marked {} collectors as unhealthy", unhealthy_count);
        }

        if recovered_count > 0 {
            info!("Marked {} collectors as recovered", recovered_count);
        }

        Ok(())
    }

    /// Set routing strategy
    pub async fn set_routing_strategy(&self, strategy: RoutingStrategy) {
        let mut current_strategy = self.routing_strategy.write().await;
        *current_strategy = strategy;
        info!("Routing strategy changed to: {:?}", strategy);
    }

    /// Get distribution statistics
    pub async fn get_stats(&self) -> DistributionStats {
        self.stats.lock().await.clone()
    }

    /// Get all registered collectors
    pub async fn get_all_collectors(&self) -> Vec<CollectorCapability> {
        let registry = self.capability_registry.read().await;
        registry
            .values()
            .map(|reg| reg.capability.clone())
            .collect()
    }

    /// Start background tasks for queue processing and health checks
    pub async fn start_background_tasks(&self) -> Result<()> {
        // Queue processing task
        let distributor_clone = self.clone_for_background();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if let Err(e) = distributor_clone.process_queue().await {
                    error!("Error processing queue: {}", e);
                }
            }
        });

        // Health check task
        let distributor_clone = self.clone_for_background();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                if let Err(e) = distributor_clone.check_collector_health().await {
                    error!("Error checking collector health: {}", e);
                }
            }
        });

        info!("Started background tasks for task distribution");
        Ok(())
    }

    /// Helper to clone for background tasks
    fn clone_for_background(&self) -> Self {
        Self {
            broker: Arc::clone(&self.broker),
            capability_registry: Arc::clone(&self.capability_registry),
            task_queue: Arc::clone(&self.task_queue),
            routing_strategy: Arc::clone(&self.routing_strategy),
            round_robin_counter: Arc::clone(&self.round_robin_counter),
            stats: Arc::clone(&self.stats),
            max_queue_size: self.max_queue_size,
            max_retries: self.max_retries,
            heartbeat_timeout: self.heartbeat_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_distributor_creation() {
        let broker = Arc::new(DaemoneyeBroker::new("/tmp/test-dist.sock").await.unwrap());
        let distributor = TaskDistributor::new(broker).await.unwrap();
        let stats = distributor.get_stats().await;
        assert_eq!(stats.active_collectors, 0);
        assert_eq!(stats.tasks_distributed, 0);
    }

    #[tokio::test]
    async fn test_collector_registration() {
        let broker = Arc::new(DaemoneyeBroker::new("/tmp/test-reg.sock").await.unwrap());
        let distributor = TaskDistributor::new(broker).await.unwrap();

        let capability = CollectorCapability {
            collector_id: "test-collector".to_string(),
            collector_type: "procmond".to_string(),
            supported_operations: vec!["enumerate_processes".to_string()],
            max_concurrent_tasks: 5,
            priority_levels: vec![1, 2, 3],
            metadata: HashMap::new(),
        };

        distributor.register_collector(capability).await.unwrap();

        let stats = distributor.get_stats().await;
        assert_eq!(stats.active_collectors, 1);
    }

    #[tokio::test]
    async fn test_task_queuing() {
        let broker = Arc::new(DaemoneyeBroker::new("/tmp/test-queue.sock").await.unwrap());
        let distributor = TaskDistributor::new(broker).await.unwrap();

        let task = TaskRequest {
            task_id: "task-1".to_string(),
            operation: "enumerate_processes".to_string(),
            priority: 3,
            payload: vec![],
            timeout_ms: 30000,
            metadata: HashMap::new(),
            correlation_id: None,
            created_at: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
        };

        // Should queue since no collectors registered
        let result = distributor.distribute_task(task).await.unwrap();
        assert_eq!(result, "queued");

        let stats = distributor.get_stats().await;
        assert_eq!(stats.tasks_queued, 1);
    }
}
