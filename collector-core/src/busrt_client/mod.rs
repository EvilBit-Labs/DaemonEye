//! Busrt client implementation for collector-core framework.
//!
//! This module provides a concrete implementation of the BusrtClient trait
//! that handles connection management, topic subscriptions, and message
//! publishing with the busrt message broker.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Busrt Client                                 │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │ Connection  │  │ Subscription│  │    Message Handler      │ │
//! │  │  Manager    │  │  Manager    │  │                         │ │
//! │  └─────────────┘  └─────────────┘  │  - Topic Routing        │ │
//! │         │                │         │  - Message Queuing      │ │
//! │         └────────────────┼─────────│  - Error Recovery       │ │
//! │                          │         │  - Reconnection Logic   │ │
//! │                          │         └─────────────────────────┘ │
//! │                          │                   │                 │
//! │                          └───────────────────┘                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Module Organization
//!
//! - `client`: Core `CollectorBusrtClient` implementation (~400 lines)
//! - `connection`: Connection state and management (~150 lines)
//! - `subscription`: Subscription management (~130 lines)
//! - `statistics`: Client statistics tracking (~50 lines)
//! - `types`: Type definitions (OutboundMessage, ReconnectionConfig, etc.) (~70 lines)
//! - `test_helpers`: Test fixtures and helpers (~100 lines)
//!
//! This organization keeps each file focused and under 500-600 lines for maintainability.

// Submodules
mod client;
mod connection;
mod statistics;
mod subscription;
#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod tests;
mod types;

// Re-exports for public API
pub use client::CollectorBusrtClient;
pub use statistics::ClientStatistics;
pub use types::ReconnectionConfig;
