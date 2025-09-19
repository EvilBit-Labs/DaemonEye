//! Integration tests for IPC functionality.

use prost::Message;
use sentinel_lib::proto::{DetectionResult, DetectionTask, ProtoProcessRecord, ProtoTaskType};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use procmond::ipc::error::IpcError;
use procmond::ipc::{IpcConfig, SimpleMessageHandler, create_ipc_server};

#[tokio::test]
async fn test_ipc_server_creation() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    let config = IpcConfig {
        path: socket_path.to_string_lossy().to_string(),
        max_connections: 5,
        connection_timeout_secs: 30,
        message_timeout_secs: 60,
    };

    let server = create_ipc_server(config).unwrap();
    // Test that we can create the server without hanging
    assert_eq!(server.config().max_connections, 5);
}

#[tokio::test]
async fn test_simple_message_handler() {
    let handler = SimpleMessageHandler::new(
        "TestHandler".to_string(),
        |task: DetectionTask| async move {
            match task.task_type {
                task_type if task_type == ProtoTaskType::EnumerateProcesses as i32 => {
                    let processes = vec![ProtoProcessRecord {
                        pid: 1234,
                        ppid: Some(1),
                        name: "test-process".to_string(),
                        executable_path: Some("/usr/bin/test".to_string()),
                        command_line: vec!["test".to_string()],
                        start_time: Some(chrono::Utc::now().timestamp()),
                        cpu_usage: Some(25.5),
                        memory_usage: Some(1024 * 1024),
                        executable_hash: Some("abc123".to_string()),
                        hash_algorithm: Some("sha256".to_string()),
                        user_id: Some("1000".to_string()),
                        accessible: true,
                        file_exists: true,
                        collection_time: chrono::Utc::now().timestamp_millis(),
                    }];
                    Ok(DetectionResult::success(&task.task_id, processes))
                }
                _ => Ok(DetectionResult::failure(
                    &task.task_id,
                    "Unsupported task type",
                )),
            }
        },
    );

    // Test enumerate processes task
    let task = DetectionTask {
        task_id: "test-123".to_string(),
        task_type: ProtoTaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
    };

    let result = handler.handle(task).await.unwrap();
    assert!(result.success);
    assert_eq!(result.task_id, "test-123");
    assert_eq!(result.processes.len(), 1);
    assert_eq!(result.processes[0].name, "test-process");
}

#[tokio::test]
async fn test_ipc_config_default() {
    let config = IpcConfig::default();
    assert_eq!(config.path, "/tmp/sentineld-procmond.sock");
    assert_eq!(config.max_connections, 10);
    assert_eq!(config.connection_timeout_secs, 30);
    assert_eq!(config.message_timeout_secs, 60);
}

#[tokio::test]
async fn test_simple_message_handler_unsupported_task() {
    let handler = SimpleMessageHandler::new(
        "TestHandler".to_string(),
        |task: DetectionTask| async move {
            match task.task_type {
                task_type if task_type == ProtoTaskType::EnumerateProcesses as i32 => {
                    Ok(DetectionResult::success(&task.task_id, vec![]))
                }
                _ => Ok(DetectionResult::failure(
                    &task.task_id,
                    "Unsupported task type",
                )),
            }
        },
    );

    // Test unsupported task type
    let task = DetectionTask {
        task_id: "test-456".to_string(),
        task_type: 999, // Invalid task type
        process_filter: None,
        hash_check: None,
        metadata: None,
    };

    let result = handler.handle(task).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.task_id, "test-456");
    assert!(result.error_message.is_some());
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unsupported task type")
    );
}

#[tokio::test]
async fn test_detection_task_creation() {
    let task = DetectionTask::new_enumerate_processes("test-task", None);
    assert_eq!(task.task_id, "test-task");
    assert_eq!(task.task_type, ProtoTaskType::EnumerateProcesses as i32);
    assert!(task.process_filter.is_none());
    assert!(task.hash_check.is_none());
}

#[tokio::test]
async fn test_detection_result_creation() {
    let processes = vec![ProtoProcessRecord {
        pid: 1234,
        ppid: Some(1),
        name: "test".to_string(),
        executable_path: None,
        command_line: vec![],
        start_time: None,
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        hash_algorithm: None,
        user_id: None,
        accessible: true,
        file_exists: false,
        collection_time: chrono::Utc::now().timestamp_millis(),
    }];

    let success_result = DetectionResult::success("test-task", processes.clone());
    assert!(success_result.success);
    assert_eq!(success_result.task_id, "test-task");
    assert_eq!(success_result.processes.len(), 1);
    assert!(success_result.error_message.is_none());

    let failure_result = DetectionResult::failure("test-task", "Test error");
    assert!(!failure_result.success);
    assert_eq!(failure_result.task_id, "test-task");
    assert_eq!(failure_result.error_message.as_deref(), Some("Test error"));
    assert!(failure_result.processes.is_empty());
}

#[tokio::test]
async fn test_ipc_message_serialization() {
    let task = DetectionTask {
        task_id: "serialization-test".to_string(),
        task_type: ProtoTaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
    };

    // Test serialization
    let serialized = task.encode_to_vec();
    assert!(!serialized.is_empty());

    // Test deserialization
    let deserialized = DetectionTask::decode(&serialized[..]).unwrap();
    assert_eq!(deserialized.task_id, task.task_id);
    assert_eq!(deserialized.task_type, task.task_type);
}

#[tokio::test]
async fn test_ipc_result_serialization() {
    let processes = vec![ProtoProcessRecord {
        pid: 9999,
        ppid: Some(1),
        name: "serialization-test".to_string(),
        executable_path: Some("/usr/bin/test".to_string()),
        command_line: vec!["test".to_string(), "--serialization".to_string()],
        start_time: Some(chrono::Utc::now().timestamp()),
        cpu_usage: Some(50.0),
        memory_usage: Some(2048 * 1024),
        executable_hash: Some("def456".to_string()),
        hash_algorithm: Some("sha256".to_string()),
        user_id: Some("2000".to_string()),
        accessible: true,
        file_exists: true,
        collection_time: chrono::Utc::now().timestamp_millis(),
    }];

    let result = DetectionResult::success("serialization-test", processes);

    // Test serialization
    let serialized = result.encode_to_vec();
    assert!(!serialized.is_empty());

    // Test deserialization
    let deserialized = DetectionResult::decode(&serialized[..]).unwrap();
    assert_eq!(deserialized.task_id, result.task_id);
    assert_eq!(deserialized.success, result.success);
    assert_eq!(deserialized.processes.len(), result.processes.len());
}

#[tokio::test]
async fn test_ipc_error_handling() {
    let handler = SimpleMessageHandler::new(
        "ErrorTestHandler".to_string(),
        |_task: DetectionTask| async move { Err(IpcError::internal("Simulated processing error")) },
    );

    let task = DetectionTask {
        task_id: "error-test".to_string(),
        task_type: ProtoTaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
    };

    let result = handler.handle(task).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        IpcError::Internal(msg) => assert!(msg.contains("Simulated processing error")),
        _ => panic!("Expected Internal error"),
    }
}

#[tokio::test]
async fn test_ipc_timeout_handling() {
    let handler = SimpleMessageHandler::new(
        "TimeoutTestHandler".to_string(),
        |task: DetectionTask| async move {
            // Simulate a long-running task
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(DetectionResult::success(&task.task_id, vec![]))
        },
    );

    let task = DetectionTask {
        task_id: "timeout-test".to_string(),
        task_type: ProtoTaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: None,
    };

    // Test with timeout
    let result = timeout(Duration::from_millis(200), handler.handle(task)).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.is_ok());
}
