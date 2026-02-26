//! Capability-based routing for dynamic collector discovery and task distribution.
//!
//! This module provides capability advertisement, discovery, and dynamic routing
//! for multi-collector coordination workflows.

use crate::source::SourceCaps;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Collector capability advertisement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorCapability {
    /// Collector identifier
    pub collector_id: String,
    /// Collector type (procmond, netmond, fsmond, etc.)
    pub collector_type: String,
    /// Supported capabilities
    pub capabilities: SourceCaps,
    /// Topic patterns this collector subscribes to
    pub topic_patterns: Vec<String>,
    /// Collector health status
    pub health_status: CollectorHealthStatus,
    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,
    /// Collector metadata
    pub metadata: HashMap<String, String>,
}

/// Collector health status for routing decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectorHealthStatus {
    /// Collector is healthy and accepting tasks
    Healthy,
    /// Collector is degraded but operational
    Degraded,
    /// Collector is unavailable
    Unavailable,
}

/// Routing decision for task distribution
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Selected collector ID
    pub collector_id: String,
    /// Target topic for task delivery
    pub target_topic: String,
    /// Routing confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Fallback collectors if primary fails
    pub fallbacks: Vec<String>,
}

/// Capability router for dynamic collector discovery
pub struct CapabilityRouter {
    /// Registered collector capabilities
    capabilities: Arc<RwLock<HashMap<String, CollectorCapability>>>,
    /// Capability index for fast lookup
    capability_index: Arc<RwLock<HashMap<SourceCaps, Vec<String>>>>,
    /// Heartbeat timeout duration
    heartbeat_timeout: Duration,
}

impl CapabilityRouter {
    /// Create a new capability router
    pub fn new(heartbeat_timeout: Duration) -> Self {
        Self {
            capabilities: Arc::new(RwLock::new(HashMap::new())),
            capability_index: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_timeout,
        }
    }

    /// Register a collector's capabilities
    pub async fn register_collector(&self, capability: CollectorCapability) -> Result<()> {
        let collector_id = capability.collector_id.clone();
        let caps = capability.capabilities;

        info!(
            collector_id = %collector_id,
            capabilities = ?caps,
            "Registering collector capabilities"
        );

        // Store capability
        {
            let mut capabilities = self.capabilities.write().await;
            capabilities.insert(collector_id.clone(), capability);
        }

        // Update capability index
        {
            let mut index = self.capability_index.write().await;
            index
                .entry(caps)
                .or_insert_with(Vec::new)
                .push(collector_id.clone());
        }

        Ok(())
    }

    /// Deregister a collector
    pub async fn deregister_collector(&self, collector_id: &str) -> Result<()> {
        info!(collector_id = %collector_id, "Deregistering collector");

        // Remove from capabilities
        let caps = {
            let mut capabilities = self.capabilities.write().await;
            capabilities.remove(collector_id).map(|c| c.capabilities)
        };

        // Remove from capability index
        if let Some(caps) = caps {
            let mut index = self.capability_index.write().await;
            if let Some(collectors) = index.get_mut(&caps) {
                collectors.retain(|id| id != collector_id);
                if collectors.is_empty() {
                    index.remove(&caps);
                }
            }
        }

        Ok(())
    }

    /// Update collector heartbeat
    pub async fn update_heartbeat(&self, collector_id: &str) -> Result<()> {
        let mut capabilities = self.capabilities.write().await;

        if let Some(capability) = capabilities.get_mut(collector_id) {
            capability.last_heartbeat = SystemTime::now();
            debug!(collector_id = %collector_id, "Updated collector heartbeat");
            Ok(())
        } else {
            Err(anyhow::anyhow!("Collector not found: {}", collector_id))
        }
    }

    /// Update collector health status
    pub async fn update_health_status(
        &self,
        collector_id: &str,
        status: CollectorHealthStatus,
    ) -> Result<()> {
        let mut capabilities = self.capabilities.write().await;

        if let Some(capability) = capabilities.get_mut(collector_id) {
            capability.health_status = status;
            info!(
                collector_id = %collector_id,
                status = ?status,
                "Updated collector health status"
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!("Collector not found: {}", collector_id))
        }
    }

