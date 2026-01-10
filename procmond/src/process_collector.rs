//! Process collector trait and implementations for cross-platform process enumeration.
//!
//! This module provides a platform-agnostic interface for process enumeration through
//! the `ProcessCollector` trait, along with concrete implementations for different
//! platforms and collection strategies.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use serde_json;
use std::time::SystemTime;
use sysinfo::{Pid, Process, System};
use thiserror::Error;
use tracing::{debug, error, warn};

/// Errors that can occur during process collection.
#[derive(Debug, Error)]
#[non_exhaustive]
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
///
/// This structure controls various aspects of process collection, including
/// what metadata to collect, filtering options, and performance limits.
///
/// # Examples
///
/// ```rust
/// use procmond::process_collector::ProcessCollectionConfig;
///
/// // Create a high-performance configuration
/// let config = ProcessCollectionConfig {
///     collect_enhanced_metadata: false,  // Skip CPU/memory for speed
///     compute_executable_hashes: false,  // Skip hashing for speed
///     max_processes: 1000,               // Limit collection size
///     skip_system_processes: true,       // Focus on user processes
///     skip_kernel_threads: true,         // Skip kernel threads
/// };
/// ```
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // Config structs naturally have boolean flags
pub struct ProcessCollectionConfig {
    /// Whether to collect enhanced metadata (CPU, memory, etc.)
    ///
    /// When enabled, collects CPU usage, memory consumption, and start times.
    /// Disabling this can improve collection performance significantly.
    pub collect_enhanced_metadata: bool,

    /// Whether to compute executable hashes
    ///
    /// When enabled, computes SHA-256 hashes of executable files for integrity
    /// verification. This can be expensive for large numbers of processes.
    pub compute_executable_hashes: bool,

    /// Maximum number of processes to collect (0 = unlimited)
    ///
    /// Limits the total number of processes collected to prevent resource
    /// exhaustion. Set to 0 for unlimited collection.
    pub max_processes: usize,

    /// Whether to skip system processes
    ///
    /// When enabled, filters out known system processes to focus on
    /// user-space applications.
    pub skip_system_processes: bool,

    /// Whether to skip kernel threads
    ///
    /// When enabled, filters out kernel threads that typically don't
    /// represent user applications.
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
/// This trait provides a unified interface for collecting process information
/// across different platforms and collection strategies. Implementations should
/// handle platform-specific details while providing consistent behavior.
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::process_collector::{ProcessCollector, ProcessCollectionConfig};
///
/// async fn collect_all_processes(collector: &dyn ProcessCollector) -> anyhow::Result<()> {
///     let (events, stats) = collector.collect_processes().await?;
///     println!("Collected {} processes in {}ms",
///              stats.successful_collections,
///              stats.collection_duration_ms);
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait ProcessCollector: Send + Sync {
    /// Returns the name of this collector implementation.
    ///
    /// This should be a static string that uniquely identifies the collector
    /// type (e.g., "sysinfo-collector", "linux-collector").
    fn name(&self) -> &'static str;

    /// Returns the platform capabilities of this collector.
    ///
    /// Capabilities indicate what features this collector supports, such as
    /// enhanced metadata collection, executable hashing, or system process access.
    fn capabilities(&self) -> ProcessCollectorCapabilities;

    /// Collects process information and returns events with statistics.
    ///
    /// This is the primary collection method that enumerates all accessible
    /// processes on the system and returns both the process events and
    /// collection statistics.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - `Vec<ProcessEvent>`: Successfully collected process events
    /// - `CollectionStats`: Statistics about the collection operation
    ///
    /// # Errors
    ///
    /// Returns `ProcessCollectionError` if system-level enumeration fails
    /// or if no processes can be accessed.
    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)>;

