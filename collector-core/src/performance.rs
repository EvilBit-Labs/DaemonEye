//! Performance monitoring and baseline establishment for collector-core framework.
//!
//! This module provides comprehensive performance monitoring capabilities including
//! throughput measurement, resource usage tracking, trigger event latency monitoring,
//! and baseline performance metrics collection for the collector-core framework.

use crate::{CollectionEvent, TriggerRequest};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Performance monitoring configuration.
#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    /// Enable performance monitoring
    pub enabled: bool,
    /// Metrics collection interval
    pub collection_interval: Duration,
    /// Maximum number of latency samples to keep
    pub max_latency_samples: usize,
    /// Maximum number of throughput samples to keep
    pub max_throughput_samples: usize,
    /// CPU usage monitoring interval
    pub cpu_monitoring_interval: Duration,
    /// Memory usage monitoring interval
    pub memory_monitoring_interval: Duration,
    /// Enable detailed trigger latency tracking
    pub enable_trigger_latency_tracking: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            collection_interval: Duration::from_secs(10),
            max_latency_samples: 1000,
            max_throughput_samples: 100,
            cpu_monitoring_interval: Duration::from_secs(5),
            memory_monitoring_interval: Duration::from_secs(5),
            enable_trigger_latency_tracking: true,
        }
    }
}

/// Throughput metrics for events per second monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputMetrics {
    /// Events processed per second
    pub events_per_second: f64,
    /// Peak events per second in the current window
    pub peak_events_per_second: f64,
    /// Average events per second over the monitoring period
    pub avg_events_per_second: f64,
    /// Total events processed
    pub total_events: u64,
    /// Timestamp of the measurement
    pub timestamp: SystemTime,
    /// Events processed in the current window
    pub window_events: u64,
    /// Window duration in seconds
    pub window_duration_secs: f64,
}

/// CPU usage metrics during continuous monitoring operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuUsageMetrics {
    /// Current CPU usage percentage
    pub current_cpu_percent: f64,
    /// Peak CPU usage percentage
    pub peak_cpu_percent: f64,
    /// Average CPU usage percentage
    pub avg_cpu_percent: f64,
    /// CPU usage samples
    pub samples: Vec<f64>,
    /// Timestamp of the measurement
    pub timestamp: SystemTime,
    /// CPU usage during process enumeration
    pub enumeration_cpu_percent: Option<f64>,
    /// CPU usage during event processing
    pub processing_cpu_percent: Option<f64>,
}

/// Memory usage metrics for process tracking and event generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUsageMetrics {
    /// Current memory usage in bytes
    pub current_memory_bytes: u64,
    /// Peak memory usage in bytes
    pub peak_memory_bytes: u64,
    /// Average memory usage in bytes
    pub avg_memory_bytes: u64,
    /// Memory usage samples
    pub samples: Vec<u64>,
    /// Timestamp of the measurement
    pub timestamp: SystemTime,
    /// Memory usage during process enumeration
    pub enumeration_memory_bytes: Option<u64>,
    /// Memory usage during event processing
    pub processing_memory_bytes: Option<u64>,
    /// Memory growth rate in bytes per second
    pub growth_rate_bytes_per_sec: f64,
}

/// Trigger event latency measurement and statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerLatencyMetrics {
    /// Average trigger generation latency in milliseconds
    pub avg_latency_ms: f64,
    /// Peak trigger generation latency in milliseconds
    pub peak_latency_ms: f64,
    /// 95th percentile latency in milliseconds
    pub p95_latency_ms: f64,
    /// 99th percentile latency in milliseconds
    pub p99_latency_ms: f64,
    /// Total trigger events processed
    pub total_triggers: u64,
    /// Timestamp of the measurement
    pub timestamp: SystemTime,
    /// Latency samples in milliseconds
    pub samples: Vec<f64>,
    /// Trigger generation rate per second
    pub triggers_per_second: f64,
}

