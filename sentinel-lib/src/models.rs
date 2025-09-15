//! Core data models for SentinelD.
//!
//! This module defines the primary data structures used throughout the system:
//! - ProcessRecord: Comprehensive process metadata with builder pattern
//! - Alert: Structured alert information with severity levels
//! - DetectionRule: SQL-based detection rules with metadata
//! - SystemInfo: System-level information and capabilities
//! - Strongly-typed IDs for better type safety and preventing ID confusion

use chrono::{DateTime, Utc};
// use redb::{RedbKey, RedbValue}; // TODO: Re-enable when we fix redb integration
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

/// Process collection and validation errors.
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("Process {pid} not found")]
    ProcessNotFound { pid: u32 },

    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },

    #[error("Invalid process data: {message}")]
    InvalidData { message: String },

    #[error("Process enumeration failed: {0}")]
    EnumerationError(String),
}

/// Alert generation and management errors.
#[derive(Debug, Error)]
pub enum AlertError {
    #[error("Invalid alert severity: {severity}")]
    InvalidSeverity { severity: String },

    #[error("Alert deduplication failed: {0}")]
    DeduplicationError(String),

    #[error("Alert delivery failed: {0}")]
    DeliveryError(String),
}

/// Detection rule validation and execution errors.
#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("Invalid SQL query: {0}")]
    InvalidSql(String),

    #[error("Rule execution timeout")]
    Timeout,

    #[error("Rule execution failed: {0}")]
    ExecutionError(String),

    #[error("Rule validation failed: {0}")]
    ValidationError(String),
}

/// Strongly-typed process ID to prevent confusion with other numeric IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct ProcessId(pub u32);

/// Strongly-typed rule ID to prevent confusion with other string IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId(pub String);

/// Strongly-typed alert ID to prevent confusion with other string IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertId(pub String);

/// Strongly-typed scan ID for tracking collection operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanId(pub String);

impl ProcessId {
    /// Create a new process ID.
    pub fn new(pid: u32) -> Self {
        Self(pid)
    }

    /// Get the raw process ID value.
    pub fn value(&self) -> u32 {
        self.0
    }
}

impl RuleId {
    /// Create a new rule ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the raw rule ID value.
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl AlertId {
    /// Create a new alert ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the raw alert ID value.
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl ScanId {
    /// Create a new scan ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the raw scan ID value.
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for AlertId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for ScanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Implement From traits for string literals
impl From<&str> for AlertId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<&str> for RuleId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<&str> for ScanId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for AlertId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<String> for RuleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<String> for ScanId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Process status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Waiting,
    Zombie,
    Stopped,
    Traced,
    Unknown(String),
}

/// Alert severity levels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Low => write!(f, "low"),
            AlertSeverity::Medium => write!(f, "medium"),
            AlertSeverity::High => write!(f, "high"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for AlertSeverity {
    type Err = AlertError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(AlertSeverity::Low),
            "medium" => Ok(AlertSeverity::Medium),
            "high" => Ok(AlertSeverity::High),
            "critical" => Ok(AlertSeverity::Critical),
            _ => Err(AlertError::InvalidSeverity {
                severity: s.to_string(),
            }),
        }
    }
}

/// Comprehensive process metadata record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRecord {
    /// Process ID
    pub pid: ProcessId,
    /// Parent Process ID
    pub ppid: Option<ProcessId>,
    /// Process name
    pub name: String,
    /// Full executable path
    pub executable_path: Option<String>,
    /// Command line arguments
    pub command_line: Option<String>,
    /// Process start time
    pub start_time: Option<DateTime<Utc>>,
    /// CPU usage percentage
    pub cpu_usage: Option<f64>,
    /// Memory usage in bytes
    pub memory_usage: Option<u64>,
    /// Process status
    pub status: ProcessStatus,
    /// Executable file hash (SHA-256)
    pub executable_hash: Option<String>,
    /// Hash algorithm used
    pub hash_algorithm: Option<String>,
    /// Collection timestamp
    pub collection_time: DateTime<Utc>,
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
            metadata: HashMap::new(),
        }
    }

    /// Create a new process record builder.
    pub fn builder() -> ProcessRecordBuilder {
        ProcessRecordBuilder::new()
    }

    /// Get a deduplication key for this process record.
    pub fn dedup_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.pid,
            self.name,
            self.executable_path.as_deref().unwrap_or("")
        )
    }

    /// Check if this process has enhanced metadata.
    pub fn has_enhanced_metadata(&self) -> bool {
        self.cpu_usage.is_some() || self.memory_usage.is_some() || self.start_time.is_some()
    }
}

