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
pub mod ipc;
pub mod models;
pub mod proto;
pub mod storage;
pub mod telemetry;

// Feature-gated modules
#[cfg(feature = "alerting")]
pub mod alerting;

#[cfg(feature = "process-collection")]
pub mod collection;

#[cfg(feature = "detection-engine")]
pub mod detection;

#[cfg(feature = "kernel-monitoring")]
pub mod kernel;

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
