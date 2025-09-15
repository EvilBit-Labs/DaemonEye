//! Core data models for SentinelD.
//!
//! This module defines the primary data structures used throughout the system:
//! - ProcessRecord: Comprehensive process metadata
//! - Alert: Structured alert information with severity levels
//! - DetectionRule: SQL-based detection rules with metadata
//! - SystemInfo: System-level information and capabilities

use chrono::{DateTime, Utc};
// use redb::{RedbKey, RedbValue}; // TODO: Re-enable when we fix redb integration
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub pid: u32,
    /// Parent Process ID
    pub ppid: Option<u32>,
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
            pid,
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

/// Structured alert information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    /// Unique alert identifier
    pub id: String,
    /// Alert severity level
    pub severity: AlertSeverity,
    /// Alert title/summary
    pub title: String,
    /// Detailed alert description
    pub description: String,
    /// Alert category/type
    pub category: String,
    /// Source detection rule ID
    pub rule_id: Option<String>,
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
        id: String,
        severity: AlertSeverity,
        title: String,
        description: String,
        category: String,
    ) -> Self {
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
    pub id: String,
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
        id: String,
        name: String,
        description: String,
        sql_query: String,
        category: String,
        severity: AlertSeverity,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            description,
            sql_query,
            category,
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
        assert_eq!(process.pid, 1234);
        assert_eq!(process.name, "test-process");
        assert_eq!(
            process.status,
            ProcessStatus::Unknown("unknown".to_string())
        );
    }

    #[test]
    fn test_alert_creation() {
        let alert = Alert::new(
            "alert-1".to_string(),
            AlertSeverity::High,
            "Test Alert".to_string(),
            "This is a test alert".to_string(),
            "test".to_string(),
        );
        assert_eq!(alert.severity, AlertSeverity::High);
        assert_eq!(alert.title, "Test Alert");
    }

    #[test]
    fn test_detection_rule_validation() {
        let rule = DetectionRule::new(
            "rule-1".to_string(),
            "Test Rule".to_string(),
            "Test detection rule".to_string(),
            "SELECT * FROM processes WHERE name = 'test'".to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );
        assert!(rule.validate_sql().is_ok());
    }

    #[test]
    fn test_detection_rule_validation_invalid() {
        let rule = DetectionRule::new(
            "rule-1".to_string(),
            "Test Rule".to_string(),
            "Test detection rule".to_string(),
            "DROP TABLE processes".to_string(),
            "test".to_string(),
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
