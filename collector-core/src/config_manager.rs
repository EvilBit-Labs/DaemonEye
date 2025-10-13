//! Configuration management system for collector lifecycle management.
//!
//! This module provides dynamic configuration management capabilities including
//! hot-reload, validation, rollback, and change notification for collector
//! components managed through the busrt message broker.

#[cfg(feature = "busrt")]
mod busrt_config_manager {
    // Busrt-specific configuration management implementation would go here
    // This is disabled during migration away from busrt
}

// Stub implementation for non-busrt builds
#[cfg(not(feature = "busrt"))]
pub struct ConfigManager {
    // Minimal stub implementation
}

#[cfg(not(feature = "busrt"))]
impl ConfigManager {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(not(feature = "busrt"))]
impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}
