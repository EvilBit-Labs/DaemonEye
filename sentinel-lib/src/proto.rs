//! Protocol Buffer definitions for IPC communication.
//!
//! This module provides type-safe protobuf message definitions for communication
//! between procmond and sentinelagent components. The types are automatically
//! generated from the .proto files in the `proto/` directory during build.

use crate::models::{ProcessId, process::ProcessRecord as NativeProcessRecord};
use chrono::{DateTime, Utc};
use std::time::UNIX_EPOCH;

// Include the generated protobuf code
#[allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::pattern_type_mismatch
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}

// Re-export everything from the generated module
pub use generated::*;

// Re-export commonly used types with cleaner names
pub use self::{
    HashCheck as ProtoHashCheck, HashResult as ProtoHashResult,
    ProcessFilter as ProtoProcessFilter, ProcessRecord as ProtoProcessRecord,
    TaskType as ProtoTaskType,
};

impl From<NativeProcessRecord> for ProtoProcessRecord {
    /// Convert from native `ProcessRecord` to protobuf `ProcessRecord`.
    ///
    /// This conversion extracts data from the native `ProcessRecord` and maps it
    /// to the corresponding protobuf fields, handling type conversions and
    /// optional fields appropriately.
    fn from(native: NativeProcessRecord) -> Self {
        let has_executable_path = native.executable_path.is_some();
        Self {
            pid: native.pid.raw(),
            ppid: native.ppid.map(super::models::process::ProcessId::raw),
            name: native.name,
            executable_path: native
                .executable_path
                .map(|p| p.to_string_lossy().into_owned()),
            command_line: native.command_line.map(|cmd| vec![cmd]).unwrap_or_default(),
            start_time: native.start_time.and_then(|st| {
                st.duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
            }),
            cpu_usage: native.cpu_usage,
            memory_usage: native.memory_usage,
            executable_hash: native.executable_hash,
            hash_algorithm: native.hash_algorithm,
            user_id: native.user_id.map(|uid| uid.to_string()),
            accessible: true, // Default to true, can be overridden by specific implementations
            file_exists: has_executable_path, // Approximate - actual file existence check would be done elsewhere
            collection_time: native.collection_time.timestamp_millis(),
        }
    }
}

impl From<ProtoProcessRecord> for NativeProcessRecord {
    /// Convert from protobuf `ProcessRecord` to native `ProcessRecord`.
    ///
    /// This conversion maps protobuf fields back to the native `ProcessRecord`
    /// structure, handling type conversions and providing sensible defaults
    /// for fields that may not be present.
    fn from(proto: ProtoProcessRecord) -> Self {
        Self {
            pid: ProcessId::new(proto.pid),
            ppid: proto.ppid.map(ProcessId::new),
            name: proto.name,
            executable_path: proto.executable_path.map(std::convert::Into::into),
            command_line: proto.command_line.first().cloned(),
            start_time: proto
                .start_time
                .and_then(|ts| u64::try_from(ts).ok())
                .and_then(|ts| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(ts))),
            cpu_usage: proto.cpu_usage,
            memory_usage: proto.memory_usage,
            status: crate::models::process::ProcessStatus::Unknown("proto".to_owned()),
            executable_hash: proto.executable_hash,
            hash_algorithm: proto.hash_algorithm,
            collection_time: DateTime::from_timestamp_millis(proto.collection_time)
                .unwrap_or_else(Utc::now),
            user_id: proto.user_id.and_then(|uid| uid.parse().ok()),
            group_id: None, // Not available in protobuf version
            environment_vars: std::collections::HashMap::new(), // Not available in protobuf version
            metadata: std::collections::HashMap::new(), // Not available in protobuf version
        }
    }
}

