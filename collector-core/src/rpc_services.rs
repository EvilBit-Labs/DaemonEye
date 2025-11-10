//! RPC service implementations for collector lifecycle management.
//!
//! This module provides RPC service integration for collector-core, enabling
//! lifecycle operations (start, stop, restart, health checks) to be performed
//! via RPC calls through the DaemonEye event bus.

use crate::collector::CollectorRuntime;
use crate::performance::PerformanceMonitor;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::rpc::{
    CollectorOperation as RpcOperation, CollectorRpcService, ComponentHealth, ConfigProvider,
    ConfigUpdateResult, HealthCheckData, HealthProvider, HealthStatus, RegistrationError,
    RegistrationProvider, RegistrationRequest, RegistrationResponse, RpcCorrelationMetadata,
    RpcRequest, ServiceCapabilities, TimeoutLimits,
};
use daemoneye_lib::telemetry::{HealthStatus as TelemetryHealthStatus, TelemetryCollector};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{Duration as TokioDuration, timeout};
use tracing::{error, info};
use uuid::Uuid;

/// Configuration for the RPC service manager
#[derive(Debug, Clone)]
pub struct RpcServiceConfig {
    /// Collector identifier
    pub collector_id: String,
    /// RPC topic to subscribe to
    pub rpc_topic: String,
    /// Default timeout for RPC operations
    pub default_timeout: Duration,
}

