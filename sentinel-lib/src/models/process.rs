//! Process monitoring data structures and types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;
use thiserror::Error;

/// Strongly-typed process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessId(u32);

impl ProcessId {
    /// Create a new process ID.
    pub fn new(pid: u32) -> Self {
        Self(pid)
    }

    /// Get the raw process ID value.
    pub fn raw(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Process status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessStatus {
    /// Process is running
    Running,
    /// Process is sleeping/waiting
    Sleeping,
    /// Process is stopped
    Stopped,
    /// Process is a zombie
    Zombie,
    /// Process is being traced
    Traced,
    /// Process is in an unknown state
    Unknown(String),
}

impl fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessStatus::Running => write!(f, "running"),
            ProcessStatus::Sleeping => write!(f, "sleeping"),
            ProcessStatus::Stopped => write!(f, "stopped"),
            ProcessStatus::Zombie => write!(f, "zombie"),
            ProcessStatus::Traced => write!(f, "traced"),
            ProcessStatus::Unknown(s) => write!(f, "unknown({})", s),
        }
    }
}

/// Comprehensive process record with all metadata fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRecord {
    /// Process identifier
    pub pid: ProcessId,
    /// Parent process identifier
    pub ppid: Option<ProcessId>,
    /// Process name
    pub name: String,
    /// Path to executable file
    pub executable_path: Option<PathBuf>,
    /// Command line arguments
    pub command_line: Option<String>,
    /// Process start time
    pub start_time: Option<SystemTime>,
    /// CPU usage percentage
    pub cpu_usage: Option<f64>,
    /// Memory usage in bytes
    pub memory_usage: Option<u64>,
    /// Process status
    pub status: ProcessStatus,
    /// Executable file hash
    pub executable_hash: Option<String>,
    /// Hash algorithm used
    pub hash_algorithm: Option<String>,
    /// Collection timestamp
    pub collection_time: DateTime<Utc>,
    /// User ID
    pub user_id: Option<u32>,
    /// Group ID
    pub group_id: Option<u32>,
    /// Environment variables
    pub environment_vars: HashMap<String, String>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl ProcessRecord {
    /// Create a new process record with minimal required fields.
    pub fn new(pid: u32, name: String) -> Self {
        Self {
            pid: ProcessId::new(pid),
            ppid: None,
            name,
            executable_path: None,
            command_line: None,
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            status: ProcessStatus::Unknown("unknown".to_string()),
            executable_hash: None,
            hash_algorithm: None,
            collection_time: Utc::now(),
            user_id: None,
            group_id: None,
            environment_vars: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Create a builder for constructing process records.
    pub fn builder() -> ProcessRecordBuilder {
        ProcessRecordBuilder::new()
    }
}

/// Builder for constructing ProcessRecord instances.
#[derive(Debug, Default)]
pub struct ProcessRecordBuilder {
    pid: Option<u32>,
    ppid: Option<ProcessId>,
    name: Option<String>,
    executable_path: Option<PathBuf>,
    command_line: Option<String>,
    start_time: Option<SystemTime>,
    cpu_usage: Option<f64>,
    memory_usage: Option<u64>,
    status: Option<ProcessStatus>,
    executable_hash: Option<String>,
    hash_algorithm: Option<String>,
    collection_time: Option<DateTime<Utc>>,
    user_id: Option<u32>,
    group_id: Option<u32>,
    environment_vars: HashMap<String, String>,
    metadata: HashMap<String, String>,
}

impl ProcessRecordBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the process ID.
    pub fn pid_raw(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the parent process ID.
    pub fn ppid(mut self, ppid: ProcessId) -> Self {
        self.ppid = Some(ppid);
        self
    }

    /// Set the process name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the executable path.
    pub fn executable_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable_path = Some(path.into());
        self
    }

    /// Set the command line.
    pub fn command_line(mut self, cmd: impl Into<String>) -> Self {
        self.command_line = Some(cmd.into());
        self
    }

    /// Set the start time.
    pub fn start_time(mut self, time: SystemTime) -> Self {
        self.start_time = Some(time);
        self
    }

    /// Set the CPU usage.
    pub fn cpu_usage(mut self, usage: f64) -> Self {
        self.cpu_usage = Some(usage);
        self
    }

    /// Set the memory usage.
    pub fn memory_usage(mut self, usage: u64) -> Self {
        self.memory_usage = Some(usage);
        self
    }

    /// Set the process status.
    pub fn status(mut self, status: ProcessStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set the executable hash.
    pub fn executable_hash(mut self, hash: impl Into<String>) -> Self {
        self.executable_hash = Some(hash.into());
        self
    }

    /// Set the hash algorithm.
    pub fn hash_algorithm(mut self, algorithm: impl Into<String>) -> Self {
        self.hash_algorithm = Some(algorithm.into());
        self
    }

    /// Set the collection time.
    pub fn collection_time(mut self, time: DateTime<Utc>) -> Self {
        self.collection_time = Some(time);
        self
    }

    /// Set the user ID.
    pub fn user_id(mut self, user_id: u32) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set the group ID.
    pub fn group_id(mut self, group_id: u32) -> Self {
        self.group_id = Some(group_id);
        self
    }

    /// Add an environment variable.
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment_vars.insert(key.into(), value.into());
        self
    }

    /// Add multiple environment variables.
    pub fn env_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.environment_vars.extend(vars);
        self
    }

    /// Add a metadata entry.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Add multiple metadata entries.
    pub fn metadata_map(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata.extend(metadata);
        self
    }

    /// Build the ProcessRecord.
    pub fn build(self) -> Result<ProcessRecord, ProcessError> {
        let pid = self.pid.ok_or(ProcessError::MissingField("pid"))?;
        let name = self.name.ok_or(ProcessError::MissingField("name"))?;

        Ok(ProcessRecord {
            pid: ProcessId::new(pid),
            ppid: self.ppid,
            name,
            executable_path: self.executable_path,
            command_line: self.command_line,
            start_time: self.start_time,
            cpu_usage: self.cpu_usage,
            memory_usage: self.memory_usage,
            status: self
                .status
                .unwrap_or(ProcessStatus::Unknown("unknown".to_string())),
            executable_hash: self.executable_hash,
            hash_algorithm: self.hash_algorithm,
            collection_time: self.collection_time.unwrap_or_else(Utc::now),
            user_id: self.user_id,
            group_id: self.group_id,
            environment_vars: self.environment_vars,
            metadata: self.metadata,
        })
    }
}

