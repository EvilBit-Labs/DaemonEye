//! Provider implementations for collector-core RPC services.
//!
//! These providers supply health, configuration, and registration data to the
//! `CollectorRpcService` from daemoneye-eventbus.

use crate::collector::CollectorRuntime;
use crate::performance::PerformanceMonitor;
use daemoneye_eventbus::rpc::{
    ComponentHealth, ConfigProvider, ConfigUpdateResult, HealthCheckData, HealthProvider,
    HealthStatus, RegistrationError, RegistrationProvider, RegistrationRequest,
    RegistrationResponse,
};
use daemoneye_lib::telemetry::{HealthStatus as TelemetryHealthStatus, TelemetryCollector};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tracing::{debug, warn};

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
        debug!(%collector_id, "get_collector_health called");

        if collector_id != self.collector_id {
            warn!(
                requested = %collector_id,
                expected = %self.collector_id,
                "collector_id mismatch in health request"
            );
            return Err(daemoneye_eventbus::ProcessManagerError::ProcessNotFound(
                collector_id.to_owned(),
            ));
        }

        let mut components = HashMap::new();
        let mut metrics = HashMap::new();

        // Get telemetry health
        debug!("acquiring outer telemetry lock");
        if let Some(telemetry) = self.telemetry.read().await.as_ref() {
            debug!("got outer telemetry lock, acquiring inner lock");
            let telemetry_guard = telemetry.read().await;
            debug!("got inner telemetry lock");
            let health_check = telemetry_guard.health_check();
            let telemetry_status = match health_check.status {
                TelemetryHealthStatus::Healthy => HealthStatus::Healthy,
                TelemetryHealthStatus::Degraded => HealthStatus::Degraded,
                TelemetryHealthStatus::Unhealthy => HealthStatus::Unhealthy,
                #[allow(clippy::wildcard_enum_match_arm)]
                TelemetryHealthStatus::Unknown | _ => HealthStatus::Unknown,
            };

            components.insert(
                "telemetry".to_owned(),
                ComponentHealth {
                    name: "telemetry".to_owned(),
                    status: telemetry_status,
                    message: Some(format!("Status: {:?}", health_check.status)),
                    last_check: SystemTime::now(),
                    check_interval_seconds: 60,
                },
            );

            let telemetry_metrics = telemetry_guard.get_metrics();
            metrics.insert(
                "operation_count".to_owned(),
                telemetry_metrics.operation_count as f64,
            );
            metrics.insert(
                "error_count".to_owned(),
                telemetry_metrics.error_count as f64,
            );

            // Add custom metrics if available
            for (key, value) in &telemetry_metrics.custom_data {
                metrics.insert(key.clone(), *value);
            }
        }

        // Get performance monitor metrics
        debug!("acquiring performance_monitor lock");
        if let Some(perf_monitor) = self.performance_monitor.read().await.as_ref() {
            debug!("got performance_monitor lock, collecting metrics");
            let perf_metrics = perf_monitor.collect_resource_metrics().await;
            debug!("got performance metrics");
            metrics.insert(
                "cpu_percent".to_owned(),
                perf_metrics.cpu.current_cpu_percent,
            );
            metrics.insert(
                "memory_bytes".to_owned(),
                perf_metrics.memory.current_memory_bytes as f64,
            );
            metrics.insert(
                "events_per_second".to_owned(),
                perf_metrics.throughput.events_per_second,
            );
        }

        // Get runtime stats if available
        debug!("acquiring outer runtime lock");
        if let Some(runtime) = self.runtime.read().await.as_ref() {
            debug!("got outer runtime lock, acquiring inner lock");
            let runtime_guard = runtime.read().await;
            debug!("got inner runtime lock");
            let stats = runtime_guard.get_runtime_stats();
            metrics.insert("events_processed".to_owned(), stats.events_processed as f64);
            metrics.insert("errors_total".to_owned(), stats.errors_total as f64);
            metrics.insert(
                "registered_sources".to_owned(),
                stats.registered_sources as f64,
            );
        }

        // Aggregate overall health
        debug!("aggregating overall health");
        let overall_status = components
            .values()
            .map(|c| c.status)
            .min()
            .unwrap_or(HealthStatus::Unknown);

        debug!(?overall_status, "returning health data");
        Ok(HealthCheckData {
            collector_id: collector_id.to_owned(),
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
                format!("Collector {collector_id} not found"),
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
                format!("Collector {collector_id} not found"),
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
            return Err(RegistrationError::NotFound(collector_id.to_owned()));
        }

        // Heartbeat update is a no-op for this provider
        Ok(())
    }
}
