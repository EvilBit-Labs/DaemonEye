#![forbid(unsafe_code)]

//! daemoneye-lib: Shared library for `DaemonEye` common functionality.
//!
//! This library provides core functionality shared across all `DaemonEye` components:
//! - Configuration management with hierarchical overrides
//! - Core data models for processes, alerts, and detection rules
//! - Database abstractions and storage management
//! - Detection engine with SQL-based rule execution
//! - Multi-channel alert delivery system
//! - Cryptographic utilities for audit trails
//! - Performance telemetry and health monitoring
//! - Network correlation and kernel monitoring (Enterprise)
//! - Cross-platform IPC communication

// Core modules (always available)
pub mod config;
pub mod crypto;
/// Cryptographic integrity verification primitives, hashing engines, and path authorization checks.
pub mod integrity;
pub mod ipc;
pub mod models;
pub mod proto;
pub mod storage;
pub mod telemetry;

// Feature-gated modules
/// Alert delivery system with trait-based plugin architecture.
///
/// Supports multi-channel alerting (stdout, syslog, webhooks, email) for SIEM integration.
/// Available in: Free Tier and above.
#[cfg(feature = "alerting")]
pub mod alerting;

/// Process collection and enumeration utilities.
///
/// Core functionality for cross-platform process monitoring.
/// Available in: Free Tier and above.
#[cfg(feature = "process-collection")]
pub mod collection;

/// SQL-based detection engine with rule evaluation.
///
/// Flexible rule creation using standard SQL queries with sandboxed execution.
/// Available in: Free Tier and above.
#[cfg(feature = "detection-engine")]
pub mod detection;

/// Enterprise-tier kernel-level monitoring support.
///
/// Real-time eBPF/ETW/EndpointSecurity event subscription.
/// Available in: Enterprise Tier only.
#[cfg(feature = "kernel-monitoring")]
pub mod kernel;

/// Enterprise-tier network correlation capabilities.
///
/// Process-to-network event correlation for lateral movement detection.
/// Available in: Enterprise Tier only.
#[cfg(feature = "network-correlation")]
pub mod network;

/// Return a greeting message from the shared library for a specific component.
#[must_use]
pub fn greet(component: &str) -> String {
    format!("Hello from daemoneye-lib to {component}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_returns_expected_message() {
        let got = greet("tests");
        assert_eq!(got, "Hello from daemoneye-lib to tests!");
    }
}
