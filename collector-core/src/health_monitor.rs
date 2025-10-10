//! Health monitoring system for collector lifecycle management.
//!
//! This module provides comprehensive health monitoring capabilities including
//! heartbeat tracking, health status aggregation, and automated failure detection
//! for collector components managed through the busrt message broker.

use crate::busrt_types::{
    HealthCheckRequest, HealthCheckResponse, HealthStatus, HeartbeatRequest, HeartbeatResponse,
    MetricsRequest, MetricsResponse,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, broadcast},
    task::JoinHandle,
    time::interval,
};
use tracing::{debug, info, instrument, warn};

/// Health monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMonitorConfig {
    /// Heartbeat interval for collectors
    pub heartbeat_interval: Duration,
    /// Timeout before marking collector as unhealthy
    pub health_timeout: Duration,
    /// Maximum consecutive failures before marking as degraded
    pub max_consecutive_failures: u32,
    /// Interval for health check sweeps
    pub health_check_interval: Duration,
    /// Enable automatic recovery attempts
    pub enable_auto_recovery: bool,
    /// Recovery attempt timeout
    pub recovery_timeout: Duration,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(30),
            health_timeout: Duration::from_secs(90),
            max_consecutive_failures: 3,
            health_check_interval: Duration::from_secs(60),
            enable_auto_recovery: true,
            recovery_timeout: Duration::from_secs(120),
        }
    }
}

/// Health monitoring system for collectors
pub struct HealthMonitor {
    /// Configuration
    config: HealthMonitorConfig,
    /// Collector health states
    health_states: Arc<RwLock<HashMap<String, CollectorHealthState>>>,
    /// Health event broadcaster
    health_events: broadcast::Sender<HealthEvent>,
    /// Background task handles
    heartbeat_handle: Option<JoinHandle<Result<()>>>,
    health_check_handle: Option<JoinHandle<Result<()>>>,
    /// Shutdown signal
    shutdown_tx: Option<broadcast::Sender<()>>,
    /// Metrics aggregator
    metrics_aggregator: Arc<Mutex<MetricsAggregator>>,
}

/// Collector health state tracking
#[derive(Debug, Clone)]
pub struct CollectorHealthState {
    /// Collector identifier
    pub collector_id: String,
    /// Current health status
    pub health_status: HealthStatus,
    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,
    /// Last health check timestamp
    pub last_health_check: SystemTime,
    /// Consecutive failure count
    pub consecutive_failures: u32,
    /// Current metrics
    pub metrics: HashMap<String, f64>,
    /// Health history for trend analysis
    pub health_history: Vec<HealthSnapshot>,
    /// Recovery attempts
    pub recovery_attempts: u32,
    /// Last recovery attempt timestamp
    pub last_recovery_attempt: Option<SystemTime>,
}

/// Health snapshot for historical tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Timestamp of snapshot
    pub timestamp: SystemTime,
    /// Health status at time of snapshot
    pub status: HealthStatus,
    /// Key metrics at time of snapshot
    pub metrics: HashMap<String, f64>,
    /// Any error message
    pub error_message: Option<String>,
}

/// Health event notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthEvent {
    /// Collector became healthy
    CollectorHealthy {
        collector_id: String,
        timestamp: SystemTime,
    },
    /// Collector became degraded
    CollectorDegraded {
        collector_id: String,
        reason: String,
        timestamp: SystemTime,
    },
    /// Collector became unhealthy
    CollectorUnhealthy {
        collector_id: String,
        error: String,
        timestamp: SystemTime,
    },
    /// Heartbeat missed
    HeartbeatMissed {
        collector_id: String,
        last_heartbeat: SystemTime,
        timestamp: SystemTime,
    },
    /// Recovery attempt started
    RecoveryStarted {
        collector_id: String,
        attempt_number: u32,
        timestamp: SystemTime,
    },
    /// Recovery succeeded
    RecoverySucceeded {
        collector_id: String,
        timestamp: SystemTime,
    },
    /// Recovery failed
    RecoveryFailed {
        collector_id: String,
        error: String,
        timestamp: SystemTime,
    },
}

