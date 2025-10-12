//! Core busrt client implementation.

use crate::busrt_types::{BrokerStats, BusrtClient, BusrtError, BusrtEvent, TransportConfig};
use async_trait::async_trait;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, info, instrument, warn};

use super::{
    connection::ConnectionState,
    statistics::ClientStatistics,
    subscription::SubscriptionManager,
    types::{OutboundMessage, OutboundMessageType, ReconnectionConfig},
};

/// Concrete implementation of BusrtClient for collector-core integration.
///
/// This client provides connection management, automatic reconnection,
/// topic subscription management, and message publishing capabilities
/// for communication with busrt message brokers.
///
/// # Features
///
/// - **Automatic Reconnection**: Exponential backoff reconnection on failures
/// - **Topic Management**: Subscription tracking and pattern matching
/// - **Message Queuing**: Buffered message handling with backpressure
/// - **Health Monitoring**: Connection health checks and statistics
/// - **Error Recovery**: Graceful error handling and recovery strategies
/// - **Multi-Transport**: Support for Unix sockets, TCP, and named pipes
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::{CollectorBusrtClient, TransportConfig, TransportType};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let transport = TransportConfig {
///         transport_type: TransportType::UnixSocket,
///         path: Some("/tmp/daemoneye-broker.sock".to_string()),
///         address: None,
///         port: None,
///     };
///
///     let mut client = CollectorBusrtClient::new(transport).await?;
///     client.connect().await?;
///
///     client.disconnect().await?;
///     Ok(())
/// }
/// ```
pub struct CollectorBusrtClient {
    /// Transport configuration
    transport_config: TransportConfig,
    /// Connection state
    connection_state: Arc<ConnectionState>,
    /// Subscription management
    subscription_manager: Arc<SubscriptionManager>,
    /// Message statistics
    statistics: Arc<ClientStatistics>,
    /// Background task handles
    connection_handle: Option<JoinHandle<Result<(), BusrtError>>>,
    heartbeat_handle: Option<JoinHandle<Result<(), BusrtError>>>,
    message_handler: Option<JoinHandle<Result<(), BusrtError>>>,
    /// Shutdown coordination
    shutdown_signal: Arc<AtomicBool>,
    /// Connection timeout configuration
    connection_timeout: Duration,
    /// Reconnection configuration
    reconnection_config: ReconnectionConfig,
}

impl CollectorBusrtClient {
    /// Creates a new busrt client with the specified transport configuration.
    pub async fn new(transport_config: TransportConfig) -> Result<Self, BusrtError> {
        Ok(Self {
            transport_config,
            connection_state: Arc::new(ConnectionState::new()),
            subscription_manager: Arc::new(SubscriptionManager::new()),
            statistics: Arc::new(ClientStatistics::new()),
            connection_handle: None,
            heartbeat_handle: None,
            message_handler: None,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            connection_timeout: Duration::from_secs(10),
            reconnection_config: ReconnectionConfig::default(),
        })
    }

    /// Connects to the busrt broker.
    #[instrument(skip(self))]
    pub async fn connect(&mut self) -> Result<(), BusrtError> {
        info!("Connecting to busrt broker");

        // Create oneshot channel to signal connection readiness
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

        // Start connection management task with readiness notification
        self.start_connection_management(ready_tx).await?;

        // Start heartbeat task
        self.start_heartbeat_task().await?;

        // Start message handling task
        self.start_message_handling().await?;

        // Wait for initial connection or error
        ready_rx.await.map_err(|_| {
            BusrtError::ConnectionFailed(
                "Connection task terminated before establishing connection".to_string(),
            )
        })??;

        info!("Busrt client connected successfully");
        Ok(())
    }

    /// Disconnects from the busrt broker.
    #[instrument(skip(self))]
    pub async fn disconnect(&mut self) -> Result<(), BusrtError> {
        info!("Disconnecting from busrt broker");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for background tasks to complete
        if let Some(handle) = self.connection_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = ?e, "Connection task join error");
            }
        }

        if let Some(handle) = self.heartbeat_handle.take() {
            if let Err(e) = handle.await {
                warn!(error = ?e, "Heartbeat task join error");
            }
        }

        if let Some(handle) = self.message_handler.take() {
            if let Err(e) = handle.await {
                warn!(error = ?e, "Message handler task join error");
            }
        }

        // Reset shutdown signal to allow reconnection
        self.shutdown_signal.store(false, Ordering::Relaxed);

        // Update connection state
        self.connection_state
            .connected
            .store(false, Ordering::Relaxed);
        {
            let mut connected_at = self.connection_state.connected_at.write().await;
            *connected_at = None;
        }

        info!("Busrt client disconnected");
        Ok(())
    }

    /// Starts the connection management background task.
    async fn start_connection_management(
        &mut self,
        ready_tx: tokio::sync::oneshot::Sender<Result<(), BusrtError>>,
    ) -> Result<(), BusrtError> {
        let transport_config = self.transport_config.clone();
        let connection_state = Arc::clone(&self.connection_state);
        let subscription_manager = Arc::clone(&self.subscription_manager);
        let statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let reconnection_config = self.reconnection_config.clone();
        let connection_timeout = self.connection_timeout;

        let handle = tokio::spawn(async move {
            let mut reconnection_delay = reconnection_config.initial_delay;
            let mut attempt_count = 0;
            let mut ready_tx = Some(ready_tx);

            while !shutdown_signal.load(Ordering::Relaxed) {
                // Attempt connection
                match ConnectionState::establish_connection(&transport_config, connection_timeout)
                    .await
                {
                    Ok((outbound_tx, inbound_rx)) => {
                        info!("Connection established to busrt broker");

                        // Update connection state
                        connection_state.connected.store(true, Ordering::Relaxed);
                        {
                            let mut connected_at = connection_state.connected_at.write().await;
                            *connected_at = Some(std::time::SystemTime::now());
                        }

                        // Store outbound sender
                        {
                            let mut sender = connection_state.outbound_sender.lock().await;
                            *sender = Some(outbound_tx);
                        }

                        // Notify initial connection success
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Ok(()));
                        }

                        // Reset reconnection parameters
                        reconnection_delay = reconnection_config.initial_delay;
                        attempt_count = 0;

                        // Handle connection until it fails or shutdown is signaled
                        if let Err(e) = ConnectionState::handle_connection(
                            inbound_rx,
                            &connection_state,
                            &subscription_manager,
                            &statistics,
                            &shutdown_signal,
                        )
                        .await
                        {
                            warn!(error = ?e, "Connection handling failed");
                        }

                        // Connection lost
                        connection_state.connected.store(false, Ordering::Relaxed);
                        statistics
                            .connection_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        statistics
                            .connection_failures
                            .fetch_add(1, Ordering::Relaxed);

                        // Notify initial connection failure
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                }

                // Handle reconnection if enabled
                if reconnection_config.enabled && !shutdown_signal.load(Ordering::Relaxed) {
                    attempt_count += 1;

                    if reconnection_config.max_attempts > 0
                        && attempt_count >= reconnection_config.max_attempts
                    {
                        break;
                    }

                    statistics
                        .reconnection_attempts
                        .fetch_add(1, Ordering::Relaxed);

                    debug!(
                        delay_ms = reconnection_delay.as_millis(),
                        attempt = attempt_count,
                        "Waiting before reconnection attempt"
                    );

                    tokio::time::sleep(reconnection_delay).await;

                    // Exponential backoff
                    reconnection_delay = std::cmp::min(
                        Duration::from_millis(
                            (reconnection_delay.as_millis() as f64
                                * reconnection_config.backoff_multiplier)
                                as u64,
                        ),
                        reconnection_config.max_delay,
                    );
                } else {
                    break;
                }
            }

            Ok(())
        });

        self.connection_handle = Some(handle);
        Ok(())
    }

    /// Starts the heartbeat background task.
    async fn start_heartbeat_task(&mut self) -> Result<(), BusrtError> {
        let connection_state = Arc::clone(&self.connection_state);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let handle = tokio::spawn(async move {
            let mut heartbeat_timer = tokio::time::interval(Duration::from_secs(30));

            while !shutdown_signal.load(Ordering::Relaxed) {
                heartbeat_timer.tick().await;

                if connection_state.connected.load(Ordering::Relaxed) {
                    // Send heartbeat message
                    let heartbeat_msg = OutboundMessage {
                        message_type: OutboundMessageType::Heartbeat,
                        payload: serde_json::json!({
                            "timestamp": std::time::SystemTime::now()
                                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64
                        }),
                        response_channel: None,
                    };

                    if let Some(sender) = connection_state.outbound_sender.lock().await.as_ref() {
                        if let Err(e) = sender.try_send(heartbeat_msg) {
                            warn!(error = %e, "Failed to send heartbeat");
                        } else {
                            debug!("Heartbeat sent to broker");
                        }
                    }
                }
            }

            Ok(())
        });

        self.heartbeat_handle = Some(handle);
        Ok(())
    }

    /// Starts the message handling background task.
    async fn start_message_handling(&mut self) -> Result<(), BusrtError> {
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let handle = tokio::spawn(async move {
            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            Ok(())
        });

        self.message_handler = Some(handle);
        Ok(())
    }

    /// Returns client statistics.
    pub async fn get_statistics(&self) -> ClientStatistics {
        self.statistics.snapshot().await
    }

    /// Checks if the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connection_state.connected.load(Ordering::Relaxed)
    }

    /// Updates reconnection configuration.
    pub fn set_reconnection_config(&mut self, config: ReconnectionConfig) {
        self.reconnection_config = config;
    }

    /// Updates connection timeout.
    pub fn set_connection_timeout(&mut self, timeout: Duration) {
        self.connection_timeout = timeout;
    }
}

