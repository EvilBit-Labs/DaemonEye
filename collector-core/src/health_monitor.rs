//! Health monitoring system for collector lifecycle management.
//!
//! Provides component-level health tracking with aggregation. The monitor maintains
//! per-component status and exposes a worst-of aggregation to compute overall collector health.
//!
//! Publishing health status to external systems should be handled by the consumer (e.g., daemoneye-agent)
//! to avoid circular dependencies.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio::time::{Interval, interval};

/// Overall health status values used by the health monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Component/system is healthy
    Healthy,
    /// Component/system is degraded but functional
    Degraded,
    /// Component/system is unhealthy
    Unhealthy,
    /// Status is unknown
    Unknown,
}

/// Per-component health snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealthStatus {
    /// Component name
    pub name: String,
    /// Current status
    pub status: HealthStatus,
    /// Last time this component was checked/updated
    pub last_check: SystemTime,
    /// Number of health updates recorded
    pub check_count: u64,
    /// Number of failures recorded (Degraded/Unhealthy)
    pub failure_count: u64,
    /// Optional diagnostic message
    pub message: Option<String>,
}

impl ComponentHealthStatus {
    fn new(name: &str, status: HealthStatus, message: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            status,
            last_check: SystemTime::now(),
            check_count: 0,
            failure_count: 0,
            message,
        }
    }
}

/// Health monitor that tracks component health and provides aggregated status.
pub struct HealthMonitor {
    /// Unique collector identifier
    collector_id: String,
    /// Component health map
    components: Arc<Mutex<HashMap<String, ComponentHealthStatus>>>,
    /// Cached overall health (worst-of aggregation)
    overall_status: Arc<RwLock<HealthStatus>>,
    /// Health check/aggregation interval
    health_check_interval: Duration,
}

impl HealthMonitor {
    /// Create a new HealthMonitor.
    #[must_use]
    pub fn new(collector_id: String, health_check_interval: Duration) -> Self {
        Self {
            collector_id,
            components: Arc::new(Mutex::new(HashMap::new())),
            overall_status: Arc::new(RwLock::new(HealthStatus::Unknown)),
            health_check_interval,
        }
    }

    /// Register a component with an initial health state.
    pub async fn register_component(&self, name: &str, initial: HealthStatus) {
        let mut components = self.components.lock().await;
        let mut entry = ComponentHealthStatus::new(name, initial, None);
        entry.check_count = 1;
        if matches!(initial, HealthStatus::Degraded | HealthStatus::Unhealthy) {
            entry.failure_count = 1;
        }
        components.insert(name.to_string(), entry);
    }

    /// Update a component health snapshot.
    pub async fn update_component_health(
        &self,
        name: &str,
        status: HealthStatus,
        message: Option<String>,
    ) {
        let mut components = self.components.lock().await;
        let entry = components
            .entry(name.to_string())
            .or_insert_with(|| ComponentHealthStatus::new(name, HealthStatus::Unknown, None));
        entry.status = status;
        entry.message = message;
        entry.last_check = SystemTime::now();
        entry.check_count = entry.check_count.saturating_add(1);
        if matches!(status, HealthStatus::Degraded | HealthStatus::Unhealthy) {
            entry.failure_count = entry.failure_count.saturating_add(1);
        }
    }

    /// Get a component health snapshot by name.
    pub async fn get_component_health(&self, name: &str) -> Option<ComponentHealthStatus> {
        let components = self.components.lock().await;
        components.get(name).cloned()
    }

    /// Get the current overall health (worst-of aggregation).
    pub async fn get_overall_health(&self) -> HealthStatus {
        let guard = self.overall_status.read().await;
        *guard
    }

    /// Get a copy of all component snapshots.
    pub async fn get_all_components(&self) -> HashMap<String, ComponentHealthStatus> {
        let components = self.components.lock().await;
        components.clone()
    }

    /// Start periodic aggregation and optional publishing.
    pub async fn start_monitoring(&self) -> JoinHandle<()> {
        let collector_id = self.collector_id.clone();
        let components = Arc::clone(&self.components);
        let overall_status = Arc::clone(&self.overall_status);
        let interval_duration = self.health_check_interval;

        tokio::spawn(async move {
            let mut ticker: Interval = interval(interval_duration);
            // Avoid accumulating ticks if the task is delayed
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                // Take a snapshot of components under lock
                let snapshot: HashMap<String, ComponentHealthStatus> = {
                    let comps = components.lock().await;
                    comps.clone()
                };

                // Compute worst-of aggregation
                let agg = aggregate_worst_of(snapshot.values().map(|c| c.status));

                // Update cached overall
                {
                    let mut guard = overall_status.write().await;
                    *guard = agg;
                }

                // Lightweight diagnostic
                tracing::trace!(collector_id = %collector_id, status = ?agg, "Aggregated collector health");
            }
        })
    }
}

/// Compute worst-of aggregation across component statuses.
fn aggregate_worst_of<I: Iterator<Item = HealthStatus>>(iter: I) -> HealthStatus {
    let mut has_healthy = false;
    let mut has_degraded = false;
    let mut has_unhealthy = false;

    for s in iter {
        match s {
            HealthStatus::Unhealthy => has_unhealthy = true,
            HealthStatus::Degraded => has_degraded = true,
            HealthStatus::Healthy => has_healthy = true,
            HealthStatus::Unknown => {}
        }
    }

    if has_unhealthy {
        HealthStatus::Unhealthy
    } else if has_degraded {
        HealthStatus::Degraded
    } else if has_healthy {
        HealthStatus::Healthy
    } else {
        // No components or all unknown
        HealthStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worst_of_aggregation() {
        // Unknown when empty
        assert_eq!(aggregate_worst_of([].into_iter()), HealthStatus::Unknown);

        // Healthy only
        assert_eq!(
            aggregate_worst_of([HealthStatus::Healthy].into_iter()),
            HealthStatus::Healthy
        );

        // Degraded wins over healthy
        assert_eq!(
            aggregate_worst_of([HealthStatus::Healthy, HealthStatus::Degraded].into_iter()),
            HealthStatus::Degraded
        );

        // Unhealthy wins over degraded/healthy
        assert_eq!(
            aggregate_worst_of(
                [
                    HealthStatus::Healthy,
                    HealthStatus::Degraded,
                    HealthStatus::Unhealthy,
                ]
                .into_iter()
            ),
            HealthStatus::Unhealthy
        );
    }
}
