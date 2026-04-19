//! `EventBus` client implementation with topic management and reconnection logic.
//!
//! This module provides a high-level client interface for connecting to the
//! daemoneye-eventbus broker with automatic reconnection, health monitoring,
//! and topic-based subscription management.

use crate::{
    error::{EventBusError, Result},
    message::{BusEvent, CollectionEvent, EventSubscription, Message, MessageType},
    topic::{Topic, TopicPattern},
    transport::{ClientConfig, SocketConfig, TransportClient},
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// High-level `EventBus` client with topic management and reconnection
pub struct EventBusClient {
    /// Client identifier
    client_id: String,
    /// Transport client
    transport: Arc<Mutex<TransportClient>>,
    /// Client configuration
    config: ClientConfig,
    /// Socket configuration (stored for reconnection)
    #[allow(dead_code)]
    socket_config: SocketConfig,
    /// Active subscriptions
    subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
    /// Message sequence counter
    sequence: Arc<Mutex<u64>>,
    /// Client statistics
    stats: Arc<Mutex<ClientStats>>,
    /// Shutdown signal
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Background task handles
    task_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Subscription information
#[derive(Debug, Clone)]
struct SubscriptionInfo {
    /// Topic patterns
    patterns: Vec<String>,
    /// Event sender (bounded for backpressure)
    sender: mpsc::Sender<BusEvent>,
    /// Optional control-message sender (bounded for backpressure)
    ///
    /// Populated when the subscription was created via
    /// [`EventBusClient::subscribe_with_control`] with
    /// [`EventSubscription::include_control`] set to `true`.
    /// `None` preserves legacy behavior — Control messages are dropped
    /// at the client-side filter and never surface to the subscriber.
    control_sender: Option<mpsc::Sender<Message>>,
    /// Subscription timestamp (for future metrics)
    #[allow(dead_code)]
    created_at: Instant,
    /// Last message received (for future metrics)
    #[allow(dead_code)]
    last_message: Option<Instant>,
}

/// Client statistics
#[derive(Debug, Clone, Default)]
pub struct ClientStats {
    /// Messages published
    pub messages_published: u64,
    /// Messages received
    pub messages_received: u64,
    /// Subscription count
    pub active_subscriptions: usize,
    /// Connection uptime
    pub uptime: Duration,
    /// Reconnection attempts
    pub reconnection_attempts: u64,
    /// Last successful connection
    pub last_connected: Option<Instant>,
    /// Health check failures
    pub health_check_failures: u64,
}

impl EventBusClient {
    /// Create a new `EventBus` client
    pub async fn new(
        client_id: String,
        socket_config: SocketConfig,
        config: ClientConfig,
    ) -> Result<Self> {
        info!("Creating EventBus client: {}", client_id);

        let transport =
            TransportClient::connect_with_config(&socket_config, config.clone()).await?;
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        let shutdown_rx = shutdown_tx.subscribe();

        let mut client = Self {
            client_id: client_id.clone(),
            transport: Arc::new(Mutex::new(transport)),
            config,
            socket_config,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            stats: Arc::new(Mutex::new(ClientStats {
                last_connected: Some(Instant::now()),
                ..Default::default()
            })),
            shutdown_tx: Some(shutdown_tx),
            task_handles: Vec::new(),
        };

        // Start background tasks
        client.start_background_tasks(shutdown_rx).await?;

        info!("EventBus client created: {}", client_id);
        Ok(client)
    }

    /// Start background tasks for health monitoring and message processing
    #[allow(clippy::unused_async)]
    async fn start_background_tasks(
        &mut self,
        shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<()> {
        // Health monitoring task
        let health_task = {
            let transport = Arc::clone(&self.transport);
            let stats = Arc::clone(&self.stats);
            let config = self.config.clone();
            let client_id = self.client_id.clone();
            let mut shutdown_rx_health = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(config.health_check_interval);
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let mut transport_guard = transport.lock().await;
                            match transport_guard.health_check().await {
                                Ok(true) => {
                                    debug!("Health check passed for client: {}", client_id);
                                }
                                Ok(false) => {
                                    warn!("Health check failed for client: {}", client_id);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.health_check_failures = stats_guard.health_check_failures.saturating_add(1);
                                }
                                Err(e) => {
                                    error!("Health check error for client {}: {}", client_id, e);
                                    let mut stats_guard = stats.lock().await;
                                    stats_guard.health_check_failures = stats_guard.health_check_failures.saturating_add(1);
                                }
                            }
                        }
                        _ = shutdown_rx_health.recv() => {
                            info!("Health monitoring task shutting down for client: {}", client_id);
                            break;
                        }
                    }
                }
            })
        };

        // Message processing task - continuously calls process_messages()
        let message_task = {
            let transport = Arc::clone(&self.transport);
            let subscriptions = Arc::clone(&self.subscriptions);
            let stats = Arc::clone(&self.stats);
            let client_id = self.client_id.clone();
            let mut shutdown_rx_msg = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(10));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            // Try to receive a message (non-blocking with timeout)
                            let message_result = {
                                let mut transport_guard = transport.lock().await;
                                tokio::time::timeout(
                                    tokio::time::Duration::from_millis(100),
                                    transport_guard.receive()
                                ).await
                            };

                            match message_result {
                                Ok(Ok(message_data)) => {
                                    // Deserialize and process message
                                    match crate::message::Message::from_bytes(&message_data) {
                                        Ok(message) => {
                                            // Handle different message types
                                            match message.message_type {
                                                crate::message::MessageType::Event => {
                                                    // Process event message (stats incremented in handle_event_message_internal)
                                                    if let Err(e) = Self::handle_event_message_internal(
                                                        &subscriptions,
                                                        &stats,
                                                        &message,
                                                        &client_id
                                                    ).await {
                                                        debug!("Error handling event message for client {}: {}", client_id, e);
                                                    }
                                                }
                                                crate::message::MessageType::Control => {
                                                    // Update statistics for control messages
                                                    let mut sg = stats.lock().await;
                                                    sg.messages_received = sg.messages_received.saturating_add(1);
                                                    drop(sg);
                                                    debug!("Received control message for client: {}", client_id);
                                                    // Deliver to opted-in subscribers.
                                                    // Ignore errors — delivery is best-effort and
                                                    // legacy subscribers simply do not opt in.
                                                    if let Err(e) = Self::handle_control_message_internal(
                                                        &subscriptions,
                                                        &message,
                                                        &client_id,
                                                    )
                                                    .await
                                                    {
                                                        debug!(
                                                            "Error delivering control message for client {client_id}: {e}"
                                                        );
                                                    }
                                                }
                                                crate::message::MessageType::Heartbeat => {
                                                    // Update statistics for heartbeat messages
                                                    let mut sg = stats.lock().await;
                                                    sg.messages_received = sg.messages_received.saturating_add(1);
                                                    drop(sg);
                                                    debug!("Received heartbeat for client: {}", client_id);
                                                }
                                                crate::message::MessageType::Shutdown => {
                                                    // Update statistics before handling shutdown (to count shutdown messages)
                                                    let mut sg = stats.lock().await;
                                                    sg.messages_received = sg.messages_received.saturating_add(1);
                                                    drop(sg);
                                                    info!("Received shutdown message for client: {}", client_id);
                                                    break;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            debug!("Failed to deserialize message for client {}: {}", client_id, e);
                                        }
                                    }
                                }
                                Ok(Err(EventBusError::Transport(msg))) if msg.contains("Server shutdown") => {
                                    info!("Server shutdown detected for client: {}", client_id);
                                    break;
                                }
                                Ok(Err(e)) => {
                                    // Log error but continue processing
                                    debug!("Error receiving message for client {}: {}", client_id, e);
                                    // Yield to avoid busy looping on errors
                                    tokio::task::yield_now().await;
                                }
                                Err(_) => {
                                    // Timeout - no message available, continue loop
                                }
                            }
                        }
                        _ = shutdown_rx_msg.recv() => {
                            info!("Message processing task shutting down for client: {}", client_id);
                            break;
                        }
                    }
                }
            })
        };

        self.task_handles.push(health_task);
        self.task_handles.push(message_task);
        Ok(())
    }

    /// Publish a message to a topic with backpressure control
    pub async fn publish(
        &self,
        topic: &str,
        event: CollectionEvent,
        correlation_id: Option<String>,
    ) -> Result<()> {
        // Validate topic
        let _topic_obj = Topic::new(topic)
            .map_err(|e| EventBusError::topic(format!("Invalid topic '{topic}': {e}")))?;

        let correlation = correlation_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        // Get next sequence number
        let sequence = {
            let mut seq_guard = self.sequence.lock().await;
            let seq = *seq_guard;
            *seq_guard = seq_guard.saturating_add(1);
            seq
        };

        // Serialize event
        let payload = postcard::to_allocvec(&event)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Validate payload size
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::serialization(
                "Payload exceeds 1MB limit".to_owned(),
            ));
        }

        // Create message
        let message = Message::event(topic.to_owned(), correlation, payload, sequence);
        let message_data = message.to_bytes()?;

        // Send message — transport.send() acquires its own backpressure permit internally
        let send_result = self.transport.lock().await.send(&message_data).await;
        send_result?;

        // Update statistics
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_published = stats_guard.messages_published.saturating_add(1);
        };

        debug!(
            "Published message to topic: {} (correlation: {})",
            topic, message.correlation_metadata.correlation_id
        );
        Ok(())
    }

    /// Publish a raw control message to a topic.
    ///
    /// Unlike [`publish`](Self::publish), this method accepts pre-serialized
    /// payload bytes and sends them as a `Control` message type, which is
    /// routed through the broker's topic matching to all matching subscribers.
    ///
    /// This is used for RPC responses, heartbeats, and other control-plane
    /// messages that are not `CollectionEvent` types.
    pub async fn publish_control(&self, topic: &str, payload: Vec<u8>) -> Result<()> {
        // Validate topic
        let _topic_obj = Topic::new(topic)
            .map_err(|e| EventBusError::topic(format!("Invalid topic '{topic}': {e}")))?;

        // Validate payload size
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_owned(),
            ));
        }

        let correlation_id = Uuid::new_v4().to_string();

        // Get next sequence number
        let sequence = {
            let mut seq_guard = self.sequence.lock().await;
            let seq = *seq_guard;
            *seq_guard = seq_guard.saturating_add(1);
            seq
        };

        // Create control message for topic-based routing
        let message = Message::control(topic.to_owned(), correlation_id, payload, sequence);
        let message_data = message.to_bytes()?;

        // Send message — transport.send() acquires its own backpressure permit internally
        let send_result = self.transport.lock().await.send(&message_data).await;
        send_result?;

        debug!("Published control message to topic: {topic}");
        Ok(())
    }

    /// Send a direct one-to-one message to a specific client
    pub async fn send_direct(&self, client_id: &str, payload: &[u8]) -> Result<()> {
        // Validate payload
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_owned(),
            ));
        }

        // Route via broker control topic
        let topic = format!("control.direct.{client_id}");
        let correlation_id = Uuid::new_v4().to_string();

        // Serialize message for broker routing
        let message =
            crate::message::Message::control(topic.clone(), correlation_id, payload.to_vec(), 0);
        let message_data = message.to_bytes()?;

        // Send message — transport.send() acquires its own backpressure permit internally
        let send_result = self.transport.lock().await.send(&message_data).await;
        send_result?;

        debug!("Sent direct message to client: {} via broker", client_id);
        Ok(())
    }

    /// Enqueue a message for a client (queue support)
    pub async fn enqueue_message(
        &self,
        client_id: &str,
        topic: &str,
        payload: &[u8],
    ) -> Result<()> {
        // Validate payload
        if payload.len() > 1024 * 1024 {
            return Err(EventBusError::transport(
                "Payload exceeds 1MB limit".to_owned(),
            ));
        }

        // Route via broker queue topic: queue.{client_id}.{topic}
        let queue_topic = format!("queue.{client_id}.{topic}");
        let correlation_id = Uuid::new_v4().to_string();

        // Serialize message for broker routing
        let message = crate::message::Message::control(
            queue_topic.clone(),
            correlation_id,
            payload.to_vec(),
            0,
        );
        let message_data = message.to_bytes()?;

        // Send message — transport.send() acquires its own backpressure permit internally
        let send_result = self.transport.lock().await.send(&message_data).await;
        send_result?;

        debug!(
            "Enqueued message for client: {} on topic: {} via broker",
            client_id, topic
        );
        Ok(())
    }

    /// Subscribe to topic patterns with backpressure support
    ///
    /// Delivers only [`MessageType::Event`] envelopes. To also receive
    /// [`MessageType::Control`] envelopes use
    /// [`subscribe_with_control`](Self::subscribe_with_control) with
    /// [`EventSubscription::include_control`] set to `true`.
    pub async fn subscribe(
        &self,
        subscription: EventSubscription,
    ) -> Result<mpsc::Receiver<BusEvent>> {
        let subscription_id = subscription.subscriber_id.clone();
        let patterns = subscription.topic_patterns.clone().unwrap_or_default();

        // Validate topic patterns
        for pattern in &patterns {
            TopicPattern::new(pattern).map_err(|e| {
                EventBusError::topic(format!("Invalid topic pattern '{pattern}': {e}"))
            })?;
        }

        // Create bounded channel for backpressure (default: 1000 messages)
        let (tx, rx) = mpsc::channel(1000);

        // Store subscription info
        let subscription_info = SubscriptionInfo {
            patterns: patterns.clone(),
            sender: tx,
            control_sender: None,
            created_at: Instant::now(),
            last_message: None,
        };

        self.subscriptions
            .write()
            .await
            .insert(subscription_id.clone(), subscription_info);

        // Update statistics
        let sub_count = self.subscriptions.read().await.len();
        self.stats.lock().await.active_subscriptions = sub_count;

        info!(
            "Subscribed to patterns: {:?} (subscription: {})",
            patterns, subscription_id
        );
        Ok(rx)
    }

    /// Subscribe to topic patterns and opt into [`MessageType::Control`] delivery.
    ///
    /// Returns a tuple `(event_rx, control_rx)` of parallel bounded receivers:
    /// - `event_rx` carries [`BusEvent`] envelopes decoded from `MessageType::Event`
    ///   messages, matching the default behavior of [`subscribe`](Self::subscribe).
    /// - `control_rx` carries raw [`Message`] envelopes for `MessageType::Control`
    ///   messages whose topic matches one of the subscription's topic patterns.
    ///
    /// If [`EventSubscription::include_control`] is `false`, the returned
    /// `control_rx` is closed immediately and receives no messages — callers
    /// who do not opt in should use [`subscribe`](Self::subscribe) instead.
    ///
    /// Both receivers use bounded 1000-slot channels for backpressure. Full
    /// channels drop messages with a `warn!` log rather than blocking the
    /// background message-processing task.
    pub async fn subscribe_with_control(
        &self,
        subscription: EventSubscription,
    ) -> Result<(mpsc::Receiver<BusEvent>, mpsc::Receiver<Message>)> {
        let subscription_id = subscription.subscriber_id.clone();
        let patterns = subscription.topic_patterns.clone().unwrap_or_default();
        let include_control = subscription.include_control;

        // Validate topic patterns
        for pattern in &patterns {
            TopicPattern::new(pattern).map_err(|e| {
                EventBusError::topic(format!("Invalid topic pattern '{pattern}': {e}"))
            })?;
        }

        // Create bounded channels for backpressure (default: 1000 messages)
        let (event_tx, event_rx) = mpsc::channel(1000);
        let (control_tx, control_rx) = mpsc::channel(1000);

        // Store subscription info
        let subscription_info = SubscriptionInfo {
            patterns: patterns.clone(),
            sender: event_tx,
            control_sender: if include_control {
                Some(control_tx)
            } else {
                // Drop the control sender so the paired receiver closes
                // immediately — legacy callers that did not opt in observe
                // exactly the same behavior as `subscribe()`.
                drop(control_tx);
                None
            },
            created_at: Instant::now(),
            last_message: None,
        };

        self.subscriptions
            .write()
            .await
            .insert(subscription_id.clone(), subscription_info);

        // Update statistics
        let sub_count = self.subscriptions.read().await.len();
        self.stats.lock().await.active_subscriptions = sub_count;

        // debug! (not info!): subscription topology including per-collector IDs
        // should not appear in default-level logs that may ship to less-trusted
        // SIEM pipelines (END-297 review SEC-004).
        debug!(
            "Subscribed (control={include_control}) to patterns: {patterns:?} (subscription: {subscription_id})"
        );
        Ok((event_rx, control_rx))
    }

    /// Unsubscribe from topics
    pub async fn unsubscribe(&self, subscription_id: &str) -> Result<()> {
        let removed = {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.remove(subscription_id).is_some()
        };

        if removed {
            // Update statistics
            let sub_count = self.subscriptions.read().await.len();
            self.stats.lock().await.active_subscriptions = sub_count;

            info!("Unsubscribed: {}", subscription_id);
            Ok(())
        } else {
            Err(EventBusError::topic(format!(
                "Subscription not found: {subscription_id}"
            )))
        }
    }

    /// Process incoming messages (should be called in a loop)
    pub async fn process_messages(&self) -> Result<()> {
        let message_data = {
            let mut transport = self.transport.lock().await;
            transport.receive().await?
        };

        // Deserialize message
        let message = Message::from_bytes(&message_data)?;

        // Update statistics before handling message (to count shutdown messages)
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.messages_received = stats_guard.messages_received.saturating_add(1);
        };

        // Handle different message types
        match message.message_type {
            MessageType::Event => {
                self.handle_event_message(message).await?;
            }
            MessageType::Control => {
                self.handle_control_message(message).await?;
            }
            MessageType::Heartbeat => {
                debug!("Received heartbeat message");
            }
            MessageType::Shutdown => {
                info!("Received shutdown message");
                return Err(EventBusError::transport("Server shutdown"));
            }
        }

        Ok(())
    }

    /// Handle event messages (internal helper for background task)
    async fn handle_event_message_internal(
        subscriptions: &Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
        stats: &Arc<Mutex<ClientStats>>,
        message: &Message,
        _client_id: &str,
    ) -> Result<()> {
        // Deserialize event
        let event: CollectionEvent = postcard::from_bytes(&message.payload)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Create bus event
        let bus_event = BusEvent {
            event_id: message.id.to_string(),
            event,
            correlation_metadata: message.correlation_metadata.clone(),
            bus_timestamp: message.timestamp,
            matched_pattern: message.topic.clone(),
            subscriber_id: String::new(), // Will be set per subscription
        };

        // Find matching subscriptions
        let subscriptions_guard = subscriptions.read().await;
        let mut delivered_count = 0_usize;

        for (subscription_id, subscription_info) in subscriptions_guard.iter() {
            // Check if any pattern matches the topic
            let matches =
                Self::subscription_matches_topic(&subscription_info.patterns, &message.topic);

            if matches {
                let mut event_copy = bus_event.clone();
                event_copy.subscriber_id.clone_from(subscription_id);

                match subscription_info.sender.try_send(event_copy) {
                    Ok(()) => {
                        delivered_count = delivered_count.saturating_add(1);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            "Subscription {} queue full, message dropped",
                            subscription_id
                        );
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        warn!(
                            "Subscription {} closed, removing from active subscriptions",
                            subscription_id
                        );
                    }
                }
            }
        }
        drop(subscriptions_guard);

        // Update statistics
        {
            let mut stats_guard = stats.lock().await;
            stats_guard.messages_received = stats_guard.messages_received.saturating_add(1);
        };

        debug!("Delivered message to {} subscriptions", delivered_count);
        Ok(())
    }

    /// Return true when any of the subscription's topic patterns matches the
    /// given topic string. Parsing errors on either side yield a non-match
    /// (failing closed) rather than panicking.
    fn subscription_matches_topic(patterns: &[String], topic: &str) -> bool {
        patterns.iter().any(|pattern| {
            TopicPattern::new(pattern).is_ok_and(|topic_pattern| {
                Topic::new(topic).is_ok_and(|topic_obj| topic_pattern.matches(&topic_obj))
            })
        })
    }

    /// Handle event messages
    async fn handle_event_message(&self, message: Message) -> Result<()> {
        Self::handle_event_message_internal(
            &self.subscriptions,
            &self.stats,
            &message,
            &self.client_id,
        )
        .await
    }

    /// Handle control messages (synchronous entry point).
    ///
    /// Delegates to [`handle_control_message_internal`](Self::handle_control_message_internal)
    /// so the background task and public entry points share delivery logic.
    async fn handle_control_message(&self, message: Message) -> Result<()> {
        debug!("Received control message: {}", message.topic);
        Self::handle_control_message_internal(&self.subscriptions, &message, &self.client_id).await
    }

    /// Deliver a [`MessageType::Control`] message to matching opted-in
    /// subscribers via their control channel.
    ///
    /// Subscriptions with `control_sender = None` are skipped — that is the
    /// legacy path where Control messages are silently dropped to preserve
    /// source compatibility for callers that only use [`subscribe`](Self::subscribe).
    ///
    /// A full control channel logs a `warn!` and drops the message rather
    /// than blocking the background receive task. A closed control channel
    /// is logged at `warn!` and the stale subscription is not auto-removed
    /// here (unsubscribe is the caller's responsibility).
    async fn handle_control_message_internal(
        subscriptions: &Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
        message: &Message,
        _client_id: &str,
    ) -> Result<()> {
        // Guard: only deliver Control envelopes through this path.
        if message.message_type != MessageType::Control {
            return Ok(());
        }

        let subscriptions_guard = subscriptions.read().await;
        let mut delivered_count = 0_usize;

        for (subscription_id, subscription_info) in subscriptions_guard.iter() {
            // Skip subscribers that did not opt into Control delivery.
            let Some(ref control_sender) = subscription_info.control_sender else {
                continue;
            };

            // Match the incoming topic against the subscriber's patterns.
            if !Self::subscription_matches_topic(&subscription_info.patterns, &message.topic) {
                continue;
            }

            match control_sender.try_send(message.clone()) {
                Ok(()) => {
                    delivered_count = delivered_count.saturating_add(1);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        "Subscription {subscription_id} control queue full, control message dropped"
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    warn!(
                        "Subscription {subscription_id} control channel closed; control message not delivered"
                    );
                }
            }
        }
        drop(subscriptions_guard);

        debug!("Delivered control message to {delivered_count} subscriptions");
        Ok(())
    }

    /// Reconnect to the broker
    pub async fn reconnect(&self) -> Result<()> {
        info!("Reconnecting client: {}", self.client_id);

        let reconnect_result = self.transport.lock().await.reconnect().await;
        reconnect_result?;

        // Update statistics
        {
            let mut stats_guard = self.stats.lock().await;
            stats_guard.reconnection_attempts = stats_guard.reconnection_attempts.saturating_add(1);
            stats_guard.last_connected = Some(Instant::now());
        };

        info!("Successfully reconnected client: {}", self.client_id);
        Ok(())
    }

    /// Get client statistics
    pub async fn get_stats(&self) -> ClientStats {
        let mut stats = self.stats.lock().await;

        // Update uptime
        if let Some(last_connected) = stats.last_connected {
            stats.uptime = last_connected.elapsed();
        }

        // Update subscription count
        let sub_count = self.subscriptions.read().await.len();
        stats.active_subscriptions = sub_count;

        stats.clone()
    }

    /// Get client ID
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Check if client is connected
    pub async fn is_connected(&self) -> bool {
        let mut transport = self.transport.lock().await;
        transport.is_alive().await
    }

    /// Signal background tasks to shut down without consuming the client.
    ///
    /// Sends on the internal broadcast channel so background receive/heartbeat
    /// tasks exit at their next loop iteration. Safe to call when the client
    /// is shared via `Arc` — the broadcast `Sender::send(&self)` signature does
    /// not require ownership.
    ///
    /// Returns `true` if the signal was delivered. Returns `false` only when
    /// no live receivers are subscribed to the broadcast channel — for example
    /// because background tasks have already exited and closed their receivers.
    /// (The "already taken by a consuming `shutdown()` call" case cannot be
    /// observed here: this method takes `&self`, so any caller holding such a
    /// reference proves that `shutdown(self)` has not yet consumed the client.)
    ///
    /// Callers that also want to await background-task completion should use
    /// the consuming [`shutdown`](Self::shutdown) method after this signal.
    pub fn shutdown_signal(&self) -> bool {
        self.shutdown_tx
            .as_ref()
            .is_some_and(|tx| tx.send(()).is_ok())
    }

    /// Shutdown the client
    pub async fn shutdown(mut self) -> Result<()> {
        info!("Shutting down EventBus client: {}", self.client_id);

        // Send shutdown signal to background tasks
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            drop(shutdown_tx.send(()));
        }

        // Wait for background tasks to complete
        for handle in self.task_handles {
            drop(handle.await);
        }

        // Close transport connection
        {
            let _transport = self.transport.lock().await;
            // Transport will be closed when dropped
        }

        info!("EventBus client shutdown complete: {}", self.client_id);
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]
mod tests {
    use super::*;
    use crate::message::ProcessEvent;
    use crate::transport::TransportServer;
    use std::time::SystemTime;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_client_creation() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-client.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let result = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await;

