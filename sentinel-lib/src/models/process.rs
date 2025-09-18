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
    /// Create a new ProcessId from a raw numeric PID.
    ///
    /// This constructs the strongly-typed `ProcessId` newtype wrapping the given
    /// process identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::process::ProcessId;
    /// let pid = ProcessId::new(1234);
    /// assert_eq!(pid.raw(), 1234);
    /// ```
    pub fn new(pid: u32) -> Self {
        Self(pid)
    }

    /// Returns the underlying u32 process identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::process::ProcessId;
    /// let pid = ProcessId::new(1234);
    /// assert_eq!(pid.raw(), 1234);
    /// ```
    pub fn raw(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for ProcessId {
    /// Formats the ProcessId as its numeric PID string.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::process::ProcessId;
    /// let pid = ProcessId::new(1234);
    /// assert_eq!(pid.to_string(), "1234");
    /// ```
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
    /// Formats the process status as a human-readable, lowercase string.
    ///
    /// `Running`, `Sleeping`, `Stopped`, `Zombie`, and `Traced` are rendered as
    /// `"running"`, `"sleeping"`, `"stopped"`, `"zombie"`, and `"traced"`
    /// respectively. `Unknown(s)` is rendered as `unknown(<s>)`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::process::ProcessStatus;
    ///
    /// assert_eq!(format!("{}", ProcessStatus::Running), "running");
    /// assert_eq!(format!("{}", ProcessStatus::Unknown("custom".into())), "unknown(custom)");
    /// ```
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
    /// Create a new ProcessRecord with the minimal required fields: `pid` and `name`.
    ///
    /// The returned record initializes optional fields to sensible defaults:
    /// - `status` is set to `ProcessStatus::Unknown("unknown")`.
    /// - `collection_time` is set to the current UTC time.
    /// - `ppid`, `executable_path`, `command_line`, `start_time`, `cpu_usage`,
    ///   `memory_usage`, `executable_hash`, `hash_algorithm`, `user_id`, and `group_id`
    ///   are `None`.
    /// - `environment_vars` and `metadata` are empty maps.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::{ProcessRecord, ProcessStatus};
    /// let rec = ProcessRecord::new(1234, "test-process".to_string());
    /// assert_eq!(rec.pid.raw(), 1234);
    /// assert_eq!(rec.name, "test-process");
    /// match rec.status {
    ///     ProcessStatus::Unknown(ref s) => assert_eq!(s, "unknown"),
    ///     _ => panic!("unexpected status"),
    /// }
    /// ```
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

    /// Returns a new ProcessRecordBuilder for fluent construction of a ProcessRecord.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let record = ProcessRecord::builder()
    ///     .pid_raw(1234)
    ///     .name("example-process")
    ///     .build()
    ///     .unwrap();
    ///
    /// assert_eq!(record.pid.raw(), 1234);
    /// assert_eq!(record.name, "example-process");
    /// ```
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
    /// Creates a new ProcessRecordBuilder with default (empty) state.
    ///
    /// The returned builder can be used with the fluent setter methods to populate
    /// fields and then finalize into a `ProcessRecord` with `build()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let builder = ProcessRecordBuilder::new();
    /// let record = builder
    ///     .pid_raw(1234)
    ///     .name("example")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.pid.raw(), 1234);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the raw process ID on the builder.
    ///
    /// This sets the builder's `pid` to the given numeric process identifier and
    /// returns the builder for fluent chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let record = ProcessRecordBuilder::new()
    ///     .pid_raw(1234)
    ///     .name("test-process")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.pid.raw(), 1234);
    /// ```
    pub fn pid_raw(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the parent process ID for the builder.
    ///
    /// This sets the `ppid` field on the `ProcessRecordBuilder` to the given `ProcessId`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{ProcessRecord, ProcessId};
    /// let rec = ProcessRecord::builder()
    ///     .pid_raw(1)
    ///     .name("proc")
    ///     .ppid(ProcessId::new(2))
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(rec.ppid.unwrap().raw(), 2);
    /// ```
    pub fn ppid(mut self, ppid: ProcessId) -> Self {
        self.ppid = Some(ppid);
        self
    }

    /// Sets the process name on the builder and returns the updated builder for chaining.
    ///
    /// This value will be used as the `name` field of the resulting `ProcessRecord` produced by
    /// `build()`. Calling this replaces any previously set name.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let builder = ProcessRecordBuilder::new().pid_raw(42).name("my-process");
    /// let record = builder.build().unwrap();
    /// assert_eq!(record.name, "my-process");
    /// ```
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the builder's executable path for the process.
    ///
    /// The provided value is converted into a `PathBuf` and stored as the
    /// process's `executable_path`. Returns the modified builder to allow
    /// method chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let record = ProcessRecord::builder()
    ///     .pid_raw(1)
    ///     .name("example")
    ///     .executable_path("/usr/bin/example")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.executable_path, Some(PathBuf::from("/usr/bin/example")));
    /// ```
    pub fn executable_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable_path = Some(path.into());
        self
    }

    /// Sets the process command-line for the builder.
    ///
    /// This assigns the full command-line string that will be stored on the resulting
    /// `ProcessRecord`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let record = ProcessRecordBuilder::new()
    ///     .pid_raw(1234)
    ///     .name("myproc")
    ///     .command_line("/usr/bin/myproc --flag")
    ///     .build()
    ///     .unwrap();
    ///
    /// assert_eq!(record.command_line.as_deref(), Some("/usr/bin/myproc --flag"));
    /// ```
    pub fn command_line(mut self, cmd: impl Into<String>) -> Self {
        self.command_line = Some(cmd.into());
        self
    }

    /// Sets the process start time on the builder and returns the builder for chaining.
    ///
    /// The provided `SystemTime` will be stored as the record's `start_time` when the
    /// `ProcessRecord` is built.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let builder = ProcessRecordBuilder::new().start_time(SystemTime::now());
    /// ```
    pub fn start_time(mut self, time: SystemTime) -> Self {
        self.start_time = Some(time);
        self
    }

    /// Sets the CPU usage value on the builder.
    ///
    /// The value is stored as an `f64` and will be placed into the resulting
    /// `ProcessRecord`'s `cpu_usage` field. Callers decide the unit/semantics (commonly a percentage).
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let record = ProcessRecord::builder()
    ///     .pid_raw(1234)
    ///     .name("example")
    ///     .cpu_usage(12.5)
    ///     .build()
    ///     .unwrap();
    ///
    /// assert_eq!(record.cpu_usage, Some(12.5));
    /// ```
    pub fn cpu_usage(mut self, usage: f64) -> Self {
        self.cpu_usage = Some(usage);
        self
    }

    /// Set the memory usage (in bytes) on the builder and return the builder for chaining.
    ///
    /// The value is stored as bytes in the resulting `ProcessRecord`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let record = ProcessRecord::builder()
    ///     .pid_raw(1234)
    ///     .name("example")
    ///     .memory_usage(10_485_760) // 10 MiB
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.memory_usage, Some(10_485_760));
    /// ```
    pub fn memory_usage(mut self, usage: u64) -> Self {
        self.memory_usage = Some(usage);
        self
    }

    /// Set the process status.
    pub fn status(mut self, status: ProcessStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets the executable hash to include in the built ProcessRecord.
    ///
    /// The provided value is stored as a String (commonly a hex-encoded digest such as SHA-256).
    /// Accepts any type convertible into `String` and returns the builder for fluent chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let rec = ProcessRecordBuilder::new()
    ///     .pid_raw(1234)
    ///     .name("svc".to_string())
    ///     .executable_hash("abc123")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(rec.executable_hash.as_deref(), Some("abc123"));
    /// ```
    pub fn executable_hash(mut self, hash: impl Into<String>) -> Self {
        self.executable_hash = Some(hash.into());
        self
    }

    /// Sets the hash algorithm name to record for the process executable and returns the builder for chaining.
    ///
    /// The provided algorithm (for example `"sha256"`) will be stored in the resulting `ProcessRecord`'s
    /// `hash_algorithm` field.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let builder = ProcessRecordBuilder::new().hash_algorithm("sha256");
    /// let record = builder.pid_raw(1).name("proc").build().unwrap();
    /// assert_eq!(record.hash_algorithm.as_deref(), Some("sha256"));
    /// ```
    pub fn hash_algorithm(mut self, algorithm: impl Into<String>) -> Self {
        self.hash_algorithm = Some(algorithm.into());
        self
    }

    /// Set the collection timestamp to use for the resulting `ProcessRecord` and return the builder for chaining.
    ///
    /// If not provided, `build()` will default the collection time to `Utc::now()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::Utc;
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let builder = ProcessRecord::builder()
    ///     .pid_raw(1)
    ///     .name("svc")
    ///     .collection_time(Utc::now());
    /// // `builder` can be chained further or passed to `build()`
    /// ```
    pub fn collection_time(mut self, time: DateTime<Utc>) -> Self {
        self.collection_time = Some(time);
        self
    }

    /// Set the effective user ID for the process record being built and return the builder for chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let builder = ProcessRecordBuilder::new().pid_raw(1000).name("proc").user_id(1001);
    /// let record = builder.build().unwrap();
    /// assert_eq!(record.user_id, Some(1001));
    /// ```
    pub fn user_id(mut self, user_id: u32) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set the process group ID on the builder and return the builder for chaining.
    ///
    /// This assigns the optional `group_id` field used when building a `ProcessRecord`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let rec = ProcessRecord::builder()
    ///     .pid_raw(1)
    ///     .name("proc")
    ///     .group_id(1000)
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(rec.group_id, Some(1000));
    /// ```
    pub fn group_id(mut self, group_id: u32) -> Self {
        self.group_id = Some(group_id);
        self
    }

    /// Adds or updates a single environment variable on the builder, returning the builder for chaining.
    ///
    /// If the key already exists, its value is overwritten.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let record = ProcessRecord::builder()
    ///     .pid_raw(1)
    ///     .name("test")
    ///     .env_var("PATH", "/usr/bin")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.environment_vars.get("PATH").map(String::as_str), Some("/usr/bin"));
    /// ```
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment_vars.insert(key.into(), value.into());
        self
    }

    /// Merge multiple environment variables into the builder's environment map, overriding any existing keys.
    ///
    /// The provided map's entries are inserted into the builder's `environment_vars`; if a key already exists it will be replaced by the new value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use sentinel_lib::models::process::ProcessRecord;
    ///
    /// let mut vars = HashMap::new();
    /// vars.insert("PATH".to_string(), "/usr/bin".to_string());
    /// vars.insert("RUST_LOG".to_string(), "debug".to_string());
    ///
    /// let builder = ProcessRecord::builder()
    ///     .pid_raw(123)
    ///     .name("example")
    ///     .env_vars(vars);
    ///
    /// // building will include the merged environment variables
    /// let record = builder.build().unwrap();
    /// assert_eq!(record.environment_vars.get("RUST_LOG").map(String::as_str), Some("debug"));
    /// ```
    pub fn env_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.environment_vars.extend(vars);
        self
    }

    /// Adds or updates a metadata key/value pair on the builder.
    ///
    /// If the key already exists, its value is replaced. Returns the builder to allow method chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let builder = ProcessRecord::builder().metadata("role", "worker").metadata("env", "prod");
    /// let record = builder.pid_raw(1).name("svc").build().unwrap();
    /// assert_eq!(record.metadata.get("role").map(String::as_str), Some("worker"));
    /// assert_eq!(record.metadata.get("env").map(String::as_str), Some("prod"));
    /// ```
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Extends the builder's metadata map with the given entries and returns the builder.
    ///
    /// Incoming entries are inserted into the builder's metadata; values for keys that already
    /// exist in the builder are overwritten by the new entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    ///
    /// let mut m = HashMap::new();
    /// m.insert("k".to_string(), "v".to_string());
    ///
    /// let builder = ProcessRecordBuilder::new()
    ///     .metadata_map(m)
    ///     .name("proc".to_string())
    ///     .pid_raw(1);
    /// ```
    pub fn metadata_map(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata.extend(metadata);
        self
    }

    /// Builds a `ProcessRecord` from the builder.
    ///
    /// The builder must have both `pid` and `name` set; if either is missing this
    /// function returns `Err(ProcessError::MissingField("<field>"))`.
    ///
    /// Fields not provided are filled with sensible defaults:
    /// - `status` defaults to `ProcessStatus::Unknown("unknown")`
    /// - `collection_time` defaults to `Utc::now()`
    ///
    /// # Returns
    ///
    /// `Ok(ProcessRecord)` on success, or `Err(ProcessError::MissingField(...))` if a
    /// required field is missing.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::ProcessRecordBuilder;
    /// let record = ProcessRecordBuilder::new()
    ///     .pid_raw(1234)
    ///     .name("example")
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(record.pid.raw(), 1234);
    /// assert_eq!(record.name, "example");
    /// ```
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
    /// Returns the default SystemInfo (equivalent to `SystemInfo::new()`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::SystemInfo;
    /// let info = SystemInfo::default();
    /// assert_eq!(info.os_name, "unknown");
    /// assert_eq!(info.capabilities.len(), 0);
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl SystemInfo {
    /// Creates a new SystemInfo initialized with unknown/zero defaults.
    ///
    /// The returned instance uses "unknown" for string fields, zeros for numeric fields,
    /// and an empty capabilities list. Use the builder-like `with_capability` to add capabilities.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::process::SystemInfo;
    /// let info = SystemInfo::new();
    /// assert_eq!(info.os_name, "unknown");
    /// assert_eq!(info.total_memory, 0);
    /// assert!(info.capabilities.is_empty());
    /// ```
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

    /// Appends a capability to the SystemInfo and returns the modified value for chaining.
    ///
    /// This consumes `self`, adds the provided capability string to the `capabilities` vector,
    /// and returns the updated `SystemInfo`.
    ///
    /// # Examples
    ///
    /// ```
    /// let info = sentinel_lib::models::process::SystemInfo::new()
    ///     .with_capability("net-monitor");
    /// assert!(info.capabilities.contains(&"net-monitor".to_string()));
    /// ```
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
