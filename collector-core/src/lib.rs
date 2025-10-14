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

pub mod analysis_chain;
// busrt modules disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub mod busrt_client;
// #[cfg(feature = "busrt")]
// pub mod busrt_event_bus;
// #[cfg(feature = "busrt")]
// pub mod busrt_types;
pub mod collector;
pub mod config;
pub mod config_manager;
pub mod daemoneye_event_bus;
// embedded_broker module disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub mod embedded_broker;
pub mod event;
pub mod event_bus;
pub mod health_monitor;
pub mod high_performance_event_bus;
pub mod ipc;
// ipc_busrt_bridge module disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub mod ipc_busrt_bridge;
pub mod monitor_collector;
pub mod performance;
pub mod rpc_services;
pub mod shutdown_coordinator;
pub mod source;
pub mod trigger;

// Re-export main types for convenience
pub use analysis_chain::{
    AnalysisChainConfig, AnalysisChainCoordinator, AnalysisResult, AnalysisStage,
    AnalysisWorkflowDefinition, StageStatus, WorkflowError, WorkflowErrorType, WorkflowExecution,
    WorkflowProgress, WorkflowStatistics, WorkflowStatus,
};
// busrt re-exports disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use busrt_client::{CollectorBusrtClient, ReconnectionConfig};
// #[cfg(feature = "busrt")]
// pub use busrt_event_bus::{BusrtEventBus, BusrtEventBusConfig, BusrtQoS};
// #[cfg(feature = "busrt")]
// pub use busrt_types::{
//     BrokerConfig, BrokerMode, BrokerStats, BusrtClient, BusrtError, BusrtEvent,
//     CollectorLifecycleService, ConfigurationService, CrossbeamCompatibilityLayer,
//     EmbeddedBrokerConfig, EventCorrelation, EventPayload, HealthCheckService,
//     StandaloneBrokerConfig, topics,
// };
pub use collector::{Collector, CollectorRuntime, RuntimeStats};
pub use config::CollectorConfig;
#[cfg(not(feature = "busrt"))]
pub use config_manager::ConfigManager;
pub use daemoneye_event_bus::DaemoneyeEventBus;
// busrt-specific config manager types disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use config_manager::{ConfigChangeEvent, ConfigManager, ConfigManagerSettings};
// embedded_broker re-exports disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use embedded_broker::{BrokerMessage, BrokerMessageType, EmbeddedBroker};
pub use event::{
    AnalysisType, CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent,
    TriggerPriority, TriggerRequest,
};
pub use event_bus::{
    BusEvent, CorrelationFilter, EventBus, EventBusConfig, EventBusStatistics, EventFilter,
    EventSubscription, LocalEventBus,
};
#[cfg(not(feature = "busrt"))]
pub use health_monitor::HealthMonitor;
// busrt-specific health monitor types disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use health_monitor::{
//     CollectorHealthState, CollectorHealthSummary, HealthEvent, HealthMonitor, HealthMonitorConfig,
//     HealthSnapshot, HealthSummary,
// };
pub use high_performance_event_bus::{
    BackpressureStrategy, HighPerformanceEventBus, HighPerformanceEventBusConfig,
    HighPerformanceEventBusImpl,
};
pub use ipc::CollectorIpcServer;
// ipc_busrt_bridge re-exports disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use ipc_busrt_bridge::{IpcBridgeConfig, IpcBusrtBridge};
pub use monitor_collector::{
    MonitorCollector, MonitorCollectorConfig, MonitorCollectorStats, MonitorCollectorStatsSnapshot,
};
pub use performance::{
    BaselineMetrics, CpuUsageMetrics, DegradationType, MemoryUsageMetrics, PerformanceComparison,
    PerformanceConfig, PerformanceDegradation, PerformanceMonitor, ResourceUsageMetrics,
    ThroughputMetrics, TriggerLatencyMetrics,
};
// busrt-specific RPC services disabled during migration away from busrt
// #[cfg(feature = "busrt")]
// pub use rpc_services::CollectorLifecycleManager;
#[cfg(not(feature = "busrt"))]
pub use rpc_services::RpcServices;
pub use shutdown_coordinator::{
    ShutdownConfig, ShutdownCoordinator, ShutdownEvent, ShutdownPhase, ShutdownRequest,
    ShutdownResponse,
};
pub use source::{EventSource, SourceCaps};
pub use trigger::{
    PriorityTriggerQueue, ProcessTriggerData, QueueStatistics, SqlTriggerEvaluator,
    TriggerCapabilities, TriggerCondition, TriggerConfig, TriggerEmissionStats, TriggerManager,
    TriggerResourceLimits, TriggerStatistics,
};

// busrt integration tests disabled during migration away from busrt