/// Resource usage tracking for system impact assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsageMetrics {
    /// CPU usage metrics
    pub cpu: CpuUsageMetrics,
    /// Memory usage metrics
    pub memory: MemoryUsageMetrics,
    /// Throughput metrics
    pub throughput: ThroughputMetrics,
    /// Trigger latency metrics
    pub trigger_latency: Option<TriggerLatencyMetrics>,
    /// System load average (Unix systems)
    pub load_average: Option<[f64; 3]>,
    /// File descriptor usage
    pub file_descriptors: Option<u64>,
    /// Network connections count
    pub network_connections: Option<u64>,
    /// Timestamp of the measurement
    pub timestamp: SystemTime,
}

/// Performance baseline metrics for comparison and regression detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineMetrics {
    /// Baseline throughput (events per second)
    pub baseline_throughput: f64,
    /// Baseline CPU usage percentage
    pub baseline_cpu_percent: f64,
    /// Baseline memory usage in bytes
    pub baseline_memory_bytes: u64,
    /// Baseline trigger latency in milliseconds
    pub baseline_trigger_latency_ms: f64,
    /// Timestamp when baseline was established
    pub established_at: SystemTime,
    /// Number of samples used to establish baseline
    pub sample_count: usize,
    /// Confidence interval for baseline metrics
    pub confidence_interval: f64,
}

/// Performance monitoring statistics and state.
#[derive(Debug)]
pub struct PerformanceMonitor {
    config: PerformanceConfig,

    // Event throughput tracking
    events_processed: Arc<AtomicU64>,
    events_in_current_window: Arc<AtomicU64>,
    window_start: Arc<Mutex<Instant>>,
    throughput_samples: Arc<Mutex<VecDeque<ThroughputMetrics>>>,

    // CPU usage tracking
    cpu_samples: Arc<Mutex<VecDeque<f64>>>,
    peak_cpu_usage: Arc<Mutex<f64>>,

    // Memory usage tracking
    memory_samples: Arc<Mutex<VecDeque<u64>>>,
    peak_memory_usage: Arc<Mutex<u64>>,
    last_memory_measurement: Arc<Mutex<Option<(u64, Instant)>>>,

    // Trigger latency tracking
    trigger_latency_samples: Arc<Mutex<VecDeque<f64>>>,
    trigger_count: Arc<AtomicU64>,

    // Resource usage aggregation
    current_metrics: Arc<RwLock<Option<ResourceUsageMetrics>>>,
    baseline_metrics: Arc<RwLock<Option<BaselineMetrics>>>,

    // Performance timers
    active_timers: Arc<Mutex<HashMap<String, Instant>>>,
}

