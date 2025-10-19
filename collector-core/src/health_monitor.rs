//! Health monitoring system for collector lifecycle management.
//!
//! The dedicated burst-based health monitor has been removed. This stub keeps the public
//! surface intact until the DaemonEye event bus health pipeline lands.

/// Temporary health monitor placeholder.
#[derive(Default)]
pub struct HealthMonitor;

impl HealthMonitor {
    /// Construct a new health monitor stub.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
