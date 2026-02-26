//! Load balancing and failover for multi-collector coordination.
//!
//! This module provides load balancing algorithms, failover detection, and
//! automatic task redistribution for collector instances.

use crate::{
    capability_router::{CollectorCapability, CollectorHealthStatus},
    source::SourceCaps,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Load balancing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadBalancingStrategy {
    /// Round-robin distribution
    RoundRobin,
    /// Least connections (least loaded)
    LeastConnections,
    /// Weighted round-robin based on capacity
    WeightedRoundRobin,
    /// Random selection
    Random,
}

/// Load balancer configuration
#[derive(Debug, Clone)]
pub struct LoadBalancerConfig {
    /// Load balancing strategy
    pub strategy: LoadBalancingStrategy,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Failover threshold (consecutive failures)
    pub failover_threshold: u32,
    /// Recovery check interval
    pub recovery_check_interval: Duration,
    /// Maximum tasks per collector
    pub max_tasks_per_collector: usize,
}

impl Default for LoadBalancerConfig {
    fn default() -> Self {
        Self {
            strategy: LoadBalancingStrategy::LeastConnections,
            health_check_interval: Duration::from_secs(10),
            failover_threshold: 3,
            recovery_check_interval: Duration::from_secs(30),
            max_tasks_per_collector: 100,
        }
    }
}

/// Collector load information
#[derive(Debug, Clone)]
struct CollectorLoad {
    /// Collector identifier
    #[allow(dead_code)] // Used for tracking in load balancing algorithms
    collector_id: String,
    /// Current task count
    current_tasks: usize,
    /// Total tasks processed
    total_tasks: u64,
    /// Consecutive failures
    consecutive_failures: u32,
    /// Last health check
    last_health_check: SystemTime,
    /// Collector weight (for weighted strategies)
    weight: f64,
}

/// Load balancer for multi-collector coordination
pub struct LoadBalancer {
    /// Configuration
    config: LoadBalancerConfig,
    /// Collector load tracking
    loads: Arc<RwLock<HashMap<String, CollectorLoad>>>,
    /// Round-robin counter
    round_robin_counter: Arc<RwLock<usize>>,
    /// Failover events
    failover_events: Arc<RwLock<Vec<FailoverEvent>>>,
    /// Load balancing statistics
    stats: Arc<RwLock<LoadBalancingStats>>,
}

/// Failover event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEvent {
    /// Event identifier
    pub event_id: String,
    /// Failed collector ID
    pub failed_collector_id: String,
    /// Failover collector ID
    pub failover_collector_id: String,
    /// Failover timestamp
    pub timestamp: SystemTime,
    /// Reason for failover
    pub reason: String,
    /// Tasks redistributed
    pub tasks_redistributed: usize,
}

/// Load balancing statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadBalancingStats {
    /// Total tasks distributed
    pub total_tasks_distributed: u64,
    /// Total failover events
    pub total_failovers: u64,
    /// Total tasks redistributed
    pub total_tasks_redistributed: u64,
    /// Average load per collector
    pub avg_load_per_collector: f64,
    /// Maximum load on any collector
    pub max_load: usize,
}

