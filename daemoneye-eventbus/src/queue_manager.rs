//! Queue manager for basic queue support and backpressure handling
//!
//! This module provides queue functionality for the event bus, allowing
//! messages to be queued for clients that are temporarily offline or
//! experiencing backpressure.

use crate::error::{EventBusError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, warn};

/// Bounded queue for client messages
type BoundedQueue = mpsc::Sender<Vec<u8>>;

/// Queue manager for handling client message queues
pub struct QueueManager {
    /// Client queues mapped by client ID
    queues: Arc<Mutex<HashMap<String, QueueInfo>>>,
    /// Maximum queue size per client (default: 1000)
    max_queue_size: usize,
    /// Maximum messages per client
    #[allow(dead_code)]
    max_messages: usize,
}

/// Queue information for a client
struct QueueInfo {
    /// Bounded sender for the queue
    sender: BoundedQueue,
    /// Bounded receiver for the queue (tracked for dequeue operations)
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
    /// Queue statistics
    stats: QueueStats,
    /// Last message timestamp
    last_message: Option<Instant>,
}

/// Queue statistics
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    /// Total messages enqueued
    pub messages_enqueued: u64,
    /// Total messages dequeued
    pub messages_dequeued: u64,
    /// Current queue depth
    pub current_depth: usize,
    /// Messages dropped due to full queue
    pub messages_dropped: u64,
}

