//! Event source abstractions and capability definitions.
//!
//! This module defines the core `EventSource` trait that all collection components
//! must implement, along with capability flags for feature negotiation.

use crate::event::CollectionEvent;
use async_trait::async_trait;
use bitflags::bitflags;
use tokio::sync::mpsc;

bitflags! {
    /// Capability flags for event sources.
    ///
    /// These flags enable capability negotiation between collector-core components
    /// and daemoneye-agent, allowing dynamic feature discovery and task routing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::SourceCaps;
    ///
    /// // Process monitoring with real-time capabilities
    /// let process_caps = SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE;
    ///
    /// // Network monitoring with kernel-level access
    /// let network_caps = SourceCaps::NETWORK | SourceCaps::KERNEL_LEVEL | SourceCaps::REALTIME;
    ///
    /// // Check if source supports specific capabilities
    /// assert!(process_caps.contains(SourceCaps::PROCESS));
    /// assert!(!process_caps.contains(SourceCaps::NETWORK));
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SourceCaps: u32 {
        /// Process monitoring capabilities
        const PROCESS = 1 << 0;

        /// Network monitoring capabilities
        const NETWORK = 1 << 1;

        /// Filesystem monitoring capabilities
        const FILESYSTEM = 1 << 2;

        /// Performance monitoring capabilities
        const PERFORMANCE = 1 << 3;

        /// Real-time event streaming
        const REALTIME = 1 << 4;

        /// Kernel-level access and monitoring
        const KERNEL_LEVEL = 1 << 5;

        /// System-wide monitoring (vs per-process/user)
        const SYSTEM_WIDE = 1 << 6;
    }
}

/// Universal trait for event sources in the collector-core framework.
///
/// This trait abstracts collection methodology from operational infrastructure,
/// enabling multiple collection components to share the same runtime foundation.
///
/// # Lifecycle
///
/// 1. **Registration**: Event sources are registered with the `Collector`
/// 2. **Capability Negotiation**: `capabilities()` is called to determine features
/// 3. **Startup**: `start()` is called with an event channel sender
/// 4. **Operation**: Source generates events and sends them via the channel
/// 5. **Shutdown**: `stop()` is called for graceful cleanup
///
/// # Thread Safety
///
/// All event sources must be `Send + Sync` to support concurrent operation
/// within the collector runtime.
///
/// # Error Handling
///
/// Event sources should handle errors gracefully and continue operation when
/// possible. Critical errors should be logged and may cause the source to stop.
#[async_trait]
pub trait EventSource: Send + Sync {
    /// Returns the unique name of this event source.
    ///
    /// This name is used for logging, metrics, and identification purposes.
    /// It should be stable across restarts and unique within a collector instance.
    fn name(&self) -> &'static str;

    /// Returns the capabilities supported by this event source.
    ///
    /// This is used for capability negotiation with daemoneye-agent and task routing.
    /// The capabilities should be static and not change during the source's lifetime.
    fn capabilities(&self) -> SourceCaps;

    /// Starts the event source with the provided event channel.
    ///
    /// This method should begin event collection and send events via the provided
    /// channel. It should run until `stop()` is called or an unrecoverable error occurs.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel sender for emitting collection events
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot be started or encounters a critical
    /// failure during initialization.
    ///
    /// # Implementation Notes
    ///
    /// - This method should not block indefinitely
    /// - Use `tokio::select!` or similar for graceful shutdown handling
    /// - Log errors appropriately but continue operation when possible
    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()>;

    /// Stops the event source gracefully.
    ///
    /// This method should signal the source to stop collecting events and
    /// perform any necessary cleanup. It should complete quickly and not block.
    ///
    /// # Errors
    ///
    /// Returns an error if cleanup fails, but the source should still be
    /// considered stopped.
    async fn stop(&self) -> anyhow::Result<()>;

    /// Performs a health check on the event source.
    ///
    /// The default implementation returns `Ok(())`, but sources can override
    /// this to provide more detailed health information.
    ///
    /// # Errors
    ///
    /// Returns an error if the source is unhealthy or cannot perform its
    /// collection duties.
    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_caps_bitflags() {
        // Test individual flags
        assert_eq!(SourceCaps::PROCESS.bits(), 1);
        assert_eq!(SourceCaps::NETWORK.bits(), 2);
        assert_eq!(SourceCaps::FILESYSTEM.bits(), 4);
        assert_eq!(SourceCaps::PERFORMANCE.bits(), 8);
        assert_eq!(SourceCaps::REALTIME.bits(), 16);
        assert_eq!(SourceCaps::KERNEL_LEVEL.bits(), 32);
        assert_eq!(SourceCaps::SYSTEM_WIDE.bits(), 64);
    }

    #[test]
    fn test_source_caps_combinations() {
        let process_caps = SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE;

        assert!(process_caps.contains(SourceCaps::PROCESS));
        assert!(process_caps.contains(SourceCaps::REALTIME));
        assert!(process_caps.contains(SourceCaps::SYSTEM_WIDE));
        assert!(!process_caps.contains(SourceCaps::NETWORK));
        assert!(!process_caps.contains(SourceCaps::FILESYSTEM));
    }

    #[test]
    fn test_source_caps_intersection() {
        let caps1 = SourceCaps::PROCESS | SourceCaps::REALTIME;
        let caps2 = SourceCaps::PROCESS | SourceCaps::NETWORK;

        let intersection = caps1 & caps2;
        assert_eq!(intersection, SourceCaps::PROCESS);
    }

    #[test]
    fn test_source_caps_union() {
        let caps1 = SourceCaps::PROCESS | SourceCaps::REALTIME;
        let caps2 = SourceCaps::NETWORK | SourceCaps::FILESYSTEM;

        let union = caps1 | caps2;
        assert!(union.contains(SourceCaps::PROCESS));
        assert!(union.contains(SourceCaps::REALTIME));
        assert!(union.contains(SourceCaps::NETWORK));
        assert!(union.contains(SourceCaps::FILESYSTEM));
    }
}