    /// Find collectors with specific capabilities
    pub async fn find_collectors_by_capability(
        &self,
        required_caps: SourceCaps,
    ) -> Vec<CollectorCapability> {
        let capabilities = self.capabilities.read().await;

        capabilities
            .values()
            .filter(|cap| {
                cap.capabilities.contains(required_caps)
                    && cap.health_status != CollectorHealthStatus::Unavailable
            })
            .cloned()
            .collect()
    }

    /// Route task to best available collector
    pub async fn route_task(&self, required_caps: SourceCaps) -> Result<RoutingDecision> {
        let collectors = self.find_collectors_by_capability(required_caps).await;

        if collectors.is_empty() {
            return Err(anyhow::anyhow!(
                "No collectors available with required capabilities: {:?}",
                required_caps
            ));
        }

        // Select best collector based on health and load
        let mut best_collector: Option<&CollectorCapability> = None;
        let mut best_score = 0.0;

        for collector in &collectors {
            let score = self.calculate_routing_score(collector);
            if score > best_score {
                best_score = score;
                best_collector = Some(collector);
            }
        }

        let selected = best_collector.context("No suitable collector found")?;

        // Determine target topic from collector's subscriptions
        let target_topic = selected
            .topic_patterns
            .first()
            .cloned()
            .unwrap_or_else(|| "control.collector.generic".to_string());

        // Collect fallback collectors
        let fallbacks: Vec<String> = collectors
            .iter()
            .filter(|c| c.collector_id != selected.collector_id)
            .map(|c| c.collector_id.clone())
            .collect();

        Ok(RoutingDecision {
            collector_id: selected.collector_id.clone(),
            target_topic,
            confidence: best_score,
            fallbacks,
        })
    }

    /// Calculate routing score for a collector
    fn calculate_routing_score(&self, collector: &CollectorCapability) -> f64 {
        let mut score = 1.0;

        // Health status factor
        match collector.health_status {
            CollectorHealthStatus::Healthy => score *= 1.0,
            CollectorHealthStatus::Degraded => score *= 0.5,
            CollectorHealthStatus::Unavailable => score *= 0.0,
        }

        // Heartbeat freshness factor
        if let Ok(elapsed) = collector.last_heartbeat.elapsed() {
            if elapsed > self.heartbeat_timeout {
                score *= 0.1; // Stale heartbeat
            } else {
                let freshness =
                    1.0 - (elapsed.as_secs_f64() / self.heartbeat_timeout.as_secs_f64());
                score *= freshness.max(0.5);
            }
        }

        score
    }

    /// Check for stale collectors and mark as unavailable
    pub async fn check_stale_collectors(&self) -> Vec<String> {
        let now = SystemTime::now();
        let mut stale_collectors = Vec::new();

        let mut capabilities = self.capabilities.write().await;

        for (collector_id, capability) in capabilities.iter_mut() {
            if let Ok(elapsed) = now.duration_since(capability.last_heartbeat)
                && elapsed > self.heartbeat_timeout
            {
                warn!(
                    collector_id = %collector_id,
                    elapsed_secs = elapsed.as_secs(),
                    "Collector heartbeat stale, marking as unavailable"
                );
                capability.health_status = CollectorHealthStatus::Unavailable;
                stale_collectors.push(collector_id.clone());
            }
        }

        stale_collectors
    }

    /// Get all registered collectors
    pub async fn get_all_collectors(&self) -> Vec<CollectorCapability> {
        let capabilities = self.capabilities.read().await;
        capabilities.values().cloned().collect()
    }

    /// Get collector by ID
    pub async fn get_collector(&self, collector_id: &str) -> Option<CollectorCapability> {
        let capabilities = self.capabilities.read().await;
        capabilities.get(collector_id).cloned()
    }

    /// Get routing statistics
    pub async fn get_routing_stats(&self) -> RoutingStats {
        let capabilities = self.capabilities.read().await;

        let total_collectors = capabilities.len();
        let healthy_collectors = capabilities
            .values()
            .filter(|c| c.health_status == CollectorHealthStatus::Healthy)
            .count();
        let degraded_collectors = capabilities
            .values()
            .filter(|c| c.health_status == CollectorHealthStatus::Degraded)
            .count();
        let unavailable_collectors = capabilities
            .values()
            .filter(|c| c.health_status == CollectorHealthStatus::Unavailable)
            .count();

        // Count capabilities
        let mut capability_counts = HashMap::new();
        for capability in capabilities.values() {
            let caps = capability.capabilities;
            *capability_counts.entry(caps).or_insert(0) += 1;
        }

        RoutingStats {
            total_collectors,
            healthy_collectors,
            degraded_collectors,
            unavailable_collectors,
            capability_counts,
        }
    }
}

