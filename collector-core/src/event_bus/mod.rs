//! Event bus system for collector coordination and communication.
//!
//! This module provides a unified event bus interface that supports both
//! local in-process communication and distributed communication through
//! message brokers. The event bus enables collectors to publish events,
//! subscribe to event patterns, and coordinate analysis workflows.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Event Bus Interface                         │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  LocalEventBus  │  DistributedEventBus  │  HybridEventBus      │
//! │  (Topic-based)  │  (Message Broker)     │  (Combined)          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

mod local_bus;
mod types;

pub use local_bus::LocalEventBus;
pub use types::*;

use crate::event::CollectionEvent;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Event bus interface for collector coordination
#[async_trait]
pub trait EventBus: Send + Sync {
    /// Publish an event to the bus with correlation metadata
    async fn publish(
        &self,
        event: CollectionEvent,
        correlation_metadata: CorrelationMetadata,
    ) -> Result<()>;

    /// Subscribe to events matching a pattern
    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<Arc<BusEvent>>>;

    /// Unsubscribe from events
    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()>;

    /// Get bus statistics
    async fn get_statistics(&self) -> Result<EventBusStatistics>;

    /// Get a reference to the underlying type for downcasting
    fn as_any(&self) -> &dyn std::any::Any;

    /// Shutdown the event bus and perform any necessary cleanup.
    ///
    /// This method should be called when the event bus is no longer needed
    /// to ensure proper resource cleanup and graceful shutdown of any
    /// background tasks or connections.
    ///
    /// # Errors
    ///
    /// Returns an error if shutdown fails or cleanup cannot be completed.
    async fn shutdown(&self) -> Result<()>;
}