impl LoadBalancer {
    /// Create a new load balancer
    pub fn new(config: LoadBalancerConfig) -> Self {
        Self {
            config,
            loads: Arc::new(RwLock::new(HashMap::new())),
            round_robin_counter: Arc::new(RwLock::new(0)),
            failover_events: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(LoadBalancingStats::default())),
        }
    }

    /// Register a collector with the load balancer
    pub async fn register_collector(&self, collector_id: String, weight: f64) -> Result<()> {
        let load = CollectorLoad {
            collector_id: collector_id.clone(),
            current_tasks: 0,
            total_tasks: 0,
            consecutive_failures: 0,
            last_health_check: SystemTime::now(),
            weight,
        };

        let mut loads = self.loads.write().await;
        loads.insert(collector_id.clone(), load);

        info!(
            collector_id = %collector_id,
            weight = weight,
            "Registered collector with load balancer"
        );

        Ok(())
    }

    /// Deregister a collector
    pub async fn deregister_collector(&self, collector_id: &str) -> Result<()> {
        let mut loads = self.loads.write().await;
        loads.remove(collector_id);

        info!(
            collector_id = %collector_id,
            "Deregistered collector from load balancer"
        );

        Ok(())
    }

    /// Select a collector for task distribution
    pub async fn select_collector(
        &self,
        required_caps: SourceCaps,
        available_collectors: &[CollectorCapability],
    ) -> Result<String> {
        if available_collectors.is_empty() {
            return Err(anyhow::anyhow!("No collectors available"));
        }

        // Filter collectors by health and capacity
        let healthy_collectors: Vec<&CollectorCapability> = available_collectors
            .iter()
            .filter(|c| {
                c.health_status == CollectorHealthStatus::Healthy
                    && c.capabilities.contains(required_caps)
            })
            .collect();

        if healthy_collectors.is_empty() {
            return Err(anyhow::anyhow!("No healthy collectors available"));
        }

        // Select based on strategy
        let selected = match self.config.strategy {
            LoadBalancingStrategy::RoundRobin => {
                self.select_round_robin(&healthy_collectors).await?
            }
            LoadBalancingStrategy::LeastConnections => {
                self.select_least_connections(&healthy_collectors).await?
            }
            LoadBalancingStrategy::WeightedRoundRobin => {
                self.select_weighted_round_robin(&healthy_collectors)
                    .await?
            }
            LoadBalancingStrategy::Random => self.select_random(&healthy_collectors).await?,
        };

        // Update load tracking
        self.increment_task_count(&selected).await?;

        Ok(selected)
    }

    /// Round-robin selection
    async fn select_round_robin(&self, collectors: &[&CollectorCapability]) -> Result<String> {
        let mut counter = self.round_robin_counter.write().await;
        let index = *counter % collectors.len();
        *counter = (*counter + 1) % collectors.len();

        Ok(collectors[index].collector_id.clone())
    }

    /// Least connections selection
    async fn select_least_connections(
        &self,
        collectors: &[&CollectorCapability],
    ) -> Result<String> {
        let loads = self.loads.read().await;

        let mut min_load = usize::MAX;
        let mut selected = None;

        for collector in collectors {
            let load = loads
                .get(&collector.collector_id)
                .map(|l| l.current_tasks)
                .unwrap_or(0);

            if load < min_load {
                min_load = load;
                selected = Some(collector.collector_id.clone());
            }
        }

        selected.context("No collector selected")
    }

    /// Weighted round-robin selection
    async fn select_weighted_round_robin(
        &self,
        collectors: &[&CollectorCapability],
    ) -> Result<String> {
        let loads = self.loads.read().await;

        // Calculate weighted scores
        let mut best_score = f64::MIN;
        let mut selected = None;

        for collector in collectors {
            let load = loads.get(&collector.collector_id);
            let weight = load.map(|l| l.weight).unwrap_or(1.0);
            let current_tasks = load.map(|l| l.current_tasks).unwrap_or(0);

            // Score = weight / (current_tasks + 1)
            let score = weight / (current_tasks + 1) as f64;

            if score > best_score {
                best_score = score;
                selected = Some(collector.collector_id.clone());
            }
        }

        selected.context("No collector selected")
    }

    /// Random selection
    async fn select_random(&self, collectors: &[&CollectorCapability]) -> Result<String> {
        use rand::Rng;
        let mut rng = rand::rng();
        let index = rng.random_range(0..collectors.len());
        Ok(collectors[index].collector_id.clone())
    }

    /// Increment task count for a collector
    async fn increment_task_count(&self, collector_id: &str) -> Result<()> {
        let mut loads = self.loads.write().await;

        if let Some(load) = loads.get_mut(collector_id) {
            load.current_tasks += 1;
            load.total_tasks += 1;

            // Check capacity
            if load.current_tasks > self.config.max_tasks_per_collector {
                warn!(
                    collector_id = %collector_id,
                    current_tasks = load.current_tasks,
                    max_tasks = self.config.max_tasks_per_collector,
                    "Collector exceeding maximum task capacity"
                );
            }
        }

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_tasks_distributed += 1;

        Ok(())
    }

    /// Decrement task count for a collector
    pub async fn decrement_task_count(&self, collector_id: &str) -> Result<()> {
        let mut loads = self.loads.write().await;

        if let Some(load) = loads.get_mut(collector_id) {
            load.current_tasks = load.current_tasks.saturating_sub(1);
        }

        Ok(())
    }

    /// Record a collector failure
    pub async fn record_failure(&self, collector_id: &str) -> Result<bool> {
        let mut loads = self.loads.write().await;

        if let Some(load) = loads.get_mut(collector_id) {
            load.consecutive_failures += 1;

            warn!(
                collector_id = %collector_id,
                consecutive_failures = load.consecutive_failures,
                "Recorded collector failure"
            );

            // Check if failover threshold reached
            if load.consecutive_failures >= self.config.failover_threshold {
                info!(
                    collector_id = %collector_id,
                    "Failover threshold reached, triggering failover"
                );
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Record a collector success (reset failure count)
    pub async fn record_success(&self, collector_id: &str) -> Result<()> {
        let mut loads = self.loads.write().await;

        if let Some(load) = loads.get_mut(collector_id) {
            load.consecutive_failures = 0;
            load.last_health_check = SystemTime::now();
        }

        Ok(())
    }

    /// Trigger failover for a collector
    ///
    /// # Invariant
    ///
    /// The selected failover collector is guaranteed to be different from the failed collector.
    /// The failed collector is pre-filtered from available_collectors before selection.
    pub async fn trigger_failover(
        &self,
        failed_collector_id: &str,
        available_collectors: &[CollectorCapability],
        required_caps: SourceCaps,
    ) -> Result<FailoverEvent> {
        // Pre-filter available collectors to exclude the failed one
        // This ensures the selected failover collector is not the failed collector
        let filtered_collectors: Vec<CollectorCapability> = available_collectors
            .iter()
            .filter(|cap| cap.collector_id != failed_collector_id)
            .cloned()
            .collect();

        if filtered_collectors.is_empty() {
            return Err(anyhow::anyhow!(
                "No available collectors for failover (all collectors filtered out)"
            ));
        }

        // Select a failover collector from the filtered list
        let failover_collector_id = self
            .select_collector(required_caps, &filtered_collectors)
            .await?;

        // Validate that the selected collector is not the failed one (defensive check)
        if failover_collector_id == failed_collector_id {
            return Err(anyhow::anyhow!(
                "Selected failover collector matches failed collector (this should not happen)"
            ));
        }

        // Get current task count for redistribution
        let tasks_to_redistribute = {
            let loads = self.loads.read().await;
            loads
                .get(failed_collector_id)
                .map(|l| l.current_tasks)
                .unwrap_or(0)
        };

        // Create failover event
        let event = FailoverEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            failed_collector_id: failed_collector_id.to_string(),
            failover_collector_id: failover_collector_id.clone(),
            timestamp: SystemTime::now(),
            reason: "Consecutive failures exceeded threshold".to_string(),
            tasks_redistributed: tasks_to_redistribute,
        };

        // Record failover event
        {
            let mut events = self.failover_events.write().await;
            events.push(event.clone());
        }

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_failovers += 1;
            stats.total_tasks_redistributed += tasks_to_redistribute as u64;
        }

        // Reset failed collector's task count
        {
            let mut loads = self.loads.write().await;
            if let Some(load) = loads.get_mut(failed_collector_id) {
                load.current_tasks = 0;
            }
        }

        info!(
            failed_collector = %failed_collector_id,
            failover_collector = %failover_collector_id,
            tasks_redistributed = tasks_to_redistribute,
            "Failover completed"
        );

        Ok(event)
    }

    /// Get load balancing statistics
    pub async fn get_stats(&self) -> LoadBalancingStats {
        let loads = self.loads.read().await;
        let mut stats = self.stats.read().await.clone();

        // Calculate current statistics
        if !loads.is_empty() {
            let total_load: usize = loads.values().map(|l| l.current_tasks).sum();
            stats.avg_load_per_collector = total_load as f64 / loads.len() as f64;
            stats.max_load = loads.values().map(|l| l.current_tasks).max().unwrap_or(0);
        }

        stats
    }

    /// Get failover events
    pub async fn get_failover_events(&self, limit: usize) -> Vec<FailoverEvent> {
        let events = self.failover_events.read().await;
        events.iter().rev().take(limit).cloned().collect()
    }

    /// Get collector load information
    pub async fn get_collector_load(&self, collector_id: &str) -> Option<(usize, u64)> {
        let loads = self.loads.read().await;
        loads
            .get(collector_id)
            .map(|l| (l.current_tasks, l.total_tasks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_round_robin_selection() {
        let config = LoadBalancerConfig {
            strategy: LoadBalancingStrategy::RoundRobin,
            ..Default::default()
        };
        let balancer = LoadBalancer::new(config);

        // Register collectors
        balancer
            .register_collector("collector-1".to_string(), 1.0)
            .await
            .unwrap();
        balancer
            .register_collector("collector-2".to_string(), 1.0)
            .await
            .unwrap();
        balancer
            .register_collector("collector-3".to_string(), 1.0)
            .await
            .unwrap();

        let collectors = create_test_collectors(3);

        // Select multiple times and verify round-robin
        let mut selections = Vec::new();
        for _ in 0..6 {
            let selected = balancer
                .select_collector(SourceCaps::PROCESS, &collectors)
                .await
                .unwrap();
            selections.push(selected);
        }

        // Should cycle through collectors
        assert_eq!(selections[0], selections[3]);
        assert_eq!(selections[1], selections[4]);
        assert_eq!(selections[2], selections[5]);
    }

    #[tokio::test]
    async fn test_least_connections_selection() {
        let config = LoadBalancerConfig {
            strategy: LoadBalancingStrategy::LeastConnections,
            ..Default::default()
        };
        let balancer = LoadBalancer::new(config);

        // Register collectors
        balancer
            .register_collector("collector-1".to_string(), 1.0)
            .await
            .unwrap();
        balancer
            .register_collector("collector-2".to_string(), 1.0)
            .await
            .unwrap();

        let collectors = create_test_collectors(2);

        // First selection should go to collector-1
        let selected1 = balancer
            .select_collector(SourceCaps::PROCESS, &collectors)
            .await
            .unwrap();

        // Second selection should go to collector-2 (least loaded)
        let selected2 = balancer
            .select_collector(SourceCaps::PROCESS, &collectors)
            .await
            .unwrap();

        assert_ne!(selected1, selected2);
    }

    #[tokio::test]
    async fn test_failover_trigger() {
        let config = LoadBalancerConfig {
            failover_threshold: 2,
            ..Default::default()
        };
        let balancer = LoadBalancer::new(config);

        balancer
            .register_collector("collector-1".to_string(), 1.0)
            .await
            .unwrap();
        balancer
            .register_collector("collector-2".to_string(), 1.0)
            .await
            .unwrap();

        // Record failures
        let should_failover1 = balancer.record_failure("collector-1").await.unwrap();
        assert!(!should_failover1);

        let should_failover2 = balancer.record_failure("collector-1").await.unwrap();
        assert!(should_failover2);

        // Trigger failover - filter out the failed collector
        let all_collectors = create_test_collectors(2);
        let available_collectors: Vec<_> = all_collectors
            .iter()
            .filter(|c| c.collector_id != "collector-1")
            .cloned()
            .collect();

        let event = balancer
            .trigger_failover("collector-1", &available_collectors, SourceCaps::PROCESS)
            .await
            .unwrap();

        assert_eq!(event.failed_collector_id, "collector-1");
        assert_eq!(event.failover_collector_id, "collector-2");
    }

    fn create_test_collectors(count: usize) -> Vec<CollectorCapability> {
        (1..=count)
            .map(|i| CollectorCapability {
                collector_id: format!("collector-{}", i),
                collector_type: "procmond".to_string(),
                capabilities: SourceCaps::PROCESS,
                topic_patterns: vec!["control.collector.process".to_string()],
                health_status: CollectorHealthStatus::Healthy,
                last_heartbeat: SystemTime::now(),
                metadata: HashMap::new(),
            })
            .collect()
    }
}
