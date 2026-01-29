//! Library module for procmond to enable unit testing
#![allow(clippy::doc_markdown)] // Many docs reference code identifiers without backticks

pub mod event_bus_connector;
pub mod event_source;
pub mod lifecycle;
pub mod monitor_collector;
pub mod process_collector;
pub mod wal;

#[cfg(target_os = "linux")]
pub mod linux_collector;

#[cfg(target_os = "macos")]
pub mod macos_collector;

#[cfg(target_os = "windows")]
pub mod windows_collector;

pub use event_bus_connector::{
    BackpressureSignal, EventBusConnector, EventBusConnectorError, EventBusConnectorResult,
    ProcessEventType,
};
pub use event_source::{ProcessEventSource, ProcessSourceConfig};
pub use lifecycle::{
    LifecycleTrackingConfig, LifecycleTrackingError, LifecycleTrackingResult,
    LifecycleTrackingStats, ProcessLifecycleEvent, ProcessLifecycleTracker, ProcessSnapshot,
    SuspiciousEventSeverity,
};
pub use monitor_collector::{ProcmondMonitorCollector, ProcmondMonitorConfig};
pub use process_collector::{
    CollectionStats, FallbackProcessCollector, ProcessCollectionConfig, ProcessCollectionError,
    ProcessCollectionResult, ProcessCollector, ProcessCollectorCapabilities,
    SysinfoProcessCollector,
};

#[cfg(target_os = "linux")]
pub use linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
pub use macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
pub use windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

// Re-export main functionality for testing
pub use daemoneye_lib::proto::{
    DetectionResult, DetectionTask, ProtoProcessRecord, ProtoTaskType, TaskType,
};
pub use daemoneye_lib::storage;

use daemoneye_lib::ipc::IpcError;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Message handler for IPC communication with process monitoring.
///
/// The `ProcessMessageHandler` is the core component of procmond that handles
/// inter-process communication with daemoneye-agent. It provides functionality for
/// process enumeration, system monitoring, and task processing through a secure
/// IPC protocol using protobuf messages and CRC32 integrity validation.
///
/// # Purpose
///
/// This handler serves as the privileged process collector in the DaemonEye
/// three-component security architecture. It runs with elevated privileges
/// when necessary but drops them immediately after initialization to maintain
/// a minimal attack surface. The handler is responsible for:
///
/// - Enumerating system processes using the ProcessCollector trait
/// - Converting process data to protobuf format for IPC transmission
/// - Handling detection tasks from daemoneye-agent via IPC
/// - Managing database operations for audit logging
/// - Providing process metadata including CPU usage, memory consumption, and execution details
///
/// # Security Model
///
/// The handler follows the principle of least privilege:
/// - No network access whatsoever
/// - Write-only access to audit ledger for tamper-evident logging
/// - Minimal complexity to reduce attack surface
/// - All complex logic (SQL parsing, networking, detection) handled by daemoneye-agent
///
/// # Usage
///
/// The handler is typically created during procmond initialization and used
/// by the IPC server to process incoming detection tasks. It requires a
/// `DatabaseManager` wrapped in `Arc<Mutex<>>` for thread-safe database access
/// and a `ProcessCollector` implementation for platform-agnostic process enumeration.
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::{ProcessMessageHandler, SysinfoProcessCollector, ProcessCollectionConfig};
/// use daemoneye_lib::storage::DatabaseManager;
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
///
/// // Create a database manager (typically done during procmond startup)
/// let db_manager = Arc::new(Mutex::new(
///     DatabaseManager::new("/var/lib/daemoneye/audit.db")
///         .expect("Failed to create database manager")
/// ));
///
/// // Create the process message handler with default collector
/// let handler = ProcessMessageHandler::new(db_manager.clone());
///
/// // Or create with a custom collector
/// let collector_config = ProcessCollectionConfig::default();
/// let collector = Box::new(SysinfoProcessCollector::new(collector_config));
/// let handler_custom = ProcessMessageHandler::with_collector(db_manager, collector);
///
/// // The handler is now ready to process detection tasks via IPC
/// // This would typically be done by the IPC server in the main procmond loop
/// ```
///
/// # Thread Safety
///
/// This struct is designed to be used in a multi-threaded environment. The
/// `DatabaseManager` is wrapped in `Arc<Mutex<>>` to ensure thread-safe access
/// to the underlying database operations. The `ProcessCollector` trait is
/// required to be `Send + Sync` for thread safety.
///
/// # Error Handling
///
/// All operations return `Result` types with appropriate error handling.
/// Database errors, process enumeration failures, and IPC communication
/// issues are properly propagated to the caller for appropriate handling.
#[allow(dead_code)]
pub struct ProcessMessageHandler {
    /// Thread-safe database manager for audit logging and data storage
    pub database: Arc<Mutex<storage::DatabaseManager>>,
    /// Process collector implementation for platform-agnostic process enumeration
    pub collector: Box<dyn ProcessCollector>,
}

