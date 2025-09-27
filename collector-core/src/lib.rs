//! # Collector Core Framework
//!
//! A reusable collection infrastructure that enables multiple monitoring components
//! while maintaining shared operational foundation.
//!
//! ## Overview
//!
//! The collector-core framework provides:
//! - Universal `EventSource` trait for pluggable collection implementations
//! - `Collector` runtime for event source management and aggregation
//! - Extensible `CollectionEvent` enum for unified event handling
//! - Capability negotiation through `SourceCaps` bitflags
//! - Shared infrastructure for configuration, logging, and health checks
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Collector Runtime                        │
//! ├─────────────────────────────────────────────────────────────┤
//! │  EventSource    EventSource    EventSource    EventSource   │
//! │  (Process)      (Network)      (Filesystem)   (Performance) │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use collector_core::{Collector, CollectorConfig, EventSource, CollectionEvent, SourceCaps};
//! use async_trait::async_trait;
//! use tokio::sync::mpsc;
//!
//! struct MyEventSource;
//!
//! #[async_trait]
//! impl EventSource for MyEventSource {
//!     fn name(&self) -> &'static str {
//!         "my-source"
//!     }
//!
//!     fn capabilities(&self) -> SourceCaps {
//!         SourceCaps::PROCESS | SourceCaps::REALTIME
//!     }
//!
//!     async fn start(&self, tx: mpsc::Sender<CollectionEvent>, _shutdown_signal: std::sync::Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<()> {
//!         // Implementation here
//!         Ok(())
//!     }
//!
//!     async fn stop(&self) -> anyhow::Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = CollectorConfig::default();
//!     let mut collector = Collector::new(config);
//!
//!     collector.register(Box::new(MyEventSource));
//!     collector.run().await
//! }
//! ```

pub mod collector;
pub mod config;
pub mod event;
pub mod ipc;
pub mod source;
pub mod trigger;

// Re-export main types for convenience
pub use collector::{Collector, CollectorRuntime, RuntimeStats};
pub use config::CollectorConfig;
pub use event::{
    AnalysisType, CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent,
    TriggerPriority, TriggerRequest,
};
pub use ipc::CollectorIpcServer;
pub use source::{EventSource, SourceCaps};
pub use trigger::{
    ProcessTriggerData, TriggerCondition, TriggerConfig, TriggerManager, TriggerStatistics,
};