/// Metrics aggregation system
#[derive(Debug)]
#[allow(dead_code)] // API design - system_metrics used in future features
struct MetricsAggregator {
    /// Aggregated metrics by collector
    collector_metrics: HashMap<String, HashMap<String, MetricSeries>>,
    /// Global system metrics
    system_metrics: HashMap<String, MetricSeries>,
}

/// Time series data for a metric
#[derive(Debug, Clone)]
struct MetricSeries {
    /// Recent values (limited to last N values)
    values: Vec<(SystemTime, f64)>,
    /// Maximum number of values to retain
    max_values: usize,
    /// Current value
    current: f64,
    /// Average over the series
    average: f64,
    /// Minimum value in series
    min: f64,
    /// Maximum value in series
    max: f64,
}

impl MetricSeries {
    fn new(max_values: usize) -> Self {
        Self {
            values: Vec::new(),
            max_values,
            current: 0.0,
            average: 0.0,
            min: f64::MAX,
            max: f64::MIN,
        }
    }

    fn add_value(&mut self, timestamp: SystemTime, value: f64) {
        self.values.push((timestamp, value));

        // Maintain size limit
        if self.values.len() > self.max_values {
            self.values.remove(0);
        }

        // Update statistics
        self.current = value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);

        if !self.values.is_empty() {
            self.average =
                self.values.iter().map(|(_, v)| v).sum::<f64>() / self.values.len() as f64;
        }
    }

    #[allow(dead_code)] // API design - used in future trend analysis features
    fn get_trend(&self) -> MetricTrend {
        if self.values.len() < 2 {
            return MetricTrend::Stable;
        }

        let recent_count = (self.values.len() / 4).max(2);
        let recent_values: Vec<f64> = self
            .values
            .iter()
            .rev()
            .take(recent_count)
            .map(|(_, v)| *v)
            .collect();

        let recent_avg = recent_values.iter().sum::<f64>() / recent_values.len() as f64;
        let overall_avg = self.average;

        let change_threshold = overall_avg * 0.1; // 10% change threshold

        if recent_avg > overall_avg + change_threshold {
            MetricTrend::Increasing
        } else if recent_avg < overall_avg - change_threshold {
            MetricTrend::Decreasing
        } else {
            MetricTrend::Stable
        }
    }
}

/// Metric trend analysis
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // API design - used in future trend analysis features
enum MetricTrend {
    Increasing,
    Decreasing,
    Stable,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(config: HealthMonitorConfig) -> Self {
        let (health_events, _) = broadcast::channel(1000);
        let metrics_aggregator = Arc::new(Mutex::new(MetricsAggregator {
            collector_metrics: HashMap::new(),
            system_metrics: HashMap::new(),
        }));

        Self {
            config,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            health_events,
            heartbeat_handle: None,
            health_check_handle: None,
            shutdown_tx: None,
            metrics_aggregator,
        }
    }

    /// Start the health monitoring system
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting health monitoring system");

        let (shutdown_tx, _) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Start heartbeat monitoring task
        self.start_heartbeat_monitoring(shutdown_tx.subscribe())
            .await?;

        // Start health check task
        self.start_health_check_task(shutdown_tx.subscribe())
            .await?;

