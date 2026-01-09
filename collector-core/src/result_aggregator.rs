//! Result aggregation system for multi-collector coordination.
//!
//! This module provides result collection, correlation, and aggregation from
//! multiple collectors publishing to domain-specific topics.

use crate::event::CollectionEvent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Aggregated result from multiple collectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedResult {
    /// Aggregation identifier
    pub aggregation_id: String,
    /// Correlation ID linking related results
    pub correlation_id: String,
    /// Individual results from collectors
    pub results: Vec<CollectorResult>,
    /// Aggregation timestamp
    pub aggregated_at: SystemTime,
    /// Aggregation status
    pub status: AggregationStatus,
    /// Result metadata
    pub metadata: HashMap<String, String>,
}

/// Individual collector result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorResult {
    /// Collector identifier
    pub collector_id: String,
    /// Result events
    pub events: Vec<CollectionEvent>,
    /// Result timestamp
    pub timestamp: SystemTime,
    /// Processing duration
    pub processing_duration: Duration,
    /// Result metadata
    pub metadata: HashMap<String, String>,
}

/// Aggregation status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregationStatus {
    /// Aggregation in progress
    Pending,
    /// Aggregation completed successfully
    Complete,
    /// Aggregation partially complete (some collectors failed)
    Partial,
    /// Aggregation timed out
    TimedOut,
    /// Aggregation failed
    Failed,
}

/// Aggregation configuration
#[derive(Debug, Clone)]
pub struct AggregationConfig {
    /// Maximum results to buffer per correlation ID
    pub max_buffer_size: usize,
    /// Aggregation timeout duration
    pub timeout: Duration,
    /// Minimum results required for completion
    pub min_results: usize,
    /// Enable result deduplication
    pub enable_deduplication: bool,
    /// Enable result ordering
    pub enable_ordering: bool,
}

impl Default for AggregationConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 1000,
            timeout: Duration::from_secs(30),
            min_results: 1,
            enable_deduplication: true,
            enable_ordering: true,
        }
    }
}

/// Result aggregator for multi-collector coordination
pub struct ResultAggregator {
    /// Aggregation configuration
    config: AggregationConfig,
    /// Pending aggregations by correlation ID
    pending: Arc<RwLock<HashMap<String, PendingAggregation>>>,
    /// Completed aggregations
    completed: Arc<RwLock<VecDeque<AggregatedResult>>>,
    /// Aggregation statistics
    stats: Arc<RwLock<AggregationStats>>,
    /// Result stream senders keyed by correlation ID
    result_senders: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<CollectorResult>>>>,
}

/// Pending aggregation state
#[derive(Debug)]
struct PendingAggregation {
    /// Correlation ID
    correlation_id: String,
    /// Collected results
    results: Vec<CollectorResult>,
    /// Expected collector count
    expected_count: Option<usize>,
    /// Aggregation start time
    started_at: SystemTime,
    /// Timeout deadline
    timeout_at: SystemTime,
}

/// Aggregation statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregationStats {
    /// Total aggregations started
    pub aggregations_started: u64,
    /// Total aggregations completed
    pub aggregations_completed: u64,
    /// Total aggregations timed out
    pub aggregations_timed_out: u64,
    /// Total aggregations failed
    pub aggregations_failed: u64,
    /// Average aggregation time (milliseconds)
    pub avg_aggregation_time_ms: f64,
    /// Total results aggregated
    pub total_results_aggregated: u64,
}

