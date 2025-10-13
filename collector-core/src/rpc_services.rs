//! RPC service implementations for collector lifecycle management.
//!
//! This module provides concrete implementations of the RPC service traits
//! defined in busrt_types.rs, enabling collector lifecycle management through
//! busrt message broker RPC calls.

#[cfg(feature = "busrt")]
mod busrt_rpc_services {
    // Busrt-specific RPC service implementations would go here
    // This is disabled during migration away from busrt
}

// Stub implementation for non-busrt builds
#[cfg(not(feature = "busrt"))]
pub struct RpcServices {
    // Minimal stub implementation
}

#[cfg(not(feature = "busrt"))]
impl RpcServices {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(not(feature = "busrt"))]
impl Default for RpcServices {
    fn default() -> Self {
        Self::new()
    }
}
