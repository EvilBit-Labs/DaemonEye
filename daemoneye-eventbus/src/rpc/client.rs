//! RPC client for collector lifecycle management.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

use crate::broker::DaemoneyeBroker;
use crate::error::{EventBusError, Result};
use crate::message::Message;

use super::messages::{
    CollectorOperation, ErrorCategory, RpcCorrelationMetadata, RpcError, RpcRequest, RpcResponse,
    RpcStatus,
};

/// RPC client for collector lifecycle management
#[derive(Debug)]
pub struct CollectorRpcClient {
    /// Target topic for RPC calls
    pub target_topic: String,
    /// Client identifier for correlation
    pub client_id: String,
    /// Default timeout for RPC calls
    pub default_timeout: Duration,
    /// Reference to the embedded broker for publishing requests
    pub broker: Arc<DaemoneyeBroker>,
    /// Channel for receiving response messages
    #[allow(dead_code)]
    pub response_receiver: Arc<Mutex<mpsc::UnboundedReceiver<Message>>>,
    /// Topic for receiving responses
    #[allow(dead_code)]
    pub response_topic: String,
    /// Pending requests awaiting responses
    pub pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>,
    /// Subscriber ID for response topic subscription
    pub subscriber_id: Uuid,
}

impl CollectorRpcClient {
    /// Create a new RPC client for the specified target topic with broker integration
    pub async fn new(target_topic: &str, broker: Arc<DaemoneyeBroker>) -> Result<Self> {
        let client_id = Uuid::new_v4().to_string();
        let response_topic = format!("control.rpc.response.{client_id}");
        let subscriber_id = Uuid::new_v4();

        // Subscribe to response topic using raw message subscription
        let response_rx = broker.subscribe_raw(&response_topic, subscriber_id).await?;
        let response_receiver = Arc::new(Mutex::new(response_rx));

        let pending_requests = Arc::new(Mutex::new(HashMap::new()));

        // Start response handler background task
        Self::start_response_handler(
            Arc::clone(&pending_requests),
            Arc::clone(&response_receiver),
            response_topic.clone(),
        );

        Ok(Self {
            target_topic: target_topic.to_owned(),
            client_id,
            default_timeout: Duration::from_secs(30),
            broker,
            response_receiver,
            response_topic,
            pending_requests,
            subscriber_id,
        })
    }

    /// Make an RPC call with the specified timeout
    pub async fn call(&self, request: RpcRequest, timeout: Duration) -> Result<RpcResponse> {
        let (tx, rx) = oneshot::channel();

        // Store sender in pending requests
        self.pending_requests
            .lock()
            .await
            .insert(request.request_id.clone(), tx);

        // Serialize RPC request
        let payload = postcard::to_allocvec(&request)
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        // Publish request to broker using the target from the request
        self.broker
            .publish(
                &request.target,
                &request.correlation_metadata.correlation_id,
                payload,
            )
            .await?;

        // Compute effective timeout from deadline if present
        let now = SystemTime::now();
        let effective_timeout = if request.deadline <= now {
            Duration::from_millis(0)
        } else {
            request.deadline.duration_since(now).unwrap_or(timeout)
        };

        // Wait for response with computed timeout
        match tokio::time::timeout(effective_timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                // Remove from pending requests and return error
                self.pending_requests
                    .lock()
                    .await
                    .remove(&request.request_id);
                Err(EventBusError::rpc("Response channel closed".to_owned()))
            }
            Err(_) => {
                // Timeout: remove from pending requests and return timeout error
                self.pending_requests
                    .lock()
                    .await
                    .remove(&request.request_id);
                Err(EventBusError::timeout("RPC request timeout".to_owned()))
            }
        }
    }

    /// Start the response handler background task
    fn start_response_handler(
        pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>,
        response_receiver: Arc<Mutex<mpsc::UnboundedReceiver<Message>>>,
        response_topic: String,
    ) {
        tokio::spawn(async move {
            loop {
                let message = {
                    let mut receiver = response_receiver.lock().await;
                    match receiver.recv().await {
                        Some(msg) => msg,
                        None => break, // Channel closed
                    }
                };

                // Filter by response topic to avoid decoding non-RPC control messages
                if message.topic != response_topic {
                    tracing::debug!(
                        "Skipping message on different topic: {} (expected: {})",
                        message.topic,
                        response_topic
                    );
                    continue;
                }

                // Deserialize response from message payload
                let response: RpcResponse = match postcard::from_bytes(&message.payload) {
                    Ok(resp) => resp,
                    Err(e) => {
                        tracing::error!("Failed to deserialize RPC response: {}", e);
                        continue;
                    }
                };

                // Route response to pending request
                let mut pending = pending_requests.lock().await;
                if let Some(tx) = pending.remove(&response.request_id) {
                    if tx.send(response).is_err() {
                        tracing::warn!("Failed to send RPC response to caller");
                    }
                } else {
                    tracing::warn!(
                        "Received response for unknown request ID: {}",
                        response.request_id
                    );
                }
            }
        });
    }

    /// Handle incoming response message
    pub async fn handle_response(&self, response: RpcResponse) -> Result<()> {
        let maybe_tx = self
            .pending_requests
            .lock()
            .await
            .remove(&response.request_id);
        if let Some(tx) = maybe_tx {
            tx.send(response)
                .map_err(|_closed| EventBusError::rpc("Failed to send response".to_owned()))?;
        } else {
            tracing::warn!(
                "Received response for unknown request ID: {}",
                response.request_id
            );
        }
        Ok(())
    }

    /// Shutdown the RPC client and cleanup resources
    pub async fn shutdown(&self) -> Result<()> {
        // Unsubscribe from response topic
        self.broker.unsubscribe(self.subscriber_id).await?;

        // Drain pending requests before building error responses
        let drained: Vec<(String, oneshot::Sender<RpcResponse>)> =
            self.pending_requests.lock().await.drain().collect();

        // Clear all pending requests with timeout errors
        for (request_id, tx) in drained {
            let error_response = RpcResponse {
                request_id,
                service_id: "client".to_owned(),
                operation: CollectorOperation::HealthCheck, // Default operation
                status: RpcStatus::Cancelled,
                payload: None,
                timestamp: SystemTime::now(),
                execution_time_ms: 0,
                queue_time_ms: None,
                total_time_ms: 0,
                error_details: Some(RpcError {
                    code: "CLIENT_SHUTDOWN".to_owned(),
                    message: "RPC client is shutting down".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Internal,
                }),
                correlation_metadata: RpcCorrelationMetadata::default(),
            };

            drop(tx.send(error_response)); // Ignore send errors during shutdown
        }

        Ok(())
    }
}