impl DetectionTask {
    /// Create a new process enumeration task.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::proto::DetectionTask;
    /// let task = DetectionTask::new_enumerate_processes("task-123", None);
    /// assert_eq!(task.task_id, "task-123");
    /// ```
    pub fn new_enumerate_processes(
        task_id: impl Into<String>,
        filter: Option<ProtoProcessFilter>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            task_type: i32::from(TaskType::EnumerateProcesses),
            process_filter: filter,
            hash_check: None,
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        }
    }

    /// Create a new hash check task.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::proto::{DetectionTask, ProtoHashCheck};
    /// let hash_check = ProtoHashCheck {
    ///     expected_hash: "abc123".to_string(),
    ///     hash_algorithm: "sha256".to_string(),
    ///     executable_path: "/usr/bin/firefox".to_string(),
    /// };
    /// let task = DetectionTask::new_hash_check("task-456", hash_check);
    /// assert_eq!(task.task_id, "task-456");
    /// ```
    pub fn new_hash_check(task_id: impl Into<String>, hash_check: ProtoHashCheck) -> Self {
        Self {
            task_id: task_id.into(),
            task_type: i32::from(TaskType::CheckProcessHash),
            process_filter: None,
            hash_check: Some(hash_check),
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        }
    }

    /// Create a new detection task with custom parameters (for testing).
    ///
    /// This is a convenience method for creating detection tasks in tests
    /// without having to specify all the new optional fields.
    pub fn new_test_task(
        task_id: impl Into<String>,
        task_type: TaskType,
        metadata: Option<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            task_type: i32::from(task_type),
            process_filter: None,
            hash_check: None,
            metadata,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        }
    }
}

impl DetectionResult {
    /// Create a successful detection result.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::proto::DetectionResult;
    /// let result = DetectionResult::success("task-123", vec![]);
    /// assert!(result.success);
    /// assert_eq!(result.task_id, "task-123");
    /// ```
    pub fn success(task_id: impl Into<String>, processes: Vec<ProtoProcessRecord>) -> Self {
        Self {
            task_id: task_id.into(),
            success: true,
            error_message: None,
            processes,
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        }
    }

    /// Create a failed detection result.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::proto::DetectionResult;
    /// let result = DetectionResult::failure("task-123", "Permission denied");
    /// assert!(!result.success);
    /// assert_eq!(result.error_message.as_deref(), Some("Permission denied"));
    /// ```
    pub fn failure(task_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            success: false,
            error_message: Some(error.into()),
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        }
    }

    /// Create a successful hash check result.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::proto::{DetectionResult, ProtoHashResult};
    /// let hash_result = ProtoHashResult {
    ///     hash_value: "abc123".to_string(),
    ///     algorithm: "sha256".to_string(),
    ///     file_path: "/usr/bin/firefox".to_string(),
    ///     success: true,
    ///     error_message: None,
    /// };
    /// let result = DetectionResult::hash_success("task-456", hash_result);
    /// assert!(result.success);
    /// ```
    pub fn hash_success(task_id: impl Into<String>, hash_result: ProtoHashResult) -> Self {
        Self {
            task_id: task_id.into(),
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: Some(hash_result),
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::shadow_unrelated,
    clippy::redundant_clone
)]
mod tests {
    use super::*;
    use crate::models::process::{ProcessRecord as NativeProcessRecord, ProcessStatus};

    #[test]
    fn test_detection_task_creation() {
        let task = DetectionTask::new_test_task(
            "test-123",
            TaskType::EnumerateProcesses,
            Some("test metadata".to_string()),
        );

        assert_eq!(task.task_id, "test-123");
        assert_eq!(task.task_type, ProtoTaskType::EnumerateProcesses as i32);
        assert_eq!(task.metadata.as_deref(), Some("test metadata"));
    }

    #[test]
    fn test_detection_task_builders() {
        // Test enumerate processes task
        let task = DetectionTask::new_enumerate_processes("enum-123", None);
        assert_eq!(task.task_id, "enum-123");
        assert_eq!(task.task_type, TaskType::EnumerateProcesses as i32);
        assert!(task.process_filter.is_none());

        // Test hash check task
        let hash_check = ProtoHashCheck {
            expected_hash: "abc123".to_string(),
            hash_algorithm: "sha256".to_string(),
            executable_path: "/usr/bin/firefox".to_string(),
        };
        let task = DetectionTask::new_hash_check("hash-456", hash_check);
        assert_eq!(task.task_id, "hash-456");
        assert_eq!(task.task_type, TaskType::CheckProcessHash as i32);
        assert!(task.hash_check.is_some());
    }

