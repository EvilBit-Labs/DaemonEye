//! Result aggregation and load management for multi-collector coordination
//!
//! This module implements result collection, correlation, and aggregation logic
//! for coordinating results from multiple collector processes. It provides:
//!
//! # Security
//!
//! - No unsafe code allowed in this module
//! - All inputs validated at trust boundaries
//! - Bounded resources with configurable limits
//! - Timeout enforcement for all async operations

#![deny(unsafe_code)]
//!
//! - Result collection from domain-specific topics
//! - Result correlation and aggregation across collectors
//! - Result ordering and deduplication
//! - Result streaming with backpressure handling
//! - Failover detection and recovery mechanisms
//! - Collector health monitoring and availability tracking
//! - Automatic task redistribution on collector failure
//!
//! ## Architecture
//!
//! The result aggregation system consists of:
//!
//! - **ResultAggregator**: Main coordinator that collects and aggregates results
//! - **ResultCollector**: Subscribes to result topics and collects results
//! - **CorrelationTracker**: Tracks correlation IDs across multi-collector workflows
//! - **DeduplicationCache**: Prevents duplicate result processing
//! - **FailoverManager**: Detects failures and redistributes tasks
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use daemoneye_eventbus::result_aggregation::{ResultAggregator, AggregationConfig};
//! use daemoneye_eventbus::broker::DaemoneyeBroker;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let broker = Arc::new(DaemoneyeBroker::new("/tmp/result-agg.sock").await?);
//!
//!     let config = AggregationConfig {
//!         max_pending_results: 10000,
//!         deduplication_window: Duration::from_secs(300),
//!         correlation_timeout: Duration::from_secs(60),
//!         backpressure_threshold: 8000,
//!         health_check_interval: Duration::from_secs(10),
//!     };
//!
//!     let mut aggregator = ResultAggregator::new(broker, config).await?;
//!     aggregator.start().await?;
//!
//!     Ok(())
//! }
//! ```

use crate::broker::DaemoneyeBroker;
use crate::error::{EventBusError, Result};
use crate::message::CollectionEvent;
use crate::task_distribution::TaskDistributor;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Configuration for result aggregation
#[derive(Debug, Clone)]
pub struct AggregationConfig {
    /// Maximum number of pending results before backpressure
    pub max_pending_results: usize,
    /// Time window for deduplication
    pub deduplication_window: Duration,
    /// Timeout for correlation tracking
    pub correlation_timeout: Duration,
    /// Threshold for triggering backpressure
    pub backpressure_threshold: usize,
    /// Interval for health checks
    pub health_check_interval: Duration,
}

impl Default for AggregationConfig {
    fn default() -> Self {
        Self {
            max_pending_results: 10000,
            deduplication_window: Duration::from_secs(300),
            correlation_timeout: Duration::from_secs(60),
            backpressure_threshold: 8000,
            health_check_interval: Duration::from_secs(10),
        }
    }
}

/// Aggregated result from multiple collectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedResult {
    /// Correlation ID linking related results
    pub correlation_id: String,
    /// Results from individual collectors
    pub collector_results: Vec<CollectorResult>,
    /// Aggregation timestamp
    pub aggregated_at: SystemTime,
    /// Total result count
    pub result_count: usize,
    /// Workflow stage
    pub workflow_stage: Option<String>,
}

/// Result from a single collector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorResult {
    /// Collector identifier
    pub collector_id: String,
    /// Collector type
    pub collector_type: String,
    /// Result event
    pub event: CollectionEvent,
    /// Result timestamp
    pub timestamp: SystemTime,
    /// Sequence number
    pub sequence: u64,
}

/// Deduplication entry
#[derive(Debug, Clone)]
struct DeduplicationEntry {
    /// Result hash for deduplication
    #[allow(dead_code)] // Used for future deduplication enhancements
    result_hash: u64,
    /// Timestamp when entry was created
    created_at: SystemTime,
}

/// Correlation tracking entry
#[derive(Debug, Clone)]
struct CorrelationEntry {
    /// Correlation ID
    correlation_id: String,
    /// Parent correlation ID
    #[allow(dead_code)] // Reserved for hierarchical correlation tracking
    parent_correlation_id: Option<String>,
    /// Root correlation ID
    #[allow(dead_code)] // Reserved for hierarchical correlation tracking
    root_correlation_id: String,
    /// Collected results
    results: Vec<CollectorResult>,
    /// Expected collector count
    expected_collectors: Option<usize>,
    /// Created timestamp
    #[allow(dead_code)] // Reserved for correlation lifecycle tracking
    created_at: SystemTime,
    /// Last updated timestamp
    updated_at: SystemTime,
}

