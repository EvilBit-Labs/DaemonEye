//! Kernel-level monitoring (Enterprise tier).
//!
//! This module provides kernel-level monitoring capabilities for the Enterprise tier,
//! including eBPF integration and low-level system event monitoring.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Kernel monitoring errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KernelError {
    #[error("eBPF program loading failed: {0}")]
    EbpfError(String),

    #[error("Kernel interface error: {0}")]
    KernelInterfaceError(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

/// Kernel event types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum KernelEventType {
    ProcessCreate,
    ProcessExit,
    FileAccess,
    NetworkConnection,
    SystemCall,
    MemoryAccess,
    Other(String),
}

/// Kernel-level event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KernelEvent {
    /// Event type
    pub event_type: KernelEventType,
    /// Event timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
    /// Event data
    pub data: serde_json::Value,
    /// Event severity
    pub severity: String,
}

impl KernelEvent {
    /// Create a new kernel event.
    pub fn new(event_type: KernelEventType, pid: u32, tid: u32, data: serde_json::Value) -> Self {
        Self {
            event_type,
            timestamp: chrono::Utc::now(),
            pid,
            tid,
            data,
            severity: "info".to_owned(),
        }
    }
}

/// Kernel monitoring interface.
pub struct KernelMonitor {
    enabled: bool,
    event_count: u64,
}

impl KernelMonitor {
    /// Create a new kernel monitor.
    pub fn new() -> Result<Self, KernelError> {
        // Check if kernel monitoring is supported on this platform
        if !Self::is_supported() {
            return Err(KernelError::UnsupportedPlatform(
                "Kernel monitoring not supported on this platform".to_owned(),
            ));
        }

        Ok(Self {
            enabled: false,
            event_count: 0,
        })
    }

    /// Check if kernel monitoring is supported on this platform.
    pub const fn is_supported() -> bool {
        // In a real implementation, this would check for eBPF support
        // and appropriate kernel capabilities
        cfg!(target_os = "linux")
    }

    /// Start kernel monitoring.
    pub fn start(&mut self) -> Result<(), KernelError> {
        if !Self::is_supported() {
            return Err(KernelError::UnsupportedPlatform(
                "Kernel monitoring not supported".to_owned(),
            ));
        }

        self.enabled = true;
        Ok(())
    }

    /// Stop kernel monitoring.
    #[allow(clippy::missing_const_for_fn)]
    pub fn stop(&mut self) -> Result<(), KernelError> {
        self.enabled = false;
        Ok(())
    }

    /// Check if monitoring is enabled.
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the number of events collected.
    pub const fn event_count(&self) -> u64 {
        self.event_count
    }

    /// Collect kernel events (placeholder implementation).
    #[allow(clippy::missing_const_for_fn)]
    pub fn collect_events(&mut self) -> Result<Vec<KernelEvent>, KernelError> {
        if !self.enabled {
            return Ok(vec![]);
        }

        // In a real implementation, this would collect actual kernel events
        // For now, return empty vector
        Ok(vec![])
    }
}