#[async_trait]
impl BusrtClient for CollectorBusrtClient {
    #[instrument(skip_all, fields(topic = %topic, event_id = %message.event_id))]
    async fn publish(&self, topic: &str, message: BusrtEvent) -> Result<(), BusrtError> {
        if !self.is_connected() {
            return Err(BusrtError::BrokerUnavailable(
                "Client not connected".to_string(),
            ));
        }

        let outbound_msg = OutboundMessage {
            message_type: OutboundMessageType::Publish {
                topic: topic.to_string(),
                event: Box::new(message),
            },
            payload: serde_json::json!({}),
            response_channel: None,
        };

        if let Some(sender) = self.connection_state.outbound_sender.lock().await.as_ref() {
            sender.send(outbound_msg).await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to queue message: {}", e))
            })?;

            self.statistics
                .messages_published
                .fetch_add(1, Ordering::Relaxed);

            debug!(topic = %topic, "Message published to broker");
            Ok(())
        } else {
            Err(BusrtError::BrokerUnavailable(
                "Outbound channel not available".to_string(),
            ))
        }
    }

    #[instrument(skip_all, fields(pattern = %pattern))]
    async fn subscribe(&self, pattern: &str) -> Result<mpsc::Receiver<BusrtEvent>, BusrtError> {
        if !self.is_connected() {
            return Err(BusrtError::BrokerUnavailable(
                "Client not connected".to_string(),
            ));
        }

        let (sender, receiver) = mpsc::channel(1000);

        // Store subscription
        {
            let mut subscriptions = self.subscription_manager.subscriptions.write().await;
            subscriptions.insert(pattern.to_string(), sender);
        }

        // Store subscription metadata
        {
            let mut metadata = self
                .subscription_manager
                .subscription_metadata
                .write()
                .await;
            metadata.insert(
                pattern.to_string(),
                super::subscription::SubscriptionMetadata {
                    subscribed_at: std::time::SystemTime::now(),
                    messages_received: std::sync::atomic::AtomicU64::new(0),
                    last_message: tokio::sync::RwLock::new(None),
                },
            );
        }

        // Send subscription message to broker
        let subscription_msg = OutboundMessage {
            message_type: OutboundMessageType::Subscribe {
                pattern: pattern.to_string(),
            },
            payload: serde_json::json!({
                "pattern": pattern
            }),
            response_channel: None,
        };

        if let Some(sender) = self.connection_state.outbound_sender.lock().await.as_ref() {
            sender.send(subscription_msg).await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to send subscription: {}", e))
            })?;
        }

        info!(pattern = %pattern, "Subscribed to topic pattern");
        Ok(receiver)
    }

    #[instrument(skip_all, fields(pattern = %pattern))]
    async fn unsubscribe(&self, pattern: &str) -> Result<(), BusrtError> {
        // Remove subscription
        {
            let mut subscriptions = self.subscription_manager.subscriptions.write().await;
            subscriptions.remove(pattern);
        }

        // Remove subscription metadata
        {
            let mut metadata = self
                .subscription_manager
                .subscription_metadata
                .write()
                .await;
            metadata.remove(pattern);
        }

        // Send unsubscription message to broker
        let unsubscription_msg = OutboundMessage {
            message_type: OutboundMessageType::Unsubscribe {
                pattern: pattern.to_string(),
            },
            payload: serde_json::json!({
                "pattern": pattern
            }),
            response_channel: None,
        };

        if let Some(sender) = self.connection_state.outbound_sender.lock().await.as_ref() {
            sender.send(unsubscription_msg).await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to send unsubscription: {}", e))
            })?;
        }

        info!(pattern = %pattern, "Unsubscribed from topic pattern");
        Ok(())
    }

    #[instrument(skip_all, fields(service = %service, method = %method))]
    async fn rpc_call_json(
        &self,
        service: &str,
        method: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, BusrtError> {
        if !self.is_connected() {
            return Err(BusrtError::BrokerUnavailable(
                "Client not connected".to_string(),
            ));
        }

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let rpc_msg = OutboundMessage {
            message_type: OutboundMessageType::RpcCall {
                service: service.to_string(),
                method: method.to_string(),
            },
            payload: request,
            response_channel: Some(response_tx),
        };

        if let Some(sender) = self.connection_state.outbound_sender.lock().await.as_ref() {
            sender.send(rpc_msg).await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to send RPC call: {}", e))
            })?;
        } else {
            return Err(BusrtError::BrokerUnavailable(
                "Outbound channel not available".to_string(),
            ));
        }

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(30), response_rx).await {
            Ok(Ok(response)) => {
                debug!(service = %service, method = %method, "RPC call completed");
                response
            }
            Ok(Err(_)) => Err(BusrtError::ConnectionFailed(
                "RPC response channel closed".to_string(),
            )),
            Err(_) => Err(BusrtError::RpcTimeout(format!(
                "RPC call timeout: {}::{}",
                service, method
            ))),
        }
    }

    async fn get_broker_stats(&self) -> Result<BrokerStats, BusrtError> {
        // Make RPC call to get broker statistics
        let response = self
            .rpc_call_json("broker", "get_stats", serde_json::json!({}))
            .await?;

        serde_json::from_value(response).map_err(|e| {
            BusrtError::SerializationFailed(format!("Failed to parse broker stats: {}", e))
        })
    }
}
