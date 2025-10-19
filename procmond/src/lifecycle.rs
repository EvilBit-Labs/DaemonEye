//! Process lifecycle event detection infrastructure.
//!
//! This module provides the infrastructure for detecting process lifecycle events
//! such as process start, stop, and modification events. It maintains process state
//! between enumeration cycles and provides efficient comparison logic to identify
//! changes in the process landscape.

use collector_core::ProcessEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur during lifecycle tracking.
#[derive(Debug, Error)]
pub enum LifecycleTrackingError {
    /// Invalid process data provided
    #[error("Invalid process data for PID {pid}: {message}")]
    InvalidProcessData { pid: u32, message: String },

    /// Timestamp validation failed
    #[error("Timestamp validation failed for PID {pid}: {message}")]
    TimestampValidationFailed { pid: u32, message: String },

    /// PID reuse detection failed
    #[error("PID reuse detection failed for PID {pid}: {message}")]
    PidReuseDetectionFailed { pid: u32, message: String },
}

/// Result type for lifecycle tracking operations.
pub type LifecycleTrackingResult<T> = Result<T, LifecycleTrackingError>;

/// Process lifecycle event types.
///
/// These events represent different types of changes in the process landscape
/// that can be detected by comparing process snapshots between enumeration cycles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessLifecycleEvent {
    /// A new process has started
    Start {
        /// The process that started
        process: Box<ProcessSnapshot>,
        /// When the start was detected
        detected_at: SystemTime,
    },

    /// A process has stopped/terminated
    Stop {
        /// The process that stopped (last known state)
        process: Box<ProcessSnapshot>,
        /// When the stop was detected
        detected_at: SystemTime,
        /// Duration the process was running
        runtime_duration: Option<Duration>,
    },

    /// A process has been modified (command line, executable, etc.)
    Modified {
        /// The previous state of the process
        previous: Box<ProcessSnapshot>,
        /// The current state of the process
        current: Box<ProcessSnapshot>,
        /// When the modification was detected
        detected_at: SystemTime,
        /// Fields that were modified
        modified_fields: Vec<String>,
    },

    /// A suspicious event was detected (PID reuse, timestamp anomaly, etc.)
    Suspicious {
        /// The process involved in the suspicious event
        process: Box<ProcessSnapshot>,
        /// When the suspicious event was detected
        detected_at: SystemTime,
        /// Description of the suspicious behavior
        reason: String,
        /// Severity level of the suspicious event
        severity: SuspiciousEventSeverity,
    },
}

/// Severity levels for suspicious events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SuspiciousEventSeverity {
    /// Low severity - minor anomaly
    Low,
    /// Medium severity - notable anomaly
    Medium,
    /// High severity - significant anomaly
    High,
    /// Critical severity - highly suspicious
    Critical,
}

/// Snapshot of a process at a specific point in time.
///
/// This structure captures the essential state of a process for lifecycle
/// tracking and comparison purposes. It includes both basic process information
/// and metadata needed for detecting changes and anomalies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessSnapshot {
    /// Process identifier
    pub pid: u32,

    /// Parent process identifier
    pub ppid: Option<u32>,

    /// Process name
    pub name: String,

    /// Full executable path
    pub executable_path: Option<String>,

    /// Command line arguments
    pub command_line: Vec<String>,

    /// Process start time (from system)
    pub start_time: Option<SystemTime>,

    /// CPU usage percentage at snapshot time
    pub cpu_usage: Option<f64>,

    /// Memory usage in bytes at snapshot time
    pub memory_usage: Option<u64>,

    /// SHA-256 hash of executable
    pub executable_hash: Option<String>,

    /// User ID running the process
    pub user_id: Option<String>,

    /// Whether process metadata was accessible
    pub accessible: bool,

    /// Whether executable file exists
    pub file_exists: bool,

    /// When this snapshot was taken
    pub snapshot_time: SystemTime,

    /// Platform-specific metadata
    pub platform_metadata: Option<serde_json::Value>,
}

impl From<ProcessEvent> for ProcessSnapshot {
    fn from(event: ProcessEvent) -> Self {
        Self {
            pid: event.pid,
            ppid: event.ppid,
            name: event.name,
            executable_path: event.executable_path,
            command_line: event.command_line,
            start_time: event.start_time,
            cpu_usage: event.cpu_usage,
            memory_usage: event.memory_usage,
            executable_hash: event.executable_hash,
            user_id: event.user_id,
            accessible: event.accessible,
            file_exists: event.file_exists,
            snapshot_time: event.timestamp,
            platform_metadata: event.platform_metadata,
        }
    }
}