/// Builder for creating ProcessRecord instances with a fluent API.
#[derive(Debug, Default)]
pub struct ProcessRecordBuilder {
    pid: Option<ProcessId>,
    ppid: Option<ProcessId>,
    name: Option<String>,
    executable_path: Option<String>,
    command_line: Option<String>,
    start_time: Option<DateTime<Utc>>,
    cpu_usage: Option<f64>,
    memory_usage: Option<u64>,
    status: Option<ProcessStatus>,
    executable_hash: Option<String>,
    hash_algorithm: Option<String>,
    collection_time: Option<DateTime<Utc>>,
    metadata: HashMap<String, String>,
}

impl ProcessRecordBuilder {
    /// Create a new process record builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the process ID.
    pub fn pid(mut self, pid: ProcessId) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the process ID from a raw u32.
    pub fn pid_raw(mut self, pid: u32) -> Self {
        self.pid = Some(ProcessId::new(pid));
        self
    }

    /// Set the parent process ID.
    pub fn ppid(mut self, ppid: ProcessId) -> Self {
        self.ppid = Some(ppid);
        self
    }

    /// Set the parent process ID from a raw u32.
    pub fn ppid_raw(mut self, ppid: u32) -> Self {
        self.ppid = Some(ProcessId::new(ppid));
        self
    }

    /// Set the process name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the executable path.
    pub fn executable_path(mut self, path: impl Into<String>) -> Self {
        self.executable_path = Some(path.into());
        self
    }

    /// Set the command line.
    pub fn command_line(mut self, cmd: impl Into<String>) -> Self {
        self.command_line = Some(cmd.into());
        self
    }

    /// Set the start time.
    pub fn start_time(mut self, time: DateTime<Utc>) -> Self {
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

    /// Add metadata.
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
        let pid = self.pid.ok_or_else(|| ProcessError::InvalidData {
            message: "Process ID is required".to_string(),
        })?;

        let name = self.name.ok_or_else(|| ProcessError::InvalidData {
            message: "Process name is required".to_string(),
        })?;

        Ok(ProcessRecord {
            pid,
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
            metadata: self.metadata,
        })
    }
}

/// Structured alert information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    /// Unique alert identifier
    pub id: AlertId,
    /// Alert severity level
    pub severity: AlertSeverity,
    /// Alert title/summary
    pub title: String,
    /// Detailed alert description
    pub description: String,
    /// Alert category/type
    pub category: String,
    /// Source detection rule ID
    pub rule_id: Option<RuleId>,
    /// Affected process information
    pub affected_process: Option<ProcessRecord>,
    /// Additional context data
    pub context: HashMap<String, String>,
    /// Alert creation timestamp
    pub created_at: DateTime<Utc>,
    /// Alert deduplication key
    pub dedup_key: String,
    /// Alert tags for filtering
    pub tags: Vec<String>,
}

impl Alert {
    /// Create a new alert with required fields.
    pub fn new(
        id: impl Into<AlertId>,
        severity: AlertSeverity,
        title: impl Into<String>,
        description: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        let id = id.into();
        let title = title.into();
        let description = description.into();
        let category = category.into();
        let dedup_key = format!("{}:{}:{}", severity, category, title);
        Self {
            id,
            severity,
            title,
            description,
            category,
            rule_id: None,
            affected_process: None,
            context: HashMap::new(),
            created_at: Utc::now(),
            dedup_key,
            tags: Vec::new(),
        }
    }

    /// Add context data to the alert.
    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }

    /// Add tags to the alert.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags.extend(tags);
        self
    }

    /// Set the affected process for this alert.
    pub fn with_process(mut self, process: ProcessRecord) -> Self {
        self.affected_process = Some(process);
        self
    }
}

/// Detection rule definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectionRule {
    /// Unique rule identifier
    pub id: RuleId,
    /// Rule name/title
    pub name: String,
    /// Rule description
    pub description: String,
    /// SQL query for detection
    pub sql_query: String,
    /// Rule category
    pub category: String,
    /// Rule severity
    pub severity: AlertSeverity,
    /// Rule version
    pub version: String,
    /// Rule author
    pub author: String,
    /// Rule creation timestamp
    pub created_at: DateTime<Utc>,
    /// Rule last modified timestamp
    pub modified_at: DateTime<Utc>,
    /// Rule enabled status
    pub enabled: bool,
    /// Rule tags
    pub tags: Vec<String>,
    /// Rule metadata
    pub metadata: HashMap<String, String>,
}