impl ResultAggregator {
    /// Create a new result aggregator
    pub fn new(config: AggregationConfig) -> Self {
        Self {
            config,
            pending: Arc::new(RwLock::new(HashMap::new())),
            completed: Arc::new(RwLock::new(VecDeque::new())),
            stats: Arc::new(RwLock::new(AggregationStats::default())),
            result_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a new aggregation
    pub async fn start_aggregation(
        &self,
        correlation_id: String,
        expected_count: Option<usize>,
    ) -> Result<()> {
        let now = SystemTime::now();
        let timeout_at = now + self.config.timeout;

        let aggregation = PendingAggregation {
            correlation_id: correlation_id.clone(),
            results: Vec::new(),
            expected_count,
            started_at: now,
            timeout_at,
        };

        let mut pending = self.pending.write().await;
        pending.insert(correlation_id.clone(), aggregation);

        let mut stats = self.stats.write().await;
        stats.aggregations_started += 1;

        debug!(
            correlation_id = %correlation_id,
            expected_count = ?expected_count,
            "Started result aggregation"
        );

        Ok(())
    }

    /// Add a result to an aggregation
    pub async fn add_result(
        &self,
        correlation_id: &str,
        result: CollectorResult,
    ) -> Result<Option<AggregatedResult>> {
        // Send result to stream if one exists
        {
            let senders = self.result_senders.read().await;
            if let Some(sender) = senders.get(correlation_id) {
                // Ignore send errors (receiver may have been dropped)
                let _ = sender.send(result.clone());
            }
        }

        let mut pending = self.pending.write().await;

        let aggregation = pending
            .get_mut(correlation_id)
            .context("Aggregation not found")?;

        // Add result
        aggregation.results.push(result);

        debug!(
            correlation_id = %correlation_id,
            result_count = aggregation.results.len(),
            "Added result to aggregation"
        );

        // Check if aggregation is complete
        let is_complete = if let Some(expected) = aggregation.expected_count {
            aggregation.results.len() >= expected
        } else {
            aggregation.results.len() >= self.config.min_results
        };

        if is_complete {
            // Remove from pending and create aggregated result
            let completed_aggregation = pending.remove(correlation_id).unwrap();
            drop(pending); // Release lock before processing

            // Clean up result sender when aggregation completes
            {
                let mut senders = self.result_senders.write().await;
                senders.remove(correlation_id);
            }

            let aggregated = self.finalize_aggregation(completed_aggregation).await?;
            Ok(Some(aggregated))
        } else {
            Ok(None)
        }
    }

    /// Finalize an aggregation
    async fn finalize_aggregation(
        &self,
        mut aggregation: PendingAggregation,
    ) -> Result<AggregatedResult> {
        let start_time = aggregation.started_at;

        // Apply deduplication if enabled
        if self.config.enable_deduplication {
            aggregation.results = self.deduplicate_results(aggregation.results);
        }

        // Apply ordering if enabled
        if self.config.enable_ordering {
            aggregation.results.sort_by_key(|r| r.timestamp);
        }

        let aggregated = AggregatedResult {
            aggregation_id: Uuid::new_v4().to_string(),
            correlation_id: aggregation.correlation_id.clone(),
            results: aggregation.results,
            aggregated_at: SystemTime::now(),
            status: AggregationStatus::Complete,
            metadata: HashMap::new(),
        };

        // Store completed aggregation
        {
            let mut completed = self.completed.write().await;
            completed.push_back(aggregated.clone());

            // Limit completed buffer size
            while completed.len() > self.config.max_buffer_size {
                completed.pop_front();
            }
        }

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.aggregations_completed += 1;
            stats.total_results_aggregated += aggregated.results.len() as u64;

            if let Ok(elapsed) = start_time.elapsed() {
                let elapsed_ms = elapsed.as_millis() as f64;
                stats.avg_aggregation_time_ms = (stats.avg_aggregation_time_ms
                    * (stats.aggregations_completed - 1) as f64
                    + elapsed_ms)
                    / stats.aggregations_completed as f64;
            }
        }

        info!(
            correlation_id = %aggregated.correlation_id,
            result_count = aggregated.results.len(),
            "Aggregation completed"
        );

        Ok(aggregated)
    }

    /// Deduplicate results based on collector ID and event content
    fn deduplicate_results(&self, results: Vec<CollectorResult>) -> Vec<CollectorResult> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut seen = HashMap::new();
        let mut deduplicated = Vec::new();

        for result in results {
            // Create a hash of the collector ID and event content
            let mut hasher = DefaultHasher::new();
            result.collector_id.hash(&mut hasher);

            // Hash each event's content by serializing to JSON
            // This ensures events with different content produce different hashes
            for event in &result.events {
                if let Ok(json) = serde_json::to_string(event) {
                    json.hash(&mut hasher);
                } else {
                    // Fallback: hash the debug representation if JSON serialization fails
                    format!("{:?}", event).hash(&mut hasher);
                }
            }

            let key = hasher.finish();
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(key) {
                e.insert(true);
                deduplicated.push(result);
            }
        }

        deduplicated
    }

