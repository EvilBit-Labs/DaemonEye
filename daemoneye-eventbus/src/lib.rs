//! # DaemonEye EventBus
//!
//! A cross-platform IPC event bus designed specifically for the DaemonEye monitoring system.
//! Provides pub/sub messaging with wildcard topic matching and embedded broker functionality.
//!
//! ## Features
//!
//! - **Cross-platform**: Windows (named pipes) and Unix (domain sockets)
//! - **Embedded broker**: Runs within daemoneye-agent orchestrator
//! - **Topic routing**: Wildcard matching for flexible event distribution
//! - **At-most-once delivery**: Simple, reliable messaging semantics
//! - **Async/await**: Built on tokio for high-performance concurrent operations
//!
//! ## Basic Usage
//!
//! ```rust,no_run
//! use daemoneye_eventbus::{DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventSubscription, SourceCaps, CollectionEvent, ProcessEvent};
//! use std::collections::HashMap;
//! use std::time::SystemTime;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Start embedded broker
//!     let broker = DaemoneyeBroker::new("/tmp/daemoneye.sock").await?;
//!     let mut event_bus = DaemoneyeEventBus::from_broker(broker).await?;
//!
//!     // Subscribe to process events
//!     let subscription = EventSubscription {
//!         subscriber_id: "example-subscriber".to_string(),
//!         capabilities: SourceCaps {
//!             event_types: vec!["process".to_string()],
//!             collectors: vec!["procmond".to_string()],
//!             max_priority: 5,
//!         },
//!         event_filter: None,
//!         correlation_filter: None,
//!         topic_patterns: Some(vec!["events.process.*".to_string()]),
//!         enable_wildcards: true,
//!     };
//!     let mut receiver = event_bus.subscribe(subscription).await?;
//!
//!     // Publish an event
//!     let process_event = CollectionEvent::Process(ProcessEvent {
//!         pid: 1234,
//!         name: "example_process".to_string(),
//!         command_line: Some("example_command".to_string()),
//!         executable_path: Some("/bin/example".to_string()),
//!         ppid: Some(1),
//!         start_time: Some(SystemTime::now()),
//!         metadata: HashMap::new(),
//!     });
//!     event_bus.publish(process_event, "example_correlation".to_string()).await?;
//!
//!     // Receive events
//!     while let Some(event) = receiver.recv().await {
//!         println!("Received: {:?}", event);
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod broker;
pub mod error;
pub mod message;
pub mod topic;
pub mod transport;

// Re-export main types for convenience
pub use broker::{DaemoneyeBroker, DaemoneyeEventBus};
pub use error::{EventBusError, Result};
pub use message::{
    BusEvent, CollectionEvent, CorrelationFilter, EventFilter, EventSubscription, FilesystemEvent,
    Message, MessageType, NetworkEvent, PerformanceEvent, ProcessEvent, SourceCaps, TriggerRequest,
};
pub use topic::{
    Topic, TopicAccessLevel, TopicDomain, TopicError, TopicMatcher, TopicPattern, TopicRegistry,
    TopicStats, TopicType,
};

/// EventBus trait for compatibility with collector-core
#[allow(async_fn_in_trait)]
pub trait EventBus: Send + Sync {
    /// Publishes an event to the event bus with correlation tracking.
    async fn publish(&mut self, event: CollectionEvent, correlation_id: String) -> Result<()>;

    /// Subscribes to events with filtering and capability matching.
    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>>;

    /// Unsubscribes a subscriber from the event bus.
    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()>;

    /// Returns statistics about event bus operation.
    async fn statistics(&self) -> EventBusStatistics;

    /// Initiates graceful shutdown of the event bus.
    async fn shutdown(&mut self) -> Result<()>;
}

/// Statistics about event bus operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct EventBusStatistics {
    /// Total messages published
    pub messages_published: u64,
    /// Total messages delivered
    pub messages_delivered: u64,
    /// Active subscribers
    pub active_subscribers: usize,
    /// Active topics
    pub active_topics: usize,
    /// Broker uptime in seconds
    pub uptime_seconds: u64,
}
