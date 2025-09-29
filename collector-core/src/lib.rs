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
pub mod collector;
pub mod config;
pub mod event;
pub mod event_bus;
pub mod high_performance_event_bus;
pub mod ipc;
pub mod monitor_collector;
pub mod performance;
pub mod source;
pub mod trigger;

// Re-export main types for convenience
pub use analysis_chain::{
    AnalysisChainConfig, AnalysisChainCoordinator, AnalysisResult, AnalysisStage,
    AnalysisWorkflowDefinition, StageStatus, WorkflowError, WorkflowErrorType, WorkflowExecution,
    WorkflowProgress, WorkflowStatistics, WorkflowStatus,
};
pub use collector::{Collector, CollectorRuntime, RuntimeStats};
pub use config::CollectorConfig;
pub use event::{
    AnalysisType, CollectionEvent, FilesystemEvent, NetworkEvent, PerformanceEvent, ProcessEvent,
    TriggerPriority, TriggerRequest,
};
pub use event_bus::{
    BusEvent, CorrelationFilter, EventBus, EventBusConfig, EventBusStatistics, EventFilter,
    EventSubscription, LocalEventBus,
};
pub use high_performance_event_bus::{
    BackpressureStrategy, HighPerformanceEventBus, HighPerformanceEventBusConfig,
    HighPerformanceEventBusImpl,
};
pub use ipc::CollectorIpcServer;
pub use monitor_collector::{
    MonitorCollector, MonitorCollectorConfig, MonitorCollectorStats, MonitorCollectorStatsSnapshot,
};
pub use performance::{
    BaselineMetrics, CpuUsageMetrics, DegradationType, MemoryUsageMetrics, PerformanceComparison,
    PerformanceConfig, PerformanceDegradation, PerformanceMonitor, ResourceUsageMetrics,
    ThroughputMetrics, TriggerLatencyMetrics,
};
pub use source::{EventSource, SourceCaps};
pub use trigger::{
    PriorityTriggerQueue, ProcessTriggerData, QueueStatistics, SqlTriggerEvaluator,
    TriggerCapabilities, TriggerCondition, TriggerConfig, TriggerEmissionStats, TriggerManager,
    TriggerResourceLimits, TriggerStatistics,
};

#[cfg(test)]
mod busrt_integration_tests {

    #[test]
    fn test_busrt_dependency_available() {
        // Simple test to verify busrt dependency is properly available
        // This test ensures the busrt crate can be imported and basic types are accessible
        use busrt::QoS;

        // Test that we can reference busrt types without compilation errors
        let qos = QoS::No;

        // Verify the QoS enum has expected variants
        match qos {
            QoS::No => {
                // Test passes if we can match on busrt types
            }
            _ => {
                panic!("Unexpected QoS variant");
            }
        }
    }
}
