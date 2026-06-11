//! RPC service for handling collector lifecycle operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{error, info};

use crate::broker::DaemoneyeBroker;
use crate::config_manager::{ConfigManager, ConfigManagerError};
use crate::{CollectorConfig, CollectorProcessManager, ProcessManagerError};

use super::messages::{
    CapabilitiesData, CollectorOperation, ErrorCategory, HealthCheckData, HealthStatus,
    ResourceRequirements, RpcError, RpcPayload, RpcRequest, RpcResponse, RpcStatus,
    ServiceCapabilities,
};
use super::providers::{
    ConfigProvider, DefaultConfigProvider, DefaultHealthProvider, DefaultRegistrationProvider,
    HealthProvider, RegistrationError, RegistrationProvider,
};

/// RPC service for handling collector lifecycle operations
pub struct CollectorRpcService {
    /// Service identifier
    pub service_id: String,
    /// Supported operations
    pub supported_operations: Vec<CollectorOperation>,
    /// Service capabilities
    pub capabilities: ServiceCapabilities,
    /// Process manager for lifecycle operations
    pub process_manager: Arc<CollectorProcessManager>,
    /// Health provider for health checks
    pub health_provider: Arc<dyn HealthProvider + Send + Sync>,
    /// Config provider for configuration operations
    pub config_provider: Arc<dyn ConfigProvider + Send + Sync>,
    /// Registration provider for collector lifecycle coordination
    pub registration_provider: Arc<dyn RegistrationProvider + Send + Sync>,
    /// Optional broker for publishing notifications
    pub broker: Option<Arc<DaemoneyeBroker>>,
}

impl CollectorRpcService {
    /// Create a new RPC service
    pub fn new(
        service_id: String,
        capabilities: ServiceCapabilities,
        process_manager: Arc<CollectorProcessManager>,
    ) -> Self {
        // Create default providers backed by internal managers
        let config_dir = std::path::PathBuf::from("/var/lib/daemoneye/configs");
        let config_manager = Arc::new(ConfigManager::new(config_dir));
        let health_provider = Arc::new(DefaultHealthProvider {
            process_manager: Arc::clone(&process_manager),
        });
        let config_provider = Arc::new(DefaultConfigProvider {
            config_manager: Arc::clone(&config_manager),
            process_manager: Arc::clone(&process_manager),
            broker: None,
        });
        let registration_provider = Arc::new(DefaultRegistrationProvider::default());

        Self {
            service_id,
            supported_operations: capabilities.operations.clone(),
            capabilities,
            process_manager,
            health_provider,
            config_provider,
            registration_provider,
            broker: None,
        }
    }

    /// Create a new RPC service with custom config manager and broker
    pub fn with_config_manager(
        service_id: String,
        capabilities: ServiceCapabilities,
        process_manager: Arc<CollectorProcessManager>,
        config_manager: &Arc<ConfigManager>,
        broker: Option<Arc<DaemoneyeBroker>>,
    ) -> Self {
        let health_provider = Arc::new(DefaultHealthProvider {
            process_manager: Arc::clone(&process_manager),
        });
        let config_provider = Arc::new(DefaultConfigProvider {
            config_manager: Arc::clone(config_manager),
            process_manager: Arc::clone(&process_manager),
            broker: broker.clone(),
        });
        let registration_provider = Arc::new(DefaultRegistrationProvider::default());
        Self {
            service_id,
            supported_operations: capabilities.operations.clone(),
            capabilities,
            process_manager,
            health_provider,
            config_provider,
            registration_provider,
            broker,
        }
    }

    /// Create a new RPC service with explicit providers
    pub fn with_providers(
        service_id: String,
        capabilities: ServiceCapabilities,
        process_manager: Arc<CollectorProcessManager>,
        health_provider: Arc<dyn HealthProvider + Send + Sync>,
        config_provider: Arc<dyn ConfigProvider + Send + Sync>,
        registration_provider: Arc<dyn RegistrationProvider + Send + Sync>,
        broker: Option<Arc<DaemoneyeBroker>>,
    ) -> Self {
        Self {
            service_id,
            supported_operations: capabilities.operations.clone(),
            capabilities,
            process_manager,
            health_provider,
            config_provider,
            registration_provider,
            broker,
        }
    }

