//! RPC service implementations for collector lifecycle management.
//!
//! This module provides RPC service integration for collector-core, enabling
//! lifecycle operations (start, stop, restart, health checks) to be performed
//! via RPC calls through the `DaemonEye` event bus.

// These module-wide allows cover patterns that are intentional in the RPC
// service plumbing. Lint levels cascade to the child modules (config, manager,
// providers), which is why the cast/format patterns there compile.
//
// - significant_drop_tightening: the async RPC loop and providers hold tokio
//   RwLock guards across awaits where tightening the scope would hurt clarity.
// - as_conversions: metric counters are cast u64/usize -> f64 for reporting;
//   see the per-site SAFETY comments in providers.rs.
// - pattern_type_mismatch: matches over borrowed enums (e.g. request.operation)
//   read more clearly without explicit ref bindings.
// - indexing_slicing: bounded slice indexing in topic parsing within manager.rs.
// - unwrap_used / expect_used: permitted for the test modules and for any
//   manager.rs paths operating on values known to be present (no non-test
//   production callers rely on these today).
// - use_debug: structured tracing fields format enums with `?` (Debug).
// - unreachable: exhaustive match arms in manager.rs that cannot be hit.
#![allow(clippy::significant_drop_tightening)]
#![allow(clippy::as_conversions)]
#![allow(clippy::pattern_type_mismatch)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::use_debug)]
#![allow(clippy::unreachable)]

mod config;
mod manager;
mod providers;

pub use config::RpcServiceConfig;
pub use manager::CollectorRpcServiceManager;
pub use providers::{
    CollectorConfigProvider, CollectorHealthProvider, CollectorRegistrationProvider,
};