        // Should succeed with real server
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_topic_validation() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-validation.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        // Test invalid topic
        let event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test".to_string(),
            command_line: None,
            executable_path: None,
            ppid: None,
            start_time: Some(SystemTime::now()),
            metadata: std::collections::HashMap::new(),
        });

        let result = client.publish("invalid..topic", event, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_subscription_management() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-subscription.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        // Start a server first
        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        // Test subscription with invalid pattern
        let subscription = EventSubscription {
            subscriber_id: "test-sub".to_string(),
            capabilities: crate::message::SourceCaps {
                event_types: vec!["process".to_string()],
                collectors: vec![],
                max_priority: 5,
            },
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.#.invalid".to_string()]),
            enable_wildcards: true,
            include_control: false,
        };

        let result = client.subscribe(subscription).await;
        assert!(result.is_err());
    }

    /// Build a `SubscriptionInfo` with the given topic patterns and optional
    /// control sender. Used by the control-delivery unit tests below to
    /// exercise `handle_control_message_internal` without wiring a full
    /// transport stack.
    fn make_subscription_info(
        patterns: Vec<String>,
        control_sender: Option<mpsc::Sender<Message>>,
    ) -> (SubscriptionInfo, mpsc::Receiver<BusEvent>) {
        let (event_tx, event_rx) = mpsc::channel::<BusEvent>(1000);
        let info = SubscriptionInfo {
            patterns,
            sender: event_tx,
            control_sender,
            created_at: Instant::now(),
            last_message: None,
        };
        (info, event_rx)
    }

    /// Happy path (Unit 1): A subscriber that opts into Control delivery
    /// receives Control messages on matching topics with correlation metadata
    /// intact.
    #[tokio::test]
    async fn test_control_message_delivered_when_opted_in() {
        let subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Set up a subscriber that opted in to Control delivery.
        let (control_tx, mut control_rx) = mpsc::channel::<Message>(10);
        let (info, _event_rx) = make_subscription_info(
            vec!["control.collector.lifecycle".to_string()],
            Some(control_tx),
        );
        subscriptions
            .write()
            .await
            .insert("test-sub".to_string(), info);

        // Build a Control message on the subscribed topic with known correlation.
        let message = Message::control(
            "control.collector.lifecycle".to_string(),
            "corr-xyz".to_string(),
            b"{\"type\":\"BeginMonitoring\"}".to_vec(),
            42,
        );

        EventBusClient::handle_control_message_internal(&subscriptions, &message, "test-client")
            .await
            .expect("delivery should succeed");

        let delivered = control_rx.recv().await.expect("should receive control msg");
        assert_eq!(delivered.topic, "control.collector.lifecycle");
        assert_eq!(delivered.correlation_metadata.correlation_id, "corr-xyz");
        assert_eq!(delivered.message_type, MessageType::Control);
    }

    /// Edge case (Unit 1): A subscriber that did NOT opt into Control delivery
    /// does not receive Control messages. Legacy Event-only subscribers are
    /// unaffected by the new field.
    #[tokio::test]
    async fn test_control_message_not_delivered_when_not_opted_in() {
        let subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Set up a subscriber with NO control_sender (legacy default).
        let (info, mut event_rx) =
            make_subscription_info(vec!["control.collector.lifecycle".to_string()], None);
        subscriptions
            .write()
            .await
            .insert("legacy-sub".to_string(), info);

        let message = Message::control(
            "control.collector.lifecycle".to_string(),
            "corr-legacy".to_string(),
            Vec::new(),
            7,
        );

        // Should not error; legacy subscriber is simply skipped.
        EventBusClient::handle_control_message_internal(&subscriptions, &message, "test-client")
            .await
            .expect("handler tolerates legacy subscribers");

        // Strengthened assertion (END-297 review T-004): verify the Control
        // envelope did not leak onto the Event channel. `try_recv` returning
        // `TryRecvError::Empty` confirms the channel is open but no message
        // was delivered — a bug that routed Control to the event path would
        // instead have produced `Ok(_)` with a delivered message.
        assert!(
            matches!(
                event_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "Control message must not leak onto the Event channel for opt-out subscribers"
        );
    }

    /// Edge case (Unit 1): A Control message on a topic that does not match
    /// the subscriber's patterns is not delivered even if the subscriber
    /// opted into Control delivery.
    #[tokio::test]
    async fn test_control_message_topic_filter_still_applies_when_opted_in() {
        let subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Subscriber opts in but only for a DIFFERENT topic pattern.
        let (control_tx, mut control_rx) = mpsc::channel::<Message>(10);
        let (info, _event_rx) =
            make_subscription_info(vec!["events.process.+".to_string()], Some(control_tx));
        subscriptions
            .write()
            .await
            .insert("events-only-sub".to_string(), info);

        // Publish a Control message on a non-matching topic.
        let message = Message::control(
            "control.collector.lifecycle".to_string(),
            "corr-noisy".to_string(),
            Vec::new(),
            5,
        );

        EventBusClient::handle_control_message_internal(&subscriptions, &message, "test-client")
            .await
            .unwrap();

        // Receiver should have nothing — the topic filter gated the delivery.
        assert!(
            control_rx.try_recv().is_err(),
            "control msg on unrelated topic must not leak through topic filter"
        );
    }

    /// Edge case (Unit 1): `handle_control_message_internal` is a no-op for
    /// Event-type messages. This guards against future refactors that might
    /// accidentally duplicate Event delivery via the control path.
    #[tokio::test]
    async fn test_control_handler_ignores_event_messages() {
        let subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let (control_tx, mut control_rx) = mpsc::channel::<Message>(10);
        let (info, _event_rx) =
            make_subscription_info(vec!["events.process.+".to_string()], Some(control_tx));
        subscriptions
            .write()
            .await
            .insert("guard-sub".to_string(), info);

        // Event-typed message — should NEVER flow through control channel.
        let message = Message::event(
            "events.process.new".to_string(),
            "corr-event".to_string(),
            Vec::new(),
            1,
        );

        EventBusClient::handle_control_message_internal(&subscriptions, &message, "test-client")
            .await
            .unwrap();

        assert!(
            control_rx.try_recv().is_err(),
            "control handler must not forward Event-type messages"
        );
    }

    /// Integration-style (Unit 1): `subscribe_with_control` wires the
    /// returned `control_rx` to the per-subscription `control_sender` so the
    /// background task's delivery path reaches it.
    #[tokio::test]
    async fn test_subscribe_with_control_round_trip_when_opted_in() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-control-opt-in.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        let subscription = EventSubscription {
            subscriber_id: "opt-in-sub".to_string(),
            capabilities: crate::message::SourceCaps::default(),
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["control.collector.lifecycle".to_string()]),
            enable_wildcards: true,
            include_control: true,
        };

        let (_events_rx, mut control_rx) = client
            .subscribe_with_control(subscription)
            .await
            .expect("subscribe_with_control should succeed");

        // Simulate an inbound Control message by directly invoking the
        // same entry point the background task uses.
        let message = Message::control(
            "control.collector.lifecycle".to_string(),
            "round-trip-corr".to_string(),
            b"BeginMonitoring".to_vec(),
            99,
        );
        client.handle_control_message(message).await.unwrap();

        let delivered = control_rx.recv().await.expect("should receive msg");
        assert_eq!(
            delivered.correlation_metadata.correlation_id,
            "round-trip-corr"
        );
    }

    /// Integration-style (Unit 1): `subscribe_with_control` with
    /// `include_control=false` returns a closed control receiver and never
    /// delivers Control messages — matches the behavior of `subscribe()`.
    #[tokio::test]
    async fn test_subscribe_with_control_closed_channel_when_not_opted_in() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-control-legacy.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let _server = TransportServer::new(socket_config.clone()).await.unwrap();

        let client = EventBusClient::new(
            "test-client".to_string(),
            socket_config,
            ClientConfig::default(),
        )
        .await
        .unwrap();

        let subscription = EventSubscription {
            subscriber_id: "no-opt-sub".to_string(),
            capabilities: crate::message::SourceCaps::default(),
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["control.collector.lifecycle".to_string()]),
            enable_wildcards: true,
            include_control: false,
        };

        let (_events_rx, mut control_rx) = client
            .subscribe_with_control(subscription)
            .await
            .expect("subscribe_with_control should still succeed for legacy");

        // Fire a control message — should NOT land on the channel.
        let message = Message::control(
            "control.collector.lifecycle".to_string(),
            "legacy-corr".to_string(),
            Vec::new(),
            1,
        );
        client.handle_control_message(message).await.unwrap();

        // The channel must be CLOSED (not just empty) — subscribe_with_control
        // drops the paired sender when include_control=false. A blocking `recv()`
        // on a closed channel returns None promptly, which is the stronger
        // assertion the opt-out contract requires (PR #178 review).
        // `Message` doesn't impl PartialEq, so pattern-match instead of assert_eq!.
        let recv_result =
            tokio::time::timeout(std::time::Duration::from_millis(100), control_rx.recv()).await;
        assert!(
            matches!(recv_result, Ok(None)),
            "control channel must be closed (not just empty) for non-opted-in subscriber; got {recv_result:?}"
        );
    }
}
