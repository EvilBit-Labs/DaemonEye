//! Health monitoring system for collector lifecycle management.
//!
//! This module provides comprehensive health monitoring capabilities including
//! heartbeat tracking, health status aggregation, and automated failure detection
//! for collector components managed through the busrt message broker.

#[cfg(feature = "busrt")]
mod busrt_health_monitor {
    // Busrt-specific health monitoring implementation would go here
    // This is disabled during migration away from busrt
}

// Stub implementation for non-busrt builds
#[cfg(not(feature = "busrt"))]
pub struct HealthMonitor {
    // Minimal stub implementation
}

#[cfg(not(feature = "busrt"))]
impl HealthMonitor {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(not(feature = "busrt"))]
impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}