    /// Handle an incoming RPC request
    pub async fn handle_request(&self, request: RpcRequest) -> RpcResponse {
        let start_time = SystemTime::now();
        let deadline = request.deadline;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(request, start_time, "initial dispatch", deadline);
        }

        // Validate operation is supported
        let operation = request.operation;
        if !self.supported_operations.contains(&operation) {
            return self.create_error_response(
                request,
                RpcError {
                    code: "UNSUPPORTED_OPERATION".to_owned(),
                    message: format!("Operation {operation:?} not supported by this service"),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        }

        // Process the request based on operation type
        match operation {
            CollectorOperation::Register => {
                self.handle_register_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Deregister => {
                self.handle_deregister_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Start => {
                self.handle_start_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Stop => {
                self.handle_stop_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Restart => {
                self.handle_restart_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::HealthCheck => {
                self.handle_health_check_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::UpdateConfig => {
                self.handle_config_update_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::GetCapabilities => {
                self.handle_capabilities_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::GracefulShutdown => {
                self.handle_graceful_shutdown_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::ForceShutdown => {
                self.handle_force_shutdown_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Pause => {
                self.handle_pause_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::Resume => {
                self.handle_resume_request(request, start_time, deadline)
                    .await
            }
            CollectorOperation::ExecuteTask => {
                // Base service does not handle generic tasks; they should be intercepted by the manager
                // or implemented by a specific provider if we add one.
                self.create_error_response(
                    request,
                    RpcError {
                        code: "UNSUPPORTED_OPERATION".to_owned(),
                        message: "ExecuteTask operation not supported by base service".to_owned(),
                        context: HashMap::new(),
                        category: ErrorCategory::Configuration,
                    },
                    start_time,
                )
            }
        }
    }

    /// Handle collector registration request
    async fn handle_register_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        let RpcPayload::Registration(registration) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Registration payload for Register operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before registration invocation",
                deadline,
            );
        }

        let result = self
            .registration_provider
            .register_collector(registration)
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after registration completion",
                deadline,
            );
        }

        match result {
            Ok(response) => self.create_success_response(
                request,
                Some(RpcPayload::RegistrationResponse(response)),
                start_time,
            ),
            Err(e) => {
                let rpc_error = Self::map_registration_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle collector deregistration request
    async fn handle_deregister_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        let RpcPayload::Deregistration(deregistration) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Deregistration payload for Deregister operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before deregistration invocation",
                deadline,
            );
        }

        let result = self
            .registration_provider
            .deregister_collector(deregistration.clone())
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after deregistration completion",
                deadline,
            );
        }

        match result {
            Ok(()) => {
                let mut payload = HashMap::new();
                payload.insert(
                    "collector_id".to_owned(),
                    serde_json::json!(deregistration.collector_id),
                );
                payload.insert("status".to_owned(), serde_json::json!("deregistered"));
                if let Some(reason) = deregistration.reason {
                    payload.insert("reason".to_owned(), serde_json::json!(reason));
                }
                payload.insert("forced".to_owned(), serde_json::json!(deregistration.force));
                self.create_success_response(
                    request,
                    Some(RpcPayload::Generic(payload)),
                    start_time,
                )
            }
            Err(e) => {
                let rpc_error = Self::map_registration_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle collector start request
    async fn handle_start_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract lifecycle request from payload
        let RpcPayload::Lifecycle(lifecycle_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Lifecycle payload for Start operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before starting collector process",
                deadline,
            );
        }

        // Create collector configuration
        let mut collector_config = CollectorConfig {
            binary_path: self
                .process_manager
                .config()
                .collector_binaries
                .get(&lifecycle_request.collector_type)
                .cloned()
                .unwrap_or_default(),
            args: vec![],
            env: lifecycle_request.environment.clone().unwrap_or_default(),
            working_dir: lifecycle_request
                .working_directory
                .as_ref()
                .map(std::convert::Into::into),
            resource_limits: lifecycle_request.resource_limits.as_ref().map(|limits| {
                crate::process_manager::ResourceLimits {
                    max_memory_bytes: limits.max_memory_bytes,
                    // SAFETY: CPU percent is clamped to 0-100, so f64-to-u32 truncation is safe.
                    #[allow(clippy::as_conversions)]
                    max_cpu_percent: limits.max_cpu_percent.map(|p| p as u32),
                }
            }),
            auto_restart: self.process_manager.config().enable_auto_restart,
            max_restarts: 3,
        };

        // Add config overrides as command-line args
        if let Some(ref overrides) = lifecycle_request.config_overrides {
            for (key, value) in overrides {
                collector_config.args.push(format!("--{key}"));
                collector_config.args.push(value.to_string());
            }
        }

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after preparing collector configuration",
                deadline,
            );
        }

        // Start the collector
        let start_result = self
            .process_manager
            .start_collector(
                &lifecycle_request.collector_id,
                &lifecycle_request.collector_type,
                collector_config,
            )
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after collector start invocation",
                deadline,
            );
        }

        match start_result {
            Ok(pid) => {
                let mut context = HashMap::new();
                context.insert("pid".to_owned(), serde_json::Value::Number(pid.into()));
                let payload = RpcPayload::Generic(context);
                self.create_success_response(request, Some(payload), start_time)
            }
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle collector stop request
    async fn handle_stop_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract lifecycle request from payload
        let RpcPayload::Lifecycle(lifecycle_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Lifecycle payload for Stop operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        // Determine timeout
        let timeout = self.process_manager.config().default_graceful_timeout;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before stop request dispatch",
                deadline,
            );
        }

        // Stop the collector
        let stop_result = self
            .process_manager
            .stop_collector(&lifecycle_request.collector_id, true, timeout)
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after stop request completion",
                deadline,
            );
        }

        match stop_result {
            Ok(exit_code) => {
                let mut context = HashMap::new();
                if let Some(code) = exit_code {
                    context.insert(
                        "exit_code".to_owned(),
                        serde_json::Value::Number(code.into()),
                    );
                }
                let payload = RpcPayload::Generic(context);
                self.create_success_response(request, Some(payload), start_time)
            }
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle collector restart request
    async fn handle_restart_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract lifecycle request from payload
        let RpcPayload::Lifecycle(lifecycle_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Lifecycle payload for Restart operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        // Determine timeout (stop + start)
        let timeout = self
            .process_manager
            .config()
            .default_graceful_timeout
            .saturating_add(Duration::from_secs(10));

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before restart request dispatch",
                deadline,
            );
        }

        // Restart the collector
        let restart_result = self
            .process_manager
            .restart_collector(&lifecycle_request.collector_id, timeout)
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after restart request completion",
                deadline,
            );
        }

        match restart_result {
            Ok(pid) => {
                let mut context = HashMap::new();
                context.insert("pid".to_owned(), serde_json::Value::Number(pid.into()));
                let payload = RpcPayload::Generic(context);
                self.create_success_response(request, Some(payload), start_time)
            }
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle health check request
    async fn handle_health_check_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract collector_id from payload, or use service_id as fallback for empty payloads
        // NOTE: `_` is required for non-exhaustive enum variants.
        #[allow(clippy::wildcard_enum_match_arm)]
        let collector_id = match request.payload.clone() {
            RpcPayload::Lifecycle(req) => req.collector_id,
            RpcPayload::Generic(map) => {
                if let Some(serde_json::Value::String(id)) = map.get("collector_id").cloned() {
                    id
                } else {
                    return self.create_error_response(
                        request,
                        RpcError {
                            code: "INVALID_PAYLOAD".to_owned(),
                            message: "Missing collector_id in payload".to_owned(),
                            context: HashMap::new(),
                            category: ErrorCategory::Configuration,
                        },
                        start_time,
                    );
                }
            }
            // Accept empty payload and use the service's own ID
            // This is useful when the health check is sent to a specific collector's topic
            RpcPayload::Empty => self.service_id.clone(),
            _ => {
                return self.create_error_response(
                    request,
                    RpcError {
                        code: "INVALID_PAYLOAD".to_owned(),
                        message: "Invalid payload for HealthCheck operation".to_owned(),
                        context: HashMap::new(),
                        category: ErrorCategory::Configuration,
                    },
                    start_time,
                );
            }
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before health provider invocation",
                deadline,
            );
        }

        match self
            .health_provider
            .get_collector_health(&collector_id)
            .await
        {
            Ok(health_data) => self.create_success_response(
                request,
                Some(RpcPayload::HealthCheck(health_data)),
                start_time,
            ),
            // NOTE: `_` is required for non-exhaustive enum variants.
            #[allow(clippy::wildcard_enum_match_arm)]
            Err(e) => match e {
                ProcessManagerError::ProcessNotFound(_) => {
                    let health_data = HealthCheckData {
                        collector_id: collector_id.clone(),
                        status: HealthStatus::Unhealthy,
                        components: HashMap::new(),
                        metrics: HashMap::new(),
                        last_heartbeat: SystemTime::now(),
                        uptime_seconds: 0,
                        error_count: 0,
                    };
                    self.create_success_response(
                        request,
                        Some(RpcPayload::HealthCheck(health_data)),
                        start_time,
                    )
                }
                other => {
                    let rpc_error = Self::map_process_error_to_rpc_error(&other);
                    self.create_error_response(request, rpc_error, start_time)
                }
            },
        }
    }

    /// Handle configuration update request
    async fn handle_config_update_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract ConfigUpdateRequest from payload
        let RpcPayload::ConfigUpdate(config_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Invalid payload for ConfigUpdate operation".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        let collector_id = config_request.collector_id.clone();

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before configuration update dispatch",
                deadline,
            );
        }

        info!(
            "Processing config update for collector {}, validate_only: {}, rollback_on_failure: {}",
            collector_id, config_request.validate_only, config_request.rollback_on_failure
        );

        let update_result = self
            .config_provider
            .update_config(
                &collector_id,
                config_request.config_changes.clone(),
                config_request.validate_only,
                config_request.rollback_on_failure,
            )
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after configuration update completion",
                deadline,
            );
        }

        match update_result {
            Ok(update) => {
                // Validate-only response
                if config_request.validate_only {
                    info!("Configuration validation successful for {}", collector_id);
                    let mut context = HashMap::new();
                    context.insert("validated".to_owned(), serde_json::json!(true));
                    context.insert("version".to_owned(), serde_json::json!(update.version));
                    return self.create_success_response(
                        request,
                        Some(RpcPayload::Generic(context)),
                        start_time,
                    );
                }

                let mut response_data = HashMap::new();
                response_data.insert("collector_id".to_owned(), serde_json::json!(collector_id));
                response_data.insert("version".to_owned(), serde_json::json!(update.version));
                response_data.insert(
                    "changed_fields".to_owned(),
                    serde_json::json!(update.changed_fields),
                );
                response_data.insert(
                    "restarted".to_owned(),
                    serde_json::json!(update.restart_performed),
                );
                response_data.insert(
                    "timestamp".to_owned(),
                    serde_json::json!(
                        update
                            .timestamp
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    ),
                );

                info!("Successfully updated configuration for {}", collector_id);

                self.create_success_response(
                    request,
                    Some(RpcPayload::Generic(response_data)),
                    start_time,
                )
            }
            Err(e) => {
                error!("Configuration update failed for {}: {}", collector_id, e);
                self.create_error_response(
                    request,
                    Self::map_config_error_to_rpc_error(e),
                    start_time,
                )
            }
        }
    }

    /// Map configuration manager error to RPC error
    fn map_config_error_to_rpc_error(error: ConfigManagerError) -> RpcError {
        use crate::config_manager::ConfigManagerError;

        let mut context = HashMap::new();

        // NOTE: `_` is required for non-exhaustive enum variants.
        #[allow(clippy::wildcard_enum_match_arm)]
        match error {
            ConfigManagerError::ConfigNotFound(msg) => {
                context.insert("details".to_owned(), serde_json::json!(msg));
                RpcError {
                    code: "CONFIG_NOT_FOUND".to_owned(),
                    message: format!("Configuration not found: {msg}"),
                    context,
                    category: ErrorCategory::Configuration,
                }
            }
            ConfigManagerError::ValidationFailed(msg) => {
                context.insert("validation_error".to_owned(), serde_json::json!(msg));
                RpcError {
                    code: "VALIDATION_FAILED".to_owned(),
                    message: format!("Configuration validation failed: {msg}"),
                    context,
                    category: ErrorCategory::Configuration,
                }
            }
            ConfigManagerError::PersistenceFailed(msg) => {
                context.insert("details".to_owned(), serde_json::json!(msg));
                RpcError {
                    code: "PERSISTENCE_FAILED".to_owned(),
                    message: format!("Failed to persist configuration: {msg}"),
                    context,
                    category: ErrorCategory::Internal,
                }
            }
            ConfigManagerError::RollbackFailed(msg) => {
                context.insert("details".to_owned(), serde_json::json!(msg));
                RpcError {
                    code: "ROLLBACK_FAILED".to_owned(),
                    message: format!("Failed to rollback configuration: {msg}"),
                    context,
                    category: ErrorCategory::Internal,
                }
            }
            ConfigManagerError::InvalidConfigChange(msg) => {
                context.insert("details".to_owned(), serde_json::json!(msg));
                RpcError {
                    code: "INVALID_CONFIG_CHANGE".to_owned(),
                    message: format!("Invalid configuration change: {msg}"),
                    context,
                    category: ErrorCategory::Configuration,
                }
            }
            _ => {
                context.insert("error".to_owned(), serde_json::json!(error.to_string()));
                RpcError {
                    code: "CONFIGURATION_ERROR".to_owned(),
                    message: format!("Configuration error: {error}"),
                    context,
                    category: ErrorCategory::Internal,
                }
            }
        }
    }

    /// Handle capabilities request
    #[allow(clippy::unused_async)]
    async fn handle_capabilities_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before capabilities retrieval",
                deadline,
            );
        }

        let capabilities_data = CapabilitiesData {
            collector_id: self.service_id.clone(),
            event_types: vec!["process".to_owned()],
            operations: self.supported_operations.clone(),
            resource_requirements: ResourceRequirements {
                min_memory_bytes: 50 * 1024 * 1024, // 50MB
                min_cpu_percent: 1.0,
                required_privileges: vec![],
                required_capabilities: vec![],
            },
            platform_support: vec!["linux".to_owned(), "macos".to_owned(), "windows".to_owned()],
            features: HashMap::new(),
        };

        self.create_success_response(
            request,
            Some(RpcPayload::Capabilities(capabilities_data)),
            start_time,
        )
    }

    /// Handle graceful shutdown request
    async fn handle_graceful_shutdown_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract shutdown request from payload
        let RpcPayload::Shutdown(shutdown_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Shutdown payload".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        let timeout = Duration::from_millis(shutdown_request.graceful_timeout_ms);

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before graceful shutdown invocation",
                deadline,
            );
        }

        // Stop the collector gracefully
        let shutdown_result = self
            .process_manager
            .stop_collector(&shutdown_request.collector_id, true, timeout)
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after graceful shutdown completion",
                deadline,
            );
        }

        match shutdown_result {
            Ok(_) => self.create_success_response(request, None, start_time),
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle force shutdown request
    async fn handle_force_shutdown_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract shutdown request from payload
        let RpcPayload::Shutdown(shutdown_request) = request.payload.clone() else {
            return self.create_error_response(
                request,
                RpcError {
                    code: "INVALID_PAYLOAD".to_owned(),
                    message: "Expected Shutdown payload".to_owned(),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        };

        // Force kill with short timeout
        let timeout = Duration::from_secs(5);
        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before force shutdown invocation",
                deadline,
            );
        }

        let shutdown_result = self
            .process_manager
            .stop_collector(&shutdown_request.collector_id, false, timeout)
            .await;

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "after force shutdown completion",
                deadline,
            );
        }

        match shutdown_result {
            Ok(_) => self.create_success_response(request, None, start_time),
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle pause request
    async fn handle_pause_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract collector_id from payload
        // NOTE: `_` is required for non-exhaustive enum variants.
        #[allow(clippy::wildcard_enum_match_arm)]
        let collector_id = match request.payload.clone() {
            RpcPayload::Lifecycle(req) => req.collector_id,
            RpcPayload::Generic(map) => {
                if let Some(serde_json::Value::String(id)) = map.get("collector_id").cloned() {
                    id
                } else {
                    return self.create_error_response(
                        request,
                        RpcError {
                            code: "INVALID_PAYLOAD".to_owned(),
                            message: "Missing collector_id in payload".to_owned(),
                            context: HashMap::new(),
                            category: ErrorCategory::Configuration,
                        },
                        start_time,
                    );
                }
            }
            _ => {
                return self.create_error_response(
                    request,
                    RpcError {
                        code: "INVALID_PAYLOAD".to_owned(),
                        message: "Invalid payload for Pause operation".to_owned(),
                        context: HashMap::new(),
                        category: ErrorCategory::Configuration,
                    },
                    start_time,
                );
            }
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before pause invocation",
                deadline,
            );
        }

        // Pause the collector
        match self.process_manager.pause_collector(&collector_id).await {
            Ok(()) => self.create_success_response(request, None, start_time),
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Handle resume request
    async fn handle_resume_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        deadline: SystemTime,
    ) -> RpcResponse {
        // Extract collector_id from payload
        // NOTE: `_` is required for non-exhaustive enum variants.
        #[allow(clippy::wildcard_enum_match_arm)]
        let collector_id = match request.payload.clone() {
            RpcPayload::Lifecycle(req) => req.collector_id,
            RpcPayload::Generic(map) => {
                if let Some(serde_json::Value::String(id)) = map.get("collector_id").cloned() {
                    id
                } else {
                    return self.create_error_response(
                        request,
                        RpcError {
                            code: "INVALID_PAYLOAD".to_owned(),
                            message: "Missing collector_id in payload".to_owned(),
                            context: HashMap::new(),
                            category: ErrorCategory::Configuration,
                        },
                        start_time,
                    );
                }
            }
            _ => {
                return self.create_error_response(
                    request,
                    RpcError {
                        code: "INVALID_PAYLOAD".to_owned(),
                        message: "Invalid payload for Resume operation".to_owned(),
                        context: HashMap::new(),
                        category: ErrorCategory::Configuration,
                    },
                    start_time,
                );
            }
        };

        if Self::deadline_has_passed(deadline) {
            return self.create_deadline_error(
                request,
                start_time,
                "before resume invocation",
                deadline,
            );
        }

        // Resume the collector
        match self.process_manager.resume_collector(&collector_id).await {
            Ok(()) => self.create_success_response(request, None, start_time),
            Err(e) => {
                let rpc_error = Self::map_process_error_to_rpc_error(&e);
                self.create_error_response(request, rpc_error, start_time)
            }
        }
    }

    /// Map `ProcessManagerError` to `RpcError`
    fn map_process_error_to_rpc_error(error: &ProcessManagerError) -> RpcError {
        let (code, message, category) = match *error {
            ProcessManagerError::ProcessNotFound(ref id) => (
                "PROCESS_NOT_FOUND",
                format!("Collector not found: {id}"),
                ErrorCategory::Resource,
            ),
            ProcessManagerError::AlreadyRunning(ref id) => (
                "ALREADY_RUNNING",
                format!("Collector already running: {id}"),
                ErrorCategory::Resource,
            ),
            ProcessManagerError::SpawnFailed(ref msg) => (
                "SPAWN_FAILED",
                format!("Failed to spawn process: {msg}"),
                ErrorCategory::Internal,
            ),
            ProcessManagerError::TerminateFailed(ref msg) => (
                "TERMINATE_FAILED",
                format!("Failed to terminate process: {msg}"),
                ErrorCategory::Internal,
            ),
            ProcessManagerError::InvalidState(ref msg) => (
                "INVALID_STATE",
                format!("Invalid state: {msg}"),
                ErrorCategory::Configuration,
            ),
            ProcessManagerError::ConfigurationError(ref msg) => (
                "CONFIGURATION_ERROR",
                format!("Configuration error: {msg}"),
                ErrorCategory::Configuration,
            ),
            ProcessManagerError::PlatformNotSupported(ref msg) => (
                "PLATFORM_NOT_SUPPORTED",
                format!("Operation not supported: {msg}"),
                ErrorCategory::Configuration,
            ),
            ProcessManagerError::Timeout(ref msg) => (
                "TIMEOUT",
                format!("Operation timed out: {msg}"),
                ErrorCategory::Timeout,
            ),
            ProcessManagerError::Io(ref err) => (
                "IO_ERROR",
                format!("I/O error: {err}"),
                ErrorCategory::Internal,
            ),
        };

        let mut context = HashMap::new();
        context.insert(
            "error_type".to_owned(),
            serde_json::Value::String(code.to_owned()),
        );

        RpcError {
            code: code.to_owned(),
            message,
            context,
            category,
        }
    }

    /// Map registration errors to RPC error payloads.
    fn map_registration_error_to_rpc_error(error: &RegistrationError) -> RpcError {
        let (code, message, category) = match *error {
            RegistrationError::AlreadyRegistered(ref id) => (
                "ALREADY_REGISTERED",
                format!("Collector `{id}` is already registered"),
                ErrorCategory::Configuration,
            ),
            RegistrationError::NotFound(ref id) => (
                "REGISTRATION_NOT_FOUND",
                format!("Collector `{id}` is not registered"),
                ErrorCategory::Configuration,
            ),
            RegistrationError::Validation(ref msg) => (
                "REGISTRATION_INVALID",
                format!("Invalid registration request: {msg}"),
                ErrorCategory::Configuration,
            ),
            RegistrationError::Expired(ref msg) => (
                "REGISTRATION_EXPIRED",
                format!("Registration expired: {msg}"),
                ErrorCategory::Timeout,
            ),
            RegistrationError::Internal(ref msg) => (
                "REGISTRATION_INTERNAL",
                format!("Registration internal error: {msg}"),
                ErrorCategory::Internal,
            ),
        };

        let mut context = HashMap::new();
        context.insert(
            "error".to_owned(),
            serde_json::Value::String(error.to_string()),
        );

        RpcError {
            code: code.to_owned(),
            message,
            context,
            category,
        }
    }

    fn create_deadline_error(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
        stage: &str,
        deadline: SystemTime,
    ) -> RpcResponse {
        let mut context = HashMap::new();
        context.insert(
            "stage".to_owned(),
            serde_json::Value::String(stage.to_owned()),
        );
        context.insert(
            "deadline_epoch_seconds".to_owned(),
            serde_json::json!(Self::unix_timestamp(deadline)),
        );
        context.insert(
            "now_epoch_seconds".to_owned(),
            serde_json::json!(Self::unix_timestamp(SystemTime::now())),
        );

        let error = RpcError {
            code: "DEADLINE_EXCEEDED".to_owned(),
            message: format!("Request deadline exceeded while {stage}"),
            context,
            category: ErrorCategory::Timeout,
        };

        self.create_error_response(request, error, start_time)
    }

    fn deadline_has_passed(deadline: SystemTime) -> bool {
        SystemTime::now().duration_since(deadline).is_ok()
    }

    fn unix_timestamp(time: SystemTime) -> u64 {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    /// Create a success response
    fn create_success_response(
        &self,
        request: RpcRequest,
        payload: Option<RpcPayload>,
        start_time: SystemTime,
    ) -> RpcResponse {
        let now = SystemTime::now();
        let execution_time = start_time.elapsed().unwrap_or(Duration::ZERO);
        let queue_time = start_time
            .duration_since(request.timestamp)
            .unwrap_or(Duration::ZERO);
        let total_time = now
            .duration_since(request.timestamp)
            .unwrap_or(Duration::ZERO);
        let correlation_metadata = request.correlation_metadata.clone().increment_hop();

        // SAFETY: Duration::as_millis() returns u128; values for elapsed times fit in u64.
        #[allow(clippy::as_conversions)]
        RpcResponse {
            request_id: request.request_id,
            service_id: self.service_id.clone(),
            operation: request.operation,
            status: RpcStatus::Success,
            payload,
            timestamp: now,
            execution_time_ms: execution_time.as_millis() as u64,
            queue_time_ms: Some(queue_time.as_millis() as u64),
            total_time_ms: total_time.as_millis() as u64,
            error_details: None,
            correlation_metadata,
        }
    }

    /// Create an error response
    fn create_error_response(
        &self,
        request: RpcRequest,
        error: RpcError,
        start_time: SystemTime,
    ) -> RpcResponse {
        let now = SystemTime::now();
        let execution_time = start_time.elapsed().unwrap_or(Duration::ZERO);
        let queue_time = start_time
            .duration_since(request.timestamp)
            .unwrap_or(Duration::ZERO);
        let total_time = now
            .duration_since(request.timestamp)
            .unwrap_or(Duration::ZERO);
        let correlation_metadata = request.correlation_metadata.clone().increment_hop();

        // SAFETY: Duration::as_millis() returns u128; values for elapsed times fit in u64.
        #[allow(clippy::as_conversions)]
        RpcResponse {
            request_id: request.request_id,
            service_id: self.service_id.clone(),
            operation: request.operation,
            status: RpcStatus::Error,
            payload: None,
            timestamp: now,
            execution_time_ms: execution_time.as_millis() as u64,
            queue_time_ms: Some(queue_time.as_millis() as u64),
            total_time_ms: total_time.as_millis() as u64,
            error_details: Some(error),
            correlation_metadata,
        }
    }
}