impl ProcessMessageHandler {
    /// Creates a new ProcessMessageHandler with the default SysinfoProcessCollector.
    ///
    /// This constructor provides backward compatibility by using the default
    /// cross-platform process collector implementation.
    ///
    /// # Arguments
    ///
    /// * `database` - Thread-safe database manager for audit logging
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::ProcessMessageHandler;
    /// use daemoneye_lib::storage::DatabaseManager;
    /// use std::sync::Arc;
    /// use tokio::sync::Mutex;
    ///
    /// let db_manager = Arc::new(Mutex::new(
    ///     DatabaseManager::new("/var/lib/daemoneye/audit.db")?
    /// ));
    /// let handler = ProcessMessageHandler::new(db_manager);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(database: Arc<Mutex<storage::DatabaseManager>>) -> Self {
        let collector_config = ProcessCollectionConfig::default();
        let collector = Box::new(SysinfoProcessCollector::new(collector_config));
        Self {
            database,
            collector,
        }
    }

    /// Creates a new ProcessMessageHandler with a custom ProcessCollector.
    ///
    /// This constructor allows for dependency injection of different ProcessCollector
    /// implementations, enabling platform-specific optimizations and testing.
    ///
    /// # Arguments
    ///
    /// * `database` - Thread-safe database manager for audit logging
    /// * `collector` - Custom process collector implementation
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::{ProcessMessageHandler, SysinfoProcessCollector, ProcessCollectionConfig};
    /// use daemoneye_lib::storage::DatabaseManager;
    /// use std::sync::Arc;
    /// use tokio::sync::Mutex;
    ///
    /// let db_manager = Arc::new(Mutex::new(
    ///     DatabaseManager::new("/var/lib/daemoneye/audit.db")?
    /// ));
    /// let collector_config = ProcessCollectionConfig::default();
    /// let collector = Box::new(SysinfoProcessCollector::new(collector_config));
    /// let handler = ProcessMessageHandler::with_collector(db_manager, collector);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_collector(
        database: Arc<Mutex<storage::DatabaseManager>>,
        collector: Box<dyn ProcessCollector>,
    ) -> Self {
        Self {
            database,
            collector,
        }
    }

    pub async fn handle_detection_task(
        &self,
        task: DetectionTask,
    ) -> Result<DetectionResult, IpcError> {
        tracing::info!("Received detection task: {}", task.task_id);

        #[allow(clippy::as_conversions)] // Necessary for protobuf enum comparison
        let enumerate_processes_type = ProtoTaskType::EnumerateProcesses as i32;
        match task.task_type {
            task_type if task_type == enumerate_processes_type => {
                self.enumerate_processes(&task).await
            }
            _ => {
                tracing::warn!("Unsupported task type: {}", task.task_type);
                Ok(DetectionResult::failure(
                    &task.task_id,
                    "Unsupported task type",
                ))
            }
        }
    }

    /// Enumerate all processes on the system using the ProcessCollector trait.
    ///
    /// This method uses the configured ProcessCollector implementation to perform
    /// platform-agnostic process enumeration with proper error handling for
    /// inaccessible processes.
    ///
    /// # Arguments
    ///
    /// * `task` - Detection task containing enumeration parameters
    ///
    /// # Returns
    ///
    /// A `DetectionResult` containing successfully enumerated processes or an error.
    /// Individual process access failures are handled gracefully and logged.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::ProcessMessageHandler;
    /// use daemoneye_lib::proto::{DetectionTask, TaskType};
    ///
    /// # async fn example(handler: ProcessMessageHandler) -> Result<(), Box<dyn std::error::Error>> {
    /// let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);
    /// let result = handler.enumerate_processes(&task).await?;
    /// println!("Collected {} processes", result.processes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn enumerate_processes(
        &self,
        task: &DetectionTask,
    ) -> Result<DetectionResult, IpcError> {
        use tracing::{debug, error};

        debug!(
            task_id = %task.task_id,
            collector = self.collector.name(),
            "Starting process enumeration"
        );

        // Use the ProcessCollector trait for enumeration
        let enumeration_result = self.collector.collect_processes().await;

        match enumeration_result {
            Ok((process_events, collection_stats)) => {
                debug!(
                    task_id = %task.task_id,
                    total_found = collection_stats.total_processes,
                    successful = collection_stats.successful_collections,
                    inaccessible = collection_stats.inaccessible_processes,
                    invalid = collection_stats.invalid_processes,
                    duration_ms = collection_stats.collection_duration_ms,
                    "Process enumeration completed successfully"
                );

                // Convert ProcessEvents to ProtoProcessRecords
                let processes: Vec<ProtoProcessRecord> = process_events
                    .into_iter()
                    .map(|event| self.convert_process_event_to_record(event))
                    .collect();

                Ok(DetectionResult::success(&task.task_id, processes))
            }
            Err(e) => {
                error!(
                    task_id = %task.task_id,
                    error = %e,
                    collector = self.collector.name(),
                    "Process enumeration failed"
                );

                // Convert ProcessCollectionError to appropriate IPC error
                let error_message = match e {
                    ProcessCollectionError::SystemEnumerationFailed { message } => {
                        format!("System enumeration failed: {message}")
                    }
                    ProcessCollectionError::CollectionTimeout { timeout_ms } => {
                        format!("Process collection timed out after {timeout_ms}ms")
                    }
                    ProcessCollectionError::PlatformError { message } => {
                        format!("Platform-specific error: {message}")
                    }
                    // Explicitly handle remaining variants + catch-all for new variants
                    ProcessCollectionError::ProcessAccessDenied { .. }
                    | ProcessCollectionError::ProcessNotFound { .. }
                    | ProcessCollectionError::InvalidProcessData { .. } => {
                        format!("Process collection error: {e}")
                    }
                };

                Ok(DetectionResult::failure(&task.task_id, &error_message))
            }
        }
    }

    /// Convert a ProcessEvent from the collector to a ProtoProcessRecord.
    ///
    /// This method converts the platform-agnostic ProcessEvent structure
    /// returned by ProcessCollector implementations into the protobuf format
    /// required for IPC communication with daemoneye-agent.
    ///
    /// # Arguments
    ///
    /// * `event` - ProcessEvent from the collector
    ///
    /// # Returns
    ///
    /// A `ProtoProcessRecord` suitable for IPC transmission
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::ProcessMessageHandler;
    /// use collector_core::ProcessEvent;
    /// use std::time::SystemTime;
    ///
    /// # fn example(handler: ProcessMessageHandler) {
    /// let event = ProcessEvent {
    ///     pid: 1234,
    ///     ppid: Some(1),
    ///     name: "test_process".to_string(),
    ///     executable_path: Some("/usr/bin/test".to_string()),
    ///     command_line: vec!["test".to_string(), "--arg".to_string()],
    ///     start_time: Some(SystemTime::now()),
    ///     cpu_usage: Some(5.0),
    ///     memory_usage: Some(1024 * 1024),
    ///     executable_hash: None,
    ///     user_id: Some("1000".to_string()),
    ///     accessible: true,
    ///     file_exists: true,
    ///     timestamp: SystemTime::now(),
    ///     platform_metadata: None,
    /// };
    /// let record = handler.convert_process_event_to_record(event);
    /// # }
    /// ```
    pub fn convert_process_event_to_record(
        &self,
        event: collector_core::ProcessEvent,
    ) -> ProtoProcessRecord {
        use std::time::UNIX_EPOCH;

        // Convert SystemTime to timestamp with proper error handling
        let start_time = event.start_time.and_then(|st| {
            if let Ok(duration) = st.duration_since(UNIX_EPOCH) {
                #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
                // Safe: process start times won't overflow i64 (max year ~292 billion)
                Some(duration.as_secs() as i64)
            } else {
                tracing::warn!(
                    pid = event.pid,
                    name = %event.name,
                    "Process start time is before Unix epoch, skipping"
                );
                None
            }
        });

        let collection_time = if let Ok(duration) = event.timestamp.duration_since(UNIX_EPOCH) {
            // Use checked arithmetic to prevent overflow
            let millis = duration.as_millis();
            #[allow(clippy::as_conversions)] // Safe: i64::MAX is a compile-time constant
            let max_millis = i64::MAX as u128;
            if millis > max_millis {
                tracing::warn!(
                    pid = event.pid,
                    name = %event.name,
                    "Collection time overflow, clamping to i64::MAX"
                );
                i64::MAX
            } else {
                #[allow(clippy::as_conversions)] // Safe: checked above that millis fits in i64
                {
                    millis as i64
                }
            }
        } else {
            tracing::warn!(
                pid = event.pid,
                name = %event.name,
                "Collection time is before Unix epoch, using 0"
            );
            0
        };

        // Check if executable hash exists before moving the value
        let has_executable_hash = event.executable_hash.is_some();

        ProtoProcessRecord {
            pid: event.pid,
            ppid: event.ppid,
            name: event.name,
            executable_path: event.executable_path,
            command_line: event.command_line,
            start_time,
            cpu_usage: event.cpu_usage,
            memory_usage: event.memory_usage,
            executable_hash: event.executable_hash,
            hash_algorithm: has_executable_hash.then(|| "sha256".to_owned()),
            user_id: event.user_id,
            accessible: event.accessible,
            file_exists: event.file_exists,
            collection_time,
        }
    }

    /// Convert a sysinfo process to a ProtoProcessRecord (legacy method).
    ///
    /// This method is kept for backward compatibility and testing purposes.
    /// New code should use the ProcessCollector trait and convert_process_event_to_record.
    ///
    /// # Deprecated
    ///
    /// This method is deprecated in favor of using ProcessCollector trait implementations.
    /// It will be removed in a future version.
    #[deprecated(
        since = "0.1.0",
        note = "Use ProcessCollector trait and convert_process_event_to_record instead"
    )]
    pub fn convert_process_to_record(
        &self,
        pid: &sysinfo::Pid,
        process: &sysinfo::Process,
    ) -> ProtoProcessRecord {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(sysinfo::Pid::as_u32);
        let name = process.name().to_string_lossy().to_string();
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
        // Safe: process start times won't overflow i64
        let start_time = Some(process.start_time() as i64);
        let cpu_usage = Some(f64::from(process.cpu_usage()));
        let memory_usage = Some(process.memory().saturating_mul(1024));
        let executable_hash = None; // Would need file hashing implementation
        let hash_algorithm = None;
        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true; // Process is accessible if we can enumerate it
        let file_exists = executable_path.is_some();
        let collection_time = chrono::Utc::now().timestamp_millis();

        ProtoProcessRecord {
            pid: pid_u32,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            hash_algorithm,
            user_id,
            accessible,
            file_exists,
            collection_time,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use collector_core::ProcessEvent;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Mock ProcessCollector for testing purposes.
    ///
    /// This mock collector provides predictable test data and allows for
    /// testing error conditions and edge cases without relying on the
    /// actual system state.
    pub struct MockProcessCollector {
        /// Processes to return during collection
        processes: Vec<ProcessEvent>,
        /// Whether to simulate a collection error
        should_error: bool,
        /// Error to return if should_error is true
        error_message: String,
        /// Whether health check should fail
        health_check_should_fail: bool,
    }

    impl MockProcessCollector {
        /// Creates a new mock collector with test process data.
        pub fn new() -> Self {
            let now = SystemTime::now();
            let start_time = now - std::time::Duration::from_secs(3600);

            let processes = vec![
                ProcessEvent {
                    pid: 1,
                    ppid: None,
                    name: "init".to_string(),
                    executable_path: Some("/sbin/init".to_string()),
                    command_line: vec!["/sbin/init".to_string()],
                    start_time: Some(start_time),
                    cpu_usage: Some(0.1),
                    memory_usage: Some(1024 * 1024),
                    executable_hash: Some("hash1".to_string()),
                    user_id: Some("0".to_string()),
                    accessible: true,
                    file_exists: true,
                    timestamp: now,
                    platform_metadata: None,
                },
                ProcessEvent {
                    pid: 1234,
                    ppid: Some(1),
                    name: "test_process".to_string(),
                    executable_path: Some("/usr/bin/test".to_string()),
                    command_line: vec!["test".to_string(), "--arg".to_string()],
                    start_time: Some(start_time),
                    cpu_usage: Some(5.0),
                    memory_usage: Some(2048 * 1024),
                    executable_hash: Some("hash2".to_string()),
                    user_id: Some("1000".to_string()),
                    accessible: true,
                    file_exists: true,
                    timestamp: now,
                    platform_metadata: None,
                },
                ProcessEvent {
                    pid: 5678,
                    ppid: Some(1),
                    name: "inaccessible_process".to_string(),
                    executable_path: None,
                    command_line: vec![],
                    start_time: None,
                    cpu_usage: None,
                    memory_usage: None,
                    executable_hash: None,
                    user_id: None,
                    accessible: false,
                    file_exists: false,
                    timestamp: now,
                    platform_metadata: None,
                },
            ];

            Self {
                processes,
                should_error: false,
                error_message: "Mock error".to_string(),
                health_check_should_fail: false,
            }
        }

        /// Creates a mock collector that will return an error during collection.
        pub fn with_error(error_message: String) -> Self {
            Self {
                processes: vec![],
                should_error: true,
                error_message,
                health_check_should_fail: false,
            }
        }

        /// Creates a mock collector that will fail health checks.
        pub fn with_health_check_failure() -> Self {
            Self {
                processes: vec![],
                should_error: false,
                error_message: "Mock error".to_string(),
                health_check_should_fail: true,
            }
        }

        /// Sets custom process data for the mock collector.
        pub fn with_processes(mut self, processes: Vec<ProcessEvent>) -> Self {
            self.processes = processes;
            self
        }
    }

    #[async_trait::async_trait]
    impl ProcessCollector for MockProcessCollector {
        fn name(&self) -> &'static str {
            "mock-collector"
        }

        fn capabilities(&self) -> ProcessCollectorCapabilities {
            ProcessCollectorCapabilities {
                basic_info: true,
                enhanced_metadata: true,
                executable_hashing: true,
                system_processes: true,
                kernel_threads: true,
                realtime_collection: true,
            }
        }

        async fn collect_processes(
            &self,
        ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
            if self.should_error {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: self.error_message.clone(),
                });
            }

            let stats = CollectionStats {
                total_processes: self.processes.len(),
                successful_collections: self.processes.iter().filter(|p| p.accessible).count(),
                inaccessible_processes: self.processes.iter().filter(|p| !p.accessible).count(),
                invalid_processes: 0,
                collection_duration_ms: 10, // Mock duration
            };

            Ok((self.processes.clone(), stats))
        }

        async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
            if self.should_error {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: self.error_message.clone(),
                });
            }

            self.processes
                .iter()
                .find(|p| p.pid == pid)
                .cloned()
                .ok_or(ProcessCollectionError::ProcessNotFound { pid })
        }

        async fn health_check(&self) -> ProcessCollectionResult<()> {
            if self.health_check_should_fail {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "Health check failed".to_string(),
                });
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_process_message_handler_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        // Verify the handler was created with the default collector
        assert_eq!(handler.collector.name(), "sysinfo-collector");

        // Verify capabilities
        let capabilities = handler.collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
    }

    #[tokio::test]
    async fn test_process_message_handler_with_custom_collector() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        // Create custom collector configuration
        let collector_config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: true,
            max_processes: 100,
            ..Default::default()
        };
        let collector = Box::new(SysinfoProcessCollector::new(collector_config));

        let handler = ProcessMessageHandler::with_collector(db_manager, collector);

        // Verify the handler was created with the custom collector
        assert_eq!(handler.collector.name(), "sysinfo-collector");

        // Verify custom capabilities
        let capabilities = handler.collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(!capabilities.enhanced_metadata); // Disabled in config
        assert!(capabilities.executable_hashing); // Enabled in config
    }

    #[tokio::test]
    async fn test_handle_detection_task_enumerate_processes() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

        let result = handler.handle_detection_task(task).await;
        assert!(result.is_ok());

        let detection_result =
            result.expect("Detection task should succeed for enumerate processes");
        assert_eq!(detection_result.task_id, "test-task");
        assert!(detection_result.success);
        assert!(!detection_result.processes.is_empty());
    }

    #[tokio::test]
    async fn test_handle_detection_task_unsupported_type() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        let task = DetectionTask {
            task_id: "test-task".to_string(),
            task_type: 999, // Unsupported task type
            process_filter: None,
            hash_check: None,
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        let result = handler.handle_detection_task(task).await;
        assert!(result.is_ok());

        let detection_result =
            result.expect("Detection task should return result even for unsupported type");
        assert_eq!(detection_result.task_id, "test-task");
        assert!(!detection_result.success);
        assert!(
            detection_result
                .error_message
                .as_ref()
                .expect("Error message should be present for unsupported task type")
                .contains("Unsupported task type")
        );
    }

    #[test]
    fn test_convert_process_event_to_record() {
        use collector_core::ProcessEvent;
        use std::time::SystemTime;

        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        // Create a test ProcessEvent
        let now = SystemTime::now();
        let start_time = now - std::time::Duration::from_secs(3600); // 1 hour ago

        let event = ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test".to_string(), "--arg".to_string()],
            start_time: Some(start_time),
            cpu_usage: Some(5.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: now,
            platform_metadata: None,
        };

        let record = handler.convert_process_event_to_record(event);

        assert_eq!(record.pid, 1234);
        assert_eq!(record.ppid, Some(1));
        assert_eq!(record.name, "test_process");
        assert_eq!(record.executable_path, Some("/usr/bin/test".to_string()));
        assert_eq!(
            record.command_line,
            vec!["test".to_string(), "--arg".to_string()]
        );
        assert!(record.start_time.is_some());
        assert_eq!(record.cpu_usage, Some(5.0));
        assert_eq!(record.memory_usage, Some(1024 * 1024));
        assert_eq!(record.executable_hash, Some("abc123".to_string()));
        assert_eq!(record.hash_algorithm, Some("sha256".to_string()));
        assert_eq!(record.user_id, Some("1000".to_string()));
        assert!(record.accessible);
        assert!(record.file_exists);
        assert!(record.collection_time > 0);
    }

    #[test]
    #[allow(deprecated)]
    fn test_convert_process_to_record_legacy() {
        use sysinfo::System;

        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        let mut system = System::new_all();
        system.refresh_all();

        if let Some((pid, process)) = system.processes().iter().next() {
            let record = handler.convert_process_to_record(pid, process);

            assert_eq!(record.pid, pid.as_u32());
            assert_eq!(record.name, process.name().to_string_lossy().to_string());
            assert!(record.accessible);
            assert!(record.collection_time > 0);
        }
    }

    #[tokio::test]
    async fn test_enumerate_processes_with_mock_collector() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let mock_collector = Box::new(MockProcessCollector::new());
        let handler = ProcessMessageHandler::with_collector(db_manager, mock_collector);

        let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

        let result = handler.enumerate_processes(&task).await;
        assert!(result.is_ok());

        let detection_result = result.expect("Process enumeration should succeed");
        assert_eq!(detection_result.task_id, "test-task");
        assert!(detection_result.success);
        assert_eq!(detection_result.processes.len(), 3); // Mock collector returns 3 processes

        // Verify process data
        let processes = &detection_result.processes;
        assert_eq!(processes[0].pid, 1);
        assert_eq!(processes[0].name, "init");
        assert!(processes[0].accessible);

        assert_eq!(processes[1].pid, 1234);
        assert_eq!(processes[1].name, "test_process");
        assert!(processes[1].accessible);

        assert_eq!(processes[2].pid, 5678);
        assert_eq!(processes[2].name, "inaccessible_process");
        assert!(!processes[2].accessible);
    }

    #[tokio::test]
    async fn test_enumerate_processes_with_collector_error() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let mock_collector = Box::new(MockProcessCollector::with_error(
            "System enumeration failed".to_string(),
        ));
        let handler = ProcessMessageHandler::with_collector(db_manager, mock_collector);

        let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

        let result = handler.enumerate_processes(&task).await;
        assert!(result.is_ok());

        let detection_result = result.expect("Should return result even on collector error");
        assert_eq!(detection_result.task_id, "test-task");
        assert!(!detection_result.success); // Should indicate failure
        assert!(detection_result.error_message.is_some());
        assert!(
            detection_result
                .error_message
                .unwrap()
                .contains("System enumeration failed")
        );
    }

    #[tokio::test]
    async fn test_enumerate_processes_with_real_collector() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

        let result = handler.enumerate_processes(&task).await;
        assert!(result.is_ok());

        let detection_result = result.expect("Process enumeration should succeed");
        assert_eq!(detection_result.task_id, "test-task");
        assert!(detection_result.success);
        assert!(!detection_result.processes.is_empty());
    }

    #[tokio::test]
    async fn test_enumerate_processes_with_inaccessible_processes() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        // Create mock processes with some inaccessible ones
        let now = SystemTime::now();
        let processes = vec![
            ProcessEvent {
                pid: 1,
                ppid: None,
                name: "accessible".to_string(),
                executable_path: Some("/bin/accessible".to_string()),
                command_line: vec![],
                start_time: Some(now),
                cpu_usage: Some(1.0),
                memory_usage: Some(1024),
                executable_hash: None,
                user_id: Some("0".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: now,
                platform_metadata: None,
            },
            ProcessEvent {
                pid: 2,
                ppid: Some(1),
                name: "inaccessible".to_string(),
                executable_path: None,
                command_line: vec![],
                start_time: None,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: false,
                file_exists: false,
                timestamp: now,
                platform_metadata: None,
            },
        ];

        let mock_collector = Box::new(MockProcessCollector::new().with_processes(processes));
        let handler = ProcessMessageHandler::with_collector(db_manager, mock_collector);

        let task = DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

        let result = handler.enumerate_processes(&task).await;
        assert!(result.is_ok());

        let detection_result = result.expect("Process enumeration should succeed");
        assert!(detection_result.success);
        assert_eq!(detection_result.processes.len(), 2);

        // Verify accessible process
        let accessible_process = detection_result
            .processes
            .iter()
            .find(|p| p.pid == 1)
            .expect("Should find accessible process");
        assert!(accessible_process.accessible);
        assert!(accessible_process.file_exists);
        assert!(accessible_process.executable_path.is_some());

        // Verify inaccessible process
        let inaccessible_process = detection_result
            .processes
            .iter()
            .find(|p| p.pid == 2)
            .expect("Should find inaccessible process");
        assert!(!inaccessible_process.accessible);
        assert!(!inaccessible_process.file_exists);
        assert!(inaccessible_process.executable_path.is_none());
    }

    #[tokio::test]
    async fn test_process_collector_error_handling() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        // Test different error types
        let error_cases = vec![
            (
                ProcessCollectionError::SystemEnumerationFailed {
                    message: "System error".to_string(),
                },
                "System enumeration failed: System error",
            ),
            (
                ProcessCollectionError::CollectionTimeout { timeout_ms: 5000 },
                "Process collection timed out after 5000ms",
            ),
            (
                ProcessCollectionError::PlatformError {
                    message: "Platform specific error".to_string(),
                },
                "Platform-specific error: Platform specific error",
            ),
        ];

        for (error, expected_message) in error_cases {
            let mock_collector = Box::new(MockProcessCollector::with_error(error.to_string()));
            let handler =
                ProcessMessageHandler::with_collector(Arc::clone(&db_manager), mock_collector);

            let task =
                DetectionTask::new_test_task("test-task", TaskType::EnumerateProcesses, None);

            let result = handler.enumerate_processes(&task).await;
            assert!(result.is_ok());

            let detection_result = result.expect("Should return result even on error");
            assert!(!detection_result.success);
            assert!(detection_result.error_message.is_some());

            let error_message = detection_result.error_message.unwrap();
            assert!(
                error_message.contains(expected_message)
                    || error_message.contains(&error.to_string()),
                "Expected error message to contain '{}' or '{}', got '{}'",
                expected_message,
                error,
                error_message
            );
        }
    }

    #[test]
    fn test_process_event_conversion_edge_cases() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let handler = ProcessMessageHandler::new(db_manager);

        // Test with minimal process event
        let minimal_event = ProcessEvent {
            pid: 999,
            ppid: None,
            name: "minimal".to_string(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: false,
            file_exists: false,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let record = handler.convert_process_event_to_record(minimal_event);
        assert_eq!(record.pid, 999);
        assert_eq!(record.ppid, None);
        assert_eq!(record.name, "minimal");
        assert_eq!(record.executable_path, None);
        assert!(record.command_line.is_empty());
        assert_eq!(record.start_time, None);
        assert_eq!(record.cpu_usage, None);
        assert_eq!(record.memory_usage, None);
        assert_eq!(record.executable_hash, None);
        assert_eq!(record.hash_algorithm, None);
        assert_eq!(record.user_id, None);
        assert!(!record.accessible);
        assert!(!record.file_exists);
        assert!(record.collection_time > 0);

        // Test with process event that has hash but no algorithm should get sha256
        let event_with_hash = ProcessEvent {
            pid: 1000,
            ppid: Some(1),
            name: "with_hash".to_string(),
            executable_path: Some("/bin/test".to_string()),
            command_line: vec!["test".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(2.5),
            memory_usage: Some(4096),
            executable_hash: Some("abcdef123456".to_string()),
            user_id: Some("1001".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let record_with_hash = handler.convert_process_event_to_record(event_with_hash);
        assert_eq!(
            record_with_hash.executable_hash,
            Some("abcdef123456".to_string())
        );
        assert_eq!(record_with_hash.hash_algorithm, Some("sha256".to_string()));
    }

    #[tokio::test]
    async fn test_mock_collector_capabilities() {
        let mock_collector = MockProcessCollector::new();

        assert_eq!(mock_collector.name(), "mock-collector");

        let capabilities = mock_collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(capabilities.executable_hashing);
        assert!(capabilities.system_processes);
        assert!(capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    async fn test_mock_collector_health_check() {
        // Test successful health check
        let mock_collector = MockProcessCollector::new();
        let result = mock_collector.health_check().await;
        assert!(result.is_ok());

        // Test failed health check
        let failing_collector = MockProcessCollector::with_health_check_failure();
        let result = failing_collector.health_check().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ProcessCollectionError::SystemEnumerationFailed { message } => {
                assert_eq!(message, "Health check failed");
            }
            other => panic!("Expected SystemEnumerationFailed, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mock_collector_collect_single_process() {
        let mock_collector = MockProcessCollector::new();

        // Test collecting existing process
        let result = mock_collector.collect_process(1234).await;
        assert!(result.is_ok());

        let event = result.unwrap();
        assert_eq!(event.pid, 1234);
        assert_eq!(event.name, "test_process");

        // Test collecting non-existent process
        let result = mock_collector.collect_process(99999).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ProcessCollectionError::ProcessNotFound { pid } => {
                assert_eq!(pid, 99999);
            }
            other => panic!("Expected ProcessNotFound, got: {:?}", other),
        }
    }
}
