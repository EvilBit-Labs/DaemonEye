//! Performance telemetry and health monitoring.
//!
//! This module provides metrics collection, health monitoring, and performance
//! telemetry for SentinelD components.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Telemetry collection errors.
#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("Metric collection failed: {0}")]
    CollectionError(String),

    #[error("Health check failed: {0}")]
    HealthCheckError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

/// Performance metrics for a component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metrics {
    /// Component name
    pub component: String,
    /// Collection timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// CPU usage percentage
    pub cpu_usage: f64,
    /// Memory usage in bytes
    pub memory_usage: u64,
    /// Number of operations performed
    pub operation_count: u64,
    /// Average operation duration in milliseconds
    pub avg_operation_duration_ms: f64,
    /// Error count
    pub error_count: u64,
    /// Custom metrics
    pub custom_metrics: HashMap<String, f64>,
}

impl Metrics {
    /// Create new metrics for a component.
    pub fn new(component: String) -> Self {
        Self {
            component,
            timestamp: chrono::Utc::now(),
            cpu_usage: 0.0,
            memory_usage: 0,
            operation_count: 0,
            avg_operation_duration_ms: 0.0,
            error_count: 0,
            custom_metrics: HashMap::new(),
        }
    }

    /// Add a custom metric.
    pub fn add_custom_metric(&mut self, name: String, value: f64) {
        self.custom_metrics.insert(name, value);
    }
}

/// Health status for a component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Health check result for a component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthCheck {
    /// Component name
    pub component: String,
    /// Health status
    pub status: HealthStatus,
    /// Check timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Check duration in milliseconds
    pub duration_ms: u64,
    /// Error message if check failed
    pub error_message: Option<String>,
    /// Additional details
    pub details: HashMap<String, String>,
}

impl HealthCheck {
    /// Create a new health check result.
    pub fn new(component: String, status: HealthStatus, duration_ms: u64) -> Self {
        Self {
            component,
            status,
            timestamp: chrono::Utc::now(),
            duration_ms,
            error_message: None,
            details: HashMap::new(),
        }
    }

    /// Add a detail to the health check.
    pub fn add_detail(&mut self, key: String, value: String) {
        self.details.insert(key, value);
    }
}

/// Performance timer for measuring operation durations.
pub struct PerformanceTimer {
    start_time: Instant,
    operation_name: String,
}

impl PerformanceTimer {
    /// Start timing an operation.
    pub fn start(operation_name: String) -> Self {
        Self {
            start_time: Instant::now(),
            operation_name,
        }
    }

    /// Finish timing and return the duration.
    pub fn finish(self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get the operation name.
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }
}

/// Telemetry collector for gathering metrics and health information.
pub struct TelemetryCollector {
    component: String,
    metrics: Metrics,
    operation_times: Vec<Duration>,
    error_count: u64,
}

impl TelemetryCollector {
    /// Create a new telemetry collector.
    pub fn new(component: String) -> Self {
        Self {
            component: component.clone(),
            metrics: Metrics::new(component),
            operation_times: Vec::new(),
            error_count: 0,
        }
    }

    /// Record an operation completion.
    pub fn record_operation(&mut self, duration: Duration) {
        self.operation_times.push(duration);
        self.metrics.operation_count += 1;

        // Update average operation duration
        let total_duration: Duration = self.operation_times.iter().sum();
        self.metrics.avg_operation_duration_ms =
            total_duration.as_millis() as f64 / self.operation_times.len() as f64;
    }

    /// Record an error occurrence.
    pub fn record_error(&mut self) {
        self.error_count += 1;
        self.metrics.error_count = self.error_count;
    }

    /// Update system resource usage.
    pub fn update_resource_usage(&mut self, cpu_usage: f64, memory_usage: u64) {
        self.metrics.cpu_usage = cpu_usage;
        self.metrics.memory_usage = memory_usage;
    }

    /// Add a custom metric.
    pub fn add_custom_metric(&mut self, name: String, value: f64) {
        self.metrics.add_custom_metric(name, value);
    }

