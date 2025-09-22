//! Library module for procmond to enable unit testing

pub mod event_source;
pub use event_source::{ProcessEventSource, ProcessSourceConfig};

// Re-export main functionality for testing
pub use daemoneye_lib::proto::{
    DetectionResult, DetectionTask, ProtoProcessRecord, ProtoTaskType, TaskType,
};
pub use daemoneye_lib::storage;

use daemoneye_lib::ipc::IpcError;
use std::sync::Arc;
use sysinfo::System;
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
/// - Enumerating system processes using the `sysinfo` crate
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
/// `DatabaseManager` wrapped in `Arc<Mutex<>>` for thread-safe database access.
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::ProcessMessageHandler;
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
/// // Create the process message handler
/// let handler = ProcessMessageHandler::new(db_manager);
///
/// // The handler is now ready to process detection tasks via IPC
/// // This would typically be done by the IPC server in the main procmond loop
/// ```
///
/// # Thread Safety
///
/// This struct is designed to be used in a multi-threaded environment. The
/// `DatabaseManager` is wrapped in `Arc<Mutex<>>` to ensure thread-safe access
/// to the underlying database operations.
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
}

impl ProcessMessageHandler {
    pub fn new(database: Arc<Mutex<storage::DatabaseManager>>) -> Self {
        Self { database }
    }

    pub async fn handle_detection_task(
        &self,
        task: DetectionTask,
    ) -> Result<DetectionResult, IpcError> {
        tracing::info!("Received detection task: {}", task.task_id);

        match task.task_type {
            task_type if task_type == ProtoTaskType::EnumerateProcesses as i32 => {
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

    /// Enumerate all processes on the system
    pub async fn enumerate_processes(
        &self,
        task: &DetectionTask,
    ) -> Result<DetectionResult, IpcError> {
        let mut system = System::new_all();
        system.refresh_all();

        let processes: Vec<ProtoProcessRecord> = system
            .processes()
            .iter()
            .map(|(pid, process)| self.convert_process_to_record(pid, process))
            .collect();

        Ok(DetectionResult::success(&task.task_id, processes))
    }

    /// Convert a sysinfo process to a ProtoProcessRecord
    pub fn convert_process_to_record(
        &self,
        pid: &sysinfo::Pid,
        process: &sysinfo::Process,
    ) -> ProtoProcessRecord {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(|p| p.as_u32());
        let name = process.name().to_string_lossy().to_string();
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        let start_time = Some(process.start_time() as i64);
        let cpu_usage = Some(process.cpu_usage() as f64);
        let memory_usage = Some(process.memory() * 1024);
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_process_message_handler_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ));

        let _handler = ProcessMessageHandler::new(db_manager);
        // Handler should be created successfully
        // Test placeholder - will be implemented in actual tests
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
    fn test_convert_process_to_record() {
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
    async fn test_enumerate_processes() {
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
}
