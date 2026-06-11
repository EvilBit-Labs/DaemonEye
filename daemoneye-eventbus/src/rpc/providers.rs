//! Provider traits for collector RPC operations and their default implementations.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{error, warn};

use crate::config_manager::{ConfigManager, ConfigManagerError};
use crate::process_manager::HealthStatus as ProcessHealthStatus;
use crate::{CollectorConfig, CollectorProcessManager, ProcessManagerError};

use super::messages::{
    ComponentHealth, ConfigChangeNotification, ConfigUpdateResult, DeregistrationRequest,
    HealthCheckData, HealthStatus, RegistrationRequest, RegistrationResponse,
};
use crate::broker::DaemoneyeBroker;

/// Provider interface for health data (typically implemented by the agent)
#[async_trait]
pub trait HealthProvider: Send + Sync {
    async fn get_collector_health(
        &self,
        collector_id: &str,
    ) -> std::result::Result<HealthCheckData, ProcessManagerError>;
}

/// Provider interface for configuration management (typically implemented by the agent)
#[async_trait]
pub trait ConfigProvider: Send + Sync {
    async fn get_config(
        &self,
        collector_id: &str,
    ) -> std::result::Result<CollectorConfig, ConfigManagerError>;

    async fn update_config(
        &self,
        collector_id: &str,
        changes: HashMap<String, serde_json::Value>,
        validate_only: bool,
        rollback_on_failure: bool,
    ) -> std::result::Result<ConfigUpdateResult, ConfigManagerError>;

    async fn validate_config(
        &self,
        _collector_id: &str,
        config: &CollectorConfig,
    ) -> std::result::Result<(), ConfigManagerError> {
        // Default no-op; concrete implementations may use this.
        let _ = config;
        Ok(())
    }
}

/// Errors that can occur during collector registration lifecycle management.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// Collector is already registered with the registry.
    #[error("collector `{0}` is already registered")]
    AlreadyRegistered(String),
    /// Collector is not present in the registry.
    #[error("collector `{0}` not found")]
    NotFound(String),
    /// Registration request failed validation.
    #[error("invalid registration: {0}")]
    Validation(String),
    /// Registration lease expired or is considered stale.
    #[error("registration expired: {0}")]
    Expired(String),
    /// Catch-all for unexpected errors.
    #[error("internal registration error: {0}")]
    Internal(String),
}

/// Provider interface for collector registration state.
#[async_trait]
pub trait RegistrationProvider: Send + Sync {
    /// Register a collector with the agent.
    async fn register_collector(
        &self,
        request: RegistrationRequest,
    ) -> std::result::Result<RegistrationResponse, RegistrationError>;

    /// Deregister a collector from the agent.
    async fn deregister_collector(
        &self,
        request: DeregistrationRequest,
    ) -> std::result::Result<(), RegistrationError>;

    /// Update the heartbeat timestamp for a collector; default implementation is a no-op.
    async fn update_heartbeat(
        &self,
        collector_id: &str,
    ) -> std::result::Result<(), RegistrationError> {
        let _ = collector_id;
        Ok(())
    }
}

// -----------------
// Default providers
// -----------------

#[derive(Debug)]
pub(super) struct DefaultHealthProvider {
    pub(super) process_manager: Arc<CollectorProcessManager>,
}

#[async_trait]
impl HealthProvider for DefaultHealthProvider {
    async fn get_collector_health(
        &self,
        collector_id: &str,
    ) -> std::result::Result<HealthCheckData, ProcessManagerError> {
        // Fetch status and health from process manager
        let status = self
            .process_manager
            .get_collector_status(collector_id)
            .await?;
        let health = self
            .process_manager
            .check_collector_health(collector_id)
            .await?;

        // Components
        let mut components = HashMap::new();
        components.insert(
            "process".to_owned(),
            ComponentHealth {
                name: "process".to_owned(),
                status: match health {
                    ProcessHealthStatus::Healthy => HealthStatus::Healthy,
                    ProcessHealthStatus::Degraded => HealthStatus::Degraded,
                    ProcessHealthStatus::Unhealthy => HealthStatus::Unhealthy,
                    ProcessHealthStatus::Unknown => HealthStatus::Unknown,
                },
                message: Some(format!("PID: {}, State: {:?}", status.pid, status.state)),
                last_check: SystemTime::now(),
                check_interval_seconds: 60,
            },
        );

        let heartbeat_status = if status.missed_heartbeats >= 3 {
            HealthStatus::Unhealthy
        } else if status.missed_heartbeats > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let heartbeat_age = SystemTime::now()
            .duration_since(status.last_heartbeat)
            .unwrap_or_default()
            .as_secs();

        components.insert(
            "heartbeat".to_owned(),
            ComponentHealth {
                name: "heartbeat".to_owned(),
                status: heartbeat_status,
                message: Some(format!(
                    "Last heartbeat: {}s ago, Missed: {}",
                    heartbeat_age, status.missed_heartbeats
                )),
                last_check: status.last_heartbeat,
                check_interval_seconds: 30,
            },
        );

        components.insert(
            "event_sources".to_owned(),
            ComponentHealth {
                name: "event_sources".to_owned(),
                status: HealthStatus::Healthy,
                message: Some("Event sources operational".to_owned()),
                last_check: SystemTime::now(),
                check_interval_seconds: 60,
            },
        );

        let overall_status = components
            .values()
            .map(|c| c.status)
            .min()
            .unwrap_or(HealthStatus::Unknown);

        let mut metrics = HashMap::new();
        metrics.insert("pid".to_owned(), f64::from(status.pid));
        metrics.insert("restart_count".to_owned(), f64::from(status.restart_count));
        // SAFETY: u64 seconds and heartbeat_age (u64) are converted to f64 for metric storage;
        // values are bounded by practical uptime limits well within f64 precision.
        #[allow(clippy::as_conversions)]
        metrics.insert("uptime_seconds".to_owned(), status.uptime.as_secs() as f64);
        metrics.insert(
            "missed_heartbeats".to_owned(),
            f64::from(status.missed_heartbeats),
        );
        // SAFETY: heartbeat_age is a u64 seconds value; safe to cast to f64 for metric purposes.
        #[allow(clippy::as_conversions)]
        metrics.insert(
            "last_heartbeat_age_seconds".to_owned(),
            heartbeat_age as f64,
        );
        metrics.insert("error_count".to_owned(), 0.0);

        Ok(HealthCheckData {
            collector_id: collector_id.to_owned(),
            status: overall_status,
            components,
            metrics,
            last_heartbeat: status.last_heartbeat,
            uptime_seconds: status.uptime.as_secs(),
            error_count: 0,
        })
    }
}

