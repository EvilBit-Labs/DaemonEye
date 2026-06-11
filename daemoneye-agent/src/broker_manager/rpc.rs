//! Collector RPC client management and RPC provider trait implementations.

use super::BrokerManager;
use crate::collector_registry::{CollectorRegistry, RegistryError};
use anyhow::{Context, Result};
use daemoneye_eventbus::rpc::{
    CollectorLifecycleRequest, CollectorOperation, CollectorRpcClient, ComponentHealth,
    ConfigProvider, ConfigUpdateResult, DeregistrationRequest, HealthCheckData, HealthProvider,
    HealthStatus, RegistrationError, RegistrationProvider, RegistrationRequest,
    RegistrationResponse, RpcPayload, RpcRequest, RpcStatus, ShutdownRequest, ShutdownType,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

impl BrokerManager {
    pub(super) async fn registry(
        &self,
    ) -> std::result::Result<Arc<CollectorRegistry>, RegistrationError> {
        let guard = self.collector_registry.read().await;
        guard.as_ref().cloned().ok_or_else(|| {
            RegistrationError::Internal("collector registry not initialized".to_owned())
        })
    }

    pub(super) fn map_registry_error(error: RegistryError) -> RegistrationError {
        match error {
            RegistryError::AlreadyRegistered(id) => RegistrationError::AlreadyRegistered(id),
            RegistryError::NotFound(id) => RegistrationError::NotFound(id),
            RegistryError::Validation(msg) => RegistrationError::Validation(msg),
        }
    }

    /// Create an RPC client for a collector
    pub async fn create_rpc_client(&self, collector_id: &str) -> Result<Arc<CollectorRpcClient>> {
        let broker = {
            let broker_guard = self.broker.read().await;
            Arc::clone(
                broker_guard
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Broker not available"))?,
            )
        };

        let target_topic = format!("control.collector.{collector_id}");
        let client = Arc::new(
            CollectorRpcClient::new(&target_topic, broker)
                .await
                .context("Failed to create RPC client")?,
        );

        // Store the client using double-checked locking. Two tasks racing on the
        // same collector_id can both reach here with a freshly built client; a
        // naive `insert` would overwrite (and leak the live subscription of) the
        // first. The `entry` API keeps whichever client was inserted first and
        // returns it, so concurrent creators converge on a single shared client.
        let stored = {
            let mut clients = self.rpc_clients.write().await;
            Arc::clone(
                clients
                    .entry(collector_id.to_owned())
                    .or_insert_with(|| Arc::clone(&client)),
            )
        };

        if Arc::ptr_eq(&stored, &client) {
            info!(
                collector_id = %collector_id,
                target_topic = %target_topic,
                "Created RPC client for collector"
            );
        } else {
            info!(
                collector_id = %collector_id,
                "RPC client already created by a concurrent task; reusing it"
            );
        }

        Ok(stored)
    }

    /// Start a collector via RPC
    #[allow(dead_code)]
    pub async fn start_collector_rpc(&self, collector_id: &str) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;
        let lifecycle_request = CollectorLifecycleRequest::start(collector_id, None);
        let request = RpcRequest::lifecycle(
            client.client_id.clone(),
            client.target_topic.clone(),
            CollectorOperation::Start,
            lifecycle_request,
            Duration::from_secs(30),
        );

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Start RPC failed: {:?}", response.error_details);
        }

        info!(collector_id = %collector_id, "Collector started via RPC");
        Ok(())
    }

    /// Stop a collector via RPC
    pub async fn stop_collector_rpc(&self, collector_id: &str, graceful: bool) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;

        if graceful {
            let shutdown_request = ShutdownRequest {
                collector_id: collector_id.to_owned(),
                shutdown_type: ShutdownType::Graceful,
                graceful_timeout_ms: 5000,
                force_after_timeout: true,
                reason: Some("Agent-initiated graceful shutdown".to_owned()),
            };
            let request = RpcRequest::shutdown(
                client.client_id.clone(),
                client.target_topic.clone(),
                shutdown_request,
                Duration::from_secs(5),
            );

            let response = client.call(request, Duration::from_secs(5)).await?;
            if response.status != RpcStatus::Success {
                anyhow::bail!("Graceful shutdown RPC failed: {:?}", response.error_details);
            }
        } else {
            let lifecycle_request = CollectorLifecycleRequest::stop(collector_id);
            let request = RpcRequest::lifecycle(
                client.client_id.clone(),
                client.target_topic.clone(),
                CollectorOperation::Stop,
                lifecycle_request,
                Duration::from_secs(5),
            );

            let response = client.call(request, Duration::from_secs(5)).await?;
            if response.status != RpcStatus::Success {
                anyhow::bail!("Stop RPC failed: {:?}", response.error_details);
            }
        }

        info!(collector_id = %collector_id, graceful = graceful, "Collector stopped via RPC");
        Ok(())
    }

    /// Restart a collector via RPC
    #[allow(dead_code)]
    pub async fn restart_collector_rpc(&self, collector_id: &str) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;
        let lifecycle_request = CollectorLifecycleRequest::restart(collector_id, None);
        let request = RpcRequest::lifecycle(
            client.client_id.clone(),
            client.target_topic.clone(),
            CollectorOperation::Restart,
            lifecycle_request,
            Duration::from_secs(30),
        );

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Restart RPC failed: {:?}", response.error_details);
        }

        info!(collector_id = %collector_id, "Collector restarted via RPC");
        Ok(())
    }

    /// Perform health check via RPC
    pub async fn health_check_rpc(&self, collector_id: &str) -> Result<HealthCheckData> {
        let client = self.get_rpc_client(collector_id).await?;
        let request = RpcRequest::health_check(
            client.client_id.clone(),
            client.target_topic.clone(),
            Duration::from_secs(10),
        );

        let response = client.call(request, Duration::from_secs(10)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Health check RPC failed: {:?}", response.error_details);
        }

        match response.payload {
            Some(RpcPayload::HealthCheck(health_data)) => Ok(health_data),
            _ => anyhow::bail!("Invalid health check response payload"),
        }
    }

    /// Execute a task via RPC
    pub async fn execute_task_rpc(
        &self,
        collector_id: &str,
        task: daemoneye_lib::proto::DetectionTask,
    ) -> Result<daemoneye_lib::proto::DetectionResult> {
        let client = self.get_rpc_client(collector_id).await?;

        let task_json =
            serde_json::to_value(&task).context("Failed to serialize detection task")?;

        #[allow(clippy::arithmetic_side_effects)] // Safe: SystemTime + Duration is well-defined
        let deadline = std::time::SystemTime::now() + Duration::from_secs(30);
        let request = RpcRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_id: client.client_id.clone(),
            target: client.target_topic.clone(),
            operation: CollectorOperation::ExecuteTask,
            payload: RpcPayload::Task(task_json),
            timestamp: std::time::SystemTime::now(),
            deadline,
            correlation_metadata: daemoneye_eventbus::rpc::RpcCorrelationMetadata::new(
                uuid::Uuid::new_v4().to_string(),
            ),
        };

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Execute task RPC failed: {:?}", response.error_details);
        }

        match response.payload {
            Some(RpcPayload::TaskResult(value)) => {
                let result: daemoneye_lib::proto::DetectionResult =
                    serde_json::from_value(value)
                        .context("Failed to deserialize detection result")?;
                Ok(result)
            }
            _ => anyhow::bail!("Invalid task execution response payload"),
        }
    }

    /// Get or create an RPC client for a collector
    pub async fn get_rpc_client(&self, collector_id: &str) -> Result<Arc<CollectorRpcClient>> {
        // Fast path: return an already-created client without taking the write lock.
        if let Some(client) = self.rpc_clients.read().await.get(collector_id) {
            return Ok(Arc::clone(client));
        }

        // Slow path: create_rpc_client re-checks the map under the write lock
        // (double-checked locking), so two tasks that both miss the read above
        // converge on a single shared client rather than leaking a duplicate.
        self.create_rpc_client(collector_id).await
    }

    /// List all registered collector IDs that have RPC clients
    pub async fn list_registered_collector_ids(&self) -> Vec<String> {
        let clients = self.rpc_clients.read().await;
        clients.keys().cloned().collect()
    }
}