impl QueueManager {
    /// Create a new queue manager
    pub fn new(max_queue_size: usize, max_messages: usize) -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            max_queue_size,
            max_messages,
        }
    }

    /// Enqueue a message for a client (non-blocking)
    pub async fn enqueue(&self, client_id: &str, msg: Vec<u8>) -> Result<()> {
        // Validate message size
        if msg.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Message exceeds 1MB limit".to_owned(),
            ));
        }

        let mut queues = self.queues.lock().await;

        // Get or create queue for client
        let queue_info = queues.entry(client_id.to_owned()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel(self.max_queue_size);
            QueueInfo {
                sender: tx,
                receiver: Some(rx),
                stats: QueueStats::default(),
                last_message: None,
            }
        });

        // Try to send (non-blocking)
        match queue_info.sender.try_send(msg) {
            Ok(()) => {
                queue_info.stats.messages_enqueued =
                    queue_info.stats.messages_enqueued.saturating_add(1);
                queue_info.stats.current_depth = queue_info.stats.current_depth.saturating_add(1);
                queue_info.last_message = Some(Instant::now());
                debug!("Enqueued message for client: {}", client_id);
                Ok(())
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                queue_info.stats.messages_dropped =
                    queue_info.stats.messages_dropped.saturating_add(1);
                warn!("Queue full for client: {}, message dropped", client_id);
                Err(EventBusError::transport(
                    "Queue full, backpressure".to_owned(),
                ))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                warn!("Queue closed for client: {}", client_id);
                queues.remove(client_id);
                drop(queues);
                Err(EventBusError::transport("Queue closed".to_owned()))
            }
        }
    }

    /// Dequeue a message for a client (with timeout)
    pub async fn dequeue(&self, client_id: &str, _timeout: Duration) -> Result<Option<Vec<u8>>> {
        // Note: This is a simplified implementation
        // In a real scenario, we'd need to track receivers per client
        let queues = self.queues.lock().await;
        if queues.get(client_id).is_some() {
            drop(queues);
        }
        Ok(None) // Placeholder - would need receiver tracking
    }

    /// Drain all queued messages for a client
    pub async fn drain_queue(&self, client_id: &str) -> Result<Vec<Vec<u8>>> {
        let mut queues_guard = self.queues.lock().await;

        if let Some(queue_info) = queues_guard.get_mut(client_id) {
            let mut messages = Vec::new();
            if let Some(mut receiver) = queue_info.receiver.take() {
                drop(queues_guard);
                // Drain all available messages
                while let Ok(msg) = receiver.try_recv() {
                    messages.push(msg);
                }
                // Put receiver back
                let mut queues_guard2 = self.queues.lock().await;
                if let Some(updated_queue_info) = queues_guard2.get_mut(client_id) {
                    updated_queue_info.receiver = Some(receiver);
                    // SAFETY: messages.len() fits in u64 on all supported platforms.
                    #[allow(clippy::as_conversions)]
                    let len_u64 = messages.len() as u64;
                    updated_queue_info.stats.messages_dequeued = updated_queue_info
                        .stats
                        .messages_dequeued
                        .saturating_add(len_u64);
                    updated_queue_info.stats.current_depth = updated_queue_info
                        .stats
                        .current_depth
                        .saturating_sub(messages.len());
                }
            } else {
                // No receiver available, create a new one
                let (tx, rx) = mpsc::channel(self.max_queue_size);
                queue_info.sender = tx;
                queue_info.receiver = Some(rx);
            }
            Ok(messages)
        } else {
            Ok(Vec::new())
        }
    }

    /// Report available credits (buffer space) for a client
    pub async fn report_credits(&self, client_id: &str, available_slots: usize) -> Result<()> {
        let client_found = self.queues.lock().await.get_mut(client_id).is_some();

        if client_found {
            // Update queue stats based on credits
            // Credits indicate available buffer space, which affects queue capacity
            debug!(
                "Client {} reported {} available slots",
                client_id, available_slots
            );
            // If credits are available and queue has messages, we could trigger draining
            // This is handled by the broker on reconnection
        } else {
            // Client not found in queue manager - this is OK, client may not have queued messages
            debug!(
                "Client {} not found in queue manager (no queued messages)",
                client_id
            );
        }
        Ok(())
    }

    /// Auto-prune old messages (FIFO)
    pub async fn prune_old_messages(&self, max_age: Duration) -> Result<usize> {
        let mut pruned_count = 0_usize;
        let now = Instant::now();

        let mut queues = self.queues.lock().await;
        queues.retain(|client_id, queue_info| {
            queue_info.last_message.is_none_or(|last_msg| {
                if now.duration_since(last_msg) > max_age {
                    debug!("Pruning old queue for client: {}", client_id);
                    pruned_count = pruned_count.saturating_add(1);
                    false
                } else {
                    true
                }
            })
        });
        drop(queues);

        Ok(pruned_count)
    }

    /// Get queue statistics for a client
    pub async fn get_stats(&self, client_id: &str) -> Option<QueueStats> {
        let queues = self.queues.lock().await;
        queues.get(client_id).map(|info| info.stats.clone())
    }

    /// Remove a client queue
    pub async fn remove_client(&self, client_id: &str) -> Result<()> {
        let removed = self.queues.lock().await.remove(client_id).is_some();
        if removed {
            debug!("Removed queue for client: {}", client_id);
        }
        // Client not found - this is OK, may not have had a queue
        Ok(())
    }
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new(1000, 10000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_queue_manager_creation() {
        let manager = QueueManager::new(100, 1000);
        assert!(
            manager
                .enqueue("test-client", b"test message".to_vec())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_queue_full_backpressure() {
        let manager = QueueManager::new(1, 10);

        // Fill the queue
        assert!(
            manager
                .enqueue("test-client", b"msg1".to_vec())
                .await
                .is_ok()
        );

        // Next message should fail due to backpressure
        assert!(
            manager
                .enqueue("test-client", b"msg2".to_vec())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_message_validation() {
        let manager = QueueManager::new(100, 1000);

        // Test oversized message
        let large_msg = vec![0u8; 2 * 1024 * 1024];
        assert!(manager.enqueue("test-client", large_msg).await.is_err());

        // Test valid message with null bytes (null bytes are allowed in binary data)
        let null_msg = b"test\0message".to_vec();
        assert!(manager.enqueue("test-client", null_msg).await.is_ok());
    }
}
