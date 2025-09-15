#![forbid(unsafe_code)]

//! sentinel-lib: Shared library for SentinelD common functionality.
//!
//! This library provides core functionality shared across all SentinelD components:
//! - Configuration management with hierarchical overrides
//! - Core data models for processes, alerts, and detection rules
//! - Database abstractions and storage management
//! - Detection engine with SQL-based rule execution
//! - Multi-channel alert delivery system
//! - Cryptographic utilities for audit trails
//! - Performance telemetry and health monitoring
//! - Network correlation and kernel monitoring (Enterprise)

pub mod alerting;
pub mod config;
pub mod crypto;
pub mod detection;
pub mod kernel;
pub mod models;
pub mod network;
pub mod storage;
pub mod telemetry;

/// Return a greeting message from the shared library for a specific component.
#[must_use]
pub fn greet(component: &str) -> String {
    format!("Hello from sentinel-lib to {component}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_returns_expected_message() {
        let got = greet("tests");
        assert_eq!(got, "Hello from sentinel-lib to tests!");
    }
}
