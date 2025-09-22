//! Performance telemetry and health monitoring.
//!
//! This module provides metrics collection, health monitoring, and performance
//! telemetry for `DaemonEye` components.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Telemetry collection errors.
#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
#[non_exhaustive]
pub enum TelemetryError {
    #[error("Metric collection failed: {0}")]
    Collection(String),

    #[error("Health check failed: {0}")]
    HealthCheck(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
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
    /// Custom metric data
    pub custom_data: HashMap<String, f64>,
    /// Legacy field name for API compatibility
    pub custom_values: HashMap<String, f64>,
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
            custom_data: HashMap::new(),
            custom_values: HashMap::new(),
        }
    }

    /// Add a custom metric.
    pub fn add_custom_metric(&mut self, name: String, value: f64) {
        self.custom_data.insert(name.clone(), value);
        self.custom_values.insert(name, value);
    }
}

/// Health status for a component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Health check result for a component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    total_operation_duration: Duration,
}

impl TelemetryCollector {
    /// Create a new telemetry collector.
    pub fn new(component: String) -> Self {
        Self {
            component: component.clone(),
            metrics: Metrics::new(component),
            operation_times: Vec::new(),
            error_count: 0,
            total_operation_duration: Duration::ZERO,
        }
    }

    /// Record an operation completion.
    #[allow(clippy::arithmetic_side_effects)]
    pub fn record_operation(&mut self, duration: Duration) {
        self.operation_times.push(duration);
        self.metrics.operation_count = self.metrics.operation_count.saturating_add(1);

        // Update running total and average operation duration (O(1))
        self.total_operation_duration = self.total_operation_duration.saturating_add(duration);
        if !self.operation_times.is_empty() {
            let duration_millis = self.total_operation_duration.as_millis();
            let len = self.operation_times.len();
            self.metrics.avg_operation_duration_ms =
                f64::from(u32::try_from(duration_millis).unwrap_or(0))
                    / f64::from(u32::try_from(len).unwrap_or(1).max(1));
        }
    }

    /// Record an error occurrence.
    pub const fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
        self.metrics.error_count = self.error_count;
    }

    /// Update system resource usage.
    pub const fn update_resource_usage(&mut self, cpu_usage: f64, memory_usage: u64) {
        self.metrics.cpu_usage = cpu_usage;
        self.metrics.memory_usage = memory_usage;
    }

    /// Add a custom metric.
    pub fn add_custom_metric(&mut self, name: String, value: f64) {
        self.metrics.add_custom_metric(name, value);
    }

    /// Get current metrics.
    pub const fn get_metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Perform a health check.
    pub fn health_check(&self) -> HealthCheck {
        let start_time = Instant::now();

        // Basic health checks
        let mut status = HealthStatus::Healthy;
        let mut error_message = None;
        let mut details = HashMap::new();

        // Check if we have too many errors
        if self.error_count > 100 {
            status = HealthStatus::Degraded;
            error_message = Some("High error count detected".to_owned());
        }

        // Check if operations are taking too long
        if let Some(avg_duration) = self.operation_times.last() {
            if avg_duration.as_millis() > 5000 {
                status = HealthStatus::Degraded;
                error_message = Some("Slow operation performance detected".to_owned());
            }
        }

        // Check memory usage
        #[allow(clippy::arithmetic_side_effects)]
        if self.metrics.memory_usage > 100 * 1024 * 1024 {
            // 100MB
            status = HealthStatus::Degraded;
            details.insert(
                "memory_usage_mb".to_owned(),
                (f64::from(u32::try_from(self.metrics.memory_usage).unwrap_or(0))
                    / (1024.0 * 1024.0))
                    .to_string(),
            );
        }

        // Check CPU usage
        if self.metrics.cpu_usage > 80.0 {
            status = HealthStatus::Degraded;
            details.insert(
                "cpu_usage_percent".to_owned(),
                self.metrics.cpu_usage.to_string(),
            );
        }

        let duration = start_time.elapsed();
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(0);
        let mut health_check = HealthCheck::new(self.component.clone(), status, duration_ms);

        if let Some(error) = error_message {
            health_check.error_message = Some(error);
        }

        for (key, value) in details {
            health_check.add_detail(key, value);
        }

        health_check
    }

    /// Perform a health check synchronously (for CLI usage).
    pub fn health_check_blocking(&self) -> HealthCheck {
        let start_time = Instant::now();

        // Basic health checks
        let mut status = HealthStatus::Healthy;
        let mut error_message = None;
        let mut details = HashMap::new();

        // Check if we have too many errors
        if self.error_count > 100 {
            status = HealthStatus::Degraded;
            error_message = Some("High error count detected".to_owned());
        }

        // Check if operations are taking too long
        if let Some(avg_duration) = self.operation_times.last() {
            if avg_duration.as_millis() > 5000 {
                status = HealthStatus::Degraded;
                error_message = Some("Slow operation performance detected".to_owned());
            }
        }

        // Check memory usage
        #[allow(clippy::arithmetic_side_effects)]
        if self.metrics.memory_usage > 100 * 1024 * 1024 {
            // 100MB
            status = HealthStatus::Degraded;
            details.insert(
                "memory_usage_mb".to_owned(),
                (f64::from(u32::try_from(self.metrics.memory_usage).unwrap_or(0))
                    / (1024.0 * 1024.0))
                    .to_string(),
            );
        }

        // Check CPU usage
        if self.metrics.cpu_usage > 80.0 {
            status = HealthStatus::Degraded;
            details.insert(
                "cpu_usage_percent".to_owned(),
                self.metrics.cpu_usage.to_string(),
            );
        }

        let duration = start_time.elapsed();
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(0);
        let mut health_check = HealthCheck::new(self.component.clone(), status, duration_ms);

        if let Some(error) = error_message {
            health_check.error_message = Some(error);
        }

        for (key, value) in details {
            health_check.add_detail(key, value);
        }

        health_check
    }

    /// Reset the collector.
    pub fn reset(&mut self) {
        self.operation_times.clear();
        self.error_count = 0;
        self.metrics = Metrics::new(self.component.clone());
        self.total_operation_duration = Duration::ZERO;
    }
}