    /// Get current metrics.
    pub fn get_metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Perform a health check.
    pub async fn health_check(&self) -> Result<HealthCheck, TelemetryError> {
        let start_time = Instant::now();

        // Basic health checks
        let mut status = HealthStatus::Healthy;
        let mut error_message = None;
        let mut details = HashMap::new();

        // Check if we have too many errors
        if self.error_count > 100 {
            status = HealthStatus::Degraded;
            error_message = Some("High error count detected".to_string());
        }

        // Check if operations are taking too long
        if let Some(avg_duration) = self.operation_times.last() {
            if avg_duration.as_millis() > 5000 {
                status = HealthStatus::Degraded;
                error_message = Some("Slow operation performance detected".to_string());
            }
        }

        // Check memory usage
        if self.metrics.memory_usage > 100 * 1024 * 1024 {
            // 100MB
            status = HealthStatus::Degraded;
            details.insert(
                "memory_usage_mb".to_string(),
                (self.metrics.memory_usage / 1024 / 1024).to_string(),
            );
        }

        // Check CPU usage
        if self.metrics.cpu_usage > 80.0 {
            status = HealthStatus::Degraded;
            details.insert(
                "cpu_usage_percent".to_string(),
                self.metrics.cpu_usage.to_string(),
            );
        }

        let duration = start_time.elapsed();
        let mut health_check =
            HealthCheck::new(self.component.clone(), status, duration.as_millis() as u64);

        if let Some(error) = error_message {
            health_check.error_message = Some(error);
        }

        for (key, value) in details {
            health_check.add_detail(key, value);
        }

        Ok(health_check)
    }

    /// Perform a health check synchronously (for CLI usage).
    pub fn health_check_blocking(&self) -> Result<HealthCheck, TelemetryError> {
        let start_time = Instant::now();

        // Basic health checks
        let mut status = HealthStatus::Healthy;
        let mut error_message = None;
        let mut details = HashMap::new();

        // Check if we have too many errors
        if self.error_count > 100 {
            status = HealthStatus::Degraded;
            error_message = Some("High error count detected".to_string());
        }

        // Check if operations are taking too long
        if let Some(avg_duration) = self.operation_times.last() {
            if avg_duration.as_millis() > 5000 {
                status = HealthStatus::Degraded;
                error_message = Some("Slow operation performance detected".to_string());
            }
        }

        // Check memory usage
        if self.metrics.memory_usage > 100 * 1024 * 1024 {
            // 100MB
            status = HealthStatus::Degraded;
            details.insert(
                "memory_usage_mb".to_string(),
                (self.metrics.memory_usage / 1024 / 1024).to_string(),
            );
        }

        // Check CPU usage
        if self.metrics.cpu_usage > 80.0 {
            status = HealthStatus::Degraded;
            details.insert(
                "cpu_usage_percent".to_string(),
                self.metrics.cpu_usage.to_string(),
            );
        }

        let duration = start_time.elapsed();
        let mut health_check =
            HealthCheck::new(self.component.clone(), status, duration.as_millis() as u64);

        if let Some(error) = error_message {
            health_check.error_message = Some(error);
        }

        for (key, value) in details {
            health_check.add_detail(key, value);
        }

        Ok(health_check)
    }

    /// Reset the collector.
    pub fn reset(&mut self) {
        self.operation_times.clear();
        self.error_count = 0;
        self.metrics = Metrics::new(self.component.clone());
    }
}

/// System resource monitor.
pub struct ResourceMonitor;

impl ResourceMonitor {
    /// Get current CPU usage percentage.
    pub fn get_cpu_usage() -> Result<f64, TelemetryError> {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        Ok(0.0)
    }

    /// Get current memory usage in bytes.
    pub fn get_memory_usage() -> Result<u64, TelemetryError> {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        Ok(0)
    }

    /// Get system uptime in seconds.
    pub fn get_uptime() -> Result<u64, TelemetryError> {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new("test-component".to_string());
        assert_eq!(metrics.component, "test-component");
        assert_eq!(metrics.operation_count, 0);
    }

    #[test]
    fn test_telemetry_collector() {
        let mut collector = TelemetryCollector::new("test-component".to_string());

        collector.record_operation(Duration::from_millis(100));
        collector.record_operation(Duration::from_millis(200));

        let metrics = collector.get_metrics();
        assert_eq!(metrics.operation_count, 2);
        assert_eq!(metrics.avg_operation_duration_ms, 150.0);
    }

    #[tokio::test]
    async fn test_health_check() {
        let collector = TelemetryCollector::new("test-component".to_string());
        let health_check = collector.health_check().await.unwrap();

        assert_eq!(health_check.component, "test-component");
        assert_eq!(health_check.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_performance_timer() {
        let timer = PerformanceTimer::start("test-operation".to_string());
        assert_eq!(timer.operation_name(), "test-operation");

        let duration = timer.finish();
        assert!(duration.as_millis() < 1000); // Should be very fast
    }
}
