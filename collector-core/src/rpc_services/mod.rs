//! RPC service implementations for collector lifecycle management.
//!
//! This module provides RPC service integration for collector-core, enabling
//! lifecycle operations (start, stop, restart, health checks) to be performed
//! via RPC calls through the `DaemonEye` event bus.

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