impl PerformanceMonitor {
    /// Creates a new performance monitor with the specified configuration.
    pub fn new(config: PerformanceConfig) -> Self {
        Self {
            config,
            events_processed: Arc::new(AtomicU64::new(0)),
            events_in_current_window: Arc::new(AtomicU64::new(0)),
            window_start: Arc::new(Mutex::new(Instant::now())),
            throughput_samples: Arc::new(Mutex::new(VecDeque::new())),
            cpu_samples: Arc::new(Mutex::new(VecDeque::new())),
            peak_cpu_usage: Arc::new(Mutex::new(0.0)),
            memory_samples: Arc::new(Mutex::new(VecDeque::new())),
            peak_memory_usage: Arc::new(Mutex::new(0)),
            last_memory_measurement: Arc::new(Mutex::new(None)),
            trigger_latency_samples: Arc::new(Mutex::new(VecDeque::new())),
            trigger_count: Arc::new(AtomicU64::new(0)),
            current_metrics: Arc::new(RwLock::new(None)),
            baseline_metrics: Arc::new(RwLock::new(None)),
            active_timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Records an event being processed for throughput monitoring.
    pub fn record_event(&self, _event: &CollectionEvent) {
        if !self.config.enabled {
            return;
        }

        self.events_processed.fetch_add(1, Ordering::Relaxed);
        self.events_in_current_window
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a trigger event for latency monitoring.
    pub fn record_trigger_start(&self, trigger_id: &str) {
        if !self.config.enabled || !self.config.enable_trigger_latency_tracking {
            return;
        }

        let mut timers = self.active_timers.lock().unwrap();
        timers.insert(trigger_id.to_string(), Instant::now());
    }

    /// Records trigger completion and calculates latency.
    pub fn record_trigger_completion(&self, trigger_id: &str, _trigger: &TriggerRequest) {
        if !self.config.enabled || !self.config.enable_trigger_latency_tracking {
            return;
        }

        let start_time = {
            let mut timers = self.active_timers.lock().unwrap();
            timers.remove(trigger_id)
        };

        if let Some(start) = start_time {
            let latency_ms = start.elapsed().as_millis() as f64;

            let mut samples = self.trigger_latency_samples.lock().unwrap();
            samples.push_back(latency_ms);

            if samples.len() > self.config.max_latency_samples {
                samples.pop_front();
            }

            self.trigger_count.fetch_add(1, Ordering::Relaxed);

            debug!(
                trigger_id = trigger_id,
                latency_ms = latency_ms,
                "Recorded trigger latency"
            );
        }
    }

    /// Updates CPU usage metrics.
    pub fn update_cpu_usage(&self, cpu_percent: f64) {
        if !self.config.enabled {
            return;
        }

        let mut samples = self.cpu_samples.lock().unwrap();
        samples.push_back(cpu_percent);

        if samples.len() > self.config.max_throughput_samples {
            samples.pop_front();
        }

        let mut peak = self.peak_cpu_usage.lock().unwrap();
        if cpu_percent > *peak {
            *peak = cpu_percent;
        }
    }

    /// Updates memory usage metrics.
    pub fn update_memory_usage(&self, memory_bytes: u64) {
        if !self.config.enabled {
            return;
        }

        let now = Instant::now();
        let mut samples = self.memory_samples.lock().unwrap();
        samples.push_back(memory_bytes);

        if samples.len() > self.config.max_throughput_samples {
            samples.pop_front();
        }

        let mut peak = self.peak_memory_usage.lock().unwrap();
        if memory_bytes > *peak {
            *peak = memory_bytes;
        }

        // Update memory growth rate
        let mut last_measurement = self.last_memory_measurement.lock().unwrap();
        *last_measurement = Some((memory_bytes, now));
    }

    /// Calculates current throughput metrics.
    pub fn calculate_throughput_metrics(&self) -> ThroughputMetrics {
        let window_start = *self.window_start.lock().unwrap();
        let window_duration = window_start.elapsed();
        let window_events = self.events_in_current_window.load(Ordering::Relaxed);
        let total_events = self.events_processed.load(Ordering::Relaxed);

        let events_per_second = if window_duration.as_secs_f64() > 0.0 {
            window_events as f64 / window_duration.as_secs_f64()
        } else {
            0.0
        };

        let samples = self.throughput_samples.lock().unwrap();
        let (peak_events_per_second, avg_events_per_second) = if samples.is_empty() {
            (events_per_second, events_per_second)
        } else {
            let peak = samples
                .iter()
                .map(|s| s.events_per_second)
                .fold(0.0, f64::max);
            let avg =
                samples.iter().map(|s| s.events_per_second).sum::<f64>() / samples.len() as f64;
            (peak.max(events_per_second), avg)
        };

        ThroughputMetrics {
            events_per_second,
            peak_events_per_second,
            avg_events_per_second,
            total_events,
            timestamp: SystemTime::now(),
            window_events,
            window_duration_secs: window_duration.as_secs_f64(),
        }
    }

    /// Calculates current CPU usage metrics.
    pub fn calculate_cpu_metrics(&self) -> CpuUsageMetrics {
        let samples = self.cpu_samples.lock().unwrap();
        let peak_cpu = *self.peak_cpu_usage.lock().unwrap();

        let (current_cpu, avg_cpu) = if samples.is_empty() {
            (0.0, 0.0)
        } else {
            let current = *samples.back().unwrap_or(&0.0);
            let avg = samples.iter().sum::<f64>() / samples.len() as f64;
            (current, avg)
        };

        CpuUsageMetrics {
            current_cpu_percent: current_cpu,
            peak_cpu_percent: peak_cpu,
            avg_cpu_percent: avg_cpu,
            samples: samples.iter().cloned().collect(),
            timestamp: SystemTime::now(),
            enumeration_cpu_percent: None, // Would be set during specific operations
            processing_cpu_percent: None,  // Would be set during specific operations
        }
    }

    /// Calculates current memory usage metrics.
    pub fn calculate_memory_metrics(&self) -> MemoryUsageMetrics {
        let samples = self.memory_samples.lock().unwrap();
        let peak_memory = *self.peak_memory_usage.lock().unwrap();

        let (current_memory, avg_memory) = if samples.is_empty() {
            (0, 0)
        } else {
            let current = *samples.back().unwrap_or(&0);
            let avg = samples.iter().sum::<u64>() / samples.len() as u64;
            (current, avg)
        };

        // Calculate memory growth rate
        let growth_rate = {
            let last_measurement = self.last_memory_measurement.lock().unwrap();
            if let Some((last_memory, last_time)) = *last_measurement {
                let time_diff = last_time.elapsed().as_secs_f64();
                if time_diff > 0.0 && current_memory >= last_memory {
                    (current_memory - last_memory) as f64 / time_diff
                } else {
                    0.0
                }
            } else {
                0.0
            }
        };

        MemoryUsageMetrics {
            current_memory_bytes: current_memory,
            peak_memory_bytes: peak_memory,
            avg_memory_bytes: avg_memory,
            samples: samples.iter().cloned().collect(),
            timestamp: SystemTime::now(),
            enumeration_memory_bytes: None, // Would be set during specific operations
            processing_memory_bytes: None,  // Would be set during specific operations
            growth_rate_bytes_per_sec: growth_rate,
        }
    }

    /// Calculates current trigger latency metrics.
    pub fn calculate_trigger_latency_metrics(&self) -> Option<TriggerLatencyMetrics> {
        if !self.config.enable_trigger_latency_tracking {
            return None;
        }

        let samples = self.trigger_latency_samples.lock().unwrap();
        let trigger_count = self.trigger_count.load(Ordering::Relaxed);

        if samples.is_empty() {
            return None;
        }

        let mut sorted_samples: Vec<f64> = samples.iter().cloned().collect();
        sorted_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let avg_latency = sorted_samples.iter().sum::<f64>() / sorted_samples.len() as f64;
        let peak_latency = sorted_samples.last().cloned().unwrap_or(0.0);

        let p95_index = (sorted_samples.len() as f64 * 0.95) as usize;
        let p99_index = (sorted_samples.len() as f64 * 0.99) as usize;

        let p95_latency = sorted_samples
            .get(p95_index.saturating_sub(1))
            .cloned()
            .unwrap_or(0.0);
        let p99_latency = sorted_samples
            .get(p99_index.saturating_sub(1))
            .cloned()
            .unwrap_or(0.0);

        // Calculate triggers per second based on recent activity
        let triggers_per_second = if samples.len() > 1 {
            // Estimate based on sample collection rate
            trigger_count as f64 / self.config.collection_interval.as_secs_f64()
        } else {
            0.0
        };

        Some(TriggerLatencyMetrics {
            avg_latency_ms: avg_latency,
            peak_latency_ms: peak_latency,
            p95_latency_ms: p95_latency,
            p99_latency_ms: p99_latency,
            total_triggers: trigger_count,
            timestamp: SystemTime::now(),
            samples: sorted_samples,
            triggers_per_second,
        })
    }

    /// Collects comprehensive resource usage metrics.
    pub async fn collect_resource_metrics(&self) -> ResourceUsageMetrics {
        let cpu_metrics = self.calculate_cpu_metrics();
        let memory_metrics = self.calculate_memory_metrics();
        let throughput_metrics = self.calculate_throughput_metrics();
        let trigger_latency_metrics = self.calculate_trigger_latency_metrics();

        let metrics = ResourceUsageMetrics {
            cpu: cpu_metrics,
            memory: memory_metrics,
            throughput: throughput_metrics,
            trigger_latency: trigger_latency_metrics,
            load_average: Self::get_load_average(),
            file_descriptors: Self::get_file_descriptor_count(),
            network_connections: Self::get_network_connection_count(),
            timestamp: SystemTime::now(),
        };

        // Update current metrics
        let mut current = self.current_metrics.write().await;
        *current = Some(metrics.clone());

        metrics
    }

    /// Establishes baseline performance metrics.
    pub async fn establish_baseline(&self, sample_count: usize) -> anyhow::Result<BaselineMetrics> {
        info!(
            "Establishing performance baseline with {} samples",
            sample_count
        );

        let mut throughput_samples = Vec::new();
        let mut cpu_samples = Vec::new();
        let mut memory_samples = Vec::new();
        let mut trigger_latency_samples = Vec::new();

        // Collect samples over time
        for i in 0..sample_count {
            let metrics = self.collect_resource_metrics().await;

            throughput_samples.push(metrics.throughput.events_per_second);
            cpu_samples.push(metrics.cpu.current_cpu_percent);
            memory_samples.push(metrics.memory.current_memory_bytes);

            if let Some(trigger_metrics) = &metrics.trigger_latency {
                trigger_latency_samples.push(trigger_metrics.avg_latency_ms);
            }

            debug!("Collected baseline sample {}/{}", i + 1, sample_count);

            // Wait between samples
            tokio::time::sleep(self.config.collection_interval).await;
        }

        // Calculate baseline metrics
        let baseline_throughput =
            throughput_samples.iter().sum::<f64>() / throughput_samples.len() as f64;
        let baseline_cpu = cpu_samples.iter().sum::<f64>() / cpu_samples.len() as f64;
        let baseline_memory = memory_samples.iter().sum::<u64>() / memory_samples.len() as u64;
        let baseline_trigger_latency = if trigger_latency_samples.is_empty() {
            0.0
        } else {
            trigger_latency_samples.iter().sum::<f64>() / trigger_latency_samples.len() as f64
        };

        // Calculate confidence interval (simplified standard deviation)
        let throughput_variance = throughput_samples
            .iter()
            .map(|x| (x - baseline_throughput).powi(2))
            .sum::<f64>()
            / throughput_samples.len() as f64;
        let confidence_interval = throughput_variance.sqrt();

        let baseline = BaselineMetrics {
            baseline_throughput,
            baseline_cpu_percent: baseline_cpu,
            baseline_memory_bytes: baseline_memory,
            baseline_trigger_latency_ms: baseline_trigger_latency,
            established_at: SystemTime::now(),
            sample_count,
            confidence_interval,
        };

        // Store baseline
        let mut baseline_metrics = self.baseline_metrics.write().await;
        *baseline_metrics = Some(baseline.clone());

        info!(
            throughput = baseline_throughput,
            cpu_percent = baseline_cpu,
            memory_mb = baseline_memory / (1024 * 1024),
            trigger_latency_ms = baseline_trigger_latency,
            "Performance baseline established"
        );

        Ok(baseline)
    }

    /// Compares current performance against baseline.
    pub async fn compare_to_baseline(&self) -> Option<PerformanceComparison> {
        let current_metrics = self.current_metrics.read().await;
        let baseline_metrics = self.baseline_metrics.read().await;

        if let (Some(current), Some(baseline)) =
            (current_metrics.as_ref(), baseline_metrics.as_ref())
        {
            Some(PerformanceComparison {
                throughput_ratio: current.throughput.events_per_second
                    / baseline.baseline_throughput,
                cpu_ratio: current.cpu.current_cpu_percent / baseline.baseline_cpu_percent,
                memory_ratio: current.memory.current_memory_bytes as f64
                    / baseline.baseline_memory_bytes as f64,
                trigger_latency_ratio: current
                    .trigger_latency
                    .as_ref()
                    .map(|t| t.avg_latency_ms / baseline.baseline_trigger_latency_ms)
                    .unwrap_or(1.0),
                baseline_established_at: baseline.established_at,
                comparison_timestamp: SystemTime::now(),
            })
        } else {
            None
        }
    }

    /// Resets the monitoring window for throughput calculations.
    pub fn reset_window(&self) {
        let mut window_start = self.window_start.lock().unwrap();
        *window_start = Instant::now();
        self.events_in_current_window.store(0, Ordering::Relaxed);
    }

    /// Gets system load average (Unix systems only).
    fn get_load_average() -> Option<[f64; 3]> {
        #[cfg(unix)]
        {
            // In a real implementation, this would read from /proc/loadavg
            // For now, return placeholder values
            Some([0.1, 0.2, 0.3])
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Gets current file descriptor count.
    fn get_file_descriptor_count() -> Option<u64> {
        #[cfg(unix)]
        {
            // In a real implementation, this would count open file descriptors
            // For now, return a placeholder value
            Some(50)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Gets current network connection count.
    fn get_network_connection_count() -> Option<u64> {
        // In a real implementation, this would count network connections
        // For now, return a placeholder value
        Some(10)
    }

    /// Returns current performance statistics.
    pub async fn get_current_metrics(&self) -> Option<ResourceUsageMetrics> {
        let current = self.current_metrics.read().await;
        current.clone()
    }

    /// Returns baseline performance metrics.
    pub async fn get_baseline_metrics(&self) -> Option<BaselineMetrics> {
        let baseline = self.baseline_metrics.read().await;
        baseline.clone()
    }

    /// Checks if performance has degraded significantly from baseline.
    pub async fn check_performance_degradation(&self) -> Option<PerformanceDegradation> {
        let comparison = self.compare_to_baseline().await?;

        let mut degradations = Vec::new();

        // Check for significant degradations (>20% worse than baseline)
        if comparison.throughput_ratio < 0.8 {
            degradations.push(DegradationType::ThroughputDegradation {
                current_ratio: comparison.throughput_ratio,
                threshold: 0.8,
            });
        }

        if comparison.cpu_ratio > 1.2 {
            degradations.push(DegradationType::CpuUsageIncrease {
                current_ratio: comparison.cpu_ratio,
                threshold: 1.2,
            });
        }

        if comparison.memory_ratio > 1.2 {
            degradations.push(DegradationType::MemoryUsageIncrease {
                current_ratio: comparison.memory_ratio,
                threshold: 1.2,
            });
        }

        if comparison.trigger_latency_ratio > 1.5 {
            degradations.push(DegradationType::TriggerLatencyIncrease {
                current_ratio: comparison.trigger_latency_ratio,
                threshold: 1.5,
            });
        }

        if degradations.is_empty() {
            None
        } else {
            Some(PerformanceDegradation {
                degradations,
                comparison,
                detected_at: SystemTime::now(),
            })
        }
    }
}

/// Performance comparison between current metrics and baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceComparison {
    /// Throughput ratio (current / baseline)
    pub throughput_ratio: f64,
    /// CPU usage ratio (current / baseline)
    pub cpu_ratio: f64,
    /// Memory usage ratio (current / baseline)
    pub memory_ratio: f64,
    /// Trigger latency ratio (current / baseline)
    pub trigger_latency_ratio: f64,
    /// When baseline was established
    pub baseline_established_at: SystemTime,
    /// When comparison was made
    pub comparison_timestamp: SystemTime,
}

/// Types of performance degradation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DegradationType {
    ThroughputDegradation { current_ratio: f64, threshold: f64 },
    CpuUsageIncrease { current_ratio: f64, threshold: f64 },
    MemoryUsageIncrease { current_ratio: f64, threshold: f64 },
    TriggerLatencyIncrease { current_ratio: f64, threshold: f64 },
}

/// Performance degradation detection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceDegradation {
    /// Types of degradation detected
    pub degradations: Vec<DegradationType>,
    /// Performance comparison data
    pub comparison: PerformanceComparison,
    /// When degradation was detected
    pub detected_at: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProcessEvent;

    #[test]
    fn test_performance_config_default() {
        let config = PerformanceConfig::default();
        assert!(config.enabled);
        assert_eq!(config.collection_interval, Duration::from_secs(10));
        assert_eq!(config.max_latency_samples, 1000);
        assert_eq!(config.max_throughput_samples, 100);
        assert!(config.enable_trigger_latency_tracking);
    }

    #[test]
    fn test_performance_monitor_creation() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        assert_eq!(monitor.events_processed.load(Ordering::Relaxed), 0);
        assert_eq!(monitor.trigger_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_event_recording() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("test_hash".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        monitor.record_event(&event);
        assert_eq!(monitor.events_processed.load(Ordering::Relaxed), 1);
        assert_eq!(monitor.events_in_current_window.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_cpu_usage_update() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        monitor.update_cpu_usage(25.5);
        monitor.update_cpu_usage(30.0);
        monitor.update_cpu_usage(20.0);

        let metrics = monitor.calculate_cpu_metrics();
        assert_eq!(metrics.current_cpu_percent, 20.0);
        assert_eq!(metrics.peak_cpu_percent, 30.0);
        assert!((metrics.avg_cpu_percent - 25.166666666666668).abs() < 0.001);
    }

    #[test]
    fn test_memory_usage_update() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        monitor.update_memory_usage(1024 * 1024);
        monitor.update_memory_usage(2 * 1024 * 1024);
        monitor.update_memory_usage(1536 * 1024);

        let metrics = monitor.calculate_memory_metrics();
        assert_eq!(metrics.current_memory_bytes, 1536 * 1024);
        assert_eq!(metrics.peak_memory_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn test_trigger_latency_tracking() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        let trigger = TriggerRequest {
            trigger_id: "test_trigger".to_string(),
            target_collector: "test_collector".to_string(),
            analysis_type: crate::AnalysisType::YaraScan,
            priority: crate::TriggerPriority::Normal,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "test_correlation".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        monitor.record_trigger_start("test_trigger");
        std::thread::sleep(Duration::from_millis(10));
        monitor.record_trigger_completion("test_trigger", &trigger);

        let metrics = monitor.calculate_trigger_latency_metrics();
        assert!(metrics.is_some());

        let metrics = metrics.unwrap();
        assert_eq!(metrics.total_triggers, 1);
        assert!(metrics.avg_latency_ms >= 10.0);
        assert!(!metrics.samples.is_empty());
    }

    #[test]
    fn test_throughput_calculation() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        // Simulate processing events
        for _ in 0..100 {
            monitor.events_processed.fetch_add(1, Ordering::Relaxed);
            monitor
                .events_in_current_window
                .fetch_add(1, Ordering::Relaxed);
        }

        let metrics = monitor.calculate_throughput_metrics();
        assert_eq!(metrics.total_events, 100);
        assert_eq!(metrics.window_events, 100);
        assert!(metrics.events_per_second > 0.0);
    }

    #[tokio::test]
    async fn test_resource_metrics_collection() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        monitor.update_cpu_usage(25.0);
        monitor.update_memory_usage(1024 * 1024);

        let metrics = monitor.collect_resource_metrics().await;
        assert_eq!(metrics.cpu.current_cpu_percent, 25.0);
        assert_eq!(metrics.memory.current_memory_bytes, 1024 * 1024);
        // total_events is u64, so it's always >= 0, just verify it exists
        assert_eq!(metrics.throughput.total_events, 0);
    }

    #[test]
    fn test_window_reset() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);

        monitor
            .events_in_current_window
            .store(50, Ordering::Relaxed);
        monitor.reset_window();

        assert_eq!(monitor.events_in_current_window.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_disabled_monitoring() {
        let config = PerformanceConfig {
            enabled: false,
            ..Default::default()
        };
        let monitor = PerformanceMonitor::new(config);

        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("test_hash".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        monitor.record_event(&event);
        monitor.update_cpu_usage(50.0);
        monitor.update_memory_usage(2 * 1024 * 1024);

        // Should remain at 0 since monitoring is disabled
        assert_eq!(monitor.events_processed.load(Ordering::Relaxed), 0);
    }
}