    /// Collects information for a specific process by PID.
    ///
    /// This method attempts to collect detailed information about a single
    /// process identified by its process ID.
    ///
    /// # Arguments
    ///
    /// * `pid` - The process ID to collect information for
    ///
    /// # Errors
    ///
    /// Returns `ProcessCollectionError::ProcessNotFound` if the process
    /// doesn't exist or `ProcessCollectionError::ProcessAccessDenied` if
    /// the process cannot be accessed.
    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent>;

    /// Performs a health check on the collector.
    ///
    /// This method verifies that the collector can successfully enumerate
    /// processes and is functioning correctly. It's typically used for
    /// monitoring and diagnostics.
    ///
    /// # Errors
    ///
    /// Returns `ProcessCollectionError` if the health check fails, indicating
    /// the collector may not be functioning properly.
    async fn health_check(&self) -> ProcessCollectionResult<()>;
}

/// Capabilities that a process collector can provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Capability flags are naturally boolean
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
pub struct SysinfoProcessCollector {
    config: ProcessCollectionConfig,
}

impl SysinfoProcessCollector {
    /// Creates a new sysinfo-based process collector with the specified configuration.
    pub const fn new(config: ProcessCollectionConfig) -> Self {
        Self { config }
    }

    /// Converts a sysinfo process to a `ProcessEvent` with comprehensive error handling.
    #[allow(clippy::trivially_copy_pass_by_ref)] // Pid reference matches sysinfo API patterns
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
                message: "Invalid PID: 0".to_owned(),
            });
        }

        let ppid = process.parent().map(sysinfo::Pid::as_u32);

        // Get process name with fallback
        let name = if process.name().is_empty() {
            format!("<unknown-{pid_u32}>")
        } else {
            process.name().to_string_lossy().to_string()
        };

        // Check if this is a system process that should be skipped
        if self.config.skip_system_processes && self.is_system_process(&name, pid_u32) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "System process skipped by configuration".to_owned(),
            });
        }

        // Check if this is a kernel thread that should be skipped
        if self.config.skip_kernel_threads && self.is_kernel_thread(&name, process) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "Kernel thread skipped by configuration".to_owned(),
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
                // Safe: checked_add handles potential overflow
                SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(start_time_secs))
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
            let cpu_usage = (cpu.is_finite() && cpu >= 0.0).then(|| f64::from(cpu));

            // Memory usage should be reasonable (convert from KB to bytes)
            let memory_usage = (memory > 0).then(|| memory.saturating_mul(1024));

            (cpu_usage, memory_usage)
        } else {
            (None, None)
        };

        // Compute executable hash if requested
        // TODO: Implement executable hashing (issue #40)
        // For now, we'll leave this as None until the hashing implementation is added
        let executable_hash: Option<String> = None;

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
            platform_metadata: None, // Generic collector doesn't collect platform metadata
        })
    }

    /// Determines if a process is a system process based on name and PID.
    #[allow(clippy::unused_self)] // May use self for configuration in future
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
    #[allow(clippy::unused_self)] // May use self for configuration in future
    fn is_kernel_thread(&self, name: &str, process: &Process) -> bool {
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

        // Kernel threads typically have no command line arguments
        if !process.cmd().is_empty() {
            return false;
        }

        // Kernel threads often have names in brackets
        if name.starts_with('[') && name.ends_with(']') {
            return true;
        }

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
                    message: "No processes found during enumeration".to_owned(),
                });
            }

            Ok(system)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {e}"),
        })?;

        let system = enumeration_result?;

        let mut events = Vec::new();
        let mut stats = CollectionStats::default();
        let mut processed_count: usize = 0;

        // Process each process with individual error handling
        for (pid, process) in system.processes() {
            // Check if we've hit the maximum process limit
            if self.config.max_processes > 0 && events.len() >= self.config.max_processes {
                debug!(
                    max_processes = self.config.max_processes,
                    collected = events.len(),
                    "Reached maximum process collection limit"
                );
                break;
            }

            processed_count = processed_count.saturating_add(1);

            match self.convert_process_to_event(pid, process) {
                Ok(event) => {
                    events.push(event);
                    stats.successful_collections = stats.successful_collections.saturating_add(1);
                }
                Err(ProcessCollectionError::ProcessAccessDenied {
                    pid: denied_pid,
                    message,
                }) => {
                    debug!(pid = denied_pid, reason = %message, "Process access denied");
                    stats.inaccessible_processes = stats.inaccessible_processes.saturating_add(1);
                }
                Err(ProcessCollectionError::InvalidProcessData {
                    pid: invalid_pid,
                    message,
                }) => {
                    warn!(pid = invalid_pid, reason = %message, "Invalid process data");
                    stats.invalid_processes = stats.invalid_processes.saturating_add(1);
                }
                Err(e) => {
                    error!(
                        pid = pid.as_u32(),
                        error = %e,
                        "Unexpected error during process conversion"
                    );
                    stats.invalid_processes = stats.invalid_processes.saturating_add(1);
                }
            }
        }

        stats.total_processes = processed_count;
        #[allow(clippy::as_conversions, clippy::semicolon_outside_block)]
        {
            // Safe: elapsed milliseconds fit in u64
            stats.collection_duration_ms = start_time.elapsed().as_millis() as u64;
        }

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
            message: format!("Single process lookup task failed: {e}"),
        })?;

        let system = lookup_result?;
        let sysinfo_pid = Pid::from_u32(pid);
        system.process(sysinfo_pid).map_or(
            Err(ProcessCollectionError::ProcessNotFound { pid }),
            |process| self.convert_process_to_event(&sysinfo_pid, process),
        )
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
                    message: "No processes found during health check".to_owned(),
                });
            }

            Ok(process_count)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Health check task failed: {e}"),
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

