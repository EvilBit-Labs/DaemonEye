//! RPC service implementations for collector lifecycle management.
//!
//! The RPC pipeline is being rebuilt on top of the DaemonEye event bus. This stub keeps the
//! collector-core API stable until the new RPC services ship.

/// Placeholder RPC services container.
#[derive(Default)]
pub struct RpcServices;

impl RpcServices {
    /// Create a new RPC services stub.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
