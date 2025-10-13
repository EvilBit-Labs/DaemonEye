//! Collection event types and data structures.
//!
//! This module defines the unified event model that supports multiple collection
//! domains while maintaining type safety and extensibility.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Unified collection event enum supporting multiple monitoring domains.
///
/// This enum provides a type-safe way to handle events from different collection
/// sources while maintaining a unified processing pipeline. Each variant contains
/// domain-specific event data.
///
/// # Design Principles
///
/// - **Extensible**: New event types can be added without breaking existing code
/// - **Type Safe**: Each domain has its own strongly-typed event structure
/// - **Serializable**: All events can be serialized for storage and transmission
/// - **Timestamped**: All events include collection timestamps for correlation
///
/// # Examples
///
/// ```rust
/// use collector_core::{CollectionEvent, ProcessEvent};
/// use std::time::SystemTime;
///
/// let event = CollectionEvent::Process(ProcessEvent {
///     pid: 1234,
///     ppid: None,
///     name: "example".to_string(),
///     executable_path: None,
///     command_line: vec![],
///     start_time: None,
///     cpu_usage: None,
///     memory_usage: None,
///     executable_hash: None,
///     user_id: None,
///     accessible: true,
///     file_exists: true,
///     timestamp: SystemTime::now(),
///     platform_metadata: None,
/// });
///
/// match event {
///     CollectionEvent::Process(proc_event) => {
///         println!("Process event: PID {}", proc_event.pid);
///     }
///     _ => {}
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CollectionEvent {
    /// Process monitoring events
    Process(ProcessEvent),

    /// Network monitoring events (future extension)
    Network(NetworkEvent),

    /// Filesystem monitoring events (future extension)
    Filesystem(FilesystemEvent),

    /// Performance monitoring events (future extension)
    Performance(PerformanceEvent),

    /// Analysis collector trigger requests for coordinated analysis
    TriggerRequest(TriggerRequest),
}

/// Process monitoring event data.
///
/// Contains information about process lifecycle, metadata, and security-relevant
/// attributes collected during process enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessEvent {
    /// Process identifier
    pub pid: u32,

    /// Parent process identifier
    pub ppid: Option<u32>,

    /// Process name
    pub name: String,

    /// Full executable path
    pub executable_path: Option<String>,

    /// Command line arguments
    pub command_line: Vec<String>,

    /// Process start time
    pub start_time: Option<SystemTime>,

    /// CPU usage percentage
    pub cpu_usage: Option<f64>,

    /// Memory usage in bytes
    pub memory_usage: Option<u64>,

    /// SHA-256 hash of executable
    pub executable_hash: Option<String>,

    /// User ID running the process
    pub user_id: Option<String>,

    /// Whether process metadata was accessible
    pub accessible: bool,

    /// Whether executable file exists
    pub file_exists: bool,

    /// Event collection timestamp
    pub timestamp: SystemTime,

    /// Platform-specific metadata (Windows, macOS, Linux)
    pub platform_metadata: Option<serde_json::Value>,
}

/// Network monitoring event data (future extension).
///
/// Will contain information about network connections, traffic patterns,
/// and security-relevant network activity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkEvent {
    /// Connection identifier
    pub connection_id: String,

    /// Source address and port
    pub source_addr: String,

    /// Destination address and port
    pub dest_addr: String,

    /// Protocol (TCP, UDP, etc.)
    pub protocol: String,

    /// Connection state
    pub state: String,

    /// Associated process ID
    pub pid: Option<u32>,

    /// Bytes transferred
    pub bytes_sent: u64,
    pub bytes_received: u64,

    /// Event collection timestamp
    pub timestamp: SystemTime,
}

/// Filesystem monitoring event data (future extension).
///
/// Will contain information about file operations, access patterns,
/// and security-relevant filesystem activity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilesystemEvent {
    /// File path
    pub path: String,

    /// Operation type (create, modify, delete, access)
    pub operation: String,

    /// Associated process ID
    pub pid: Option<u32>,

    /// File size
    pub size: Option<u64>,

    /// File permissions
    pub permissions: Option<String>,

    /// File hash (for integrity monitoring)
    pub file_hash: Option<String>,

    /// Event collection timestamp
    pub timestamp: SystemTime,
}

