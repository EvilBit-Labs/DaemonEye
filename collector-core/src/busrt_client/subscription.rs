//! Subscription management for busrt client.

use crate::busrt_types::BusrtEvent;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

/// Subscription management for topic patterns.
#[derive(Debug)]
pub(super) struct SubscriptionManager {
    /// Active subscriptions (pattern -> receiver)
    pub subscriptions: RwLock<HashMap<String, mpsc::Sender<BusrtEvent>>>,
    /// Subscription metadata
    pub subscription_metadata: RwLock<HashMap<String, SubscriptionMetadata>>,
}

/// Metadata for subscription tracking.
#[derive(Debug)]
#[allow(dead_code)] // Full implementation in progress
pub(super) struct SubscriptionMetadata {
    /// Subscription timestamp
    pub subscribed_at: SystemTime,
    /// Message count received
    pub messages_received: AtomicU64,
    /// Last message timestamp
    pub last_message: RwLock<Option<SystemTime>>,
}

impl SubscriptionManager {
    /// Creates a new subscription manager.
    pub(super) fn new() -> Self {
        Self {
            subscriptions: RwLock::new(HashMap::new()),
            subscription_metadata: RwLock::new(HashMap::new()),
        }
    }

    /// Forwards an event to all matching subscriptions.
    ///
    /// This method checks each subscription pattern against the event topic,
    /// forwards the event to matching subscriptions, and removes dead subscriptions.
    pub(super) async fn forward_to_subscriptions(&self, event: BusrtEvent) {
        let topic = &event.topic;
        let mut dead_subscriptions = Vec::new();

        // Read subscriptions and attempt to send
        {
            let subscriptions = self.subscriptions.read().await;

            for (pattern, sender) in subscriptions.iter() {
                // Check if pattern matches topic (simple prefix match for now)
                if Self::pattern_matches(pattern, topic) {
                    // Try to send event (clone for each subscription)
                    match sender.try_send(event.clone()) {
                        Ok(_) => {
                            debug!(pattern = %pattern, topic = %topic, "Event forwarded to subscription");

                            // Update subscription metadata
                            let metadata = self.subscription_metadata.read().await;
                            if let Some(meta) = metadata.get(pattern) {
                                meta.messages_received.fetch_add(1, Ordering::Relaxed);
                                let mut last_message = meta.last_message.write().await;
                                *last_message = Some(SystemTime::now());
                            }
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            warn!(pattern = %pattern, "Subscription channel full, event dropped");
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            warn!(pattern = %pattern, "Subscription channel closed, marking for removal");
                            dead_subscriptions.push(pattern.clone());
                        }
                    }
                }
            }
        }

        // Clean up dead subscriptions
        if !dead_subscriptions.is_empty() {
            let mut subscriptions = self.subscriptions.write().await;
            let mut metadata = self.subscription_metadata.write().await;

            for pattern in dead_subscriptions {
                subscriptions.remove(&pattern);
                metadata.remove(&pattern);
                info!(pattern = %pattern, "Removed dead subscription");
            }
        }
    }

    /// Checks if a pattern matches a topic.
    ///
    /// Supports wildcard patterns:
    /// - `*` matches a single path segment
    /// - `**` matches any number of path segments
    fn pattern_matches(pattern: &str, topic: &str) -> bool {
        // Simple wildcard matching implementation
        if pattern == topic {
            return true;
        }

        // Handle ** wildcard (match anything)
        if pattern.contains("**") {
            let prefix = pattern.split("**").next().unwrap_or("");
            return topic.starts_with(prefix);
        }

        // Handle * wildcard (match single segment)
        if pattern.contains('*') {
            let pattern_parts: Vec<&str> = pattern.split('/').collect();
            let topic_parts: Vec<&str> = topic.split('/').collect();

            if pattern_parts.len() != topic_parts.len() {
                return false;
            }

            for (p, t) in pattern_parts.iter().zip(topic_parts.iter()) {
                if *p != "*" && p != t {
                    return false;
                }
            }
            return true;
        }

        false
    }
}
