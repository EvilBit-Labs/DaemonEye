//! Process collector trait and implementations for cross-platform process enumeration.
//!
//! This module provides a platform-agnostic interface for process enumeration through
//! the `ProcessCollector` trait, along with concrete implementations for different
//! platforms and collection strategies.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use std::time::SystemTime;
use sysinfo::{Pid, Process, System};
use thiserror::Error;
use tracing::{debug, error, warn};

/// Errors that can occur during process collection.
#[derive(Debug, Error)]
pub enum ProcessCollectionError {
    /// System-level enumeration failed
    #[error("System process enumeration failed: {message}")]
    SystemEnumerationFailed { message: String },

    /// Individual process access denied
    #[error("Access denied for process {pid}: {message}")]
    ProcessAccessDenied { pid: u32, message: String },

    /// Process no longer exists
    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },

    /// Invalid process data
    #[error("Invalid process data for PID {pid}: {message}")]
    InvalidProcessData { pid: u32, message: String },

    /// Platform-specific error
    #[error("Platform-specific error: {message}")]
    PlatformError { message: String },

    /// Timeout during collection
    #[error("Process collection timed out after {timeout_ms}ms")]
    CollectionTimeout { timeout_ms: u64 },
}

/// Result type for process collection operations.
pub type ProcessCollectionResult<T> = Result<T, ProcessCollectionError>;

/// Statistics about a process collection operation.
#[derive(Debug, Clone, Default)]
pub struct CollectionStats {
    /// Total number of processes found
    pub total_processes: usize,
    /// Number of processes successfully collected
    pub successful_collections: usize,
    /// Number of processes that were inaccessible
    pub inaccessible_processes: usize,
    /// Number of processes with invalid data
    pub invalid_processes: usize,
    /// Collection duration in milliseconds
    pub collection_duration_ms: u64,
}

/// Configuration for process collection behavior.
#[derive(Debug, Clone)]
pub struct ProcessCollectionConfig {
    /// Whether to collect enhanced metadata (CPU, memory, etc.)
    pub collect_enhanced_metadata: bool,
    /// Whether to compute executable hashes
    pub compute_executable_hashes: bool,
    /// Maximum number of processes to collect (0 = unlimited)
    pub max_processes: usize,
    /// Whether to skip system processes
    pub skip_system_processes: bool,
    /// Whether to skip kernel threads
    pub skip_kernel_threads: bool,
}

impl Default for ProcessCollectionConfig {
    fn default() -> Self {
        Self {
            collect_enhanced_metadata: true,
            compute_executable_hashes: false,
            max_processes: 0, // Unlimited
            skip_system_processes: false,
            skip_kernel_threads: false,
        }
    }
}

/// Platform-agnostic trait for process enumeration.
///
/// This trait abstracts the process collection methodology from the specific
/// platform implementation, enabling different collection strategies while
/// maintaining a consistent interface.
///
/// # Design Principles
///
/// - **Platform Agnostic**: Works across Linux, macOS, and Windows
/// - **Error Resilient**: Individual process failures don't stop collection
/// - **Configurable**: Supports different collection strategies and filters
/// - **Observable**: Provides detailed statistics and error reporting
/// - **Async**: Non-blocking operations suitable for high-frequency collection
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::process_collector::{ProcessCollector, SysinfoProcessCollector, ProcessCollectionConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = ProcessCollectionConfig::default();
///     let collector = SysinfoProcessCollector::new(config);
///
///     let (events, stats) = collector.collect_processes().await?;
///     println!("Collected {} processes", events.len());
///
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait ProcessCollector: Send + Sync {
    /// Returns the name of this collector implementation.
    fn name(&self) -> &'static str;

    /// Returns the platform capabilities of this collector.
    fn capabilities(&self) -> ProcessCollectorCapabilities;

    /// Collects process information and returns events with statistics.
    ///
    /// This method performs the actual process enumeration and converts
    /// the results to `ProcessEvent`s. Individual process failures are
    /// handled gracefully and reported in the statistics.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - Vector of successfully collected `ProcessEvent`s
    /// - `CollectionStats` with detailed collection information
    ///
    /// # Errors
    ///
    /// Returns an error only for critical system-level failures that
    /// prevent any process enumeration. Individual process access failures
    /// are handled gracefully and reported in statistics.
    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)>;

    /// Collects information for a specific process by PID.
    ///
    /// This method attempts to collect information for a single process.
    /// It's useful for targeted collection or verification scenarios.
    ///
    /// # Arguments
    ///
    /// * `pid` - Process identifier to collect information for
    ///
    /// # Returns
    ///
    /// A `ProcessEvent` for the specified process, or an error if the
    /// process cannot be accessed or doesn't exist.
    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent>;

    /// Performs a health check on the collector.
    ///
    /// This method verifies that the collector can perform basic process
    /// enumeration operations. It's used for monitoring and diagnostics.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the collector is healthy, or an error describing
    /// the health issue.
    async fn health_check(&self) -> ProcessCollectionResult<()>;
}