/// Performance monitoring event data (future extension).
///
/// Will contain system resource utilization, performance metrics,
/// and anomaly detection data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerformanceEvent {
    /// Metric name
    pub metric_name: String,

    /// Metric value
    pub value: f64,

    /// Metric unit
    pub unit: String,

    /// Associated process ID (if applicable)
    pub pid: Option<u32>,

    /// System component (CPU, memory, disk, network)
    pub component: String,

    /// Event collection timestamp
    pub timestamp: SystemTime,
}

/// Analysis collector trigger request for coordinated analysis.
///
/// This event type enables process monitoring collectors to trigger analysis
/// collectors (like Binary Hasher, Memory Analyzer, etc.) when suspicious
/// behavior is detected. The trigger system provides coordination between
/// different collection components while maintaining loose coupling.
///
/// # Design Principles
///
/// - **Deduplication**: Prevents redundant analysis requests for the same target
/// - **Rate Limiting**: Protects analysis collectors from overload
/// - **Priority-based**: Enables urgent analysis for critical threats
/// - **Correlation**: Maintains metadata for debugging and forensic analysis
///
/// # Examples
///
/// ```rust
/// use collector_core::{TriggerRequest, AnalysisType, TriggerPriority};
/// use std::time::SystemTime;
/// use std::collections::HashMap;
///
/// let trigger = TriggerRequest {
///     trigger_id: "trigger_123".to_string(),
///     target_collector: "binary-hasher".to_string(),
///     analysis_type: AnalysisType::BinaryHash,
///     priority: TriggerPriority::High,
///     target_pid: Some(1234),
///     target_path: Some("/usr/bin/suspicious".to_string()),
///     correlation_id: "corr_456".to_string(),
///     metadata: HashMap::new(),
///     timestamp: SystemTime::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerRequest {
    /// Unique identifier for this trigger request
    pub trigger_id: String,

    /// Target analysis collector name
    pub target_collector: String,

    /// Type of analysis to perform
    pub analysis_type: AnalysisType,

    /// Priority level for this analysis request
    pub priority: TriggerPriority,

    /// Process ID to analyze (if applicable)
    pub target_pid: Option<u32>,

    /// File path to analyze (if applicable)
    pub target_path: Option<String>,

    /// Correlation ID for tracking related events
    pub correlation_id: String,

    /// Additional metadata for the analysis request
    pub metadata: HashMap<String, String>,

    /// Trigger generation timestamp
    pub timestamp: SystemTime,
}

/// Types of analysis that can be triggered.
///
/// This enum defines the different types of analysis that can be requested
/// from analysis collectors. Each type corresponds to a specific collector
/// capability and analysis methodology.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AnalysisType {
    /// Binary hash computation and integrity verification
    BinaryHash,

    /// Memory analysis and process inspection
    MemoryAnalysis,

    /// YARA rule scanning
    YaraScan,

    /// Network traffic analysis
    NetworkAnalysis,

    /// Behavioral analysis and anomaly detection
    BehavioralAnalysis,

    /// Custom analysis type (extensible)
    Custom(String),
}

/// Priority levels for trigger requests.
///
/// Priority determines the urgency of analysis and affects queue ordering
/// and resource allocation in analysis collectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TriggerPriority {
    /// Low priority - routine analysis
    Low,

    /// Normal priority - standard analysis
    Normal,

    /// High priority - suspicious activity detected
    High,

    /// Critical priority - immediate threat response required
    Critical,
}

impl CollectionEvent {
    /// Returns the timestamp of the event regardless of type.
    pub fn timestamp(&self) -> SystemTime {
        match self {
            CollectionEvent::Process(event) => event.timestamp,
            CollectionEvent::Network(event) => event.timestamp,
            CollectionEvent::Filesystem(event) => event.timestamp,
            CollectionEvent::Performance(event) => event.timestamp,
            CollectionEvent::TriggerRequest(event) => event.timestamp,
        }
    }

