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
pub mod capability_router;
pub mod collector;
pub mod config;
pub mod config_manager;
pub mod daemoneye_event_bus;
pub mod event;
pub mod event_bus;
pub mod health_monitor;
pub mod high_performance_event_bus;
pub mod ipc;
pub mod load_balancer;
pub mod monitor_collector;
pub mod performance;
pub mod process_manager;
pub mod result_aggregator;
pub mod rpc_services;
pub mod shutdown_coordinator;
pub mod source;
pub mod task_distributor;
pub mod trigger;

// Re-export main types for convenience
pub use analysis_chain::{
    AnalysisChainConfig, AnalysisChainCoordinator, AnalysisResult, AnalysisStage,
    AnalysisWorkflowDefinition, StageStatus, WorkflowError, WorkflowErrorType, WorkflowExecution,
    WorkflowProgress, WorkflowStatistics, WorkflowStatus,
};
pub use capability_router::{
    CapabilityRouter, CollectorCapability, CollectorHealthStatus, RoutingDecision, RoutingStats,
};
pub use collector::{Collector, CollectorRuntime, RuntimeStats};
pub use config::{CollectorConfig, CollectorRegistrationConfig};
pub use config_manager::ConfigManager;
pub use daemoneye_event_bus::{
    BrokerHealthStatus, ClientStatisticsAggregate, DaemoneyeEventBus, DaemoneyeEventBusMetrics,
    PerformanceMetrics, TopicStatistics, TransportStatistics,
};
pub use event::{
    AnalysisType, CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent,
    TriggerPriority, TriggerRequest,
};
pub use event_bus::{
    BusEvent, CorrelationFilter, EventBus, EventBusConfig, EventFilter, EventSubscription,
    LocalEventBus,
};
pub use health_monitor::HealthMonitor;
pub use high_performance_event_bus::{
    BackpressureStrategy, HighPerformanceEventBus, HighPerformanceEventBusConfig,
    HighPerformanceEventBusImpl,
};
pub use ipc::CollectorIpcServer;
pub use load_balancer::{
    FailoverEvent, LoadBalancer, LoadBalancerConfig, LoadBalancingStats, LoadBalancingStrategy,
};
pub use monitor_collector::{
    MonitorCollector, MonitorCollectorConfig, MonitorCollectorStats, MonitorCollectorStatsSnapshot,
};
pub use performance::{
    BaselineMetrics, CpuUsageMetrics, DegradationType, MemoryUsageMetrics, PerformanceComparison,
    PerformanceConfig, PerformanceDegradation, PerformanceMonitor, ResourceUsageMetrics,
    ThroughputMetrics, TriggerLatencyMetrics,
};
pub use result_aggregator::{
    AggregatedResult, AggregationConfig, AggregationStats, AggregationStatus, CollectorResult,
    ResultAggregator,
};
pub use rpc_services::{CollectorRpcServiceManager, RpcServiceConfig};
pub use shutdown_coordinator::{
    ShutdownConfig, ShutdownCoordinator, ShutdownEvent, ShutdownPhase, ShutdownRequest,
    ShutdownResponse,
};
pub use source::{EventSource, SourceCaps};
pub use task_distributor::{DistributionStats, DistributionTask, TaskDistributor, TaskPriority};
pub use trigger::{
    PriorityTriggerQueue, ProcessTriggerData, QueueStatistics, SqlTriggerEvaluator,
    TriggerCapabilities, TriggerCondition, TriggerConfig, TriggerEmissionStats, TriggerManager,
    TriggerResourceLimits, TriggerStatistics,
};