/// Capabilities that a process collector can provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessCollectorCapabilities {
    /// Can collect basic process information (PID, name, etc.)
    pub basic_info: bool,
    /// Can collect enhanced metadata (CPU, memory, start time)
    pub enhanced_metadata: bool,
    /// Can compute executable hashes
    pub executable_hashing: bool,
    /// Can access system processes
    pub system_processes: bool,
    /// Can access kernel threads
    pub kernel_threads: bool,
    /// Supports real-time collection
    pub realtime_collection: bool,
}

impl Default for ProcessCollectorCapabilities {
    fn default() -> Self {
        Self {
            basic_info: true,
            enhanced_metadata: true,
            executable_hashing: false,
            system_processes: true,
            kernel_threads: false,
            realtime_collection: true,
        }
    }
}

/// Default cross-platform process collector using the sysinfo crate.
///
/// This implementation provides reliable cross-platform process enumeration
/// using the `sysinfo` crate as the underlying mechanism. It serves as the
/// default collector and fallback for platforms without specialized implementations.
///
/// # Features
///
/// - Cross-platform compatibility (Linux, macOS, Windows)
/// - Graceful error handling for inaccessible processes
/// - Configurable collection behavior
/// - Detailed statistics and error reporting
/// - Async-friendly design with blocking task isolation
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::process_collector::{ProcessCollector, SysinfoProcessCollector, ProcessCollectionConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = ProcessCollectionConfig {
///         collect_enhanced_metadata: true,
///         max_processes: 1000,
///         ..Default::default()
///     };
///
///     let collector = SysinfoProcessCollector::new(config);
///     let (events, stats) = collector.collect_processes().await?;
///
///     println!("Collected {} of {} processes",
///              stats.successful_collections,
///              stats.total_processes);
///
///     Ok(())
/// }
/// ```
pub struct SysinfoProcessCollector {
    config: ProcessCollectionConfig,
}

impl SysinfoProcessCollector {
    /// Creates a new sysinfo-based process collector with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for collection behavior
    ///
    /// # Examples
    ///
    /// ```rust
    /// use procmond::process_collector::{SysinfoProcessCollector, ProcessCollectionConfig};
    ///
    /// let config = ProcessCollectionConfig::default();
    /// let collector = SysinfoProcessCollector::new(config);
    /// ```
    pub fn new(config: ProcessCollectionConfig) -> Self {
        Self { config }
    }

    /// Converts a sysinfo process to a ProcessEvent with comprehensive error handling.
    ///
    /// This method handles all the edge cases and potential errors that can occur
    /// during process data conversion, providing detailed error information for
    /// debugging and monitoring.
    fn convert_process_to_event(
        &self,
        pid: &Pid,
        process: &Process,
    ) -> ProcessCollectionResult<ProcessEvent> {
        let pid_u32 = pid.as_u32();

        // Validate PID
        if pid_u32 == 0 {
            return Err(ProcessCollectionError::InvalidProcessData {
                pid: pid_u32,
                message: "Invalid PID: 0".to_string(),
            });
        }

        let ppid = process.parent().map(|p| p.as_u32());

        // Get process name with fallback
        let name = if process.name().is_empty() {
            format!("<unknown-{}>", pid_u32)
        } else {
            process.name().to_string_lossy().to_string()
        };

        // Check if this is a system process that should be skipped
        if self.config.skip_system_processes && self.is_system_process(&name, pid_u32) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "System process skipped by configuration".to_string(),
            });
        }

        // Check if this is a kernel thread that should be skipped
        if self.config.skip_kernel_threads && self.is_kernel_thread(&name, process) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "Kernel thread skipped by configuration".to_string(),
            });
        }

        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());

        // Get command line with error handling
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        // Convert start time from sysinfo format to SystemTime with validation
        let start_time = if self.config.collect_enhanced_metadata {
            let start_time_secs = process.start_time();
            if start_time_secs > 0 {
                Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(start_time_secs))
            } else {
                None
            }
        } else {
            None
        };

        // Collect CPU and memory usage if enhanced metadata is enabled
        let (cpu_usage, memory_usage) = if self.config.collect_enhanced_metadata {
            let cpu = process.cpu_usage();
            let memory = process.memory();

            // Validate CPU usage (should be between 0 and 100 * num_cpus)
            let cpu_usage = if cpu.is_finite() && cpu >= 0.0 {
                Some(cpu as f64)
            } else {
                None
            };

            // Memory usage should be reasonable (convert from KB to bytes)
            let memory_usage = if memory > 0 {
                Some(memory * 1024)
            } else {
                None
            };

            (cpu_usage, memory_usage)
        } else {
            (None, None)
        };

        // Compute executable hash if requested
        let executable_hash = if self.config.compute_executable_hashes {
            // TODO: Implement executable hashing (issue #40)
            // For now, we'll leave this as None until the hashing implementation is added
            None
        } else {
            None
        };

        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true; // Process is accessible if we can enumerate it
        let file_exists = executable_path.is_some();

        Ok(ProcessEvent {
            pid: pid_u32,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            user_id,
            accessible,
            file_exists,
            timestamp: SystemTime::now(),
        })
    }

    /// Determines if a process is a system process based on name and PID.
    fn is_system_process(&self, name: &str, pid: u32) -> bool {
        // Common system process patterns
        const SYSTEM_PROCESSES: &[&str] = &[
            "kernel",
            "kthreadd",
            "ksoftirqd",
            "migration",
            "rcu_",
            "watchdog",
            "systemd",
            "init",
            "swapper",
            "idle",
        ];

        // Very low PIDs are typically system processes
        if pid < 10 {
            return true;
        }

        // Check against known system process names
        let name_lower = name.to_lowercase();
        SYSTEM_PROCESSES
            .iter()
            .any(|&sys_proc| name_lower.contains(sys_proc))
    }

    /// Determines if a process is a kernel thread.
    fn is_kernel_thread(&self, name: &str, process: &Process) -> bool {
        // Kernel threads typically have no command line arguments
        if !process.cmd().is_empty() {
            return false;
        }

        // Kernel threads often have names in brackets
        if name.starts_with('[') && name.ends_with(']') {
            return true;
        }

        // Common kernel thread patterns
        const KERNEL_THREAD_PATTERNS: &[&str] = &[
            "kworker",
            "ksoftirqd",
            "migration",
            "rcu_",
            "watchdog",
            "kcompactd",
            "kswapd",
            "kthreadd",
            "kauditd",
        ];

        let name_lower = name.to_lowercase();
        KERNEL_THREAD_PATTERNS
            .iter()
            .any(|&pattern| name_lower.contains(pattern))
    }
}