/// Fallback process collector for secondary and minimally supported platforms.
///
/// This collector provides basic process enumeration capabilities for platforms
/// that don't have dedicated optimized implementations (FreeBSD, OpenBSD, NetBSD, etc.).
/// It uses the sysinfo crate as the primary collection mechanism for maximum compatibility
/// and implements graceful capability detection and feature availability reporting.
pub struct FallbackProcessCollector {
    config: ProcessCollectionConfig,
    platform_name: &'static str,
    detected_capabilities: ProcessCollectorCapabilities,
}

impl FallbackProcessCollector {
    /// Creates a new fallback process collector with platform detection.
    pub fn new(config: ProcessCollectionConfig) -> Self {
        let platform_name = Self::detect_platform();
        let detected_capabilities = Self::detect_capabilities(&config);

        debug!(
            platform = platform_name,
            capabilities = ?detected_capabilities,
            "Created fallback process collector"
        );

        Self {
            config,
            platform_name,
            detected_capabilities,
        }
    }

    /// Detects the current platform name for logging and diagnostics.
    const fn detect_platform() -> &'static str {
        if cfg!(target_os = "freebsd") {
            "freebsd"
        } else if cfg!(target_os = "openbsd") {
            "openbsd"
        } else if cfg!(target_os = "netbsd") {
            "netbsd"
        } else if cfg!(target_os = "dragonfly") {
            "dragonfly"
        } else if cfg!(target_os = "solaris") {
            "solaris"
        } else if cfg!(target_os = "illumos") {
            "illumos"
        } else if cfg!(target_os = "aix") {
            "aix"
        } else if cfg!(target_os = "haiku") {
            "haiku"
        } else {
            "unknown"
        }
    }

    /// Detects platform capabilities by testing sysinfo functionality.
    fn detect_capabilities(config: &ProcessCollectionConfig) -> ProcessCollectorCapabilities {
        // Test basic sysinfo functionality
        let mut system = System::new();

        // Test if we can refresh processes
        let basic_info = {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            !system.processes().is_empty()
        };

        // Test enhanced metadata collection
        let enhanced_metadata = if config.collect_enhanced_metadata {
            system.refresh_all();
            // Check if we can get CPU and memory information
            system
                .processes()
                .values()
                .any(|p| p.cpu_usage() > 0.0 || p.memory() > 0)
        } else {
            false
        };

        // Executable hashing is always available if configured
        let executable_hashing = config.compute_executable_hashes;

        // System processes access depends on platform and permissions
        let system_processes = !config.skip_system_processes;

        // Kernel threads support varies by platform
        let kernel_threads = !config.skip_kernel_threads && Self::supports_kernel_threads();

        // Real-time collection is generally supported
        let realtime_collection = true;

        ProcessCollectorCapabilities {
            basic_info,
            enhanced_metadata,
            executable_hashing,
            system_processes,
            kernel_threads,
            realtime_collection,
        }
    }

    /// Checks if the platform supports kernel thread enumeration.
    const fn supports_kernel_threads() -> bool {
        // Most BSD variants support kernel threads, but with limitations
        cfg!(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))
    }

    /// Converts a sysinfo process to a `ProcessEvent` with platform-specific handling.
    #[allow(clippy::trivially_copy_pass_by_ref)] // Pid reference matches sysinfo API patterns
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
                message: "Invalid PID: 0".to_owned(),
            });
        }

        let ppid = process.parent().map(sysinfo::Pid::as_u32);

        // Get process name with fallback
        let name = if process.name().is_empty() {
            format!("<unknown-{pid_u32}>")
        } else {
            process.name().to_string_lossy().to_string()
        };

        // Check if this is a system process that should be skipped
        if self.config.skip_system_processes && self.is_system_process(&name, pid_u32) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "System process skipped by configuration".to_owned(),
            });
        }

        // Check if this is a kernel thread that should be skipped
        if self.config.skip_kernel_threads && self.is_kernel_thread(&name, pid_u32) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "Kernel thread skipped by configuration".to_owned(),
            });
        }

        // Get executable path with platform-specific handling
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());

        // Get command line arguments
        let command_line = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        // Get enhanced metadata if available and configured
        let (cpu_usage, memory_usage, start_time) = if self.detected_capabilities.enhanced_metadata
        {
            let cpu = process.cpu_usage();
            let memory = process.memory();
            let start = process.start_time();

            let start_time_opt = if start > 0 {
                // Safe: checked_add handles potential overflow
                SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(start))
            } else {
                None
            };
            (
                (cpu > 0.0).then(|| f64::from(cpu)),
                if memory > 0 {
                    // Convert from KiB to bytes, handling potential overflow
                    memory.checked_mul(1024).or_else(|| {
                        tracing::warn!(
                            memory_kib = memory,
                            "Memory value too large, using saturated value"
                        );
                        Some(u64::MAX)
                    })
                } else {
                    None
                },
                start_time_opt,
            )
        } else {
            (None, None, None)
        };

        // Compute executable hash if configured and path is available
        // TODO: Implement executable hashing (issue #40)
        // For now, we'll leave this as None until the hashing implementation is added
        let executable_hash: Option<String> = None;

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
            platform_metadata: Some(serde_json::json!({
                "platform": self.platform_name,
                "collector": "fallback"
            })),
        })
    }

    /// Determines if a process is a system process based on name and PID.
    #[allow(clippy::unused_self)] // May use self for configuration in future
    fn is_system_process(&self, name: &str, pid: u32) -> bool {
        // Common system process patterns across BSD variants
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
            // BSD-specific system processes
            "pagedaemon",
            "vmdaemon",
            "bufdaemon",
            "syncer",
            "vnlru",
            "softdepflush",
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

    /// Determines if a process is a kernel thread (platform-specific logic).
    #[allow(clippy::unused_self)] // May use self for configuration in future
    fn is_kernel_thread(&self, name: &str, pid: u32) -> bool {
        // BSD-specific kernel thread patterns
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
            // BSD-specific patterns
            "pagedaemon",
            "vmdaemon",
            "bufdaemon",
            "syncer",
            "vnlru",
            "softdepflush",
            "intr",
            "geom",
            "usb",
        ];

        // Kernel threads typically have very low PIDs on BSD systems
        if pid < 5 {
            return true;
        }

        // Kernel threads often have names in brackets or specific patterns
        if name.starts_with('[') && name.ends_with(']') {
            return true;
        }

        let name_lower = name.to_lowercase();
        KERNEL_THREAD_PATTERNS
            .iter()
            .any(|&pattern| name_lower.contains(pattern))
    }
}

