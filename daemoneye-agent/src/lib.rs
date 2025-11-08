//! DaemonEye Agent Library
//!
//! This library provides the core functionality for the DaemonEye detection orchestrator,
//! including embedded EventBus broker management, IPC client functionality, and IPC server
//! management for CLI communication.

#![forbid(unsafe_code)]

pub mod broker_manager;
pub mod collector_registry;
pub mod ipc_client;
pub mod ipc_server;

pub use broker_manager::{BrokerHealth, BrokerManager};
pub use collector_registry::{CollectorRegistry, RegistryError};
pub use ipc_client::{IpcClientManager, create_default_ipc_config};
pub use ipc_server::{IpcServerHealth, IpcServerManager, create_cli_ipc_config};