#[async_trait]
impl ProcessCollector for SysinfoProcessCollector {
    fn name(&self) -> &'static str {
        "sysinfo-collector"
    }

    fn capabilities(&self) -> ProcessCollectorCapabilities {
        ProcessCollectorCapabilities {
            basic_info: true,
            enhanced_metadata: self.config.collect_enhanced_metadata,
            executable_hashing: self.config.compute_executable_hashes,
            system_processes: !self.config.skip_system_processes,
            kernel_threads: !self.config.skip_kernel_threads,
            realtime_collection: true,
        }
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        let start_time = std::time::Instant::now();

        debug!(
            collector = self.name(),
            enhanced_metadata = self.config.collect_enhanced_metadata,
            max_processes = self.config.max_processes,
            "Starting process collection"
        );

        // Perform process enumeration in a blocking task to avoid blocking the async runtime
        let config = self.config.clone();
        let enumeration_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();

            if config.collect_enhanced_metadata {
                system.refresh_all();
            } else {
                system.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::All,
                    true,
                    sysinfo::ProcessRefreshKind::everything(),
                );
            }

            if system.processes().is_empty() {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during enumeration".to_string(),
                });
            }

            Ok(system)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {}", e),
        })?;

        let system = enumeration_result?;

        let mut events = Vec::new();
        let mut stats = CollectionStats {
            total_processes: system.processes().len(),
            ..Default::default()
        };

        // Process each process with individual error handling
        for (pid, process) in system.processes().iter() {
            // Check if we've hit the maximum process limit
            if self.config.max_processes > 0 && events.len() >= self.config.max_processes {
                debug!(
                    max_processes = self.config.max_processes,
                    collected = events.len(),
                    "Reached maximum process collection limit"
                );
                break;
            }

            match self.convert_process_to_event(pid, process) {
                Ok(event) => {
                    events.push(event);
                    stats.successful_collections += 1;
                }
                Err(ProcessCollectionError::ProcessAccessDenied { pid, message }) => {
                    debug!(pid = pid, reason = %message, "Process access denied");
                    stats.inaccessible_processes += 1;
                }
                Err(ProcessCollectionError::InvalidProcessData { pid, message }) => {
                    warn!(pid = pid, reason = %message, "Invalid process data");
                    stats.invalid_processes += 1;
                }
                Err(e) => {
                    error!(
                        pid = pid.as_u32(),
                        error = %e,
                        "Unexpected error during process conversion"
                    );
                    stats.invalid_processes += 1;
                }
            }
        }

        stats.collection_duration_ms = start_time.elapsed().as_millis() as u64;

        debug!(
            collector = self.name(),
            total_processes = stats.total_processes,
            successful = stats.successful_collections,
            inaccessible = stats.inaccessible_processes,
            invalid = stats.invalid_processes,
            duration_ms = stats.collection_duration_ms,
            "Process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(
            collector = self.name(),
            pid = pid,
            "Collecting single process"
        );

        // Perform single process lookup in a blocking task
        let config = self.config.clone();
        let lookup_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();

            if config.collect_enhanced_metadata {
                system.refresh_all();
            } else {
                system.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::All,
                    true,
                    sysinfo::ProcessRefreshKind::everything(),
                );
            }

            let sysinfo_pid = Pid::from_u32(pid);
            if system.process(sysinfo_pid).is_some() {
                Ok(system)
            } else {
                Err(ProcessCollectionError::ProcessNotFound { pid })
            }
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Single process lookup task failed: {}", e),
        })?;

        let system = lookup_result?;
        let sysinfo_pid = Pid::from_u32(pid);
        if let Some(process) = system.process(sysinfo_pid) {
            self.convert_process_to_event(&sysinfo_pid, process)
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(collector = self.name(), "Performing health check");

        // Perform a quick health check by trying to enumerate a few processes
        let health_result = tokio::task::spawn_blocking(|| {
            let mut system = System::new();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );

            let process_count = system.processes().len();
            if process_count == 0 {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during health check".to_string(),
                });
            }

            Ok(process_count)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Health check task failed: {}", e),
        })?;

        let process_count = health_result?;

        debug!(
            collector = self.name(),
            process_count = process_count,
            "Health check passed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Duration import removed - not needed in tests

    #[tokio::test]
    async fn test_sysinfo_collector_creation() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        assert_eq!(collector.name(), "sysinfo-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(!capabilities.executable_hashing);
        assert!(capabilities.system_processes);
        assert!(capabilities.kernel_threads); // Default config doesn't skip kernel threads
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    async fn test_sysinfo_collector_capabilities() {
        let config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: true,
            skip_system_processes: true,
            skip_kernel_threads: true,
            ..Default::default()
        };
        let collector = SysinfoProcessCollector::new(config);

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(!capabilities.enhanced_metadata);
        assert!(capabilities.executable_hashing);
        assert!(!capabilities.system_processes);
        assert!(!capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    async fn test_sysinfo_collector_health_check() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        let result = collector.health_check().await;
        assert!(
            result.is_ok(),
            "Health check should pass on a system with processes"
        );
    }

    #[tokio::test]
    async fn test_sysinfo_collector_collect_processes() {
        let config = ProcessCollectionConfig {
            max_processes: 10, // Limit for faster testing
            ..Default::default()
        };
        let collector = SysinfoProcessCollector::new(config);

        let result = collector.collect_processes().await;
        assert!(result.is_ok(), "Process collection should succeed");

        let (events, stats) = result.unwrap();
        assert!(!events.is_empty(), "Should collect at least one process");
        assert!(
            stats.total_processes > 0,
            "Should find processes on the system"
        );
        assert!(
            stats.successful_collections > 0,
            "Should successfully collect some processes"
        );
        assert!(
            stats.collection_duration_ms > 0,
            "Collection should take some time"
        );

        // Verify event data quality
        for event in &events {
            assert!(event.pid > 0, "Process PID should be valid");
            assert!(!event.name.is_empty(), "Process name should not be empty");
            assert!(event.accessible, "Collected processes should be accessible");
            assert!(
                event.timestamp <= SystemTime::now(),
                "Timestamp should be reasonable"
            );
        }
    }

    #[tokio::test]
    async fn test_sysinfo_collector_collect_single_process() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        // Try to collect information for the current process
        let current_pid = std::process::id();
        let result = collector.collect_process(current_pid).await;

        assert!(result.is_ok(), "Should be able to collect current process");

        let event = result.unwrap();
        assert_eq!(event.pid, current_pid);
        assert!(!event.name.is_empty());
        assert!(event.accessible);
    }

    #[tokio::test]
    async fn test_sysinfo_collector_collect_nonexistent_process() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        // Try to collect information for a non-existent process
        let nonexistent_pid = 999999u32;
        let result = collector.collect_process(nonexistent_pid).await;

        assert!(result.is_err(), "Should fail for non-existent process");

        match result.unwrap_err() {
            ProcessCollectionError::ProcessNotFound { pid } => {
                assert_eq!(pid, nonexistent_pid);
            }
            other => panic!("Expected ProcessNotFound error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sysinfo_collector_with_limits() {
        let config = ProcessCollectionConfig {
            max_processes: 5,
            skip_system_processes: true,
            ..Default::default()
        };
        let collector = SysinfoProcessCollector::new(config);

        let result = collector.collect_processes().await;
        assert!(result.is_ok(), "Process collection should succeed");

        let (events, stats) = result.unwrap();
        assert!(events.len() <= 5, "Should respect max_processes limit");
        assert!(
            stats.total_processes > 0,
            "Should find processes on the system"
        );
    }

    #[test]
    fn test_process_collection_error_display() {
        let error = ProcessCollectionError::ProcessAccessDenied {
            pid: 1234,
            message: "Permission denied".to_string(),
        };

        let error_string = format!("{}", error);
        assert!(error_string.contains("1234"));
        assert!(error_string.contains("Permission denied"));
    }

    #[test]
    fn test_collection_stats_default() {
        let stats = CollectionStats::default();
        assert_eq!(stats.total_processes, 0);
        assert_eq!(stats.successful_collections, 0);
        assert_eq!(stats.inaccessible_processes, 0);
        assert_eq!(stats.invalid_processes, 0);
        assert_eq!(stats.collection_duration_ms, 0);
    }

    #[test]
    fn test_process_collection_config_default() {
        let config = ProcessCollectionConfig::default();
        assert!(config.collect_enhanced_metadata);
        assert!(!config.compute_executable_hashes);
        assert_eq!(config.max_processes, 0);
        assert!(!config.skip_system_processes);
        assert!(!config.skip_kernel_threads);
    }

    #[test]
    fn test_process_collector_capabilities_default() {
        let capabilities = ProcessCollectorCapabilities::default();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(!capabilities.executable_hashing);
        assert!(capabilities.system_processes);
        assert!(!capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }
}