/// System resource monitor.
pub struct ResourceMonitor;

impl ResourceMonitor {
    /// Get current CPU usage percentage.
    pub const fn get_cpu_usage() -> f64 {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        0.0
    }

    /// Get current memory usage in bytes.
    pub const fn get_memory_usage() -> u64 {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        0
    }

    /// Get system uptime in seconds.
    pub const fn get_uptime() -> u64 {
        // In a real implementation, this would use system APIs
        // For now, return a placeholder value
        0
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new("test-component".to_owned());
        assert_eq!(metrics.component, "test-component");
        assert_eq!(metrics.operation_count, 0);
        assert!((metrics.cpu_usage - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.memory_usage, 0);
        assert_eq!(metrics.error_count, 0);
        assert!(metrics.custom_data.is_empty());
    }

    #[test]
    fn test_metrics_custom_metrics() {
        let mut metrics = Metrics::new("test-component".to_owned());
        metrics.add_custom_metric("test_metric".to_owned(), 42.0);

        assert_eq!(metrics.custom_data.get("test_metric"), Some(&42.0));
        assert_eq!(metrics.custom_data.len(), 1);
    }

    #[test]
    fn test_metrics_serialization() {
        let mut metrics = Metrics::new("test-component".to_owned());
        metrics.add_custom_metric("test_metric".to_owned(), 42.0);

        let json = serde_json::to_string(&metrics).expect("Failed to serialize metrics");
        let deserialized: Metrics =
            serde_json::from_str(&json).expect("Failed to deserialize metrics");

        assert_eq!(metrics.component, deserialized.component);
        assert_eq!(metrics.custom_data, deserialized.custom_data);
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "degraded");
        assert_eq!(format!("{}", HealthStatus::Unhealthy), "unhealthy");
        assert_eq!(format!("{}", HealthStatus::Unknown), "unknown");
    }

    #[test]
    fn test_health_status_serialization() {
        let statuses = vec![
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
            HealthStatus::Unknown,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).expect("Failed to serialize status");
            let deserialized: HealthStatus =
                serde_json::from_str(&json).expect("Failed to deserialize status");
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_health_check_creation() {
        let health_check =
            HealthCheck::new("test-component".to_owned(), HealthStatus::Healthy, 100);

        assert_eq!(health_check.component, "test-component");
        assert_eq!(health_check.status, HealthStatus::Healthy);
        assert_eq!(health_check.duration_ms, 100);
        assert!(health_check.error_message.is_none());
        assert!(health_check.details.is_empty());
    }

    #[test]
    fn test_health_check_details() {
        let mut health_check =
            HealthCheck::new("test-component".to_owned(), HealthStatus::Healthy, 100);
        health_check.add_detail("key1".to_owned(), "value1".to_owned());
        health_check.add_detail("key2".to_owned(), "value2".to_owned());

        assert_eq!(health_check.details.len(), 2);
        assert_eq!(health_check.details.get("key1"), Some(&"value1".to_owned()));
        assert_eq!(health_check.details.get("key2"), Some(&"value2".to_owned()));
    }

    #[test]
    fn test_health_check_serialization() {
        let mut health_check =
            HealthCheck::new("test-component".to_owned(), HealthStatus::Healthy, 100);
        health_check.add_detail("key1".to_owned(), "value1".to_owned());

        let json = serde_json::to_string(&health_check).expect("Failed to serialize health check");
        let deserialized: HealthCheck =
            serde_json::from_str(&json).expect("Failed to deserialize health check");

        assert_eq!(health_check.component, deserialized.component);
        assert_eq!(health_check.status, deserialized.status);
        assert_eq!(health_check.details, deserialized.details);
    }

    #[test]
    fn test_telemetry_collector() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        collector.record_operation(Duration::from_millis(100));
        collector.record_operation(Duration::from_millis(200));

        let metrics = collector.get_metrics();
        assert_eq!(metrics.operation_count, 2);
        assert!((metrics.avg_operation_duration_ms - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_telemetry_collector_errors() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        collector.record_error();
        collector.record_error();

        let metrics = collector.get_metrics();
        assert_eq!(metrics.error_count, 2);
    }

    #[test]
    fn test_telemetry_collector_resource_usage() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        collector.update_resource_usage(50.0, 1024 * 1024); // 50% CPU, 1MB memory

        let metrics = collector.get_metrics();
        assert!((metrics.cpu_usage - 50.0).abs() < f64::EPSILON);
        assert_eq!(metrics.memory_usage, 1024 * 1024);
    }

    #[test]
    fn test_telemetry_collector_custom_metrics() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        collector.add_custom_metric("custom1".to_owned(), 10.0);
        collector.add_custom_metric("custom2".to_owned(), 20.0);

        let metrics = collector.get_metrics();
        assert_eq!(metrics.custom_values.get("custom1"), Some(&10.0));
        assert_eq!(metrics.custom_values.get("custom2"), Some(&20.0));
    }

