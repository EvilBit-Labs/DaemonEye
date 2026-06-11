//! Broker lifecycle management: startup, shutdown, health checks, and accessors.

use super::BrokerManager;
use super::health::BrokerHealth;
use crate::health;
use anyhow::{Context, Result};
use daemoneye_eventbus::ConfigManager;
use daemoneye_eventbus::{
    DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventBusStatistics,
    process_manager::CollectorProcessManager,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

impl BrokerManager {
    /// Initialize and start the embedded broker
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Embedded broker is disabled in configuration");
            return Ok(());
        }

        // Update health status to starting
        *self.health_status.write().await = BrokerHealth::Starting;

        info!(
            socket_path = %self.config.socket_path,
            max_connections = self.config.max_connections,
            "Starting embedded DaemonEye EventBus broker"
        );

        // Ensure config directory exists
        if !self.config.config_directory.exists() {
            tokio::fs::create_dir_all(&self.config.config_directory)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create config directory: {}",
                        self.config.config_directory.display()
                    )
                })?;
            info!(
                config_dir = %self.config.config_directory.display(),
                "Created config directory"
            );
        }

        self.config
            .ensure_socket_directory()
            .context("Failed to prepare broker socket directory")?;

        // Create the broker instance
        let broker = DaemoneyeBroker::new(&self.config.socket_path)
            .await
            .context("Failed to create DaemoneyeBroker")?;

        // Create EventBus client from the broker (this will start the broker internally)
        let event_bus = DaemoneyeEventBus::from_broker(broker)
            .await
            .context("Failed to create DaemoneyeEventBus from broker")?;

        // Get the broker reference from the event bus
        let broker_arc = Arc::clone(event_bus.broker());

        // Store the broker and event bus
        *self.broker.write().await = Some(Arc::clone(&broker_arc));
        *self.event_bus.lock().await = Some(event_bus);

        // Initialize collector registry
        *self.collector_registry.write().await = Some(Arc::new(
            crate::collector_registry::CollectorRegistry::default(),
        ));

        // Update health status to healthy
        *self.health_status.write().await = BrokerHealth::Healthy;

        info!("Embedded DaemonEye EventBus broker started successfully");
        Ok(())
    }

    /// Gracefully shutdown the embedded broker
    pub async fn shutdown(&self) -> Result<()> {
        info!("Initiating graceful shutdown of embedded broker");

        // Update health status to shutting down
        *self.health_status.write().await = BrokerHealth::ShuttingDown;

        // Send graceful shutdown RPC to all collectors first
        info!("Sending graceful shutdown RPC to all collectors");
        let collector_ids: Vec<String> = {
            let clients = self.rpc_clients.read().await;
            clients.keys().cloned().collect()
        };

        for collector_id in &collector_ids {
            if let Err(e) = self.stop_collector_rpc(collector_id, true).await {
                warn!(
                    collector_id = %collector_id,
                    error = %e,
                    "Failed to send graceful shutdown RPC, will fall back to signal-based shutdown"
                );
            }
        }

        // Wait a bit for RPC responses
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Shutdown all managed collector processes (fallback to signals)
        info!("Shutting down managed collector processes");
        if let Err(e) = self.process_manager.shutdown_all().await {
            error!(error = %e, "Failed to shutdown all collector processes");
            // Continue with broker shutdown even if collector shutdown fails
        } else {
            info!("All collector processes shut down successfully");
        }

        // Clean up RPC clients
        {
            let mut clients = self.rpc_clients.write().await;
            for (collector_id, client) in clients.drain() {
                if let Err(e) = client.shutdown().await {
                    warn!(
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to shutdown RPC client"
                    );
                }
            }
        }

        // Send shutdown signal if available
        {
            let mut shutdown_tx_guard = self.shutdown_tx.lock().await;
            if let Some(tx) = shutdown_tx_guard.take()
                && tx.send(()).is_err()
            {
                warn!("Failed to send shutdown signal - receiver may have been dropped");
            }
        }

        // Shutdown the event bus first
        {
            let mut event_bus_guard = self.event_bus.lock().await;
            if let Some(mut event_bus) = event_bus_guard.take()
                && let Err(e) = event_bus.shutdown().await
            {
                error!(error = %e, "Failed to shutdown EventBus client");
            }
        }

        // Shutdown the broker with timeout
        let shutdown_timeout = Duration::from_secs(self.config.shutdown_timeout_seconds);
        let shutdown_result = tokio::time::timeout(shutdown_timeout, async {
            let broker_guard = self.broker.read().await;
            if let Some(broker) = broker_guard.as_ref() {
                broker.shutdown().await
            } else {
                Ok(())
            }
        })
        .await;

        match shutdown_result {
            Ok(Ok(())) => {
                info!("Embedded broker shutdown completed successfully");
            }
            Ok(Err(e)) => {
                error!(error = %e, "Error during broker shutdown");
                return Err(e.into());
            }
            Err(_) => {
                warn!(
                    timeout_seconds = self.config.shutdown_timeout_seconds,
                    "Broker shutdown timed out, forcing termination"
                );
            }
        }

        // Clear the broker reference
        *self.broker.write().await = None;

        // Clear collector registry
        self.collector_registry.write().await.take();

        // Update health status to stopped
        *self.health_status.write().await = BrokerHealth::Stopped;

        info!("Embedded broker shutdown complete");
        Ok(())
    }

    /// Get the current health status of the broker
    pub async fn health_status(&self) -> BrokerHealth {
        let health = self.health_status.read().await;
        health.clone()
    }

    /// Get broker statistics if available
    pub async fn statistics(&self) -> Option<EventBusStatistics> {
        let broker_guard = self.broker.read().await;
        if let Some(broker) = broker_guard.as_ref() {
            Some(broker.statistics().await)
        } else {
            None
        }
    }

    /// Get a reference to the `EventBus` client for agent operations
    #[allow(dead_code)]
    pub fn event_bus(&self) -> Arc<Mutex<Option<DaemoneyeEventBus>>> {
        Arc::clone(&self.event_bus)
    }

    /// Check if the broker is currently running
    #[allow(dead_code)]
    pub async fn is_running(&self) -> bool {
        let health = self.health_status.read().await;
        matches!(*health, BrokerHealth::Healthy | BrokerHealth::Starting)
    }

    /// Get the socket path for the broker
    pub fn socket_path(&self) -> &str {
        &self.config.socket_path
    }

    /// Get a reference to the process manager
    #[allow(dead_code)] // Public accessor for future use
    pub const fn process_manager(&self) -> &Arc<CollectorProcessManager> {
        &self.process_manager
    }

    /// Get a reference to the configuration manager
    #[allow(dead_code)]
    pub fn config_manager(&self) -> Arc<ConfigManager> {
        Arc::clone(&self.config_manager)
    }

    /// Perform a health check on the broker
    pub async fn health_check(&self) -> BrokerHealth {
        let current_health = self.health_status().await;

        match current_health {
            BrokerHealth::Healthy => {
                // Verify broker is actually responsive
                if let Some(stats) = self.statistics().await {
                    debug!(
                        messages_published = stats.messages_published,
                        active_subscribers = stats.active_subscribers,
                        uptime_seconds = stats.uptime_seconds,
                        "Broker health check passed"
                    );
                    // Aggregate collector health across all managed collectors
                    let collector_ids = self.process_manager.list_collector_ids().await;
                    let mut any_unhealthy = false;
                    let mut any_degraded = false;
                    for id in collector_ids {
                        match self.process_manager.check_collector_health(&id).await {
                            Ok(daemoneye_eventbus::process_manager::HealthStatus::Unhealthy) => {
                                any_unhealthy = true;
                                break;
                            }
                            Ok(daemoneye_eventbus::process_manager::HealthStatus::Degraded) => {
                                any_degraded = true;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(collector_id = %id, error = %e, "Failed to check collector health");
                                any_degraded = true;
                            }
                        }
                    }

                    if any_unhealthy {
                        let unhealthy_status = BrokerHealth::Unhealthy(
                            "One or more collectors are unhealthy".to_owned(),
                        );
                        let mut health = self.health_status.write().await;
                        *health = unhealthy_status.clone();
                        unhealthy_status
                    } else if any_degraded {
                        // Represent degraded collector state as Unhealthy with reason
                        let degraded_status = BrokerHealth::Unhealthy(
                            "One or more collectors are degraded".to_owned(),
                        );
                        let mut health = self.health_status.write().await;
                        *health = degraded_status.clone();
                        degraded_status
                    } else {
                        BrokerHealth::Healthy
                    }
                } else {
                    warn!("Broker health check failed - unable to get statistics");
                    let unhealthy_status =
                        BrokerHealth::Unhealthy("Unable to get statistics".to_owned());
                    let mut health = self.health_status.write().await;
                    *health = unhealthy_status.clone();
                    unhealthy_status
                }
            }
            BrokerHealth::Starting
            | BrokerHealth::ShuttingDown
            | BrokerHealth::Unhealthy(_)
            | BrokerHealth::Stopped => current_health,
        }
    }

    /// Wait for the broker to become healthy with a timeout
    pub async fn wait_for_healthy(&self, timeout: Duration) -> Result<()> {
        let health_status = Arc::clone(&self.health_status);
        health::wait_for_healthy(timeout, || async { health_status.read().await.clone() }).await
    }
}