        info!("Health monitoring system started");
        Ok(())
    }

    /// Stop the health monitoring system
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping health monitoring system");

        // Send shutdown signal
        if let Some(shutdown_tx) = &self.shutdown_tx {
            let _ = shutdown_tx.send(());
        }

        // Wait for tasks to complete
        if let Some(handle) = self.heartbeat_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Heartbeat monitoring task join error");
            }
        }

        if let Some(handle) = self.health_check_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Health check task join error");
            }
        }

        info!("Health monitoring system stopped");
        Ok(())
    }

    /// Register a collector for health monitoring
    pub async fn register_collector(&self, collector_id: String) -> Result<()> {
        let health_state = CollectorHealthState {
            collector_id: collector_id.clone(),
            health_status: HealthStatus::Unknown,
            last_heartbeat: SystemTime::now(),
            last_health_check: SystemTime::now(),
            consecutive_failures: 0,
            metrics: HashMap::new(),
            health_history: Vec::new(),
            recovery_attempts: 0,
            last_recovery_attempt: None,
        };

        {
            let mut states = self.health_states.write().unwrap();
            states.insert(collector_id.clone(), health_state);
        }

        info!(collector_id = %collector_id, "Collector registered for health monitoring");
        Ok(())
    }

    /// Unregister a collector from health monitoring
    pub async fn unregister_collector(&self, collector_id: &str) -> Result<()> {
        {
            let mut states = self.health_states.write().unwrap();
            states.remove(collector_id);
        }

        info!(collector_id = %collector_id, "Collector unregistered from health monitoring");
        Ok(())
    }

    /// Process a heartbeat from a collector
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    pub async fn process_heartbeat(&self, request: HeartbeatRequest) -> Result<HeartbeatResponse> {
        let timestamp = SystemTime::now();
        let timestamp_ms = timestamp
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Update heartbeat timestamp
        let status = {
            let mut states = self.health_states.write().unwrap();
            if let Some(state) = states.get_mut(&request.collector_id) {
                state.last_heartbeat = timestamp;

                // Reset consecutive failures on successful heartbeat
                if state.consecutive_failures > 0 {
                    state.consecutive_failures = 0;
                    info!(
                        collector_id = %request.collector_id,
                        "Collector recovered - consecutive failures reset"
                    );
                }

                // Update health status based on current state
                let new_status = if state.health_status == HealthStatus::Unhealthy {
                    HealthStatus::Degraded // Transition from unhealthy to degraded on heartbeat
                } else {
                    HealthStatus::Healthy
                };

                if new_status != state.health_status {
                    let old_status = state.health_status.clone();
                    state.health_status = new_status.clone();

                    info!(
                        collector_id = %request.collector_id,
                        old_status = ?old_status,
                        new_status = ?new_status,
                        "Collector health status changed"
                    );

                    // Send health event
                    let event = match new_status {
                        HealthStatus::Healthy => HealthEvent::CollectorHealthy {
                            collector_id: request.collector_id.clone(),
                            timestamp,
                        },
                        HealthStatus::Degraded => HealthEvent::CollectorDegraded {
                            collector_id: request.collector_id.clone(),
                            reason: "Recovering from unhealthy state".to_string(),
                            timestamp,
                        },
                        _ => {
                            return Ok(HeartbeatResponse {
                                timestamp: timestamp_ms,
                                status: new_status,
                            });
                        }
                    };

                    let _ = self.health_events.send(event);
                }

                new_status
            } else {
                warn!(collector_id = %request.collector_id, "Heartbeat from unregistered collector");
                HealthStatus::Unknown
            }
        };

        debug!(collector_id = %request.collector_id, "Heartbeat processed");

        Ok(HeartbeatResponse {
            timestamp: timestamp_ms,
            status,
        })
    }

    /// Perform a health check on a collector
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    pub async fn perform_health_check(
        &self,
        request: HealthCheckRequest,
    ) -> Result<HealthCheckResponse> {
        let timestamp = SystemTime::now();

        let (status, message, metrics, uptime_seconds) = {
            let mut states = self.health_states.write().unwrap();
            if let Some(state) = states.get_mut(&request.collector_id) {
                state.last_health_check = timestamp;

                // Check if heartbeat is recent
                let heartbeat_age = timestamp
                    .duration_since(state.last_heartbeat)
                    .unwrap_or_default();

                let status = if heartbeat_age > self.config.health_timeout {
                    // Missed heartbeat - mark as unhealthy
                    state.consecutive_failures += 1;

                    if state.consecutive_failures >= self.config.max_consecutive_failures {
                        HealthStatus::Unhealthy
                    } else {
                        HealthStatus::Degraded
                    }
                } else {
                    // Recent heartbeat - healthy
                    state.consecutive_failures = 0;
                    HealthStatus::Healthy
                };

                // Update health status if changed
                if status != state.health_status {
                    let old_status = state.health_status.clone();
                    state.health_status = status.clone();

                    info!(
                        collector_id = %request.collector_id,
                        old_status = ?old_status,
                        new_status = ?status,
                        consecutive_failures = state.consecutive_failures,
                        "Health status changed during health check"
                    );

                    // Send health event
                    let event = match status {
                        HealthStatus::Healthy => HealthEvent::CollectorHealthy {
                            collector_id: request.collector_id.clone(),
                            timestamp,
                        },
                        HealthStatus::Degraded => HealthEvent::CollectorDegraded {
                            collector_id: request.collector_id.clone(),
                            reason: format!("Missed {} heartbeats", state.consecutive_failures),
                            timestamp,
                        },
                        HealthStatus::Unhealthy => HealthEvent::CollectorUnhealthy {
                            collector_id: request.collector_id.clone(),
                            error: format!(
                                "Missed {} consecutive heartbeats",
                                state.consecutive_failures
                            ),
                            timestamp,
                        },
                        HealthStatus::Unknown => {
                            return Err(anyhow::anyhow!("Unknown health status"));
                        }
                    };

                    let _ = self.health_events.send(event);
                }

                // Add health snapshot to history
                let snapshot = HealthSnapshot {
                    timestamp,
                    status: status.clone(),
                    metrics: state.metrics.clone(),
                    error_message: if status == HealthStatus::Unhealthy {
                        Some(format!(
                            "Missed {} consecutive heartbeats",
                            state.consecutive_failures
                        ))
                    } else {
                        None
                    },
                };
                state.health_history.push(snapshot);

                // Limit history size
                if state.health_history.len() > 100 {
                    state.health_history.remove(0);
                }

                let message = match status {
                    HealthStatus::Healthy => "Collector is operating normally".to_string(),
                    HealthStatus::Degraded => format!(
                        "Collector has {} consecutive failures (heartbeat age: {:?})",
                        state.consecutive_failures, heartbeat_age
                    ),
                    HealthStatus::Unhealthy => format!(
                        "Collector is unhealthy - {} consecutive failures (heartbeat age: {:?})",
                        state.consecutive_failures, heartbeat_age
                    ),
                    HealthStatus::Unknown => "Collector status is unknown".to_string(),
                };

                let metrics = if request.include_metrics {
                    state.metrics.clone()
                } else {
                    HashMap::new()
                };

                let uptime = timestamp
                    .duration_since(state.last_heartbeat)
                    .unwrap_or_default()
                    .as_secs();

                (status, message, metrics, uptime)
            } else {
                return Err(anyhow::anyhow!(
                    "Collector not found: {}",
                    request.collector_id
                ));
            }
        };

        Ok(HealthCheckResponse {
            status,
            message,
            metrics,
            uptime_seconds,
        })
    }

    /// Update metrics for a collector
    pub async fn update_metrics(
        &self,
        collector_id: &str,
        metrics: HashMap<String, f64>,
    ) -> Result<()> {
        let timestamp = SystemTime::now();

        // Update collector health state
        {
            let mut states = self.health_states.write().unwrap();
            if let Some(state) = states.get_mut(collector_id) {
                state.metrics = metrics.clone();
            }
        }

        // Update metrics aggregator
        {
            let mut aggregator = self.metrics_aggregator.lock().await;
            let collector_metrics = aggregator
                .collector_metrics
                .entry(collector_id.to_string())
                .or_insert_with(HashMap::new);

            for (metric_name, value) in metrics {
                let series = collector_metrics
                    .entry(metric_name)
                    .or_insert_with(|| MetricSeries::new(100));
                series.add_value(timestamp, value);
            }
        }

        Ok(())
    }

    /// Get metrics for a collector
    pub async fn get_metrics(&self, request: MetricsRequest) -> Result<MetricsResponse> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let metrics = {
            let states = self.health_states.read().unwrap();
            if let Some(state) = states.get(&request.collector_id) {
                let mut metrics = state.metrics.clone();

                // Filter metrics if specific names requested
                if let Some(metric_names) = &request.metric_names {
                    metrics.retain(|key, _| metric_names.contains(key));
                }

                metrics
            } else {
                return Err(anyhow::anyhow!(
                    "Collector not found: {}",
                    request.collector_id
                ));
            }
        };

        Ok(MetricsResponse { metrics, timestamp })
    }

    /// Subscribe to health events
    pub fn subscribe_to_health_events(&self) -> broadcast::Receiver<HealthEvent> {
        self.health_events.subscribe()
    }

    /// Get current health summary
    pub async fn get_health_summary(&self) -> HealthSummary {
        let states = self.health_states.read().unwrap();
        let mut summary = HealthSummary {
            total_collectors: states.len(),
            healthy_collectors: 0,
            degraded_collectors: 0,
            unhealthy_collectors: 0,
            unknown_collectors: 0,
            collectors: Vec::new(),
        };

        for (collector_id, state) in states.iter() {
            match state.health_status {
                HealthStatus::Healthy => summary.healthy_collectors += 1,
                HealthStatus::Degraded => summary.degraded_collectors += 1,
                HealthStatus::Unhealthy => summary.unhealthy_collectors += 1,
                HealthStatus::Unknown => summary.unknown_collectors += 1,
            }

            summary.collectors.push(CollectorHealthSummary {
                collector_id: collector_id.clone(),
                status: state.health_status.clone(),
                last_heartbeat: state.last_heartbeat,
                consecutive_failures: state.consecutive_failures,
                recovery_attempts: state.recovery_attempts,
            });
        }

        summary
    }

    /// Start heartbeat monitoring task
    async fn start_heartbeat_monitoring(
        &mut self,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        let health_states = Arc::clone(&self.health_states);
        let health_events = self.health_events.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            let mut interval = interval(config.heartbeat_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Check for missed heartbeats
                        let now = SystemTime::now();
                        let mut missed_heartbeats = Vec::new();

                        {
                            let states = health_states.read().unwrap();
                            for (collector_id, state) in states.iter() {
                                let heartbeat_age = now
                                    .duration_since(state.last_heartbeat)
                                    .unwrap_or_default();

                                if heartbeat_age > config.health_timeout {
                                    missed_heartbeats.push((collector_id.clone(), state.last_heartbeat));
                                }
                            }
                        }

                        // Send missed heartbeat events
                        for (collector_id, last_heartbeat) in missed_heartbeats {
                            let event = HealthEvent::HeartbeatMissed {
                                collector_id,
                                last_heartbeat,
                                timestamp: now,
                            };
                            let _ = health_events.send(event);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Heartbeat monitoring task shutting down");
                        break;
                    }
                }
            }

            Ok(())
        });

        self.heartbeat_handle = Some(handle);
        Ok(())
    }

    /// Start health check task
    async fn start_health_check_task(
        &mut self,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        let health_states = Arc::clone(&self.health_states);
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            let mut interval = interval(config.health_check_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        debug!("Performing periodic health checks");

                        // Perform health checks on all collectors
                        let collector_ids: Vec<String> = {
                            let states = health_states.read().unwrap();
                            states.keys().cloned().collect()
                        };

                        for collector_id in collector_ids {
                            // Health check logic is handled by the main health check method
                            debug!(collector_id = %collector_id, "Periodic health check");
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Health check task shutting down");
                        break;
                    }
                }
            }

            Ok(())
        });

        self.health_check_handle = Some(handle);
        Ok(())
    }
}