    /// Returns the event type as a string for logging and metrics.
    pub fn event_type(&self) -> &'static str {
        match self {
            CollectionEvent::Process(_) => "process",
            CollectionEvent::Network(_) => "network",
            CollectionEvent::Filesystem(_) => "filesystem",
            CollectionEvent::Performance(_) => "performance",
            CollectionEvent::TriggerRequest(_) => "trigger_request",
        }
    }

    /// Returns the associated process ID if available.
    pub fn pid(&self) -> Option<u32> {
        match self {
            CollectionEvent::Process(event) => Some(event.pid),
            CollectionEvent::Network(event) => event.pid,
            CollectionEvent::Filesystem(event) => event.pid,
            CollectionEvent::Performance(event) => event.pid,
            CollectionEvent::TriggerRequest(event) => event.target_pid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::SystemTime;

    #[test]
    fn test_process_event_creation() {
        let timestamp = SystemTime::now();
        let event = ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test".to_string(), "--flag".to_string()],
            start_time: Some(timestamp),
            cpu_usage: Some(5.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };

        assert_eq!(event.pid, 1234);
        assert_eq!(event.ppid, Some(1));
        assert_eq!(event.name, "test_process");
        assert!(event.accessible);
        assert!(event.file_exists);
    }

    #[test]
    fn test_collection_event_timestamp() {
        let timestamp = SystemTime::now();
        let process_event = ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test".to_string(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };

        let collection_event = CollectionEvent::Process(process_event);
        assert_eq!(collection_event.timestamp(), timestamp);
        assert_eq!(collection_event.event_type(), "process");
        assert_eq!(collection_event.pid(), Some(1234));
    }

    #[test]
    fn test_network_event_creation() {
        let timestamp = SystemTime::now();
        let event = NetworkEvent {
            connection_id: "conn_123".to_string(),
            source_addr: "192.168.1.100:12345".to_string(),
            dest_addr: "10.0.0.1:80".to_string(),
            protocol: "TCP".to_string(),
            state: "ESTABLISHED".to_string(),
            pid: Some(1234),
            bytes_sent: 1024,
            bytes_received: 2048,
            timestamp,
        };

        let collection_event = CollectionEvent::Network(event);
        assert_eq!(collection_event.event_type(), "network");
        assert_eq!(collection_event.pid(), Some(1234));
    }

    #[test]
    fn test_trigger_request_creation() {
        let timestamp = SystemTime::now();
        let mut metadata = HashMap::new();
        metadata.insert("test_key".to_string(), "test_value".to_string());

        let trigger = TriggerRequest {
            trigger_id: "trigger_123".to_string(),
            target_collector: "binary-hasher".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/suspicious".to_string()),
            correlation_id: "corr_456".to_string(),
            metadata,
            timestamp,
        };

        let collection_event = CollectionEvent::TriggerRequest(trigger);
        assert_eq!(collection_event.event_type(), "trigger_request");
        assert_eq!(collection_event.pid(), Some(1234));
        assert_eq!(collection_event.timestamp(), timestamp);
    }

    #[test]
    fn test_analysis_type_equality() {
        assert_eq!(AnalysisType::BinaryHash, AnalysisType::BinaryHash);
        assert_ne!(AnalysisType::BinaryHash, AnalysisType::MemoryAnalysis);

        let custom1 = AnalysisType::Custom("test".to_string());
        let custom2 = AnalysisType::Custom("test".to_string());
        let custom3 = AnalysisType::Custom("other".to_string());

        assert_eq!(custom1, custom2);
        assert_ne!(custom1, custom3);
    }

    #[test]
    fn test_trigger_priority_ordering() {
        assert!(TriggerPriority::Critical > TriggerPriority::High);
        assert!(TriggerPriority::High > TriggerPriority::Normal);
        assert!(TriggerPriority::Normal > TriggerPriority::Low);

        let mut priorities = vec![
            TriggerPriority::Low,
            TriggerPriority::Critical,
            TriggerPriority::Normal,
            TriggerPriority::High,
        ];

        priorities.sort();

        assert_eq!(
            priorities,
            vec![
                TriggerPriority::Low,
                TriggerPriority::Normal,
                TriggerPriority::High,
                TriggerPriority::Critical,
            ]
        );
    }

    #[test]
    fn test_event_serialization() {
        let timestamp = SystemTime::now();
        let process_event = ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "test".to_string(),
            executable_path: Some("/bin/test".to_string()),
            command_line: vec!["test".to_string()],
            start_time: Some(timestamp),
            cpu_usage: Some(1.5),
            memory_usage: Some(4096),
            executable_hash: Some("hash123".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };

        let collection_event = CollectionEvent::Process(process_event);

        // Test serialization to JSON
        let json = serde_json::to_string(&collection_event).expect("Failed to serialize");
        assert!(json.contains("\"pid\":1234"));
        assert!(json.contains("\"name\":\"test\""));

        // Test deserialization from JSON
        let deserialized: CollectionEvent =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.event_type(), "process");
        assert_eq!(deserialized.pid(), Some(1234));
    }
}