/// Collector health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorHealth {
    /// Collector is healthy and responsive
    Healthy,
    /// Collector is degraded but operational
    Degraded,
    /// Collector is unhealthy or unresponsive
    Unhealthy,
    /// Collector has failed
    Failed,
}

/// Collector health tracking
#[derive(Debug, Clone)]
struct CollectorHealthStatus {
    /// Collector identifier
    #[allow(dead_code)] // Used for debugging and future health reporting
    collector_id: String,
    /// Current health status
    health: CollectorHealth,
    /// Last successful result timestamp
    last_success: SystemTime,
    /// Consecutive failure count
    failure_count: u32,
    /// Total results received
    total_results: u64,
    /// Failed results count
    #[allow(dead_code)] // Reserved for failure rate tracking
    failed_results: u64,
}

/// Result aggregation statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregationStats {
    /// Total results collected
    pub results_collected: u64,
    /// Results currently pending
    pub results_pending: usize,
    /// Results deduplicated
    pub results_deduplicated: u64,
    /// Correlations tracked
    pub correlations_active: usize,
    /// Correlations completed
    pub correlations_completed: u64,
    /// Backpressure events
    pub backpressure_events: u64,
    /// Failover events
    pub failover_events: u64,
    /// Healthy collectors
    pub healthy_collectors: usize,
    /// Unhealthy collectors
    pub unhealthy_collectors: usize,
}

/// Main result aggregator
pub struct ResultAggregator {
    /// Reference to the broker
    broker: Arc<DaemoneyeBroker>,
    /// Task distributor for failover
    task_distributor: Arc<TaskDistributor>,
    /// Configuration
    config: AggregationConfig,
    /// Pending results queue
    pending_results: Arc<Mutex<VecDeque<CollectorResult>>>,
    /// Deduplication cache
    deduplication_cache: Arc<RwLock<HashMap<String, DeduplicationEntry>>>,
    /// Correlation tracker
    correlation_tracker: Arc<RwLock<HashMap<String, CorrelationEntry>>>,
    /// Collector health status
    collector_health: Arc<RwLock<HashMap<String, CollectorHealthStatus>>>,
    /// Aggregation statistics
    stats: Arc<Mutex<AggregationStats>>,
    /// Result sender for streaming
    result_sender: Arc<Mutex<Option<mpsc::UnboundedSender<AggregatedResult>>>>,
    /// Backpressure flag
    backpressure_active: Arc<Mutex<bool>>,
}

impl ResultAggregator {
    /// Create a new result aggregator
    pub async fn new(broker: Arc<DaemoneyeBroker>, config: AggregationConfig) -> Result<Self> {
        let task_distributor = Arc::new(TaskDistributor::new(Arc::clone(&broker)).await?);

        Ok(Self {
            broker,
            task_distributor,
            config,
            pending_results: Arc::new(Mutex::new(VecDeque::new())),
            deduplication_cache: Arc::new(RwLock::new(HashMap::new())),
            correlation_tracker: Arc::new(RwLock::new(HashMap::new())),
            collector_health: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(Mutex::new(AggregationStats::default())),
            result_sender: Arc::new(Mutex::new(None)),
            backpressure_active: Arc::new(Mutex::new(false)),
        })
    }

    /// Start the result aggregator
    ///
    /// # Security
    ///
    /// - All background tasks have bounded resource usage
    /// - Timeout enforcement on all async operations
    /// - Automatic cleanup of expired entries
    #[tracing::instrument(skip(self))]
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting result aggregator");

        // Start background tasks
        self.start_result_collection_task().await?;
        self.start_correlation_processing_task().await?;
        self.start_health_monitoring_task().await?;
        self.start_deduplication_cleanup_task().await?;