impl Default for RpcServiceConfig {
    fn default() -> Self {
        Self {
            collector_id: "collector".to_string(),
            rpc_topic: "control.collector.collector".to_string(),
            default_timeout: Duration::from_secs(30),
        }
    }
}

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
            ],
            max_concurrent_requests: 10,
            timeout_limits: TimeoutLimits {
                min_timeout_ms: 1000,
                max_timeout_ms: 300000,
                default_timeout_ms: 30000,
            },
            supported_collectors: vec!["procmond".to_string()],
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
            .map_err(|e| format!("Failed to subscribe to RPC topic: {}", e))?;

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

        // Spawn background task to handle RPC requests
        let handle = tokio::spawn(async move {
            while let Some(message) = message_receiver.recv().await {
                // Skip response messages - parse topic to check for rpc.response segment
                // Check if topic contains "rpc.response" as adjacent dot-separated segments
                let topic_parts: Vec<&str> = message.topic.split('.').collect();
                let is_rpc_response = topic_parts
                    .windows(2)
                    .any(|window| window[0] == "rpc" && window[1] == "response");
                if is_rpc_response {
                    continue;
                }

                // Deserialize RPC request
                let request: RpcRequest = match bincode::serde::decode_from_slice::<RpcRequest, _>(
                    &message.payload,
                    bincode::config::standard(),
                ) {
                    Ok((req, _)) => req,
                    Err(e) => {
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
                    // Handle lifecycle operations via runtime instead of dummy ProcessManager
                    let runtime_guard = runtime_ref.read().await;
                    if let Some(runtime) = runtime_guard.as_ref() {
                        let _runtime_inner = runtime.read().await;
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
                                // Trigger shutdown signal
                                shutdown_signal.store(true, std::sync::atomic::Ordering::Relaxed);
                                info!("Stop operation triggered via RPC");
                                let start_time = SystemTime::now();

                                // Wait for shutdown confirmation with bounded timeout
                                // Check if runtime becomes unavailable (indicating shutdown progress)
                                let shutdown_timeout = TokioDuration::from_secs(5);
                                let runtime_clone = Arc::clone(&runtime_ref);

                                let shutdown_confirmed = timeout(shutdown_timeout, async {
                                    // Poll every 100ms to check if runtime is still available
                                    // If runtime becomes None, shutdown has likely completed
                                    let mut check_count = 0;
                                    let max_checks = 50; // 5 seconds / 100ms

                                    while check_count < max_checks {
                                        tokio::time::sleep(TokioDuration::from_millis(100)).await;
                                        let runtime_guard = runtime_clone.read().await;
                                        if runtime_guard.is_none() {
                                            // Runtime has been dropped, shutdown likely completed
                                            return true;
                                        }
                                        // Check if shutdown signal is still set (it should be)
                                        if !shutdown_signal
                                            .load(std::sync::atomic::Ordering::Relaxed)
                                        {
                                            // Shutdown signal was cleared, something unexpected happened
                                            return false;
                                        }
                                        check_count += 1;
                                    }
                                    false // Timeout - couldn't confirm shutdown completion
                                })
                                .await;

                                let (status, error_details) = match shutdown_confirmed {
                                    Ok(true) => {
                                        // Shutdown confirmed
                                        (daemoneye_eventbus::rpc::RpcStatus::Success, None)
                                    }
                                    Ok(false) | Err(_) => {
                                        // Timeout or couldn't confirm - return InProgress status
                                        // Clients should poll health or listen for completion events
                                        (
                                            daemoneye_eventbus::rpc::RpcStatus::InProgress,
                                            Some(daemoneye_eventbus::rpc::RpcError {
                                                code: "SHUTDOWN_INITIATED".to_string(),
                                                message: "Shutdown signal set, but shutdown completion not confirmed within timeout. Client should poll health status or listen for completion events.".to_string(),
                                                context: std::collections::HashMap::new(),
                                                category: daemoneye_eventbus::rpc::ErrorCategory::Internal,
                                            }),
                                        )
                                    }
                                };

                                let execution_time = start_time.elapsed().unwrap_or_default();
                                daemoneye_eventbus::rpc::RpcResponse {
                                    request_id: request.request_id.clone(),
                                    service_id: config_collector_id.clone(),
                                    operation: RpcOperation::Stop,
                                    status,
                                    payload: None,
                                    error_details,
                                    timestamp: start_time,
                                    execution_time_ms: execution_time.as_millis() as u64,
                                    queue_time_ms: Some(0),
                                    total_time_ms: execution_time.as_millis() as u64,
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
                    rpc_service_clone.handle_request(request.clone()).await
                };

                // Determine response topic
                let response_topic = format!("control.rpc.response.{}", request.client_id);

                // Serialize and publish response
                let payload = match bincode::serde::encode_to_vec(
                    &response,
                    bincode::config::standard(),
                ) {
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
                                code: "SERIALIZATION_ERROR".to_string(),
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
                        match bincode::serde::encode_to_vec(
                            &error_response,
                            bincode::config::standard(),
                        ) {
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

                if let Err(e) = broker_clone
                    .publish(&response_topic, &response.request_id, payload)
                    .await
                {
                    error!(
                        request_id = %response.request_id,
                        topic = %response_topic,
                        error = %e,
                        "Failed to publish RPC response"
                    );
                }
            }
        });

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

/// Health provider implementation for collector-core
pub struct CollectorHealthProvider {
    pub(crate) runtime: Arc<RwLock<Option<Arc<RwLock<CollectorRuntime>>>>>,
    pub(crate) telemetry: Arc<RwLock<Option<Arc<RwLock<TelemetryCollector>>>>>,
    pub(crate) performance_monitor: Arc<RwLock<Option<Arc<PerformanceMonitor>>>>,
    pub(crate) collector_id: String,
}

#[async_trait::async_trait]
impl HealthProvider for CollectorHealthProvider {
    async fn get_collector_health(
        &self,
        collector_id: &str,
    ) -> std::result::Result<HealthCheckData, daemoneye_eventbus::ProcessManagerError> {
        if collector_id != self.collector_id {
            return Err(daemoneye_eventbus::ProcessManagerError::ProcessNotFound(
                collector_id.to_string(),
            ));
        }

        let mut components = HashMap::new();
        let mut metrics = HashMap::new();

        // Get telemetry health
        if let Some(telemetry) = self.telemetry.read().await.as_ref() {
            let telemetry_guard = telemetry.read().await;
            let health_check = telemetry_guard.health_check();
            let telemetry_status = match health_check.status {
                TelemetryHealthStatus::Healthy => HealthStatus::Healthy,
                TelemetryHealthStatus::Degraded => HealthStatus::Degraded,
                TelemetryHealthStatus::Unhealthy => HealthStatus::Unhealthy,
                TelemetryHealthStatus::Unknown => HealthStatus::Unknown,
                _ => HealthStatus::Unknown,
            };

            components.insert(
                "telemetry".to_string(),
                ComponentHealth {
                    name: "telemetry".to_string(),
                    status: telemetry_status,
                    message: Some(format!("Status: {:?}", health_check.status)),
                    last_check: SystemTime::now(),
                    check_interval_seconds: 60,
                },
            );

            let telemetry_metrics = telemetry_guard.get_metrics();
            metrics.insert(
                "operation_count".to_string(),
                telemetry_metrics.operation_count as f64,
            );
            metrics.insert(
                "error_count".to_string(),
                telemetry_metrics.error_count as f64,
            );

            // Add custom metrics if available
            for (key, value) in &telemetry_metrics.custom_data {
                metrics.insert(key.clone(), *value);
            }
        }

        // Get performance monitor metrics
        if let Some(perf_monitor) = self.performance_monitor.read().await.as_ref() {
            let perf_metrics = perf_monitor.collect_resource_metrics().await;
            metrics.insert(
                "cpu_percent".to_string(),
                perf_metrics.cpu.current_cpu_percent,
            );
            metrics.insert(
                "memory_bytes".to_string(),
                perf_metrics.memory.current_memory_bytes as f64,
            );
            metrics.insert(
                "events_per_second".to_string(),
                perf_metrics.throughput.events_per_second,
            );
        }

        // Get runtime stats if available
        if let Some(runtime) = self.runtime.read().await.as_ref() {
            let runtime_guard = runtime.read().await;
            let stats = runtime_guard.get_runtime_stats();
            metrics.insert(
                "events_processed".to_string(),
                stats.events_processed as f64,
            );
            metrics.insert("errors_total".to_string(), stats.errors_total as f64);
            metrics.insert(
                "registered_sources".to_string(),
                stats.registered_sources as f64,
            );
        }

        // Aggregate overall health
        let overall_status = components
            .values()
            .map(|c| c.status)
            .min()
            .unwrap_or(HealthStatus::Unknown);

        Ok(HealthCheckData {
            collector_id: collector_id.to_string(),
            status: overall_status,
            components,
            metrics,
            last_heartbeat: SystemTime::now(),
            uptime_seconds: 0, // Would need to track start time
            error_count: 0,
        })
    }
}

/// Config provider implementation for collector-core
pub struct CollectorConfigProvider {
    pub(crate) collector_id: String,
}

#[async_trait::async_trait]
impl ConfigProvider for CollectorConfigProvider {
    async fn get_config(
        &self,
        collector_id: &str,
    ) -> std::result::Result<
        daemoneye_eventbus::CollectorConfig,
        daemoneye_eventbus::ConfigManagerError,
    > {
        if collector_id != self.collector_id {
            return Err(daemoneye_eventbus::ConfigManagerError::ConfigNotFound(
                format!("Collector {} not found", collector_id),
            ));
        }

        // Return default config for now
        Ok(daemoneye_eventbus::CollectorConfig::default())
    }

    async fn update_config(
        &self,
        collector_id: &str,
        changes: HashMap<String, serde_json::Value>,
        _validate_only: bool,
        _rollback_on_failure: bool,
    ) -> std::result::Result<ConfigUpdateResult, daemoneye_eventbus::ConfigManagerError> {
        if collector_id != self.collector_id {
            return Err(daemoneye_eventbus::ConfigManagerError::ConfigNotFound(
                format!("Collector {} not found", collector_id),
            ));
        }

        // For now, return a no-op result
        // In a full implementation, this would update the collector configuration
        Ok(ConfigUpdateResult {
            version: 1,
            changed_fields: changes.keys().cloned().collect(),
            restart_performed: false,
            timestamp: SystemTime::now(),
        })
    }

    async fn validate_config(
        &self,
        _collector_id: &str,
        _config: &daemoneye_eventbus::CollectorConfig,
    ) -> std::result::Result<(), daemoneye_eventbus::ConfigManagerError> {
        // Default validation passes
        Ok(())
    }
}

/// Registration provider implementation for collector-core
pub struct CollectorRegistrationProvider {
    collector_id: String,
    registered: Arc<RwLock<bool>>,
}

impl CollectorRegistrationProvider {
    pub fn new(collector_id: String) -> Self {
        Self {
            collector_id,
            registered: Arc::new(RwLock::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl RegistrationProvider for CollectorRegistrationProvider {
    async fn register_collector(
        &self,
        request: RegistrationRequest,
    ) -> std::result::Result<RegistrationResponse, RegistrationError> {
        if request.collector_id != self.collector_id {
            return Err(RegistrationError::Validation(format!(
                "Collector ID mismatch: expected {}, got {}",
                self.collector_id, request.collector_id
            )));
        }

        let mut registered = self.registered.write().await;
        if *registered {
            return Err(RegistrationError::AlreadyRegistered(
                self.collector_id.clone(),
            ));
        }

        *registered = true;

        Ok(RegistrationResponse {
            collector_id: request.collector_id,
            accepted: true,
            heartbeat_interval_ms: request.heartbeat_interval_ms.unwrap_or(30000),
            assigned_topics: vec![],
            message: None,
        })
    }

    async fn deregister_collector(
        &self,
        request: daemoneye_eventbus::rpc::DeregistrationRequest,
    ) -> std::result::Result<(), RegistrationError> {
        if request.collector_id != self.collector_id {
            return Err(RegistrationError::NotFound(request.collector_id));
        }

        let mut registered = self.registered.write().await;
        if !*registered {
            return Err(RegistrationError::NotFound(self.collector_id.clone()));
        }

        *registered = false;
        Ok(())
    }

    async fn update_heartbeat(
        &self,
        collector_id: &str,
    ) -> std::result::Result<(), RegistrationError> {
        if collector_id != self.collector_id {
            return Err(RegistrationError::NotFound(collector_id.to_string()));
        }

        // Heartbeat update is a no-op for this provider
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
