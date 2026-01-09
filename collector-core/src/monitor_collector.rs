//! Monitor Collector framework for event-driven process monitoring.
//!
//! This module provides the shared infrastructure for implementing Monitor Collectors
//! that integrate process lifecycle tracking, trigger system coordination, and
//! analysis chain management while maintaining full compliance with the collector-core
//! EventSource trait.

use crate::{AnalysisChainConfig, EventSource, TriggerConfig};
use std::{
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

/// Monitor Collector configuration with comprehensive validation.
///
/// This configuration structure provides fine-grained control over all aspects
/// of a Monitor Collector's behavior, including performance tuning, security
/// boundaries, and operational parameters.
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::MonitorCollectorConfig;
/// use std::time::Duration;
///
/// let config = MonitorCollectorConfig {
///     collection_interval: Duration::from_secs(10),
///     max_events_in_flight: 2000,
///     enable_event_driven: true,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct MonitorCollectorConfig {
    /// Trigger system configuration
    pub trigger_config: TriggerConfig,
    /// Analysis chain configuration
    pub analysis_config: AnalysisChainConfig,
    /// Collection interval for process monitoring (minimum 1 second for performance)
    pub collection_interval: Duration,
    /// Maximum number of events in flight (bounded for memory safety)
    pub max_events_in_flight: usize,
    /// Timeout for graceful shutdown operations
    pub shutdown_timeout: Duration,
    /// Enable event-driven architecture for advanced correlation
    pub enable_event_driven: bool,
    /// Enable debug logging for troubleshooting
    pub enable_debug_logging: bool,
}

impl Default for MonitorCollectorConfig {
    fn default() -> Self {
        Self {
            trigger_config: TriggerConfig::default(),
            analysis_config: AnalysisChainConfig::default(),
            collection_interval: Duration::from_secs(30),
            max_events_in_flight: 1000,
            shutdown_timeout: Duration::from_secs(10),
            enable_event_driven: true,
            enable_debug_logging: false,
        }
    }
}

impl MonitorCollectorConfig {
    /// Validates the configuration parameters for security and performance constraints.
    ///
    /// # Errors
    ///
    /// Returns an error if any configuration parameter violates security or
    /// performance constraints.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.collection_interval < Duration::from_secs(1) {
            anyhow::bail!("Collection interval must be at least 1 second for performance safety");
        }

        if self.max_events_in_flight == 0 || self.max_events_in_flight > 100_000 {
            anyhow::bail!("Max events in flight must be between 1 and 100,000 for memory safety");
        }

        if self.shutdown_timeout > Duration::from_secs(300) {
            anyhow::bail!("Shutdown timeout must not exceed 5 minutes");
        }

        Ok(())
    }
}

/// Monitor Collector statistics for monitoring and diagnostics.
///
/// These statistics provide comprehensive visibility into a Monitor Collector's
/// operational state and performance characteristics for monitoring and alerting.
#[derive(Debug, Default)]
pub struct MonitorCollectorStats {
    /// Total collection cycles completed
    pub collection_cycles: AtomicU64,
    /// Total lifecycle events detected
    pub lifecycle_events: AtomicU64,
    /// Total trigger requests generated
    pub trigger_requests: AtomicU64,
    /// Total analysis workflows initiated
    pub analysis_workflows: AtomicU64,
    /// Number of events currently in flight
    pub events_in_flight: AtomicUsize,
    /// Collection errors encountered
    pub collection_errors: AtomicU64,
    /// Trigger generation errors
    pub trigger_errors: AtomicU64,
    /// Analysis coordination errors
    pub analysis_errors: AtomicU64,
    /// Backpressure events (events dropped or buffered due to backpressure)
    pub backpressure_events: AtomicU64,
}

