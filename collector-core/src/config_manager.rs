//! Configuration management system for collector lifecycle management.
//!
//! The busrt-backed implementation has been retired; this placeholder keeps the build
//! surface stable while the DaemonEye event bus configuration flow is finalized.

/// Temporary configuration manager stub.
#[derive(Default)]
pub struct ConfigManager;

impl ConfigManager {
    /// Create a new configuration manager instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