#[async_trait]
impl ProcessCollector for FallbackProcessCollector {
    fn name(&self) -> &'static str {
        "fallback-collector"
    }

    fn capabilities(&self) -> ProcessCollectorCapabilities {
        self.detected_capabilities
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        let start_time = std::time::Instant::now();

        debug!(
            collector = self.name(),
            platform = self.platform_name,
            enhanced_metadata = self.detected_capabilities.enhanced_metadata,
            max_processes = self.config.max_processes,
            "Starting fallback process collection"
        );

        // Perform process enumeration in a blocking task to avoid blocking the async runtime
        let config = self.config.clone();
        let platform_name = self.platform_name;
        let enumeration_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();

            // Try enhanced refresh first, fall back to basic if it fails
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
                    message: format!(
                        "No processes found during enumeration on platform: {platform_name}"
                    ),
                });
            }

            Ok(system)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {e}"),
        })?;

        let system = enumeration_result?;

        let mut events = Vec::new();
        let mut stats = CollectionStats::default();
        let mut processed_count: usize = 0;

        // Process each process with individual error handling
        for (pid, process) in system.processes() {
            // Check if we've hit the maximum process limit
            if self.config.max_processes > 0 && events.len() >= self.config.max_processes {
                debug!(
                    max_processes = self.config.max_processes,
                    collected = events.len(),
                    platform = self.platform_name,
                    "Reached maximum process collection limit"
                );
                break;
            }

            processed_count = processed_count.saturating_add(1);
            match self.convert_process_to_event(pid, process) {
                Ok(event) => {
                    events.push(event);
                    stats.successful_collections = stats.successful_collections.saturating_add(1);
                }
                Err(ProcessCollectionError::ProcessAccessDenied {
                    pid: denied_pid,
                    message,
                }) => {
                    debug!(
                        pid = denied_pid,
                        reason = %message,
                        platform = self.platform_name,
                        "Process access denied"
                    );
                    stats.inaccessible_processes = stats.inaccessible_processes.saturating_add(1);
                }
                Err(ProcessCollectionError::InvalidProcessData {
                    pid: invalid_pid,
                    message,
                }) => {
                    warn!(
                        pid = invalid_pid,
                        reason = %message,
                        platform = self.platform_name,
                        "Invalid process data"
                    );
                    stats.invalid_processes = stats.invalid_processes.saturating_add(1);
                }
                Err(e) => {
                    error!(
                        pid = pid.as_u32(),
                        error = %e,
                        platform = self.platform_name,
                        "Unexpected error during process conversion"
                    );
                    stats.invalid_processes = stats.invalid_processes.saturating_add(1);
                }
            }
        }

        stats.total_processes = processed_count;
        #[allow(clippy::as_conversions, clippy::semicolon_outside_block)]
        {
            // Safe: elapsed milliseconds fit in u64
            stats.collection_duration_ms = start_time.elapsed().as_millis() as u64;
        }

        debug!(
            collector = self.name(),
            platform = self.platform_name,
            total_processes = stats.total_processes,
            successful = stats.successful_collections,
            inaccessible = stats.inaccessible_processes,
            invalid = stats.invalid_processes,
            duration_ms = stats.collection_duration_ms,
            "Fallback process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(
            collector = self.name(),
            platform = self.platform_name,
            pid = pid,
            "Collecting single process"
        );

        // Perform single process lookup in a blocking task
        let config = self.config.clone();
        let _platform_name = self.platform_name;
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
            message: format!("Single process lookup task failed: {e}"),
        })?;

        let system = lookup_result?;
        let sysinfo_pid = Pid::from_u32(pid);
        system.process(sysinfo_pid).map_or(
            Err(ProcessCollectionError::ProcessNotFound { pid }),
            |process| self.convert_process_to_event(&sysinfo_pid, process),
        )
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(
            collector = self.name(),
            platform = self.platform_name,
            "Performing health check"
        );

        // Perform a quick health check by trying to enumerate a few processes
        let platform_name = self.platform_name;
        let health_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );

            let process_count = system.processes().len();
            if process_count == 0 {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: format!(
                        "No processes found during health check on platform: {platform_name}"
                    ),
                });
            }

            Ok(process_count)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Health check task failed: {e}"),
        })?;

        let process_count = health_result?;

        debug!(
            collector = self.name(),
            platform = self.platform_name,
            process_count = process_count,
            "Health check passed"
        );

        Ok(())
    }
}