/// System information and capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemInfo {
    /// Operating system name
    pub os_name: String,
    /// Operating system version
    pub os_version: String,
    /// System architecture
    pub architecture: String,
    /// Total system memory in bytes
    pub total_memory: u64,
    /// Number of CPU cores
    pub cpu_cores: usize,
    /// System uptime in seconds
    pub uptime: u64,
    /// System hostname
    pub hostname: String,
    /// System capabilities
    pub capabilities: Vec<String>,
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemInfo {
    /// Create a new system info instance.
    pub fn new() -> Self {
        Self {
            os_name: "unknown".to_string(),
            os_version: "unknown".to_string(),
            architecture: "unknown".to_string(),
            total_memory: 0,
            cpu_cores: 0,
            uptime: 0,
            hostname: "unknown".to_string(),
            capabilities: Vec::new(),
        }
    }

    /// Add a capability to the system.
    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capabilities.push(capability.into());
        self
    }
}

/// Process-related errors.
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("Missing required field: {0}")]
    MissingField(&'static str),
    #[error("Invalid process ID: {0}")]
    InvalidProcessId(u32),
    #[error("Process not found: {0}")]
    ProcessNotFound(u32),
    #[error("Permission denied accessing process {0}")]
    PermissionDenied(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_record_creation() {
        let process = ProcessRecord::new(1234, "test-process".to_string());
        assert_eq!(process.pid.raw(), 1234);
        assert_eq!(process.name, "test-process");
        assert_eq!(
            process.status,
            ProcessStatus::Unknown("unknown".to_string())
        );
    }

    #[test]
    fn test_process_record_builder() {
        let process = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test-process")
            .executable_path("/usr/bin/test")
            .command_line("test --arg value")
            .cpu_usage(25.5)
            .memory_usage(1024 * 1024)
            .status(ProcessStatus::Running)
            .user_id(1000)
            .group_id(1000)
            .env_var("PATH", "/usr/bin:/bin")
            .metadata("source", "test")
            .build()
            .unwrap();

        assert_eq!(process.pid.raw(), 1234);
        assert_eq!(process.name, "test-process");
        assert_eq!(
            process.executable_path,
            Some(PathBuf::from("/usr/bin/test"))
        );
        assert_eq!(process.command_line, Some("test --arg value".to_string()));
        assert_eq!(process.cpu_usage, Some(25.5));
        assert_eq!(process.memory_usage, Some(1024 * 1024));
        assert_eq!(process.status, ProcessStatus::Running);
        assert_eq!(process.user_id, Some(1000));
        assert_eq!(process.group_id, Some(1000));
        assert_eq!(
            process.environment_vars.get("PATH"),
            Some(&"/usr/bin:/bin".to_string())
        );
        assert_eq!(process.metadata.get("source"), Some(&"test".to_string()));
    }

    #[test]
    fn test_process_record_serialization() {
        let process = ProcessRecord::builder()
            .pid_raw(1234)
            .name("test-process")
            .executable_path("/usr/bin/test")
            .command_line("test --arg value")
            .cpu_usage(25.5)
            .memory_usage(1024 * 1024)
            .status(ProcessStatus::Running)
            .user_id(1000)
            .group_id(1000)
            .env_var("PATH", "/usr/bin:/bin")
            .metadata("source", "test")
            .build()
            .unwrap();

        // Test JSON serialization
        let json = serde_json::to_string(&process).unwrap();
        let deserialized: ProcessRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(process, deserialized);
    }

    #[test]
    fn test_process_id_operations() {
        let pid = ProcessId::new(1234);
        assert_eq!(pid.raw(), 1234);
        assert_eq!(pid.to_string(), "1234");
    }

    #[test]
    fn test_process_status_display() {
        assert_eq!(ProcessStatus::Running.to_string(), "running");
        assert_eq!(ProcessStatus::Sleeping.to_string(), "sleeping");
        assert_eq!(ProcessStatus::Stopped.to_string(), "stopped");
        assert_eq!(
            ProcessStatus::Unknown("test".to_string()).to_string(),
            "unknown(test)"
        );
    }
}