impl From<ProcessSnapshot> for ProcessEvent {
    fn from(snapshot: ProcessSnapshot) -> Self {
        Self {
            pid: snapshot.pid,
            ppid: snapshot.ppid,
            name: snapshot.name,
            executable_path: snapshot.executable_path,
            command_line: snapshot.command_line,
            start_time: snapshot.start_time,
            cpu_usage: snapshot.cpu_usage,
            memory_usage: snapshot.memory_usage,
            executable_hash: snapshot.executable_hash,
            user_id: snapshot.user_id,
            accessible: snapshot.accessible,
            file_exists: snapshot.file_exists,
            timestamp: snapshot.snapshot_time,
            platform_metadata: snapshot.platform_metadata,
        }
    }
}

/// Configuration for process lifecycle tracking.
#[derive(Debug, Clone)]
pub struct LifecycleTrackingConfig {
    /// Maximum age of process snapshots before they're considered stale
    pub max_snapshot_age: Duration,

    /// Minimum time between process start and detection to avoid false positives
    pub min_process_lifetime: Duration,

    /// Whether to detect PID reuse scenarios
    pub detect_pid_reuse: bool,

    /// Whether to track command line changes
    pub track_command_line_changes: bool,

    /// Whether to track executable path changes
    pub track_executable_changes: bool,

    /// Whether to track memory usage changes (threshold-based)
    pub track_memory_changes: bool,

    /// Memory usage change threshold (percentage)
    pub memory_change_threshold: f64,

    /// Whether to track CPU usage changes (threshold-based)
    pub track_cpu_changes: bool,

    /// CPU usage change threshold (percentage)
    pub cpu_change_threshold: f64,

    /// Maximum number of snapshots to keep in memory
    pub max_snapshots: usize,
}

impl Default for LifecycleTrackingConfig {
    fn default() -> Self {
        Self {
            max_snapshot_age: Duration::from_secs(300), // 5 minutes
            min_process_lifetime: Duration::from_millis(100),
            detect_pid_reuse: true,
            track_command_line_changes: true,
            track_executable_changes: true,
            track_memory_changes: false,   // Can be noisy
            memory_change_threshold: 50.0, // 50% change
            track_cpu_changes: false,      // Can be very noisy
            cpu_change_threshold: 25.0,    // 25% change
            max_snapshots: 10000,          // Reasonable limit for memory usage
        }
    }
}

/// Process lifecycle tracker for detecting process start, stop, and modification events.
///
/// This tracker maintains a snapshot of the process landscape between enumeration
/// cycles and provides efficient comparison logic to identify lifecycle changes.
/// It handles PID reuse detection, timestamp validation, and various types of
/// process modifications.
///
/// # Examples
///
/// ```rust
/// use procmond::lifecycle::{ProcessLifecycleTracker, LifecycleTrackingConfig};
/// use collector_core::ProcessEvent;
/// use std::time::SystemTime;
///
/// let config = LifecycleTrackingConfig::default();
/// let mut tracker = ProcessLifecycleTracker::new(config);
///
/// // First enumeration cycle
/// let processes1 = vec![/* ProcessEvent instances */];
/// let events1 = tracker.update_and_detect_changes(processes1).unwrap();
/// assert!(events1.is_empty()); // No changes on first run
///
/// // Second enumeration cycle with changes
/// let processes2 = vec![/* Modified ProcessEvent instances */];
/// let events2 = tracker.update_and_detect_changes(processes2).unwrap();
/// // events2 now contains detected lifecycle events
/// ```
pub struct ProcessLifecycleTracker {
    /// Configuration for lifecycle tracking
    config: LifecycleTrackingConfig,

    /// Current process snapshots indexed by PID
    current_snapshots: HashMap<u32, ProcessSnapshot>,

    /// Previous process snapshots for comparison
    previous_snapshots: HashMap<u32, ProcessSnapshot>,

    /// Last update timestamp
    last_update: Option<SystemTime>,

    /// Statistics for monitoring
    stats: LifecycleTrackingStats,
}

