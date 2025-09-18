//! Protocol Buffer definitions for IPC communication.
//!
//! This module provides type-safe protobuf message definitions for communication
//! between procmond and sentinelagent components. The types are automatically
//! generated from the .proto files in the `proto/` directory during build.

// Include the generated protobuf code
include!(concat!(env!("OUT_DIR"), "/_.rs"));

// Re-export commonly used types with cleaner names
pub use self::{
    HashCheck as ProtoHashCheck, HashResult as ProtoHashResult,
    ProcessFilter as ProtoProcessFilter, ProcessRecord as ProtoProcessRecord,
    TaskType as ProtoTaskType,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detection_task_creation() {
        let task = DetectionTask {
            task_id: "test-123".to_string(),
            task_type: ProtoTaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some("test metadata".to_string()),
        };

        assert_eq!(task.task_id, "test-123");
        assert_eq!(task.task_type, ProtoTaskType::EnumerateProcesses as i32);
        assert_eq!(task.metadata.as_deref(), Some("test metadata"));
    }

    #[test]
    fn test_detection_result_creation() {
        let result = DetectionResult {
            task_id: "test-123".to_string(),
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
        };

        assert_eq!(result.task_id, "test-123");
        assert!(result.success);
        assert!(result.processes.is_empty());
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
}