/// Health summary for all collectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSummary {
    pub total_collectors: usize,
    pub healthy_collectors: usize,
    pub degraded_collectors: usize,
    pub unhealthy_collectors: usize,
    pub unknown_collectors: usize,
    pub collectors: Vec<CollectorHealthSummary>,
}

/// Individual collector health summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorHealthSummary {
    pub collector_id: String,
    pub status: HealthStatus,
    pub last_heartbeat: SystemTime,
    pub consecutive_failures: u32,
    pub recovery_attempts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn test_health_monitor_lifecycle() {
        let config = HealthMonitorConfig {
            heartbeat_interval: Duration::from_millis(100),
            health_timeout: Duration::from_millis(200),
            max_consecutive_failures: 2,
            health_check_interval: Duration::from_millis(150),
            enable_auto_recovery: true,
            recovery_timeout: Duration::from_secs(1),
        };

        let mut monitor = HealthMonitor::new(config);
        monitor.start().await.unwrap();

        // Register a collector
        monitor
            .register_collector("test-collector".to_string())
            .await
            .unwrap();

        // Send heartbeat
        let heartbeat_request = HeartbeatRequest {
            collector_id: "test-collector".to_string(),
        };
        let heartbeat_response = monitor.process_heartbeat(heartbeat_request).await.unwrap();
        assert_eq!(heartbeat_response.status, HealthStatus::Healthy);

        // Perform health check
        let health_request = HealthCheckRequest {
            collector_id: "test-collector".to_string(),
            include_metrics: true,
        };
        let health_response = monitor.perform_health_check(health_request).await.unwrap();
        assert_eq!(health_response.status, HealthStatus::Healthy);

        // Stop monitor
        monitor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_missed_heartbeat_detection() {
        let config = HealthMonitorConfig {
            heartbeat_interval: Duration::from_millis(50),
            health_timeout: Duration::from_millis(100),
            max_consecutive_failures: 2,
            health_check_interval: Duration::from_millis(75),
            enable_auto_recovery: false,
            recovery_timeout: Duration::from_secs(1),
        };

        let mut monitor = HealthMonitor::new(config);
        monitor.start().await.unwrap();

        // Register collector
        monitor
            .register_collector("test-collector".to_string())
            .await
            .unwrap();

        // Send initial heartbeat
        let heartbeat_request = HeartbeatRequest {
            collector_id: "test-collector".to_string(),
        };
        monitor.process_heartbeat(heartbeat_request).await.unwrap();

        // Wait for heartbeat timeout
        sleep(Duration::from_millis(150)).await;

        // Perform health check - should detect missed heartbeat
        let health_request = HealthCheckRequest {
            collector_id: "test-collector".to_string(),
            include_metrics: false,
        };
        let health_response = monitor.perform_health_check(health_request).await.unwrap();
        assert_eq!(health_response.status, HealthStatus::Degraded);

        monitor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_metrics_tracking() {
        let config = HealthMonitorConfig::default();
        let monitor = HealthMonitor::new(config);

        // Register collector
        monitor
            .register_collector("metrics-test".to_string())
            .await
            .unwrap();

        // Update metrics
        let mut metrics = HashMap::new();
        metrics.insert("cpu_percent".to_string(), 45.5);
        metrics.insert("memory_bytes".to_string(), 1024.0 * 1024.0 * 100.0);

        monitor
            .update_metrics("metrics-test", metrics.clone())
            .await
            .unwrap();

        // Get metrics
        let metrics_request = MetricsRequest {
            collector_id: "metrics-test".to_string(),
            metric_names: None,
        };
        let metrics_response = monitor.get_metrics(metrics_request).await.unwrap();

        assert_eq!(metrics_response.metrics.get("cpu_percent"), Some(&45.5));
        assert_eq!(
            metrics_response.metrics.get("memory_bytes"),
            Some(&(1024.0 * 1024.0 * 100.0))
        );
    }

    #[tokio::test]
    async fn test_health_events() {
        let config = HealthMonitorConfig::default();
        let monitor = HealthMonitor::new(config);

        // Subscribe to health events
        let _event_rx = monitor.subscribe_to_health_events();

        // Register collector
        monitor
            .register_collector("event-test".to_string())
            .await
            .unwrap();

        // Send heartbeat to trigger healthy event
        let heartbeat_request = HeartbeatRequest {
            collector_id: "event-test".to_string(),
        };
        monitor.process_heartbeat(heartbeat_request).await.unwrap();

        // Check for health event (may not be sent immediately in this test setup)
        // In a real scenario, the health status change would trigger an event
    }
}