/// Statistics for lifecycle tracking operations.
#[derive(Debug, Clone, Default)]
pub struct LifecycleTrackingStats {
    /// Total number of updates performed
    pub total_updates: u64,

    /// Total number of start events detected
    pub start_events: u64,

    /// Total number of stop events detected
    pub stop_events: u64,

    /// Total number of modification events detected
    pub modification_events: u64,

    /// Total number of suspicious events detected
    pub suspicious_events: u64,

    /// Number of PID reuse events detected
    pub pid_reuse_events: u64,

    /// Number of timestamp anomalies detected
    pub timestamp_anomalies: u64,

    /// Average number of processes tracked per update
    pub avg_processes_tracked: f64,
}

impl ProcessLifecycleTracker {
    /// Creates a new process lifecycle tracker with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for lifecycle tracking behavior
    ///
    /// # Examples
    ///
    /// ```rust
    /// use procmond::lifecycle::{ProcessLifecycleTracker, LifecycleTrackingConfig};
    /// use std::time::Duration;
    ///
    /// let config = LifecycleTrackingConfig {
    ///     max_snapshot_age: Duration::from_secs(600),
    ///     detect_pid_reuse: true,
    ///     track_command_line_changes: true,
    ///     ..Default::default()
    /// };
    ///
    /// let tracker = ProcessLifecycleTracker::new(config);
    /// ```
    pub fn new(config: LifecycleTrackingConfig) -> Self {
        Self {
            config,
            current_snapshots: HashMap::new(),
            previous_snapshots: HashMap::new(),
            last_update: None,
            stats: LifecycleTrackingStats::default(),
        }
    }

    /// Updates the tracker with new process data and detects lifecycle changes.
    ///
    /// This is the main method for lifecycle tracking. It compares the new process
    /// data with the previous snapshot to detect start, stop, modification, and
    /// suspicious events.
    ///
    /// # Arguments
    ///
    /// * `processes` - Current process events from enumeration
    ///
    /// # Returns
    ///
    /// A vector of detected lifecycle events
    ///
    /// # Errors
    ///
    /// Returns `LifecycleTrackingError` if process data validation fails or
    /// if timestamp/PID reuse detection encounters errors.
    pub fn update_and_detect_changes(
        &mut self,
        processes: Vec<ProcessEvent>,
    ) -> LifecycleTrackingResult<Vec<ProcessLifecycleEvent>> {
        let update_time = SystemTime::now();
        let mut events = Vec::new();

        // Convert processes to snapshots
        let new_snapshots: HashMap<u32, ProcessSnapshot> = processes
            .into_iter()
            .map(|p| (p.pid, ProcessSnapshot::from(p)))
            .collect();

        // If this is the first update, just store the snapshots
        if self.last_update.is_none() {
            self.current_snapshots = new_snapshots;
            self.last_update = Some(update_time);
            self.stats.total_updates += 1;
            self.stats.avg_processes_tracked = self.current_snapshots.len() as f64;
            return Ok(events);
        }

        // Move current snapshots to previous
        self.previous_snapshots = std::mem::take(&mut self.current_snapshots);
        self.current_snapshots = new_snapshots;

        // Detect lifecycle events
        events.extend(self.detect_start_events(&update_time)?);
        events.extend(self.detect_stop_events(&update_time)?);
        events.extend(self.detect_modification_events(&update_time)?);
        events.extend(self.detect_suspicious_events(&update_time)?);

        // Update statistics
        self.update_statistics(&events);
        self.last_update = Some(update_time);

        // Clean up old snapshots if needed
        self.cleanup_old_snapshots();

        debug!(
            "Lifecycle tracking update completed: {} events detected, {} processes tracked",
            events.len(),
            self.current_snapshots.len()
        );

        Ok(events)
    }

    /// Returns current tracking statistics.
    pub fn stats(&self) -> &LifecycleTrackingStats {
        &self.stats
    }

    /// Returns the number of processes currently being tracked.
    pub fn tracked_process_count(&self) -> usize {
        self.current_snapshots.len()
    }

    /// Clears all tracking state (useful for testing or reset scenarios).
    pub fn clear(&mut self) {
        self.current_snapshots.clear();
        self.previous_snapshots.clear();
        self.last_update = None;
        debug!("Process lifecycle tracker state cleared");
    }
}