impl Default for KernelMonitor {
    fn default() -> Self {
        Self::new().unwrap_or(Self {
            enabled: false,
            event_count: 0,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::shadow_reuse, clippy::match_same_arms)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_event_creation() {
        let event = KernelEvent::new(
            KernelEventType::ProcessCreate,
            1234,
            1234,
            serde_json::json!({"name": "test-process"}),
        );

        assert_eq!(event.pid, 1234);
        assert_eq!(event.tid, 1234);
        assert_eq!(event.event_type, KernelEventType::ProcessCreate);
        assert_eq!(event.severity, "info");
    }

    #[test]
    fn test_kernel_event_types() {
        let process_create = KernelEventType::ProcessCreate;
        let process_exit = KernelEventType::ProcessExit;
        let file_access = KernelEventType::FileAccess;
        let network_connection = KernelEventType::NetworkConnection;
        let system_call = KernelEventType::SystemCall;
        let memory_access = KernelEventType::MemoryAccess;
        let other = KernelEventType::Other("custom".to_owned());

        assert_eq!(process_create, KernelEventType::ProcessCreate);
        assert_eq!(process_exit, KernelEventType::ProcessExit);
        assert_eq!(file_access, KernelEventType::FileAccess);
        assert_eq!(network_connection, KernelEventType::NetworkConnection);
        assert_eq!(system_call, KernelEventType::SystemCall);
        assert_eq!(memory_access, KernelEventType::MemoryAccess);
        assert_eq!(other, KernelEventType::Other("custom".to_owned()));
    }

    #[test]
    fn test_kernel_event_serialization() {
        let event = KernelEvent::new(
            KernelEventType::ProcessCreate,
            1234,
            1234,
            serde_json::json!({"name": "test-process"}),
        );

        let json = serde_json::to_string(&event).expect("Failed to serialize event");
        let deserialized: KernelEvent =
            serde_json::from_str(&json).expect("Failed to deserialize event");

        assert_eq!(event.pid, deserialized.pid);
        assert_eq!(event.tid, deserialized.tid);
        assert_eq!(event.event_type, deserialized.event_type);
        assert_eq!(event.data, deserialized.data);
    }

    #[test]
    fn test_kernel_monitor_creation() {
        let monitor = KernelMonitor::new();
        // This will succeed on Linux, fail on other platforms
        if KernelMonitor::is_supported() {
            assert!(monitor.is_ok());
            let monitor = monitor.expect("Failed to create monitor");
            assert!(!monitor.is_enabled());
            assert_eq!(monitor.event_count(), 0);
        } else {
            assert!(monitor.is_err());
        }
    }

    #[test]
    fn test_kernel_monitor_default() {
        let monitor = KernelMonitor::default();
        assert!(!monitor.is_enabled());
        assert_eq!(monitor.event_count(), 0);
    }

    #[test]
    fn test_kernel_monitor_support_check() {
        let is_supported = KernelMonitor::is_supported();
        // Should be true on Linux, false on other platforms
        if cfg!(target_os = "linux") {
            assert!(is_supported);
        } else {
            assert!(!is_supported);
        }
    }

    #[test]
    fn test_kernel_monitor_lifecycle() {
        if let Ok(mut monitor) = KernelMonitor::new() {
            assert!(!monitor.is_enabled());

            monitor.start().expect("Failed to start monitor");
            assert!(monitor.is_enabled());

            monitor.stop().expect("Failed to stop monitor");
            assert!(!monitor.is_enabled());
        }
    }

    #[test]
    fn test_kernel_monitor_start_unsupported_platform() {
        // Test the error path when platform is not supported
        if !KernelMonitor::is_supported() {
            let mut monitor = KernelMonitor::default();
            let result = monitor.start();
            assert!(result.is_err());
            match result.expect_err("Expected error") {
                KernelError::UnsupportedPlatform(_) => {}
                KernelError::EbpfError(_)
                | KernelError::KernelInterfaceError(_)
                | KernelError::PermissionDenied(_) => {
                    // Other expected errors on unsupported platforms
                }
            }
        }
    }

    #[test]
    fn test_kernel_monitor_collect_events_disabled() {
        let mut monitor = KernelMonitor::default();
        let events = monitor.collect_events().expect("Failed to collect events");
        assert!(events.is_empty());
    }

    #[test]
    fn test_kernel_monitor_collect_events_enabled() {
        if let Ok(mut monitor) = KernelMonitor::new() {
            monitor.start().expect("Failed to start monitor");
            let events = monitor.collect_events().expect("Failed to collect events");
            // Currently returns empty vector, but tests the enabled path
            assert!(events.is_empty());
        }
    }

    #[test]
    fn test_kernel_error_display() {
        let errors = vec![
            KernelError::EbpfError("test error".to_owned()),
            KernelError::KernelInterfaceError("test error".to_owned()),
            KernelError::PermissionDenied("test error".to_owned()),
            KernelError::UnsupportedPlatform("test error".to_owned()),
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(error_string.contains("test error"));
        }
    }
}