impl DetectionRule {
    /// Create a new detection rule.
    pub fn new(
        id: impl Into<RuleId>,
        name: impl Into<String>,
        description: impl Into<String>,
        sql_query: impl Into<String>,
        category: impl Into<String>,
        severity: AlertSeverity,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            sql_query: sql_query.into(),
            category: category.into(),
            severity,
            version: "1.0.0".to_string(),
            author: "system".to_string(),
            created_at: now,
            modified_at: now,
            enabled: true,
            tags: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Validate the SQL query syntax.
    pub fn validate_sql(&self) -> Result<(), DetectionError> {
        // Basic SQL validation - in a real implementation, we'd use sqlparser
        if self.sql_query.trim().is_empty() {
            return Err(DetectionError::InvalidSql("Empty SQL query".to_string()));
        }

        if !self.sql_query.trim().to_lowercase().starts_with("select") {
            return Err(DetectionError::InvalidSql(
                "Only SELECT queries are allowed".to_string(),
            ));
        }

        // Check for dangerous SQL patterns
        let dangerous_patterns = ["drop", "delete", "insert", "update", "alter", "create"];
        let query_lower = self.sql_query.to_lowercase();
        for pattern in &dangerous_patterns {
            if query_lower.contains(pattern) {
                return Err(DetectionError::InvalidSql(format!(
                    "Dangerous SQL pattern detected: {}",
                    pattern
                )));
            }
        }

        Ok(())
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
    /// Total memory in bytes
    pub total_memory: u64,
    /// Number of CPU cores
    pub cpu_cores: usize,
    /// System uptime in seconds
    pub uptime_seconds: u64,
    /// Available capabilities
    pub capabilities: Vec<String>,
    /// Collection timestamp
    pub collected_at: DateTime<Utc>,
}

impl SystemInfo {
    /// Create a new system info record.
    pub fn new(
        os_name: String,
        os_version: String,
        architecture: String,
        total_memory: u64,
        cpu_cores: usize,
        uptime_seconds: u64,
    ) -> Self {
        Self {
            os_name,
            os_version,
            architecture,
            total_memory,
            cpu_cores,
            uptime_seconds,
            capabilities: Vec::new(),
            collected_at: Utc::now(),
        }
    }

    /// Add a capability to the system info.
    pub fn with_capability(mut self, capability: String) -> Self {
        self.capabilities.push(capability);
        self
    }
}

/// Process collection result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionResult {
    /// Collected process records
    pub processes: Vec<ProcessRecord>,
    /// System information
    pub system_info: SystemInfo,
    /// Collection metadata
    pub metadata: CollectionMetadata,
}

/// Collection operation metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionMetadata {
    /// Total processes found
    pub total_processes: usize,
    /// Successfully collected processes
    pub collected_processes: usize,
    /// Failed process collections
    pub failed_processes: usize,
    /// Collection duration in milliseconds
    pub duration_ms: u64,
    /// Collection timestamp
    pub collected_at: DateTime<Utc>,
}

impl CollectionMetadata {
    /// Create new collection metadata.
    pub fn new(
        total_processes: usize,
        collected_processes: usize,
        failed_processes: usize,
        duration_ms: u64,
    ) -> Self {
        Self {
            total_processes,
            collected_processes,
            failed_processes,
            duration_ms,
            collected_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_record_creation() {
        let process = ProcessRecord::new(1234, "test-process".to_string());
        assert_eq!(process.pid, ProcessId::new(1234));
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
            .cpu_usage(50.0)
            .memory_usage(1024)
            .status(ProcessStatus::Running)
            .build()
            .unwrap();

        assert_eq!(process.pid, ProcessId::new(1234));
        assert_eq!(process.name, "test-process");
        assert_eq!(process.cpu_usage, Some(50.0));
        assert_eq!(process.memory_usage, Some(1024));
        assert_eq!(process.status, ProcessStatus::Running);
    }

    #[test]
    fn test_alert_creation() {
        let alert = Alert::new(
            "alert-1",
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test",
        );
        assert_eq!(alert.id, AlertId::new("alert-1"));
        assert_eq!(alert.severity, AlertSeverity::High);
        assert_eq!(alert.title, "Test Alert");
    }

    #[test]
    fn test_detection_rule_validation() {
        let rule = DetectionRule::new(
            "rule-1",
            "Test Rule",
            "Test detection rule",
            "SELECT * FROM processes WHERE name = 'test'",
            "test",
            AlertSeverity::Medium,
        );
        assert!(rule.validate_sql().is_ok());
    }

    #[test]
    fn test_detection_rule_validation_invalid() {
        let rule = DetectionRule::new(
            "rule-1",
            "Test Rule",
            "Test detection rule",
            "DROP TABLE processes",
            "test",
            AlertSeverity::Medium,
        );
        assert!(rule.validate_sql().is_err());
    }

    #[test]
    fn test_alert_severity_parsing() {
        assert_eq!(
            "high".parse::<AlertSeverity>().unwrap(),
            AlertSeverity::High
        );
        assert!("invalid".parse::<AlertSeverity>().is_err());
    }
}

// TODO: Implement redb Value trait implementations in Task 8
// For now, just focus on getting the basic structure compiling for Task 1