// Private implementation methods
impl ProcessLifecycleTracker {
    /// Detects process start events by finding PIDs in current but not in previous.
    fn detect_start_events(
        &self,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Vec<ProcessLifecycleEvent>> {
        let mut events = Vec::with_capacity(self.current_snapshots.len());

        for (pid, snapshot) in &self.current_snapshots {
            if !self.previous_snapshots.contains_key(pid) {
                // Validate that this is a legitimate start event
                if let Err(e) = self.validate_start_event(snapshot) {
                    warn!("Invalid start event for PID {}: {}", pid, e);
                    continue;
                }

                events.push(ProcessLifecycleEvent::Start {
                    process: Box::new(snapshot.clone()),
                    detected_at: *update_time,
                });
            }
        }

        Ok(events)
    }

    /// Detects process stop events by finding PIDs in previous but not in current.
    fn detect_stop_events(
        &self,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Vec<ProcessLifecycleEvent>> {
        let mut events = Vec::with_capacity(self.previous_snapshots.len());

        for (pid, snapshot) in &self.previous_snapshots {
            if !self.current_snapshots.contains_key(pid) {
                // Calculate runtime duration if possible
                let runtime_duration = if let Some(start_time) = snapshot.start_time {
                    update_time.duration_since(start_time).ok()
                } else {
                    None
                };

                events.push(ProcessLifecycleEvent::Stop {
                    process: Box::new(snapshot.clone()),
                    detected_at: *update_time,
                    runtime_duration,
                });
            }
        }

        Ok(events)
    }

    /// Detects process modification events by comparing snapshots.
    fn detect_modification_events(
        &self,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Vec<ProcessLifecycleEvent>> {
        let mut events = Vec::with_capacity(self.current_snapshots.len());

        for (pid, current_snapshot) in &self.current_snapshots {
            if let Some(previous_snapshot) = self.previous_snapshots.get(pid) {
                let modified_fields = self.compare_snapshots(previous_snapshot, current_snapshot);

                if !modified_fields.is_empty() {
                    events.push(ProcessLifecycleEvent::Modified {
                        previous: Box::new(previous_snapshot.clone()),
                        current: Box::new(current_snapshot.clone()),
                        detected_at: *update_time,
                        modified_fields,
                    });
                }
            }
        }

        Ok(events)
    }

    /// Detects suspicious events like PID reuse or timestamp anomalies.
    fn detect_suspicious_events(
        &self,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Vec<ProcessLifecycleEvent>> {
        let mut events = Vec::with_capacity(self.current_snapshots.len());

        if !self.config.detect_pid_reuse {
            return Ok(events);
        }

        for (pid, current_snapshot) in &self.current_snapshots {
            if let Some(previous_snapshot) = self.previous_snapshots.get(pid) {
                // Check for PID reuse (same PID, different process)
                if let Some(suspicious_event) =
                    self.detect_pid_reuse(previous_snapshot, current_snapshot, update_time)?
                {
                    events.push(suspicious_event);
                }

                // Check for timestamp anomalies
                if let Some(suspicious_event) =
                    self.detect_timestamp_anomaly(current_snapshot, update_time)?
                {
                    events.push(suspicious_event);
                }
            }
        }

        Ok(events)
    }

    /// Validates that a start event is legitimate.
    fn validate_start_event(&self, snapshot: &ProcessSnapshot) -> LifecycleTrackingResult<()> {
        // Check minimum process lifetime
        if let Some(start_time) = snapshot.start_time {
            let lifetime = snapshot
                .snapshot_time
                .duration_since(start_time)
                .map_err(|_| LifecycleTrackingError::TimestampValidationFailed {
                    pid: snapshot.pid,
                    message: "Process start time is in the future".to_string(),
                })?;

            if lifetime < self.config.min_process_lifetime {
                return Err(LifecycleTrackingError::TimestampValidationFailed {
                    pid: snapshot.pid,
                    message: format!("Process lifetime too short: {:?}", lifetime),
                });
            }
        }

        Ok(())
    }

    /// Compares two snapshots and returns a list of modified fields.
    fn compare_snapshots(
        &self,
        previous: &ProcessSnapshot,
        current: &ProcessSnapshot,
    ) -> Vec<String> {
        let mut modified_fields = Vec::with_capacity(8);

        // Check command line changes
        if self.config.track_command_line_changes && previous.command_line != current.command_line {
            modified_fields.push("command_line".to_string());
        }

        // Check executable path changes
        if self.config.track_executable_changes
            && previous.executable_path != current.executable_path
        {
            modified_fields.push("executable_path".to_string());
        }

        // Check memory usage changes
        if self.config.track_memory_changes {
            if let (Some(prev_mem), Some(curr_mem)) = (previous.memory_usage, current.memory_usage)
            {
                if prev_mem == 0 {
                    if curr_mem > 0 {
                        modified_fields.push("memory_usage".to_string());
                    }
                } else {
                    let change_percent =
                        ((curr_mem as f64 - prev_mem as f64) / prev_mem as f64).abs() * 100.0;
                    if change_percent > self.config.memory_change_threshold {
                        modified_fields.push("memory_usage".to_string());
                    }
                }
            }
        }

        // Check CPU usage changes
        if self.config.track_cpu_changes {
            if let (Some(prev_cpu), Some(curr_cpu)) = (previous.cpu_usage, current.cpu_usage) {
                if prev_cpu == 0.0 {
                    if curr_cpu > 0.0 {
                        modified_fields.push("cpu_usage".to_string());
                    }
                } else {
                    let change_percent = ((curr_cpu - prev_cpu) / prev_cpu).abs() * 100.0;
                    if change_percent > self.config.cpu_change_threshold {
                        modified_fields.push("cpu_usage".to_string());
                    }
                }
            }
        }

        // Check executable hash changes (potential process hollowing)
        if previous.executable_hash != current.executable_hash
            && previous.executable_hash.is_some()
            && current.executable_hash.is_some()
        {
            modified_fields.push("executable_hash".to_string());
        }

        // Check user ID changes (potential privilege escalation)
        if previous.user_id != current.user_id {
            modified_fields.push("user_id".to_string());
        }

        modified_fields
    }

    /// Detects PID reuse scenarios.
    fn detect_pid_reuse(
        &self,
        previous: &ProcessSnapshot,
        current: &ProcessSnapshot,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Option<ProcessLifecycleEvent>> {
        // Check if this looks like PID reuse
        let is_pid_reuse = previous.name != current.name
            || previous.executable_path != current.executable_path
            || previous.start_time != current.start_time;

        if is_pid_reuse {
            let severity = if previous.executable_path != current.executable_path {
                SuspiciousEventSeverity::High
            } else if previous.name != current.name {
                SuspiciousEventSeverity::Medium
            } else {
                SuspiciousEventSeverity::Low
            };

            return Ok(Some(ProcessLifecycleEvent::Suspicious {
                process: Box::new(current.clone()),
                detected_at: *update_time,
                reason: format!(
                    "Potential PID reuse detected: previous process '{}' vs current '{}'",
                    previous.name, current.name
                ),
                severity,
            }));
        }

        Ok(None)
    }

    /// Detects timestamp anomalies.
    fn detect_timestamp_anomaly(
        &self,
        snapshot: &ProcessSnapshot,
        update_time: &SystemTime,
    ) -> LifecycleTrackingResult<Option<ProcessLifecycleEvent>> {
        if let Some(start_time) = snapshot.start_time {
            // Check if start time is in the future
            if start_time > *update_time {
                return Ok(Some(ProcessLifecycleEvent::Suspicious {
                    process: Box::new(snapshot.clone()),
                    detected_at: *update_time,
                    reason: "Process start time is in the future".to_string(),
                    severity: SuspiciousEventSeverity::High,
                }));
            }

            // Check if process is impossibly old (more than system uptime would allow)
            let age = update_time.duration_since(start_time).map_err(|_| {
                LifecycleTrackingError::TimestampValidationFailed {
                    pid: snapshot.pid,
                    message: "Failed to calculate process age".to_string(),
                }
            })?;

            // This is a simple heuristic - in practice, you might want to check actual system uptime
            if age > Duration::from_secs(365 * 24 * 3600) {
                // More than a year
                return Ok(Some(ProcessLifecycleEvent::Suspicious {
                    process: Box::new(snapshot.clone()),
                    detected_at: *update_time,
                    reason: format!("Process age seems unrealistic: {:?}", age),
                    severity: SuspiciousEventSeverity::Medium,
                }));
            }
        }

        Ok(None)
    }

    /// Updates tracking statistics based on detected events.
    fn update_statistics(&mut self, events: &[ProcessLifecycleEvent]) {
        self.stats.total_updates += 1;

        for event in events {
            match event {
                ProcessLifecycleEvent::Start { .. } => self.stats.start_events += 1,
                ProcessLifecycleEvent::Stop { .. } => self.stats.stop_events += 1,
                ProcessLifecycleEvent::Modified { .. } => self.stats.modification_events += 1,
                ProcessLifecycleEvent::Suspicious { reason, .. } => {
                    self.stats.suspicious_events += 1;
                    if reason.contains("PID reuse") {
                        self.stats.pid_reuse_events += 1;
                    } else if reason.contains("timestamp") || reason.contains("time") {
                        self.stats.timestamp_anomalies += 1;
                    }
                }
            }
        }

        // Update average processes tracked
        let current_count = self.current_snapshots.len() as f64;
        let total_updates = self.stats.total_updates as f64;
        self.stats.avg_processes_tracked =
            (self.stats.avg_processes_tracked * (total_updates - 1.0) + current_count)
                / total_updates;
    }

    /// Cleans up old snapshots to prevent memory growth.
    fn cleanup_old_snapshots(&mut self) {
        if self.current_snapshots.len() > self.config.max_snapshots {
            warn!(
                "Process snapshot count ({}) exceeds maximum ({}), clearing previous snapshots",
                self.current_snapshots.len(),
                self.config.max_snapshots
            );
            self.previous_snapshots.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn create_test_process_event(pid: u32, name: &str) -> ProcessEvent {
        ProcessEvent {
            pid,
            ppid: Some(1),
            name: name.to_string(),
            executable_path: Some(format!("/usr/bin/{}", name)),
            command_line: vec![name.to_string()],
            start_time: Some(SystemTime::now() - Duration::from_secs(60)),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }
    }

    #[test]
    fn test_process_snapshot_conversion() {
        let event = create_test_process_event(1234, "test_process");
        let snapshot = ProcessSnapshot::from(event.clone());
        let converted_back = ProcessEvent::from(snapshot);

        assert_eq!(event.pid, converted_back.pid);
        assert_eq!(event.name, converted_back.name);
        assert_eq!(event.executable_path, converted_back.executable_path);
    }

    #[test]
    fn test_lifecycle_tracker_creation() {
        let config = LifecycleTrackingConfig::default();
        let tracker = ProcessLifecycleTracker::new(config);

        assert_eq!(tracker.tracked_process_count(), 0);
        assert_eq!(tracker.stats().total_updates, 0);
    }

    #[test]
    fn test_first_update_no_events() {
        let config = LifecycleTrackingConfig::default();
        let mut tracker = ProcessLifecycleTracker::new(config);

        let processes = vec![
            create_test_process_event(1, "process1"),
            create_test_process_event(2, "process2"),
        ];

        let events = tracker.update_and_detect_changes(processes).unwrap();
        assert!(events.is_empty());
        assert_eq!(tracker.tracked_process_count(), 2);
        assert_eq!(tracker.stats().total_updates, 1);
    }

    #[test]
    fn test_process_start_detection() {
        let config = LifecycleTrackingConfig::default();
        let mut tracker = ProcessLifecycleTracker::new(config);

        // First update
        let processes1 = vec![create_test_process_event(1, "process1")];
        let events1 = tracker.update_and_detect_changes(processes1).unwrap();
        assert!(events1.is_empty());

        // Second update with new process
        let processes2 = vec![
            create_test_process_event(1, "process1"),
            create_test_process_event(2, "process2"),
        ];
        let events2 = tracker.update_and_detect_changes(processes2).unwrap();

        // Find the start event
        let start_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, ProcessLifecycleEvent::Start { .. }))
            .collect();
        assert_eq!(start_events.len(), 1);

        match start_events[0] {
            ProcessLifecycleEvent::Start { process, .. } => {
                assert_eq!(process.pid, 2);
                assert_eq!(process.name, "process2");
            }
            _ => panic!("Expected Start event"),
        }
    }

    #[test]
    fn test_process_stop_detection() {
        let config = LifecycleTrackingConfig::default();
        let mut tracker = ProcessLifecycleTracker::new(config);

        // First update
        let processes1 = vec![
            create_test_process_event(1, "process1"),
            create_test_process_event(2, "process2"),
        ];
        let events1 = tracker.update_and_detect_changes(processes1).unwrap();
        assert!(events1.is_empty());

        // Second update with one process removed
        let processes2 = vec![create_test_process_event(1, "process1")];
        let events2 = tracker.update_and_detect_changes(processes2).unwrap();

        // Find the stop event
        let stop_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, ProcessLifecycleEvent::Stop { .. }))
            .collect();
        assert_eq!(stop_events.len(), 1);

        match stop_events[0] {
            ProcessLifecycleEvent::Stop {
                process,
                runtime_duration,
                ..
            } => {
                assert_eq!(process.pid, 2);
                assert_eq!(process.name, "process2");
                assert!(runtime_duration.is_some());
            }
            _ => panic!("Expected Stop event"),
        }
    }

    #[test]
    fn test_process_modification_detection() {
        let config = LifecycleTrackingConfig {
            track_command_line_changes: true,
            ..Default::default()
        };
        let mut tracker = ProcessLifecycleTracker::new(config);

        // First update
        let processes1 = vec![create_test_process_event(1, "process1")];
        let events1 = tracker.update_and_detect_changes(processes1).unwrap();
        assert!(events1.is_empty());

        // Second update with modified command line
        let mut modified_process = create_test_process_event(1, "process1");
        modified_process.command_line = vec!["process1".to_string(), "--new-flag".to_string()];
        let processes2 = vec![modified_process];
        let events2 = tracker.update_and_detect_changes(processes2).unwrap();

        // Find the modification event
        let modified_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, ProcessLifecycleEvent::Modified { .. }))
            .collect();
        assert_eq!(modified_events.len(), 1);

        match modified_events[0] {
            ProcessLifecycleEvent::Modified {
                previous,
                current,
                modified_fields,
                ..
            } => {
                assert_eq!(previous.pid, 1);
                assert_eq!(current.pid, 1);
                assert!(modified_fields.contains(&"command_line".to_string()));
            }
            _ => panic!("Expected Modified event"),
        }
    }

    #[test]
    fn test_pid_reuse_detection() {
        let config = LifecycleTrackingConfig {
            detect_pid_reuse: true,
            ..Default::default()
        };
        let mut tracker = ProcessLifecycleTracker::new(config);

        // First update
        let processes1 = vec![create_test_process_event(1, "process1")];
        let events1 = tracker.update_and_detect_changes(processes1).unwrap();
        assert!(events1.is_empty());

        // Second update with same PID but different process
        let processes2 = vec![create_test_process_event(1, "different_process")];
        let events2 = tracker.update_and_detect_changes(processes2).unwrap();

        // Find the suspicious event
        let suspicious_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, ProcessLifecycleEvent::Suspicious { .. }))
            .collect();
        assert_eq!(suspicious_events.len(), 1);

