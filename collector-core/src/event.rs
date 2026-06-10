//! Collection event types and data structures.
//!
//! This module defines the unified event model that supports multiple collection
//! domains while maintaining type safety and extensibility.

#![allow(clippy::significant_drop_tightening)]
#![allow(clippy::pattern_type_mismatch)]

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
///     name: "example".to_owned(),
///     executable_path: None,
///     command_line: vec![],
///     start_time: None,
///     cpu_usage: None,
///     memory_usage: None,
///     executable_hash: None,
///     hash_algorithm: None,
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
#[non_exhaustive]
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

    /// Hex-encoded cryptographic hash of the executable file. Populated
    /// when the collector was constructed with
    /// `compute_executable_hashes = true` and the executable was readable.
    /// See [`Self::hash_algorithm`] for the algorithm that produced this
    /// value.
    ///
    /// `None` means one of:
    /// - Hashing was disabled at the collector level
    /// - The executable was inaccessible, deleted, oversized, or failed to
    ///   open for reading
    /// - Hash computation timed out or failed
    ///
    /// Consumers **must** compare `(executable_hash, hash_algorithm)` as a
    /// tuple when checking for lifecycle drift. Comparing only the hex
    /// string would silently alias if the canonical algorithm changes
    /// across procmond versions.
    pub executable_hash: Option<String>,

    /// Canonical lowercase name of the algorithm used for
    /// [`Self::executable_hash`] (e.g. `"sha256"`, `"blake3"`). Always
    /// `None` when `executable_hash` is `None`; always `Some` when it is
    /// populated.
    pub hash_algorithm: Option<String>,

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
///     trigger_id: "trigger_123".to_owned(),
///     target_collector: "binary-hasher".to_owned(),
///     analysis_type: AnalysisType::BinaryHash,
///     priority: TriggerPriority::High,
///     target_pid: Some(1234),
///     target_path: Some("/usr/bin/suspicious".to_owned()),
///     correlation_id: "corr_456".to_owned(),
///     metadata: HashMap::new(),
///     timestamp: SystemTime::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl TriggerRequest {
    /// Validates the trigger request and returns an error if invalid.
    ///
    /// # Validation Rules
    ///
    /// - `trigger_id` and `correlation_id` must be non-empty strings
    /// - `target_collector` must be non-empty
    /// - At least one of `target_pid` or `target_path` must be present
    /// - `metadata` must have at most 100 entries
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if valid, or an error message describing the validation failure.
    pub fn validate(&self) -> Result<(), String> {
        // Validate trigger_id
        if self.trigger_id.is_empty() {
            return Err("trigger_id must be non-empty".to_owned());
        }

        // Validate correlation_id
        if self.correlation_id.is_empty() {
            return Err("correlation_id must be non-empty".to_owned());
        }

        // Validate target_collector
        if self.target_collector.is_empty() {
            return Err("target_collector must be non-empty".to_owned());
        }

        // Validate at least one target is present
        if self.target_pid.is_none() && self.target_path.is_none() {
            return Err("at least one of target_pid or target_path must be present".to_owned());
        }

        // Validate metadata size
        if self.metadata.len() > 100 {
            return Err(format!(
                "metadata exceeds maximum size of 100 entries (found {})",
                self.metadata.len()
            ));
        }

        Ok(())
    }
}

/// Types of analysis that can be triggered.
///
/// This enum defines the different types of analysis that can be requested
/// from analysis collectors. Each type corresponds to a specific collector
/// capability and analysis methodology.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
#[non_exhaustive]
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