/// Routing statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingStats {
    /// Total registered collectors
    pub total_collectors: usize,
    /// Healthy collectors
    pub healthy_collectors: usize,
    /// Degraded collectors
    pub degraded_collectors: usize,
    /// Unavailable collectors
    pub unavailable_collectors: usize,
    /// Capability distribution
    pub capability_counts: HashMap<SourceCaps, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collector_registration() {
        let router = CapabilityRouter::new(Duration::from_secs(30));

        let capability = CollectorCapability {
            collector_id: "procmond-1".to_string(),
            collector_type: "procmond".to_string(),
            capabilities: SourceCaps::PROCESS | SourceCaps::REALTIME,
            topic_patterns: vec!["control.collector.process".to_string()],
            health_status: CollectorHealthStatus::Healthy,
            last_heartbeat: SystemTime::now(),
            metadata: HashMap::new(),
        };

        router.register_collector(capability.clone()).await.unwrap();

        let collectors = router
            .find_collectors_by_capability(SourceCaps::PROCESS)
            .await;
        assert_eq!(collectors.len(), 1);
        assert_eq!(collectors[0].collector_id, "procmond-1");
    }

    #[tokio::test]
    async fn test_routing_decision() {
        let router = CapabilityRouter::new(Duration::from_secs(30));

        // Register multiple collectors
        for i in 1..=3 {
            let capability = CollectorCapability {
                collector_id: format!("procmond-{}", i),
                collector_type: "procmond".to_string(),
                capabilities: SourceCaps::PROCESS,
                topic_patterns: vec!["control.collector.process".to_string()],
                health_status: CollectorHealthStatus::Healthy,
                last_heartbeat: SystemTime::now(),
                metadata: HashMap::new(),
            };
            router.register_collector(capability).await.unwrap();
        }

        let decision = router.route_task(SourceCaps::PROCESS).await.unwrap();
        assert!(decision.collector_id.starts_with("procmond-"));
        assert_eq!(decision.target_topic, "control.collector.process");
        assert_eq!(decision.fallbacks.len(), 2);
    }

    #[tokio::test]
    async fn test_health_status_routing() {
        let router = CapabilityRouter::new(Duration::from_secs(30));

        // Register healthy collector
        let healthy = CollectorCapability {
            collector_id: "healthy-1".to_string(),
            collector_type: "procmond".to_string(),
            capabilities: SourceCaps::PROCESS,
            topic_patterns: vec!["control.collector.process".to_string()],
            health_status: CollectorHealthStatus::Healthy,
            last_heartbeat: SystemTime::now(),
            metadata: HashMap::new(),
        };

        // Register degraded collector
        let degraded = CollectorCapability {
            collector_id: "degraded-1".to_string(),
            collector_type: "procmond".to_string(),
            capabilities: SourceCaps::PROCESS,
            topic_patterns: vec!["control.collector.process".to_string()],
            health_status: CollectorHealthStatus::Degraded,
            last_heartbeat: SystemTime::now(),
            metadata: HashMap::new(),
        };

        router.register_collector(healthy).await.unwrap();
        router.register_collector(degraded).await.unwrap();

        let decision = router.route_task(SourceCaps::PROCESS).await.unwrap();
        // Should prefer healthy collector
        assert_eq!(decision.collector_id, "healthy-1");
    }

    #[tokio::test]
    async fn test_stale_collector_detection() {
        let router = CapabilityRouter::new(Duration::from_millis(100));

        let capability = CollectorCapability {
            collector_id: "stale-1".to_string(),
            collector_type: "procmond".to_string(),
            capabilities: SourceCaps::PROCESS,
            topic_patterns: vec!["control.collector.process".to_string()],
            health_status: CollectorHealthStatus::Healthy,
            last_heartbeat: SystemTime::now() - Duration::from_secs(1),
            metadata: HashMap::new(),
        };

        router.register_collector(capability).await.unwrap();

        // Wait for heartbeat to become stale
        tokio::time::sleep(Duration::from_millis(200)).await;

        let stale = router.check_stale_collectors().await;
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "stale-1");

        // Verify collector is marked unavailable
        let collector = router.get_collector("stale-1").await.unwrap();
        assert_eq!(collector.health_status, CollectorHealthStatus::Unavailable);
    }
}