/// Creates the appropriate process collector based on platform detection and configuration.
pub fn create_process_collector(config: ProcessCollectionConfig) -> Box<dyn ProcessCollector> {
    // Platform detection and collector selection
    if cfg!(target_os = "linux") {
        // TODO: Implement LinuxProcessCollector (task 5.3)
        // For now, use sysinfo collector
        debug!(
            "Linux detected, using sysinfo collector (LinuxProcessCollector not yet implemented)"
        );
        Box::new(SysinfoProcessCollector::new(config))
    } else if cfg!(target_os = "macos") {
        // TODO: Implement MacOSProcessCollector (task 5.4)
        // For now, use sysinfo collector
        debug!(
            "macOS detected, using sysinfo collector (MacOSProcessCollector not yet implemented)"
        );
        Box::new(SysinfoProcessCollector::new(config))
    } else if cfg!(target_os = "windows") {
        // TODO: Implement WindowsProcessCollector (task 5.6)
        // For now, use sysinfo collector
        debug!(
            "Windows detected, using sysinfo collector (WindowsProcessCollector not yet implemented)"
        );
        Box::new(SysinfoProcessCollector::new(config))
    } else if cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_os = "haiku"
    )) {
        // Use fallback collector for secondary platforms
        debug!("Secondary platform detected, using fallback collector");
        Box::new(FallbackProcessCollector::new(config))
    } else {
        // Unknown platform, use sysinfo as universal fallback
        debug!("Unknown platform detected, using sysinfo collector as universal fallback");
        Box::new(SysinfoProcessCollector::new(config))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sysinfo_collector_creation() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        assert_eq!(collector.name(), "sysinfo-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
    }

    #[tokio::test]
    async fn test_fallback_collector_creation() {
        let config = ProcessCollectionConfig::default();
        let collector = FallbackProcessCollector::new(config);

        assert_eq!(collector.name(), "fallback-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
    }

    #[tokio::test]
    async fn test_collector_factory() {
        let config = ProcessCollectionConfig::default();
        let collector = create_process_collector(config);

        // Should create some valid collector
        assert!(!collector.name().is_empty());

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
    }

    #[tokio::test]
    async fn test_sysinfo_collector_health_check() {
        let config = ProcessCollectionConfig::default();
        let collector = SysinfoProcessCollector::new(config);

        // Health check should pass on any system with processes
        let result = collector.health_check().await;
        assert!(result.is_ok(), "Health check failed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fallback_collector_health_check() {
        let config = ProcessCollectionConfig::default();
        let collector = FallbackProcessCollector::new(config);

        // Health check should pass on any system with processes
        let result = collector.health_check().await;
        assert!(result.is_ok(), "Health check failed: {:?}", result);
    }
}