        match suspicious_events[0] {
            ProcessLifecycleEvent::Suspicious {
                process,
                reason,
                severity,
                ..
            } => {
                assert_eq!(process.pid, 1);
                assert!(reason.contains("PID reuse"));
                assert_eq!(*severity, SuspiciousEventSeverity::High);
            }
            _ => panic!("Expected Suspicious event"),
        }
    }

    #[test]
    fn test_tracker_clear() {
        let config = LifecycleTrackingConfig::default();
        let mut tracker = ProcessLifecycleTracker::new(config);

        let processes = vec![create_test_process_event(1, "process1")];
        let _events = tracker.update_and_detect_changes(processes).unwrap();
        assert_eq!(tracker.tracked_process_count(), 1);

        tracker.clear();
        assert_eq!(tracker.tracked_process_count(), 0);
        assert!(tracker.last_update.is_none());
    }

    #[test]
    fn test_statistics_tracking() {
        let config = LifecycleTrackingConfig::default();
        let mut tracker = ProcessLifecycleTracker::new(config);

        // First update
        let processes1 = vec![create_test_process_event(1, "process1")];
        let _events1 = tracker.update_and_detect_changes(processes1).unwrap();

        // Second update with start and stop events
        let processes2 = vec![create_test_process_event(2, "process2")];
        let _events2 = tracker.update_and_detect_changes(processes2).unwrap();

        let stats = tracker.stats();
        assert_eq!(stats.total_updates, 2);
        assert_eq!(stats.start_events, 1);
        assert_eq!(stats.stop_events, 1);
        assert!(stats.avg_processes_tracked > 0.0);
    }
}
