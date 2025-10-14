//! DaemonEye Agent Library
//!
//! This library provides the core functionality for the DaemonEye detection orchestrator,
//! including embedded EventBus broker management and IPC client functionality.

#![forbid(unsafe_code)]

pub mod broker_manager;
pub mod ipc_client;

pub use broker_manager::{BrokerHealth, BrokerManager};
pub use ipc_client::{IpcClientManager, create_default_ipc_config};
