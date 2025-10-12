//! Client statistics tracking for busrt client.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};
use tokio::sync::RwLock;

/// Client statistics tracking.
#[derive(Debug)]
pub struct ClientStatistics {
    /// Messages published
    pub(super) messages_published: AtomicU64,
    /// Messages received
    pub(super) messages_received: AtomicU64,
    /// Connection failures
    pub(super) connection_failures: AtomicU64,
    /// Reconnection attempts
    pub(super) reconnection_attempts: AtomicU64,
    /// Last error timestamp
    pub(super) last_error: RwLock<Option<SystemTime>>,
}

impl ClientStatistics {
    /// Creates a new statistics tracker.
    pub(super) fn new() -> Self {
        Self {
            messages_published: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            connection_failures: AtomicU64::new(0),
            reconnection_attempts: AtomicU64::new(0),
            last_error: RwLock::new(None),
        }
    }

    /// Creates a snapshot of current statistics.
    pub(super) async fn snapshot(&self) -> Self {
        Self {
            messages_published: AtomicU64::new(self.messages_published.load(Ordering::Relaxed)),
            messages_received: AtomicU64::new(self.messages_received.load(Ordering::Relaxed)),
            connection_failures: AtomicU64::new(self.connection_failures.load(Ordering::Relaxed)),
            reconnection_attempts: AtomicU64::new(
                self.reconnection_attempts.load(Ordering::Relaxed),
            ),
            last_error: RwLock::new(*self.last_error.read().await),
        }
    }
}