    #[test]
    fn test_telemetry_collector_reset() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        collector.record_operation(Duration::from_millis(100));
        collector.record_error();
        collector.add_custom_metric("test".to_owned(), 42.0);

        collector.reset();

        let metrics = collector.get_metrics();
        assert_eq!(metrics.operation_count, 0);
        assert_eq!(metrics.error_count, 0);
        assert!(metrics.custom_values.is_empty());
    }

    #[tokio::test]
    async fn test_health_check_healthy() {
        let collector = TelemetryCollector::new("test-component".to_owned());
        let health_check = collector.health_check();

        assert_eq!(health_check.component, "test-component");
        assert_eq!(health_check.status, HealthStatus::Healthy);
        assert!(health_check.error_message.is_none());
    }

    #[tokio::test]
    async fn test_health_check_high_error_count() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        // Record more than 100 errors
        for _ in 0..101 {
            collector.record_error();
        }

        let health_check = collector.health_check();
        assert_eq!(health_check.status, HealthStatus::Degraded);
        assert!(health_check.error_message.is_some());
        assert!(
            health_check
                .error_message
                .expect("Expected error message")
                .contains("High error count")
        );
    }

    #[tokio::test]
    async fn test_health_check_slow_operations() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        // Record a slow operation (> 5000ms)
        collector.record_operation(Duration::from_millis(6000));

        let health_check = collector.health_check();
        assert_eq!(health_check.status, HealthStatus::Degraded);
        assert!(health_check.error_message.is_some());
        assert!(
            health_check
                .error_message
                .expect("Expected error message")
                .contains("Slow operation")
        );
    }

    #[tokio::test]
    async fn test_health_check_high_memory_usage() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        // Set high memory usage (> 100MB)
        collector.update_resource_usage(0.0, 200 * 1024 * 1024);

        let health_check = collector.health_check();
        assert_eq!(health_check.status, HealthStatus::Degraded);
        assert!(health_check.details.contains_key("memory_usage_mb"));
    }

    #[tokio::test]
    async fn test_health_check_high_cpu_usage() {
        let mut collector = TelemetryCollector::new("test-component".to_owned());

        // Set high CPU usage (> 80%)
        collector.update_resource_usage(90.0, 0);

        let health_check = collector.health_check();
        assert_eq!(health_check.status, HealthStatus::Degraded);
        assert!(health_check.details.contains_key("cpu_usage_percent"));
    }

    #[test]
    fn test_health_check_blocking() {
        let collector = TelemetryCollector::new("test-component".to_owned());
        let health_check = collector.health_check_blocking();

        assert_eq!(health_check.component, "test-component");
        assert_eq!(health_check.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_performance_timer() {
        let timer = PerformanceTimer::start("test-operation".to_owned());
        assert_eq!(timer.operation_name(), "test-operation");

        let duration = timer.finish();
        assert!(duration.as_millis() < 1000); // Should be very fast
    }

    #[test]
    fn test_resource_monitor() {
        let cpu_usage = ResourceMonitor::get_cpu_usage();
        let memory_usage = ResourceMonitor::get_memory_usage();
        let uptime = ResourceMonitor::get_uptime();

        assert!((cpu_usage - 0.0).abs() < f64::EPSILON);
        assert_eq!(memory_usage, 0);
        assert_eq!(uptime, 0);
    }

    #[test]
    fn test_telemetry_error_display() {
        let errors = vec![
            TelemetryError::Collection("test error".to_owned()),
            TelemetryError::HealthCheck("test error".to_owned()),
            TelemetryError::Serialization(serde_json::Error::io(std::io::Error::other(
                "test error",
            ))),
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(error_string.contains("test error"));
        }
    }
}