    #[test]
    fn test_detection_result_creation() {
        // Test success result
        let result = DetectionResult::success("test-123", vec![]);
        assert_eq!(result.task_id, "test-123");
        assert!(result.success);
        assert!(result.error_message.is_none());
        assert!(result.processes.is_empty());

        // Test failure result
        let result = DetectionResult::failure("test-456", "Permission denied");
        assert_eq!(result.task_id, "test-456");
        assert!(!result.success);
        assert_eq!(result.error_message.as_deref(), Some("Permission denied"));
    }

    #[test]
    fn test_process_filter_creation() {
        let filter = ProtoProcessFilter {
            process_names: vec!["firefox".to_string(), "chrome".to_string()],
            pids: vec![1234, 5678],
            executable_pattern: Some("/usr/bin/*".to_string()),
        };

        assert_eq!(filter.process_names.len(), 2);
        assert_eq!(filter.pids.len(), 2);
        assert_eq!(filter.executable_pattern.as_deref(), Some("/usr/bin/*"));
    }

    #[test]
    fn test_hash_check_creation() {
        let hash_check = ProtoHashCheck {
            expected_hash: "abc123def456".to_string(),
            hash_algorithm: "sha256".to_string(),
            executable_path: "/usr/bin/firefox".to_string(),
        };

        assert_eq!(hash_check.expected_hash, "abc123def456");
        assert_eq!(hash_check.hash_algorithm, "sha256");
        assert_eq!(hash_check.executable_path, "/usr/bin/firefox");
    }

    #[test]
    fn test_process_record_conversion() {
        // Create a native ProcessRecord
        let native = NativeProcessRecord::builder()
            .pid_raw(1234)
            .name("test-process")
            .executable_path("/usr/bin/test")
            .command_line("test --arg value")
            .cpu_usage(0.5)
            .memory_usage(1024 * 1024)
            .status(ProcessStatus::Running)
            .user_id(1000)
            .build()
            .expect("Failed to build process record");

        // Convert to protobuf
        let proto: ProtoProcessRecord = native.clone().into();
        assert_eq!(proto.pid, 1234);
        assert_eq!(proto.name, "test-process");
        assert_eq!(proto.executable_path.as_deref(), Some("/usr/bin/test"));
        assert_eq!(proto.command_line, vec!["test --arg value"]);
        assert_eq!(proto.cpu_usage, Some(0.5));
        assert_eq!(proto.memory_usage, Some(1024 * 1024));
        assert_eq!(proto.user_id.as_deref(), Some("1000"));

        // Convert back to native
        let converted_native: NativeProcessRecord = proto.into();
        assert_eq!(converted_native.pid.raw(), 1234);
        assert_eq!(converted_native.name, "test-process");
        assert_eq!(
            converted_native.executable_path.as_deref(),
            Some(std::path::Path::new("/usr/bin/test"))
        );
        assert_eq!(
            converted_native.command_line.as_deref(),
            Some("test --arg value")
        );
        assert_eq!(converted_native.cpu_usage, Some(0.5));
        assert_eq!(converted_native.memory_usage, Some(1024 * 1024));
        assert_eq!(converted_native.user_id, Some(1000));
    }

    #[test]
    fn test_protobuf_serialization() {
        let task = DetectionTask::new_enumerate_processes("test-task", None);

        // Test that the types implement the necessary traits for serialization
        let _json = serde_json::to_string(&task).expect("Should serialize to JSON");

        let result = DetectionResult::success("test-task", vec![]);
        let _json = serde_json::to_string(&result).expect("Should serialize to JSON");
    }

    #[test]
    fn test_task_type_values() {
        assert_eq!(TaskType::EnumerateProcesses as i32, 0);
        assert_eq!(TaskType::CheckProcessHash as i32, 1);
        assert_eq!(TaskType::MonitorProcessTree as i32, 2);
        assert_eq!(TaskType::VerifyExecutable as i32, 3);
    }
}
