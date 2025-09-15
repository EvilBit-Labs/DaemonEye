//! Core data models for SentinelD process monitoring and threat detection.
//!
//! This module provides the foundational data structures used throughout
//! the SentinelD system for representing processes, alerts, and detection rules.

pub mod alert;
pub mod process;
pub mod rule;

// Re-export the main types for convenience
pub use alert::{Alert, AlertContext, AlertError, AlertId, AlertSeverity};
pub use process::{ProcessError, ProcessId, ProcessRecord, ProcessStatus, SystemInfo};
pub use rule::{DetectionRule, RuleError, RuleId, RuleMetadata};

// Re-export external types
pub use uuid::Uuid;