    /// Check for timed out aggregations
    pub async fn check_timeouts(&self) -> Result<Vec<AggregatedResult>> {
        let now = SystemTime::now();
        let mut timed_out = Vec::new();

        let mut pending = self.pending.write().await;
        let mut to_remove = Vec::new();

        for (correlation_id, aggregation) in pending.iter() {
            if now >= aggregation.timeout_at {
                warn!(
                    correlation_id = %correlation_id,
                    result_count = aggregation.results.len(),
                    "Aggregation timed out"
                );
                to_remove.push(correlation_id.clone());
            }
        }

        // Remove timed out aggregations and clean up their senders
        for correlation_id in to_remove {
            // Clean up result sender
            {
                let mut senders = self.result_senders.write().await;
                senders.remove(&correlation_id);
            }

            if let Some(aggregation) = pending.remove(&correlation_id) {
                let mut aggregated = AggregatedResult {
                    aggregation_id: Uuid::new_v4().to_string(),
                    correlation_id: aggregation.correlation_id.clone(),
                    results: aggregation.results,
                    aggregated_at: SystemTime::now(),
                    status: AggregationStatus::TimedOut,
                    metadata: HashMap::new(),
                };

                // Apply deduplication and ordering even for timed out results
                if self.config.enable_deduplication {
                    aggregated.results = self.deduplicate_results(aggregated.results);
                }
                if self.config.enable_ordering {
                    aggregated.results.sort_by_key(|r| r.timestamp);
                }

                timed_out.push(aggregated);
            }
        }

        // Update statistics
        if !timed_out.is_empty() {
            let mut stats = self.stats.write().await;
            stats.aggregations_timed_out += timed_out.len() as u64;
        }

        Ok(timed_out)
    }

    /// Get completed aggregations
    pub async fn get_completed(&self, limit: usize) -> Vec<AggregatedResult> {
        let completed = self.completed.read().await;
        completed.iter().rev().take(limit).cloned().collect()
    }

    /// Get aggregation by correlation ID
    pub async fn get_aggregation(&self, correlation_id: &str) -> Option<AggregatedResult> {
        let completed = self.completed.read().await;
        completed
            .iter()
            .find(|a| a.correlation_id == correlation_id)
            .cloned()
    }

    /// Get aggregation statistics
    pub async fn get_stats(&self) -> AggregationStats {
        let stats = self.stats.read().await;
        let mut stats_copy = stats.clone();

        // Add pending count
        let pending = self.pending.read().await;
        stats_copy.aggregations_started = stats_copy.aggregations_started.max(pending.len() as u64);

        stats_copy
    }

    /// Create a result stream for continuous aggregation
    pub async fn create_result_stream(
        &self,
        correlation_id: String,
        expected_count: Option<usize>,
    ) -> Result<mpsc::UnboundedReceiver<CollectorResult>> {
        self.start_aggregation(correlation_id.clone(), expected_count)
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();

        // Store sender for this aggregation
        {
            let mut senders = self.result_senders.write().await;
            senders.insert(correlation_id, tx);
        }

        Ok(rx)
    }

