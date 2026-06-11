//! RPC service manager for collector-core.
//!
//! Wraps the `CollectorRpcService` from daemoneye-eventbus and runs the background
//! task that subscribes to the RPC topic, dispatches requests, and publishes responses.

use crate::collector::CollectorRuntime;
use crate::performance::PerformanceMonitor;
use crate::rpc_services::config::RpcServiceConfig;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::rpc::{
    CollectorOperation as RpcOperation, CollectorRpcService, ConfigProvider, HealthProvider,
    RegistrationProvider, RpcCorrelationMetadata, RpcRequest, ServiceCapabilities, TimeoutLimits,
};
use daemoneye_lib::telemetry::TelemetryCollector;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info};
use uuid::Uuid;

type TaskHandler = Box<
    dyn Fn(
            daemoneye_lib::proto::DetectionTask,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<daemoneye_lib::proto::DetectionResult>,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// RPC service manager for collector-core
///
/// This manager wraps the `CollectorRpcService` from daemoneye-eventbus and provides
/// collector-specific provider implementations that aggregate health from event sources,
/// telemetry, and performance monitoring.
pub struct CollectorRpcServiceManager {
    /// Service configuration
    config: RpcServiceConfig,
    /// RPC service instance
    rpc_service: Arc<CollectorRpcService>,
    /// Broker reference for message subscription
    broker: Arc<DaemoneyeBroker>,
    /// Background task handle
    service_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Collector runtime reference (for lifecycle operations)
    runtime: Arc<RwLock<Option<Arc<RwLock<CollectorRuntime>>>>>,
    /// Telemetry collector reference
    telemetry: Arc<RwLock<Option<Arc<RwLock<TelemetryCollector>>>>>,
    /// Performance monitor reference
    performance_monitor: Arc<RwLock<Option<Arc<PerformanceMonitor>>>>,
    /// Shutdown signal for triggering collector shutdown (shared with runtime)
    runtime_shutdown: Arc<RwLock<Arc<std::sync::atomic::AtomicBool>>>,
    /// Task handler for executing detection tasks
    task_handler: Arc<RwLock<Option<TaskHandler>>>,
}

impl CollectorRpcServiceManager {
    /// Create a new RPC service manager
    pub fn new(
        config: RpcServiceConfig,
        broker: Arc<DaemoneyeBroker>,
        health_provider: Arc<dyn HealthProvider + Send + Sync>,
        config_provider: Arc<dyn ConfigProvider + Send + Sync>,
        registration_provider: Arc<dyn RegistrationProvider + Send + Sync>,
    ) -> Self {
        let capabilities = ServiceCapabilities {
            operations: vec![
                RpcOperation::Start,
                RpcOperation::Stop,
                RpcOperation::Restart,
                RpcOperation::HealthCheck,
                RpcOperation::UpdateConfig,
                RpcOperation::GetCapabilities,
                RpcOperation::GracefulShutdown,
                RpcOperation::ForceShutdown,
                RpcOperation::Pause,
                RpcOperation::Resume,
                RpcOperation::ExecuteTask,
            ],
            max_concurrent_requests: 10,
            timeout_limits: TimeoutLimits {
                min_timeout_ms: 1000,
                max_timeout_ms: 300_000,
                default_timeout_ms: 30000,
            },
            supported_collectors: vec!["procmond".to_owned()],
        };

        // Create process manager for the RPC service
        // Note: Lifecycle operations (Start/Stop/Restart) are intercepted and handled by
        // the collector runtime, but the process manager is still required by the RPC service
        // for other operations that may need it
        let process_manager = daemoneye_eventbus::process_manager::CollectorProcessManager::new(
            daemoneye_eventbus::process_manager::ProcessManagerConfig::default(),
        );

        let rpc_service = Arc::new(CollectorRpcService::with_providers(
            config.collector_id.clone(),
            capabilities,
            process_manager,
            health_provider,
            config_provider,
            registration_provider,
            Some(Arc::clone(&broker)),
        ));

        Self {
            config,
            rpc_service,
            broker,
            service_handle: Arc::new(Mutex::new(None)),
            runtime: Arc::new(RwLock::new(None)),
            telemetry: Arc::new(RwLock::new(None)),
            performance_monitor: Arc::new(RwLock::new(None)),
            runtime_shutdown: Arc::new(RwLock::new(Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )))),
            task_handler: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the collector runtime reference
    pub async fn set_runtime(&self, runtime: Arc<RwLock<CollectorRuntime>>) {
        let mut guard = self.runtime.write().await;
        *guard = Some(runtime);
    }

    /// Set the telemetry collector reference
    pub async fn set_telemetry(&self, telemetry: Arc<RwLock<TelemetryCollector>>) {
        let mut guard = self.telemetry.write().await;
        *guard = Some(telemetry);
    }

    /// Set the performance monitor reference
    pub async fn set_performance_monitor(&self, monitor: Arc<PerformanceMonitor>) {
        let mut guard = self.performance_monitor.write().await;
        *guard = Some(monitor);
    }

    /// Set the shutdown signal reference (shared with runtime)
    pub async fn set_shutdown_signal(&self, signal: Arc<std::sync::atomic::AtomicBool>) {
        // Store the shared runtime shutdown signal
        // This ensures RPC-triggered shutdowns propagate to the runtime
        let mut guard = self.runtime_shutdown.write().await;
        *guard = signal;
    }

    /// Set the task handler
    pub async fn set_task_handler<F>(&self, handler: F)
    where
        F: Fn(
                daemoneye_lib::proto::DetectionTask,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = anyhow::Result<daemoneye_lib::proto::DetectionResult>,
                        > + Send,
                >,
            > + Send
            + Sync
            + 'static,
    {
        let mut guard = self.task_handler.write().await;
        *guard = Some(Box::new(handler));
    }

    /// Start the RPC service background task
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut handle_guard = self.service_handle.lock().await;
        if handle_guard.is_some() {
            return Err("RPC service already started".into());
        }

        let rpc_topic = self.config.rpc_topic.clone();
        let broker = Arc::clone(&self.broker);
        let subscriber_id = Uuid::new_v4();

        // Subscribe to RPC topic
        let mut message_receiver = broker
            .subscribe_raw(&rpc_topic, subscriber_id)
            .await
            .map_err(|e| format!("Failed to subscribe to RPC topic: {e}"))?;

        info!(
            collector_id = %self.config.collector_id,
            rpc_topic = %rpc_topic,
            "Starting RPC service for collector"
        );

        // Clone shutdown signal for the background task
        let shutdown_signal = {
            let guard = self.runtime_shutdown.read().await;
            Arc::clone(&*guard)
        };

        // Clone runtime reference and config for the background task
        let runtime_ref = Arc::clone(&self.runtime);
        let config_collector_id = self.config.collector_id.clone();
        let rpc_service_clone = Arc::clone(&self.rpc_service);
        let broker_clone = Arc::clone(&self.broker);
        let task_handler_clone = Arc::clone(&self.task_handler);

        // Create oneshot channel to signal when the service is ready to receive messages
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn background task to handle RPC requests
        let handle = tokio::spawn(async move {
            // Signal readiness before entering the message processing loop
            // This ensures callers know the service is subscribed and ready
            eprintln!(
                "RPC_SERVICE_LOOP: Ready signal sent, entering message loop for collector: {config_collector_id}"
            );
            ready_tx.send(()).ok();

            while let Some(message) = message_receiver.recv().await {
                eprintln!(
                    "RPC_SERVICE_LOOP: Received message on topic: {} (payload size: {} bytes)",
                    message.topic,
                    message.payload.len()
                );

                // Skip response messages - parse topic to check for rpc.response segment
                // Check if topic contains "rpc.response" as adjacent dot-separated segments
                let topic_parts: Vec<&str> = message.topic.split('.').collect();
                let is_rpc_response = topic_parts
                    .windows(2)
                    .any(|window| window[0] == "rpc" && window[1] == "response");
                if is_rpc_response {
                    eprintln!("RPC_SERVICE_LOOP: Skipping response message");
                    continue;
                }

                // Deserialize RPC request
                let request: RpcRequest = match postcard::from_bytes::<RpcRequest>(&message.payload)
                {
                    Ok(req) => {
                        eprintln!(
                            "RPC_SERVICE_LOOP: Deserialized request: operation={:?}, client_id={}, request_id={}",
                            req.operation, req.client_id, req.request_id
                        );
                        req
                    }
                    Err(e) => {
                        eprintln!("RPC_SERVICE_LOOP: Failed to deserialize RPC request: {e}");
                        error!("Failed to deserialize RPC request: {}", e);
                        continue;
                    }
                };

                // Handle shutdown requests by triggering shutdown signal
                let is_shutdown = matches!(
                    request.operation,
                    RpcOperation::GracefulShutdown | RpcOperation::ForceShutdown
                );

                // Intercept lifecycle operations and route to runtime
                let response = if matches!(
                    request.operation,
                    RpcOperation::Start | RpcOperation::Stop | RpcOperation::Restart
                ) {
                    // ... existing lifecycle logic ...
                    // Handle lifecycle operations via runtime instead of dummy ProcessManager
                    let runtime_guard = runtime_ref.read().await;
                    if let Some(runtime) = runtime_guard.as_ref() {
                        let _runtime_inner = runtime.read().await;
                        #[allow(clippy::wildcard_enum_match_arm)]
                        match request.operation {
                            RpcOperation::Start => {
                                // Runtime is already started, return success
                                let start_time = SystemTime::now();
                                daemoneye_eventbus::rpc::RpcResponse {
                                    request_id: request.request_id.clone(),
                                    service_id: config_collector_id.clone(),
                                    operation: RpcOperation::Start,
                                    status: daemoneye_eventbus::rpc::RpcStatus::Success,
                                    payload: None,
                                    error_details: None,
                                    timestamp: start_time,
                                    execution_time_ms: 0,
                                    queue_time_ms: Some(0),
                                    total_time_ms: 0,
                                    correlation_metadata: RpcCorrelationMetadata::default(),
                                }
                            }
                            RpcOperation::Stop => {
                                // Trigger shutdown signal and return immediately
                                // Note: We return success immediately rather than waiting for shutdown
                                // confirmation, because the graceful shutdown process will stop the
                                // RPC service before we could send the response otherwise.
                                // Clients can poll health status or listen for completion events
                                // if they need to confirm shutdown completion.
                                shutdown_signal.store(true, std::sync::atomic::Ordering::Relaxed);
                                info!("Stop operation triggered via RPC (shutdown signal set)");
                                let start_time = SystemTime::now();
                                daemoneye_eventbus::rpc::RpcResponse {
                                    request_id: request.request_id.clone(),
                                    service_id: config_collector_id.clone(),
                                    operation: RpcOperation::Stop,
                                    status: daemoneye_eventbus::rpc::RpcStatus::Success,
                                    payload: None,
                                    error_details: None,
                                    timestamp: start_time,
                                    execution_time_ms: 0,
                                    queue_time_ms: Some(0),
                                    total_time_ms: 0,
                                    correlation_metadata: RpcCorrelationMetadata::default(),
                                }
                            }
                            RpcOperation::Restart => {
                                // For restart, trigger shutdown (runtime will need to be restarted externally)
                                shutdown_signal.store(true, std::sync::atomic::Ordering::Relaxed);
                                info!("Restart operation triggered via RPC (shutdown signal set)");
                                let start_time = SystemTime::now();
                                daemoneye_eventbus::rpc::RpcResponse {
                                    request_id: request.request_id.clone(),
                                    service_id: config_collector_id.clone(),
                                    operation: RpcOperation::Restart,
                                    status: daemoneye_eventbus::rpc::RpcStatus::Success,
                                    payload: None,
                                    error_details: None,
                                    timestamp: start_time,
                                    execution_time_ms: 0,
                                    queue_time_ms: Some(0),
                                    total_time_ms: 0,
                                    correlation_metadata: RpcCorrelationMetadata::default(),
                                }
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        // Runtime not available, fall back to RPC service
                        rpc_service_clone.handle_request(request.clone()).await
                    }
                } else if request.operation == RpcOperation::ExecuteTask {
                    let start_time = SystemTime::now();
                    // Handle ExecuteTask
                    let result = if let daemoneye_eventbus::rpc::RpcPayload::Task(value) =
                        &request.payload
                    {
                        // Deserialize DetectionTask from JSON Value
                        match serde_json::from_value::<daemoneye_lib::proto::DetectionTask>(
                            value.clone(),
                        ) {
                            Ok(task) => {
                                let handler_guard = task_handler_clone.read().await;
                                if let Some(handler) = handler_guard.as_ref() {
                                    // Execute handler
                                    match handler(task).await {
                                        Ok(detection_result) => {
                                            // Serialize result to JSON Value
                                            match serde_json::to_value(&detection_result) {
                                                Ok(result_value) => Ok(
                                                    daemoneye_eventbus::rpc::RpcPayload::TaskResult(
                                                        result_value,
                                                    ),
                                                ),
                                                Err(e) => {
                                                    Err(format!("Failed to serialize result: {e}"))
                                                }
                                            }
                                        }
                                        Err(e) => Err(format!("Task execution failed: {e}")),
                                    }
                                } else {
                                    Err("No task handler registered".to_owned())
                                }
                            }
                            Err(e) => Err(format!("Failed to deserialize task: {e}")),
                        }
                    } else {
                        Err("Invalid payload for ExecuteTask".to_owned())
                    };

                    let execution_time = start_time.elapsed().unwrap_or_default();

                    match result {
                        Ok(payload) => daemoneye_eventbus::rpc::RpcResponse {
                            request_id: request.request_id.clone(),
                            service_id: config_collector_id.clone(),
                            operation: RpcOperation::ExecuteTask,
                            status: daemoneye_eventbus::rpc::RpcStatus::Success,
                            payload: Some(payload),
                            error_details: None,
                            timestamp: start_time,
                            execution_time_ms: execution_time.as_millis() as u64,
                            queue_time_ms: Some(0),
                            total_time_ms: execution_time.as_millis() as u64,
                            correlation_metadata: RpcCorrelationMetadata::default(),
                        },
                        Err(msg) => daemoneye_eventbus::rpc::RpcResponse {
                            request_id: request.request_id.clone(),
                            service_id: config_collector_id.clone(),
                            operation: RpcOperation::ExecuteTask,
                            status: daemoneye_eventbus::rpc::RpcStatus::Error,
                            payload: None,
                            error_details: Some(daemoneye_eventbus::rpc::RpcError {
                                code: "EXECUTION_ERROR".to_owned(),
                                message: msg,
                                context: std::collections::HashMap::new(),
                                category: daemoneye_eventbus::rpc::ErrorCategory::Internal,
                            }),
                            timestamp: start_time,
                            execution_time_ms: execution_time.as_millis() as u64,
                            queue_time_ms: Some(0),
                            total_time_ms: execution_time.as_millis() as u64,
                            correlation_metadata: RpcCorrelationMetadata::default(),
                        },
                    }
                } else if is_shutdown {
                    // Trigger shutdown signal for collector runtime
                    shutdown_signal.store(true, std::sync::atomic::Ordering::Relaxed);
                    info!("Shutdown signal triggered via RPC");
                    // Return success response
                    let start_time = SystemTime::now();
                    daemoneye_eventbus::rpc::RpcResponse {
                        request_id: request.request_id.clone(),
                        service_id: config_collector_id.clone(),
                        operation: request.operation,
                        status: daemoneye_eventbus::rpc::RpcStatus::Success,
                        payload: None,
                        error_details: None,
                        timestamp: start_time,
                        execution_time_ms: 0,
                        queue_time_ms: Some(0),
                        total_time_ms: 0,
                        correlation_metadata: RpcCorrelationMetadata::default(),
                    }
                } else {
                    // Handle other requests via RPC service
                    eprintln!(
                        "RPC_SERVICE_LOOP: Routing to rpc_service.handle_request() for operation: {:?}",
                        request.operation
                    );
                    let resp = rpc_service_clone.handle_request(request.clone()).await;
                    eprintln!(
                        "RPC_SERVICE_LOOP: Got response from handle_request: status={:?}, request_id={}",
                        resp.status, resp.request_id
                    );
                    resp
                };

                // Determine response topic
                let response_topic = format!("control.rpc.response.{}", request.client_id);
                eprintln!("RPC_SERVICE_LOOP: Will publish response to topic: {response_topic}");

                // Serialize and publish response
                let payload = match postcard::to_allocvec(&response) {
                    Ok(data) => data,
                    Err(serialization_error) => {
                        // Build minimal error response containing original request info and serialization error
                        error!(
                            request_id = %request.request_id,
                            collector_id = %config_collector_id,
                            operation = ?request.operation,
                            error = %serialization_error,
                            "Failed to serialize RPC response - attempting to send error response"
                        );

                        // Increment error metric (via logging - metrics system should pick this up)
                        // Note: If a metrics system is available, increment a counter here

                        let error_response = daemoneye_eventbus::rpc::RpcResponse {
                            request_id: request.request_id.clone(),
                            service_id: config_collector_id.clone(),
                            operation: request.operation,
                            status: daemoneye_eventbus::rpc::RpcStatus::Error,
                            payload: None,
                            error_details: Some(daemoneye_eventbus::rpc::RpcError {
                                code: "SERIALIZATION_ERROR".to_owned(),
                                message: format!(
                                    "Failed to serialize RPC response for request {}: {}",
                                    request.request_id, serialization_error
                                ),
                                context: std::collections::HashMap::new(),
                                category: daemoneye_eventbus::rpc::ErrorCategory::Internal,
                            }),
                            timestamp: SystemTime::now(),
                            execution_time_ms: 0,
                            queue_time_ms: Some(0),
                            total_time_ms: 0,
                            correlation_metadata: RpcCorrelationMetadata::default(),
                        };

                        // Attempt to serialize the error response
                        match postcard::to_allocvec(&error_response) {
                            Ok(error_payload) => error_payload,
                            Err(error_serialization_error) => {
                                // Even error response serialization failed - log and continue
                                error!(
                                    request_id = %request.request_id,
                                    collector_id = %config_collector_id,
                                    operation = ?request.operation,
                                    original_error = %serialization_error,
                                    error_response_error = %error_serialization_error,
                                    "Failed to serialize error response - client will timeout"
                                );
                                // Cannot send response, client will timeout
                                continue;
                            }
                        }
                    }
                };

                eprintln!(
                    "RPC_SERVICE_LOOP: Publishing response (size: {} bytes) to topic: {}",
                    payload.len(),
                    response_topic
                );
                match broker_clone
                    .publish(&response_topic, &response.request_id, payload)
                    .await
                {
                    Ok(()) => {
                        eprintln!(
                            "RPC_SERVICE_LOOP: Successfully published response to topic: {response_topic}"
                        );
                    }
                    Err(e) => {
                        eprintln!("RPC_SERVICE_LOOP: Failed to publish response: {e}");
                        error!(
                            request_id = %response.request_id,
                            topic = %response_topic,
                            error = %e,
                            "Failed to publish RPC response"
                        );
                    }
                }
            }
        });

        // Wait for the service to signal readiness before returning
        // This ensures the caller knows the service is actually receiving messages
        drop(ready_rx.await);

        *handle_guard = Some(handle);
        Ok(())
    }

    /// Stop the RPC service background task
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut handle_guard = self.service_handle.lock().await;
        if let Some(handle) = handle_guard.take() {
            handle.abort();
            info!("RPC service stopped");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// Helper function to check if a topic should be skipped (matches the logic in the handler)
    fn is_rpc_response_topic(topic: &str) -> bool {
        let topic_parts: Vec<&str> = topic.split('.').collect();
        topic_parts
            .windows(2)
            .any(|window| window[0] == "rpc" && window[1] == "response")
    }

    #[test]
    fn test_rpc_response_topic_parsing() {
        // Topics that should be skipped (contain "rpc.response" as adjacent segments)
        assert!(is_rpc_response_topic("control.rpc.response.client123"));
        assert!(is_rpc_response_topic("rpc.response"));
        assert!(is_rpc_response_topic("control.rpc.response"));
        assert!(is_rpc_response_topic("a.b.rpc.response.c.d"));
        assert!(is_rpc_response_topic("rpc.response.client"));

        // Topics that should NOT be skipped (don't have "rpc.response" as adjacent segments)
        assert!(!is_rpc_response_topic("control.rpc.other.response"));
        assert!(!is_rpc_response_topic("control.response.rpc"));
        assert!(!is_rpc_response_topic("rpc.other.response"));
        assert!(!is_rpc_response_topic("response.rpc"));
        assert!(!is_rpc_response_topic("control.collector.collector"));
        assert!(!is_rpc_response_topic("rpc"));
        assert!(!is_rpc_response_topic("response"));
        assert!(!is_rpc_response_topic("rpc.other"));
        assert!(!is_rpc_response_topic("other.response"));

        // Edge cases
        assert!(!is_rpc_response_topic(""));
        assert!(!is_rpc_response_topic("."));
        assert!(is_rpc_response_topic("rpc.response."));
        assert!(is_rpc_response_topic(".rpc.response"));
    }
}