/// Snapshot of Monitor Collector statistics.
///
/// This structure provides a consistent view of all Monitor Collector metrics
/// at a specific point in time for monitoring and diagnostics.
#[derive(Debug, Clone)]
pub struct MonitorCollectorStatsSnapshot {
    /// Total collection cycles completed
    pub collection_cycles: u64,
    /// Total lifecycle events detected
    pub lifecycle_events: u64,
    /// Total trigger requests generated
    pub trigger_requests: u64,
    /// Total analysis workflows initiated
    pub analysis_workflows: u64,
    /// Number of events currently in flight
    pub events_in_flight: usize,
    /// Collection errors encountered
    pub collection_errors: u64,
    /// Trigger generation errors
    pub trigger_errors: u64,
    /// Analysis coordination errors
    pub analysis_errors: u64,
    /// Backpressure events (events dropped or buffered due to backpressure)
    pub backpressure_events: u64,
}

impl MonitorCollectorStats {
    /// Returns a consistent snapshot of all statistics.
    pub fn snapshot(&self) -> MonitorCollectorStatsSnapshot {
        MonitorCollectorStatsSnapshot {
            collection_cycles: self.collection_cycles.load(Ordering::Relaxed),
            lifecycle_events: self.lifecycle_events.load(Ordering::Relaxed),
            trigger_requests: self.trigger_requests.load(Ordering::Relaxed),
            analysis_workflows: self.analysis_workflows.load(Ordering::Relaxed),
            events_in_flight: self.events_in_flight.load(Ordering::Relaxed),
            collection_errors: self.collection_errors.load(Ordering::Relaxed),
            trigger_errors: self.trigger_errors.load(Ordering::Relaxed),
            analysis_errors: self.analysis_errors.load(Ordering::Relaxed),
            backpressure_events: self.backpressure_events.load(Ordering::Relaxed),
        }
    }
}

/// Trait for Monitor Collector implementations.
///
/// This trait extends EventSource with Monitor Collector-specific functionality
/// for statistics reporting and health monitoring.
pub trait MonitorCollector: EventSource {
    /// Returns current runtime statistics.
    fn stats(&self) -> MonitorCollectorStatsSnapshot;

    /// Performs a health check specific to monitor collector functionality.
    ///
    /// This extends the base EventSource health_check with monitor-specific
    /// validation such as error rate checking and resource utilization.
    #[allow(async_fn_in_trait)]
    async fn monitor_health_check(&self) -> anyhow::Result<()> {
        // Default implementation checks error rates
        let stats = self.stats();

        let total_operations =
            stats.collection_cycles + stats.trigger_requests + stats.analysis_workflows;
        let total_errors = stats.collection_errors + stats.trigger_errors + stats.analysis_errors;

        if total_operations > 0 {
            let error_rate = total_errors as f64 / total_operations as f64;
            if error_rate > 0.2 {
                return Err(anyhow::anyhow!(
                    "High error rate detected: {:.2}% ({} errors out of {} operations)",
                    error_rate * 100.0,
                    total_errors,
                    total_operations
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        // Test valid configuration
        let valid_config = MonitorCollectorConfig::default();
        assert!(valid_config.validate().is_ok());

        // Test invalid collection interval
        let invalid_interval_config = MonitorCollectorConfig {
            collection_interval: Duration::from_millis(500),
            ..Default::default()
        };
        assert!(invalid_interval_config.validate().is_err());

        // Test invalid max events in flight
        let invalid_events_config = MonitorCollectorConfig {
            max_events_in_flight: 0,
            ..Default::default()
        };
        assert!(invalid_events_config.validate().is_err());

        // Test invalid shutdown timeout
        let invalid_timeout_config = MonitorCollectorConfig {
            shutdown_timeout: Duration::from_secs(400),
            ..Default::default()
        };
        assert!(invalid_timeout_config.validate().is_err());
    }

    #[test]
    fn test_stats_snapshot() {
        let stats = MonitorCollectorStats::default();

        // Modify some values
        stats.collection_cycles.store(10, Ordering::Relaxed);
        stats.lifecycle_events.store(25, Ordering::Relaxed);
        stats.events_in_flight.store(5, Ordering::Relaxed);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.collection_cycles, 10);
        assert_eq!(snapshot.lifecycle_events, 25);
        assert_eq!(snapshot.events_in_flight, 5);
        assert_eq!(snapshot.trigger_requests, 0);
        assert_eq!(snapshot.backpressure_events, 0);
    }
}