    /// Clear completed aggregations
    pub async fn clear_completed(&self) -> usize {
        let mut completed = self.completed.write().await;
        let count = completed.len();
        completed.clear();
        info!(cleared_count = count, "Cleared completed aggregations");
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ProcessEvent;

    #[tokio::test]
    async fn test_aggregation_lifecycle() {
        let config = AggregationConfig::default();
        let aggregator = ResultAggregator::new(config);

        let correlation_id = "test-correlation-1".to_string();

        // Start aggregation
        aggregator
            .start_aggregation(correlation_id.clone(), Some(2))
            .await
            .unwrap();

        // Add first result
        let result1 = create_test_result("collector-1");
        let completed = aggregator
            .add_result(&correlation_id, result1)
            .await
            .unwrap();
        assert!(completed.is_none()); // Not complete yet

        // Add second result
        let result2 = create_test_result("collector-2");
        let completed = aggregator
            .add_result(&correlation_id, result2)
            .await
            .unwrap();
        assert!(completed.is_some()); // Should be complete

        let aggregated = completed.unwrap();
        assert_eq!(aggregated.results.len(), 2);
        assert_eq!(aggregated.status, AggregationStatus::Complete);
    }

    #[tokio::test]
    async fn test_aggregation_timeout() {
        let config = AggregationConfig {
            timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let aggregator = ResultAggregator::new(config);

        let correlation_id = "test-timeout-1".to_string();

        // Start aggregation
        aggregator
            .start_aggregation(correlation_id.clone(), Some(2))
            .await
            .unwrap();

        // Add only one result
        let result1 = create_test_result("collector-1");
        aggregator
            .add_result(&correlation_id, result1)
            .await
            .unwrap();

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Check for timeouts
        let timed_out = aggregator.check_timeouts().await.unwrap();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0].status, AggregationStatus::TimedOut);
        assert_eq!(timed_out[0].results.len(), 1);
    }

    #[tokio::test]
    async fn test_result_deduplication() {
        let config = AggregationConfig {
            enable_deduplication: true,
            ..Default::default()
        };
        let aggregator = ResultAggregator::new(config);

        let correlation_id = "test-dedup-1".to_string();

        // Start aggregation
        aggregator
            .start_aggregation(correlation_id.clone(), Some(3))
            .await
            .unwrap();

        // Create a shared timestamp for truly duplicate results
        let shared_timestamp = SystemTime::now();
        let shared_start_time = Some(shared_timestamp);

        // Add duplicate results - result1 and result2 should be identical
        let result1 = CollectorResult {
            collector_id: "collector-1".to_string(),
            events: vec![CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                name: "test-process".to_string(),
                command_line: vec![],
                executable_path: None,
                ppid: None,
                start_time: shared_start_time,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: true,
                file_exists: true,
                timestamp: shared_timestamp,
                platform_metadata: None,
            })],
            timestamp: shared_timestamp,
            processing_duration: Duration::from_millis(100),
            metadata: HashMap::new(),
        };

        // Create an identical duplicate
        let result2 = CollectorResult {
            collector_id: "collector-1".to_string(),
            events: vec![CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                name: "test-process".to_string(),
                command_line: vec![],
                executable_path: None,
                ppid: None,
                start_time: shared_start_time,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: true,
                file_exists: true,
                timestamp: shared_timestamp,
                platform_metadata: None,
            })],
            timestamp: shared_timestamp,
            processing_duration: Duration::from_millis(100),
            metadata: HashMap::new(),
        };

        let result3 = create_test_result("collector-2");

        aggregator
            .add_result(&correlation_id, result1)
            .await
            .unwrap();
        aggregator
            .add_result(&correlation_id, result2)
            .await
            .unwrap();
        let completed = aggregator
            .add_result(&correlation_id, result3)
            .await
            .unwrap();

        let aggregated = completed.unwrap();
        // Should have 2 results after deduplication (result1 and result2 are identical)
        assert_eq!(aggregated.results.len(), 2);
    }

    fn create_test_result(collector_id: &str) -> CollectorResult {
        CollectorResult {
            collector_id: collector_id.to_string(),
            events: vec![CollectionEvent::Process(ProcessEvent {
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
            })],
            timestamp: SystemTime::now(),
            processing_duration: Duration::from_millis(100),
            metadata: HashMap::new(),
        }
    }
}
