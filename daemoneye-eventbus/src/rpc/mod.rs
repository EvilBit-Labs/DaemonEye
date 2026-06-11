//! RPC call patterns for collector lifecycle management
//!
//! This module defines RPC service patterns for managing collector processes through
//! the daemoneye-eventbus message broker. It provides structured request/response
//! patterns for collector start/stop/restart operations, health checks, configuration
//! updates, and graceful shutdown coordination.
//!
//! ## Design Principles
//!
//! - **Request/Response Pattern**: All RPC calls follow a structured request/response model
//! - **Timeout Handling**: All operations have configurable timeouts with graceful degradation
//! - **Error Propagation**: Comprehensive error types with context for debugging
//! - **Correlation Tracking**: All RPC calls include correlation IDs for distributed tracing
//! - **Capability-Based**: Operations respect collector capabilities and constraints
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use daemoneye_eventbus::rpc::{CollectorRpcClient, CollectorLifecycleRequest, RpcRequest, CollectorOperation};
//! use daemoneye_eventbus::broker::DaemoneyeBroker;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create an embedded broker and wrap in Arc
//!     let broker = Arc::new(DaemoneyeBroker::new("/tmp/example-eventbus.sock").await?);
//!
//!     // Create the RPC client targeting the collector control topic
//!     let mut rpc_client = CollectorRpcClient::new("control.collector.procmond", broker).await?;
//!
//!     // Create a lifecycle request
//!     let lifecycle_request = CollectorLifecycleRequest::start("procmond", None);
//!     let request = RpcRequest::lifecycle(
//!         "client-id".to_string(),
//!         "control.collector.procmond".to_string(),
//!         CollectorOperation::Start,
//!         lifecycle_request,
//!         Duration::from_secs(30)
//!     );
//!     let response = rpc_client.call(request, Duration::from_secs(30)).await?;
//!
//!     println!("Collector started: {:?}", response);
//!     Ok(())
//! }
//! ```

mod client;
mod messages;
mod providers;
mod service;

#[cfg(test)]
mod tests;

pub use client::*;
pub use messages::*;
pub use providers::{ConfigProvider, HealthProvider, RegistrationError, RegistrationProvider};
pub use service::*;