// -------------------------------
// RPC Provider trait implementations
// -------------------------------

#[async_trait::async_trait]
impl HealthProvider for BrokerManager {
    async fn get_collector_health(
        &self,
        collector_id: &str,
    ) -> std::result::Result<HealthCheckData, daemoneye_eventbus::ProcessManagerError> {
        let status = self
            .process_manager
            .get_collector_status(collector_id)
            .await?;
        let health = self
            .process_manager
            .check_collector_health(collector_id)
            .await?;

        // Build component details
        let mut components = std::collections::HashMap::new();
        components.insert(
            "process".to_owned(),
            ComponentHealth {
                name: "process".to_owned(),
                status: match health {
                    daemoneye_eventbus::process_manager::HealthStatus::Healthy => {
                        HealthStatus::Healthy
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Degraded => {
                        HealthStatus::Degraded
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Unhealthy => {
                        HealthStatus::Unhealthy
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Unknown | _ => {
                        HealthStatus::Unknown
                    }
                },
                message: Some(format!("PID: {}, State: {:?}", status.pid, status.state)),
                last_check: std::time::SystemTime::now(),
                check_interval_seconds: self
                    .process_manager
                    .config()
                    .health_check_interval
                    .as_secs(),
            },
        );

        let hb_status = if status.missed_heartbeats >= 3 {
            HealthStatus::Unhealthy
        } else if status.missed_heartbeats > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let heartbeat_age = std::time::SystemTime::now()
            .duration_since(status.last_heartbeat)
            .unwrap_or_default()
            .as_secs();

        components.insert(
            "heartbeat".to_owned(),
            ComponentHealth {
                name: "heartbeat".to_owned(),
                status: hb_status,
                message: Some(format!(
                    "Last heartbeat: {}s ago, Missed: {}",
                    heartbeat_age, status.missed_heartbeats
                )),
                last_check: status.last_heartbeat,
                check_interval_seconds: self
                    .process_manager
                    .config()
                    .health_check_interval
                    .as_secs(),
            },
        );

        // Event sources health is not yet implemented
        components.insert(
            "event_sources".to_owned(),
            ComponentHealth {
                name: "event_sources".to_owned(),
                status: HealthStatus::Unknown,
                message: Some("Event sources health monitoring not yet implemented".to_owned()),
                last_check: std::time::SystemTime::now(),
                check_interval_seconds: 60,
            },
        );

        // Compute overall health using worst-of aggregation
        let overall = aggregate_worst_of(components.values().map(|c| c.status));

        #[allow(clippy::as_conversions)]
        // Safe: uptime_seconds and heartbeat_age are small u64 values
        let uptime_seconds_f64 = status.uptime.as_secs() as f64;
        #[allow(clippy::as_conversions)]
        let heartbeat_age_f64 = heartbeat_age as f64;

        let mut metrics = std::collections::HashMap::new();
        metrics.insert("pid".to_owned(), f64::from(status.pid));
        metrics.insert("restart_count".to_owned(), f64::from(status.restart_count));
        metrics.insert("uptime_seconds".to_owned(), uptime_seconds_f64);
        metrics.insert(
            "missed_heartbeats".to_owned(),
            f64::from(status.missed_heartbeats),
        );
        metrics.insert("last_heartbeat_age_seconds".to_owned(), heartbeat_age_f64);
        metrics.insert("error_count".to_owned(), 0.0);

        Ok(HealthCheckData {
            collector_id: collector_id.to_owned(),
            status: overall,
            components,
            metrics,
            last_heartbeat: status.last_heartbeat,
            uptime_seconds: status.uptime.as_secs(),
            error_count: 0,
        })
    }
}

#[async_trait::async_trait]
impl ConfigProvider for BrokerManager {
    async fn get_config(
        &self,
        collector_id: &str,
    ) -> std::result::Result<
        daemoneye_eventbus::CollectorConfig,
        daemoneye_eventbus::ConfigManagerError,
    > {
        self.config_manager.get_config(collector_id).await
    }

    async fn update_config(
        &self,
        collector_id: &str,
        changes: std::collections::HashMap<String, serde_json::Value>,
        validate_only: bool,
        rollback_on_failure: bool,
    ) -> std::result::Result<ConfigUpdateResult, daemoneye_eventbus::ConfigManagerError> {
        let current = self.config_manager.get_config(collector_id).await?;
        let snapshot = self
            .config_manager
            .update_config(
                collector_id,
                changes.clone(),
                validate_only,
                rollback_on_failure,
            )
            .await?;

        if validate_only {
            return Ok(ConfigUpdateResult {
                version: snapshot.version,
                changed_fields: Vec::new(),
                restart_performed: false,
                timestamp: std::time::SystemTime::now(),
            });
        }

        let restart_needed =
            daemoneye_eventbus::ConfigManager::requires_restart(&current, &snapshot.config);
        let changed_fields =
            daemoneye_eventbus::ConfigManager::get_changed_fields(&current, &snapshot.config);

        if restart_needed {
            let restart_timeout = Duration::from_secs(30);
            self.process_manager
                .restart_collector(collector_id, restart_timeout)
                .await
                .map_err(|e| {
                    daemoneye_eventbus::ConfigManagerError::PersistenceFailed(format!(
                        "Failed to restart collector after config update: {e}"
                    ))
                })?;
        } else {
            // Publish hot-reload notification if broker is present
            let broker_guard = self.broker.read().await;
            if let Some(broker) = broker_guard.as_ref() {
                let topic = format!("control.collector.config.{collector_id}");
                let notification = daemoneye_eventbus::rpc::ConfigChangeNotification {
                    collector_id: collector_id.to_owned(),
                    changed_fields: changed_fields.clone(),
                    version: snapshot.version,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                match serde_json::to_vec(&notification) {
                    Ok(payload) => {
                        if let Err(e) = broker
                            .publish(&topic, &format!("config-change-{collector_id}"), payload)
                            .await
                        {
                            tracing::warn!(
                                collector_id = %collector_id,
                                topic = %topic,
                                error = %e,
                                "Failed to publish config change notification - collector may not hot-reload"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            collector_id = %collector_id,
                            error = %e,
                            "Failed to serialize config change notification"
                        );
                    }
                }
            }
        }

        Ok(ConfigUpdateResult {
            version: snapshot.version,
            changed_fields,
            restart_performed: restart_needed,
            timestamp: std::time::SystemTime::now(),
        })
    }

    async fn validate_config(
        &self,
        _collector_id: &str,
        config: &daemoneye_eventbus::CollectorConfig,
    ) -> std::result::Result<(), daemoneye_eventbus::ConfigManagerError> {
        self.config_manager.validate_config(config).await
    }
}

// -------------------------------
// RPC Provider trait implementations
// -------------------------------

#[async_trait::async_trait]
impl RegistrationProvider for BrokerManager {
    async fn register_collector(
        &self,
        request: RegistrationRequest,
    ) -> std::result::Result<RegistrationResponse, RegistrationError> {
        let registry = self.registry().await?;
        let response = registry
            .register(request.clone())
            .await
            .map_err(Self::map_registry_error)?;

        // Create RPC client after successful registration
        if response.accepted {
            if let Err(e) = self.create_rpc_client(&request.collector_id).await {
                warn!(
                    collector_id = %request.collector_id,
                    error = %e,
                    "Failed to create RPC client after registration"
                );
            }

            // Mark collector as ready when it registers successfully
            // Registration indicates the collector has completed its initialization
            let all_ready = self.mark_collector_ready(&request.collector_id).await;

            // Check if we should transition to Ready state
            if all_ready {
                let current_state = self.agent_state().await;
                if matches!(current_state, super::state::AgentState::Loading) {
                    info!("All expected collectors are ready, agent can transition to Ready state");
                    // Note: The actual transition is triggered by main.rs or startup coordination
                    // to ensure proper privilege dropping and other startup steps
                }
            }
        }

        Ok(response)
    }

    async fn deregister_collector(
        &self,
        request: DeregistrationRequest,
    ) -> std::result::Result<(), RegistrationError> {
        let registry = self.registry().await?;
        registry
            .deregister(request.clone())
            .await
            .map_err(Self::map_registry_error)?;

        // Remove RPC client and shut it down
        let removed_client = self.rpc_clients.write().await.remove(&request.collector_id);
        if let Some(client) = removed_client
            && let Err(e) = client.shutdown().await
        {
            warn!(
                collector_id = %request.collector_id,
                error = %e,
                "Failed to shutdown RPC client during deregistration"
            );
        }

        Ok(())
    }

    async fn update_heartbeat(
        &self,
        collector_id: &str,
    ) -> std::result::Result<(), RegistrationError> {
        let registry = self.registry().await?;
        registry
            .update_heartbeat(collector_id)
            .await
            .map_err(Self::map_registry_error)
    }
}

/// Compute worst-of aggregation across component health statuses.
/// Returns Unhealthy if any component is Unhealthy, else Degraded if any is Degraded,
/// else Healthy if any is Healthy, else Unknown.
fn aggregate_worst_of<I: Iterator<Item = HealthStatus>>(iter: I) -> HealthStatus {
    let mut has_healthy = false;
    let mut has_degraded = false;
    let mut has_unhealthy = false;
    let mut has_unresponsive = false;

    for s in iter {
        match s {
            HealthStatus::Unhealthy => has_unhealthy = true,
            HealthStatus::Unresponsive => has_unresponsive = true,
            HealthStatus::Degraded => has_degraded = true,
            HealthStatus::Healthy => has_healthy = true,
            HealthStatus::Unknown | _ => {}
        }
    }

    // Worst-of aggregation: Unresponsive > Unhealthy > Degraded > Healthy > Unknown
    if has_unresponsive {
        HealthStatus::Unresponsive
    } else if has_unhealthy {
        HealthStatus::Unhealthy
    } else if has_degraded {
        HealthStatus::Degraded
    } else if has_healthy {
        HealthStatus::Healthy
    } else {
        // No components or all unknown
        HealthStatus::Unknown
    }
}