impl ProcessEvent {
    /// Validate the `(executable_hash, hash_algorithm)` paired
    /// invariant: both fields must be `Some` together or `None`
    /// together. A mismatch indicates wire-format drift or a buggy
    /// constructor and should be treated as data corruption by
    /// downstream consumers.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a human-readable message if the invariant
    /// is violated.
    pub const fn validate_hash_tuple(&self) -> Result<(), &'static str> {
        if self.executable_hash.is_some() != self.hash_algorithm.is_some() {
            return Err("ProcessEvent invariant violated: executable_hash and \
                 hash_algorithm must both be Some or both None");
        }
        Ok(())
    }

    /// Reserved [`Self::platform_metadata`] key for the ssdeep fuzzy hash.
    pub const META_SSDEEP_HASH: &'static str = "integrity.ssdeep_hash";
    /// Reserved [`Self::platform_metadata`] key for the on-disk-vs-running mismatch flag.
    pub const META_ON_DISK_MISMATCH: &'static str = "integrity.on_disk_mismatch";
    /// Reserved [`Self::platform_metadata`] key for the degraded-coverage flag.
    pub const META_SSDEEP_DEGRADED: &'static str = "integrity.ssdeep_degraded";

    /// Take the `platform_metadata` object map (or a fresh one), preserving any
    /// existing keys so the composable integrity setters never clobber unrelated
    /// metadata or each other.
    fn take_metadata_object(&mut self) -> serde_json::Map<String, serde_json::Value> {
        match self.platform_metadata.take() {
            Some(serde_json::Value::Object(existing)) => existing,
            _ => serde_json::Map::new(),
        }
    }

    /// Remove the given integrity keys from `platform_metadata` without creating
    /// a metadata object, and drop the metadata entirely if it becomes empty.
    ///
    /// Default integrity signals (no digest, no flags) must not force an
    /// otherwise-absent `platform_metadata` object to exist — on the hot 10k+
    /// process path that would be avoidable allocation per process. The getters
    /// already default to `None`/`false` when the keys are absent.
    fn clear_integrity_keys(&mut self, keys: &[&str]) {
        if let Some(serde_json::Value::Object(map)) = self.platform_metadata.as_mut() {
            for key in keys {
                map.remove(*key);
            }
            if map.is_empty() {
                self.platform_metadata = None;
            }
        }
    }

    /// Record the ssdeep fuzzy-hash signals (produced by procmond's hash pass).
    ///
    /// These signals ride [`Self::platform_metadata`] rather than dedicated
    /// struct fields so the widely-constructed `ProcessEvent` literal API stays
    /// stable. The *typed* wire contract is carried by the protobuf
    /// `ProcessRecord` fields; the IPC conversion is the only consumer.
    ///
    /// `ssdeep_hash` is independent of the `(executable_hash, hash_algorithm)`
    /// paired invariant: it may be `None` (with `ssdeep_degraded` set) while the
    /// SHA-256 identity hash is present. This setter leaves the on-disk-mismatch
    /// flag untouched (set independently by the collector). The all-default case
    /// (`None`, `false`) clears any stale keys without materializing metadata.
    pub fn set_ssdeep_signal(&mut self, ssdeep_hash: Option<String>, ssdeep_degraded: bool) {
        if ssdeep_hash.is_none() && !ssdeep_degraded {
            self.clear_integrity_keys(&[Self::META_SSDEEP_HASH, Self::META_SSDEEP_DEGRADED]);
            return;
        }
        let mut map = self.take_metadata_object();
        match ssdeep_hash {
            Some(hash) => {
                map.insert(
                    Self::META_SSDEEP_HASH.to_owned(),
                    serde_json::Value::String(hash),
                );
            }
            None => {
                map.remove(Self::META_SSDEEP_HASH);
            }
        }
        if ssdeep_degraded {
            map.insert(
                Self::META_SSDEEP_DEGRADED.to_owned(),
                serde_json::Value::Bool(true),
            );
        } else {
            map.remove(Self::META_SSDEEP_DEGRADED);
        }
        self.platform_metadata = Some(serde_json::Value::Object(map));
    }

    /// Record the on-disk-vs-running mismatch flag (produced by the collector).
    ///
    /// Leaves the ssdeep signals untouched so producers compose independently.
    /// The default (`false`) case clears the key without materializing metadata,
    /// so the common no-mismatch path (every Linux process) allocates nothing.
    pub fn set_on_disk_mismatch(&mut self, on_disk_mismatch: bool) {
        if !on_disk_mismatch {
            self.clear_integrity_keys(&[Self::META_ON_DISK_MISMATCH]);
            return;
        }
        let mut map = self.take_metadata_object();
        map.insert(
            Self::META_ON_DISK_MISMATCH.to_owned(),
            serde_json::Value::Bool(true),
        );
        self.platform_metadata = Some(serde_json::Value::Object(map));
    }

    /// The ssdeep fuzzy hash recorded in [`Self::platform_metadata`], if any.
    #[must_use]
    pub fn ssdeep_hash(&self) -> Option<&str> {
        self.platform_metadata
            .as_ref()?
            .get(Self::META_SSDEEP_HASH)?
            .as_str()
    }

    /// Whether the on-disk-vs-running mismatch flag is set.
    #[must_use]
    pub fn on_disk_mismatch(&self) -> bool {
        self.platform_metadata
            .as_ref()
            .and_then(|meta| meta.get(Self::META_ON_DISK_MISMATCH))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    /// Whether the degraded-integrity-coverage flag is set (ssdeep failed while
    /// the SHA-256 identity hash succeeded).
    #[must_use]
    pub fn ssdeep_degraded(&self) -> bool {
        self.platform_metadata
            .as_ref()
            .and_then(|meta| meta.get(Self::META_SSDEEP_DEGRADED))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

impl CollectionEvent {
    /// Returns the timestamp of the event regardless of type.
    pub const fn timestamp(&self) -> SystemTime {
        match self {
            Self::Process(event) => event.timestamp,
            Self::Network(event) => event.timestamp,
            Self::Filesystem(event) => event.timestamp,
            Self::Performance(event) => event.timestamp,
            Self::TriggerRequest(event) => event.timestamp,
        }
    }

    /// Returns the event type as a string for logging and metrics.
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::Process(_) => "process",
            Self::Network(_) => "network",
            Self::Filesystem(_) => "filesystem",
            Self::Performance(_) => "performance",
            Self::TriggerRequest(_) => "trigger_request",
        }
    }

    /// Returns the associated process ID if available.
    pub const fn pid(&self) -> Option<u32> {
        match self {
            Self::Process(event) => Some(event.pid),
            Self::Network(event) => event.pid,
            Self::Filesystem(event) => event.pid,
            Self::Performance(event) => event.pid,
            Self::TriggerRequest(event) => event.target_pid,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]
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
            name: "test_process".to_owned(),
            executable_path: Some("/usr/bin/test".to_owned()),
            command_line: vec!["test".to_owned(), "--flag".to_owned()],
            start_time: Some(timestamp),
            cpu_usage: Some(5.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: Some("1000".to_owned()),
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

    fn sample_event() -> ProcessEvent {
        let timestamp = SystemTime::now();
        ProcessEvent {
            pid: 1,
            ppid: None,
            name: "p".to_owned(),
            executable_path: Some("/bin/p".to_owned()),
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: Some("abc".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        }
    }

    #[test]
    fn integrity_signals_default_to_absent() {
        let event = sample_event();
        assert_eq!(event.ssdeep_hash(), None);
        assert!(!event.on_disk_mismatch());
        assert!(!event.ssdeep_degraded());
    }

    #[test]
    fn default_signals_do_not_materialize_metadata() {
        // The common no-signal path (every clean process) must not allocate a
        // platform_metadata object just to store default false/None values.
        let mut event = sample_event();
        assert!(event.platform_metadata.is_none());
        event.set_on_disk_mismatch(false);
        event.set_ssdeep_signal(None, false);
        assert!(
            event.platform_metadata.is_none(),
            "default integrity signals must not create platform_metadata"
        );
        assert!(!event.on_disk_mismatch());
        assert!(!event.ssdeep_degraded());
        assert_eq!(event.ssdeep_hash(), None);
    }

    #[test]
    fn clearing_signals_drops_emptied_metadata() {
        let mut event = sample_event();
        event.set_ssdeep_signal(Some("3:x:y".to_owned()), true);
        assert!(event.platform_metadata.is_some());
        // Resetting to defaults removes the keys and drops the now-empty object.
        event.set_ssdeep_signal(None, false);
        assert!(event.platform_metadata.is_none());
    }

    #[test]
    fn ssdeep_signal_round_trip() {
        let mut event = sample_event();
        event.set_ssdeep_signal(Some("3:abc:def".to_owned()), false);
        assert_eq!(event.ssdeep_hash(), Some("3:abc:def"));
        assert!(!event.ssdeep_degraded());
    }

    #[test]
    fn ssdeep_degraded_is_decoupled_from_hash_invariant() {
        // ssdeep failed (None) while the SHA-256 identity hash is present:
        // the paired (executable_hash, hash_algorithm) invariant still holds.
        let mut event = sample_event();
        event.set_ssdeep_signal(None, true);
        assert_eq!(event.ssdeep_hash(), None);
        assert!(event.ssdeep_degraded());
        assert!(event.validate_hash_tuple().is_ok());
    }

    #[test]
    fn ssdeep_and_mismatch_setters_compose_independently() {
        let mut event = sample_event();
        event.set_ssdeep_signal(Some("3:x:y".to_owned()), false);
        event.set_on_disk_mismatch(true);
        // Setting the mismatch flag must not clobber the ssdeep signal.
        assert_eq!(event.ssdeep_hash(), Some("3:x:y"));
        assert!(event.on_disk_mismatch());
        assert!(!event.ssdeep_degraded());
    }

    #[test]
    fn setters_preserve_existing_metadata() {
        let mut event = sample_event();
        event.platform_metadata = Some(serde_json::json!({ "collector": "linux" }));
        event.set_ssdeep_signal(Some("3:x:y".to_owned()), false);
        event.set_on_disk_mismatch(true);
        let meta = event.platform_metadata.as_ref().expect("metadata present");
        assert_eq!(
            meta.get("collector").and_then(serde_json::Value::as_str),
            Some("linux")
        );
        assert_eq!(event.ssdeep_hash(), Some("3:x:y"));
        assert!(event.on_disk_mismatch());
    }

    #[test]
    fn test_validate_hash_tuple_inconsistent_rejected() {
        let timestamp = SystemTime::now();
        // hash without algorithm -> invariant violated
        let bad_hash_only = ProcessEvent {
            pid: 1,
            ppid: None,
            name: "bad".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: Some("abc".to_owned()),
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };
        assert!(bad_hash_only.validate_hash_tuple().is_err());

        // algorithm without hash -> invariant violated
        let bad_algo_only = ProcessEvent {
            pid: 2,
            ppid: None,
            name: "bad".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: Some("sha256".to_owned()),
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };
        assert!(bad_algo_only.validate_hash_tuple().is_err());

        // both None -> valid
        let neither = ProcessEvent {
            pid: 3,
            ppid: None,
            name: "ok".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };
        assert!(neither.validate_hash_tuple().is_ok());

        // both Some -> valid
        let both = ProcessEvent {
            pid: 4,
            ppid: None,
            name: "ok".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: Some("abc".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp,
            platform_metadata: None,
        };
        assert!(both.validate_hash_tuple().is_ok());
    }

    #[test]
    fn test_collection_event_timestamp() {
        let timestamp = SystemTime::now();
        let process_event = ProcessEvent {
            pid: 1234,
            ppid: None,
            name: "test".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
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
            connection_id: "conn_123".to_owned(),
            source_addr: "192.168.1.100:12345".to_owned(),
            dest_addr: "10.0.0.1:80".to_owned(),
            protocol: "TCP".to_owned(),
            state: "ESTABLISHED".to_owned(),
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
        metadata.insert("test_key".to_owned(), "test_value".to_owned());

        let trigger = TriggerRequest {
            trigger_id: "trigger_123".to_owned(),
            target_collector: "binary-hasher".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/suspicious".to_owned()),
            correlation_id: "corr_456".to_owned(),
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

        let custom1 = AnalysisType::Custom("test".to_owned());
        let custom2 = AnalysisType::Custom("test".to_owned());
        let custom3 = AnalysisType::Custom("other".to_owned());

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
            name: "test".to_owned(),
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned()],
            start_time: Some(timestamp),
            cpu_usage: Some(1.5),
            memory_usage: Some(4096),
            executable_hash: Some("hash123".to_owned()),
            hash_algorithm: Some("sha256".to_owned()),
            user_id: Some("1000".to_owned()),
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