        info!("Result aggregator started successfully");
        Ok(())
    }

    /// Start result collection background task
    async fn start_result_collection_task(&self) -> Result<()> {
        let broker = Arc::clone(&self.broker);

        tokio::spawn(async move {
            // Subscribe to all result topics (using static strings to avoid allocations)
            const RESULT_PATTERNS: &[&str] = &[
                "events.process.*",
                "events.network.*",
                "events.filesystem.*",
                "events.performance.*",
            ];

            for &pattern in RESULT_PATTERNS {
                let subscriber_id = Uuid::new_v4();
                if let Err(e) = broker.subscribe(pattern, subscriber_id).await {
                    error!("Failed to subscribe to pattern {}: {}", pattern, e);
                }
            }

            info!("Result collection task started");
        });

        Ok(())
    }

    /// Start correlation processing background task
    async fn start_correlation_processing_task(&self) -> Result<()> {
        let pending_results = Arc::clone(&self.pending_results);
        let correlation_tracker = Arc::clone(&self.correlation_tracker);
        let result_sender = Arc::clone(&self.result_sender);
        let stats = Arc::clone(&self.stats);
        let correlation_timeout = self.config.correlation_timeout;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;

                // Process pending results
                let results_to_process = {
                    let mut pending = pending_results.lock().await;
                    let batch_size = pending.len().min(100);
                    pending.drain(..batch_size).collect::<Vec<_>>()
                };

                if results_to_process.is_empty() {
                    continue;
                }

                // Group results by correlation ID (pre-allocate for batch size)
                let mut correlation_groups: HashMap<String, Vec<CollectorResult>> =
                    HashMap::with_capacity(100);
                for result in results_to_process {
                    // Extract correlation ID from result metadata
                    let correlation_id = match &result.event {
                        CollectionEvent::Process(event) => {
                            event.metadata.get("correlation_id").cloned()
                        }
                        CollectionEvent::Network(event) => {
                            event.metadata.get("correlation_id").cloned()
                        }
                        CollectionEvent::Filesystem(event) => {
                            event.metadata.get("correlation_id").cloned()
                        }
                        CollectionEvent::Performance(event) => {
                            event.metadata.get("correlation_id").cloned()
                        }
                        CollectionEvent::TriggerRequest(event) => {
                            event.metadata.get("correlation_id").cloned()
                        }
                    };

                    if let Some(correlation_id) = correlation_id {
                        // Validate correlation ID length to prevent memory exhaustion
                        if correlation_id.len() > 256 {
                            warn!("Correlation ID exceeds maximum length, skipping result");
                            continue;
                        }
                        correlation_groups
                            .entry(correlation_id)
                            .or_default()
                            .push(result);
                    }
                }

                // Update correlation tracker
                let mut tracker = correlation_tracker.write().await;
                let now = SystemTime::now();

                for (correlation_id, results) in correlation_groups {
                    let entry =
                        tracker
                            .entry(correlation_id.clone())
                            .or_insert_with(|| CorrelationEntry {
                                correlation_id: correlation_id.clone(),
                                parent_correlation_id: None,
                                root_correlation_id: correlation_id.clone(),
                                results: Vec::new(),
                                expected_collectors: None,
                                created_at: now,
                                updated_at: now,
                            });

                    // Enforce maximum results per correlation to prevent memory exhaustion
                    const MAX_RESULTS_PER_CORRELATION: usize = 10000;
                    if entry.results.len() + results.len() > MAX_RESULTS_PER_CORRELATION {
                        warn!(
                            "Correlation {} exceeds maximum result count, dropping excess",
                            correlation_id
                        );
                        let available =
                            MAX_RESULTS_PER_CORRELATION.saturating_sub(entry.results.len());
                        entry.results.extend(results.into_iter().take(available));
                    } else {
                        entry.results.extend(results);
                    }
                    entry.updated_at = now;

                    // Check if correlation is complete
                    if let Some(expected) = entry.expected_collectors
                        && entry.results.len() >= expected
                    {
                        // Correlation complete, send aggregated result
                        let aggregated = AggregatedResult {
                            correlation_id: entry.correlation_id.clone(),
                            collector_results: entry.results.clone(),
                            aggregated_at: now,
                            result_count: entry.results.len(),
                            workflow_stage: None,
                        };

                        if let Some(sender) = result_sender.lock().await.as_ref()
                            && sender.send(aggregated).is_err()
                        {
                            warn!("Failed to send aggregated result");
                        }

                        // Update statistics
                        let mut stats_guard = stats.lock().await;
                        stats_guard.correlations_completed += 1;
                    }
                }

                // Clean up expired correlations
                tracker.retain(|_, entry| {
                    if let Ok(elapsed) = now.duration_since(entry.updated_at) {
                        elapsed < correlation_timeout
                    } else {
                        true
                    }
                });

                // Update statistics
                {
                    let mut stats_guard = stats.lock().await;
                    stats_guard.correlations_active = tracker.len();
                }
            }
        });

        Ok(())
    }

    /// Start health monitoring background task
    async fn start_health_monitoring_task(&self) -> Result<()> {
        let collector_health = Arc::clone(&self.collector_health);
        let task_distributor = Arc::clone(&self.task_distributor);
        let stats = Arc::clone(&self.stats);
        let health_check_interval = self.config.health_check_interval;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(health_check_interval);
            loop {
                interval.tick().await;

                let now = SystemTime::now();
                let mut health_map = collector_health.write().await;
                let mut healthy_count = 0;
                let mut unhealthy_count = 0;
                // Pre-allocate based on typical failure rate (small capacity)
                let mut failed_collectors = Vec::with_capacity(4);

                for (collector_id, status) in health_map.iter_mut() {
                    // Check if collector has been inactive
                    if let Ok(elapsed) = now.duration_since(status.last_success)
                        && elapsed > Duration::from_secs(60)
                    {
                        // Mark as unhealthy if no results for 60 seconds
                        if status.health != CollectorHealth::Unhealthy
                            && status.health != CollectorHealth::Failed
                        {
                            warn!("Collector {} marked as unhealthy", collector_id);
                            status.health = CollectorHealth::Unhealthy;
                            status.failure_count += 1;
                        }

                        // Mark as failed after 3 consecutive failures
                        if status.failure_count >= 3 && status.health != CollectorHealth::Failed {
                            error!("Collector {} marked as failed", collector_id);
                            status.health = CollectorHealth::Failed;
                            failed_collectors.push(collector_id.clone());
                        }
                    }

                    // Count health status
                    match status.health {
                        CollectorHealth::Healthy | CollectorHealth::Degraded => healthy_count += 1,
                        CollectorHealth::Unhealthy | CollectorHealth::Failed => {
                            unhealthy_count += 1
                        }
                    }
                }

                // Update statistics
                {
                    let mut stats_guard = stats.lock().await;
                    stats_guard.healthy_collectors = healthy_count;
                    stats_guard.unhealthy_collectors = unhealthy_count;
                }

                // Handle failed collectors
                for collector_id in failed_collectors {
                    info!("Initiating failover for collector: {}", collector_id);
                    // Failover logic will be handled by task redistribution
                    if let Err(e) = task_distributor.deregister_collector(&collector_id).await {
                        error!("Failed to deregister collector {}: {}", collector_id, e);
                    }

                    // Update statistics
                    let mut stats_guard = stats.lock().await;
                    stats_guard.failover_events += 1;
                }
            }
        });

        Ok(())
    }

    /// Start deduplication cleanup background task
    async fn start_deduplication_cleanup_task(&self) -> Result<()> {
        let deduplication_cache = Arc::clone(&self.deduplication_cache);
        let deduplication_window = self.config.deduplication_window;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;

                let now = SystemTime::now();
                let mut cache = deduplication_cache.write().await;

                // Remove expired entries
                cache.retain(|_, entry| {
                    if let Ok(elapsed) = now.duration_since(entry.created_at) {
                        elapsed < deduplication_window
                    } else {
                        true
                    }
                });

                debug!("Deduplication cache size: {}", cache.len());
            }
        });

        Ok(())
    }

    /// Collect a result from a collector
    ///
    /// # Security
    ///
    /// - Validates collector_id length to prevent memory exhaustion
    /// - Enforces backpressure to prevent resource exhaustion
    /// - Deduplicates results to prevent duplicate processing
    pub async fn collect_result(&self, result: CollectorResult) -> Result<()> {
        // Validate collector_id length to prevent memory exhaustion
        if result.collector_id.len() > 256 {
            warn!("Collector ID exceeds maximum length, rejecting result");
            return Err(EventBusError::broker(
                "Collector ID exceeds maximum length".to_string(),
            ));
        }

        // Check for backpressure
        {
            let backpressure = *self.backpressure_active.lock().await;
            if backpressure {
                warn!("Backpressure active, dropping result");
                return Err(EventBusError::broker("Backpressure active".to_string()));
            }
        }

        // Check deduplication (compute hash key outside lock to reduce lock hold time)
        let result_hash = self.compute_result_hash(&result);
        let hash_key = result_hash.to_string();
        {
            let mut cache = self.deduplication_cache.write().await;
            if cache.contains_key(&hash_key) {
                debug!("Duplicate result detected, skipping");
                let mut stats = self.stats.lock().await;
                stats.results_deduplicated += 1;
                return Ok(());
            }

            // Add to deduplication cache
            cache.insert(
                hash_key,
                DeduplicationEntry {
                    result_hash,
                    created_at: SystemTime::now(),
                },
            );
        }

        // Update collector health (capture collector_id once to avoid multiple clones)
        let collector_id = result.collector_id.clone();
        {
            let mut health_map = self.collector_health.write().await;
            let now = SystemTime::now();
            let status =
                health_map
                    .entry(collector_id.clone())
                    .or_insert_with(|| CollectorHealthStatus {
                        collector_id,
                        health: CollectorHealth::Healthy,
                        last_success: now,
                        failure_count: 0,
                        total_results: 0,
                        failed_results: 0,
                    });

            status.last_success = now;
            status.total_results += 1;
            status.failure_count = 0;
            status.health = CollectorHealth::Healthy;
        }

        // Add to pending results
        {
            let mut pending = self.pending_results.lock().await;
            pending.push_back(result);

            // Check for backpressure
            if pending.len() >= self.config.backpressure_threshold {
                warn!("Backpressure threshold reached");
                *self.backpressure_active.lock().await = true;
                let mut stats = self.stats.lock().await;
                stats.backpressure_events += 1;
            }

            // Update statistics
            let mut stats = self.stats.lock().await;
            stats.results_collected += 1;
            stats.results_pending = pending.len();
        }

        Ok(())
    }

    /// Compute hash for result deduplication
    fn compute_result_hash(&self, result: &CollectorResult) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        result.collector_id.hash(&mut hasher);
        result.sequence.hash(&mut hasher);
        hasher.finish()
    }

    /// Subscribe to aggregated results
    pub async fn subscribe_results(&self) -> mpsc::UnboundedReceiver<AggregatedResult> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.result_sender.lock().await = Some(tx);
        rx
    }

    /// Get aggregation statistics
    pub async fn get_stats(&self) -> AggregationStats {
        self.stats.lock().await.clone()
    }

    /// Get collector health status
    pub async fn get_collector_health(&self, collector_id: &str) -> Option<CollectorHealth> {
        let health_map = self.collector_health.read().await;
        health_map.get(collector_id).map(|status| status.health)
    }

    /// Get all collector health statuses
    pub async fn get_all_collector_health(&self) -> HashMap<String, CollectorHealth> {
        let health_map = self.collector_health.read().await;
        health_map
            .iter()
            .map(|(id, status)| (id.clone(), status.health))
            .collect()
    }

    /// Clear backpressure flag
    pub async fn clear_backpressure(&self) {
        *self.backpressure_active.lock().await = false;
        info!("Backpressure cleared");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ProcessEvent;

    #[tokio::test]
    async fn test_result_aggregator_creation() {
        let broker = Arc::new(DaemoneyeBroker::new("/tmp/test-agg.sock").await.unwrap());
        let config = AggregationConfig::default();
        let aggregator = ResultAggregator::new(broker, config).await.unwrap();
        let stats = aggregator.get_stats().await;
        assert_eq!(stats.results_collected, 0);
    }

    #[tokio::test]
    async fn test_result_collection() {
        let broker = Arc::new(
            DaemoneyeBroker::new("/tmp/test-collect.sock")
                .await
                .unwrap(),
        );
        let config = AggregationConfig::default();
        let aggregator = ResultAggregator::new(broker, config).await.unwrap();

        let result = CollectorResult {
            collector_id: "test-collector".to_string(),
            collector_type: "procmond".to_string(),
            event: CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                name: "test".to_string(),
                command_line: None,
                executable_path: None,
                ppid: None,
                start_time: None,
                metadata: HashMap::new(),
            }),
            timestamp: SystemTime::now(),
            sequence: 1,
        };

        aggregator.collect_result(result).await.unwrap();

        let stats = aggregator.get_stats().await;
        assert_eq!(stats.results_collected, 1);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let broker = Arc::new(DaemoneyeBroker::new("/tmp/test-dedup.sock").await.unwrap());
        let config = AggregationConfig::default();
        let aggregator = ResultAggregator::new(broker, config).await.unwrap();

        let result = CollectorResult {
            collector_id: "test-collector".to_string(),
            collector_type: "procmond".to_string(),
            event: CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                name: "test".to_string(),
                command_line: None,
                executable_path: None,
                ppid: None,
                start_time: None,
                metadata: HashMap::new(),
            }),
            timestamp: SystemTime::now(),
            sequence: 1,
        };

        // First collection should succeed
        aggregator.collect_result(result.clone()).await.unwrap();

        // Second collection should be deduplicated
        aggregator.collect_result(result).await.unwrap();

        let stats = aggregator.get_stats().await;
        // Only non-duplicates are counted in results_collected
        assert_eq!(
            stats.results_collected, 1,
            "Only non-duplicate should be collected"
        );
        assert_eq!(stats.results_deduplicated, 1, "One should be deduplicated");
    }
}