#[derive(Debug)]
pub(super) struct DefaultRegistrationProvider {
    records: RwLock<HashMap<String, RegistrationRecord>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RegistrationRecord {
    _request: RegistrationRequest,
    _registered_at: SystemTime,
    last_heartbeat: SystemTime,
}

impl Default for DefaultRegistrationProvider {
    fn default() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }
}

#[derive(Debug)]
pub(super) struct DefaultConfigProvider {
    pub(super) config_manager: Arc<ConfigManager>,
    pub(super) process_manager: Arc<CollectorProcessManager>,
    pub(super) broker: Option<Arc<DaemoneyeBroker>>,
}

#[async_trait]
impl ConfigProvider for DefaultConfigProvider {
    async fn get_config(
        &self,
        collector_id: &str,
    ) -> std::result::Result<CollectorConfig, ConfigManagerError> {
        self.config_manager.get_config(collector_id).await
    }

    async fn update_config(
        &self,
        collector_id: &str,
        changes: HashMap<String, serde_json::Value>,
        validate_only: bool,
        rollback_on_failure: bool,
    ) -> std::result::Result<ConfigUpdateResult, ConfigManagerError> {
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
                timestamp: SystemTime::now(),
            });
        }

        let restart_needed = ConfigManager::requires_restart(&current, &snapshot.config);
        let changed_fields = ConfigManager::get_changed_fields(&current, &snapshot.config);

        if restart_needed {
            let restart_timeout = Duration::from_secs(30);
            // Propagate PM errors as config errors? Keep separate; here we bubble via panic? No, map later at service.
            if let Err(e) = self
                .process_manager
                .restart_collector(collector_id, restart_timeout)
                .await
            {
                // Surface as validation-like failure to preserve existing mapping category
                return Err(ConfigManagerError::PersistenceFailed(format!(
                    "Failed to restart collector after config update: {e}"
                )));
            }
        } else if let Some(ref broker) = self.broker {
            // Publish hot-reload notification
            let topic = format!("control.collector.config.{collector_id}");
            let notification = ConfigChangeNotification {
                collector_id: collector_id.to_owned(),
                changed_fields: changed_fields.clone(),
                version: snapshot.version,
                timestamp: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };
            match serde_json::to_vec(&notification) {
                Ok(payload) => {
                    if let Err(e) = broker
                        .publish(&topic, &format!("config-change-{collector_id}"), payload)
                        .await
                    {
                        warn!(
                            collector_id = %collector_id,
                            topic = %topic,
                            error = %e,
                            "Failed to publish config change notification - collector may not hot-reload"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to serialize config change notification"
                    );
                }
            }
        }

        Ok(ConfigUpdateResult {
            version: snapshot.version,
            changed_fields,
            restart_performed: restart_needed,
            timestamp: SystemTime::now(),
        })
    }

    async fn validate_config(
        &self,
        _collector_id: &str,
        config: &CollectorConfig,
    ) -> std::result::Result<(), ConfigManagerError> {
        self.config_manager.validate_config(config).await
    }
}

#[async_trait]
impl RegistrationProvider for DefaultRegistrationProvider {
    async fn register_collector(
        &self,
        request: RegistrationRequest,
    ) -> std::result::Result<RegistrationResponse, RegistrationError> {
        let heartbeat_interval = request.heartbeat_interval_ms.unwrap_or(30_000);
        let mut guard = self.records.write().await;
        if guard.contains_key(&request.collector_id) {
            return Err(RegistrationError::AlreadyRegistered(
                request.collector_id.clone(),
            ));
        }

        let now = SystemTime::now();
        let response = RegistrationResponse {
            collector_id: request.collector_id.clone(),
            accepted: true,
            heartbeat_interval_ms: heartbeat_interval,
            assigned_topics: Vec::new(),
            message: None,
        };

        guard.insert(
            request.collector_id.clone(),
            RegistrationRecord {
                _request: request,
                _registered_at: now,
                last_heartbeat: now,
            },
        );
        drop(guard);

        Ok(response)
    }

    async fn deregister_collector(
        &self,
        request: DeregistrationRequest,
    ) -> std::result::Result<(), RegistrationError> {
        let mut guard = self.records.write().await;
        match guard.remove(&request.collector_id) {
            Some(_) => Ok(()),
            None => Err(RegistrationError::NotFound(request.collector_id)),
        }
    }

    async fn update_heartbeat(
        &self,
        collector_id: &str,
    ) -> std::result::Result<(), RegistrationError> {
        let mut guard = self.records.write().await;
        if let Some(record) = guard.get_mut(collector_id) {
            record.last_heartbeat = SystemTime::now();
            Ok(())
        } else {
            Err(RegistrationError::NotFound(collector_id.to_owned()))
        }
    }
}
