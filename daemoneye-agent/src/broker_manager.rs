//! Embedded EventBus broker management for daemoneye-agent
//!
//! This module provides the `BrokerManager` which embeds a `DaemoneyeBroker` instance
//! within the daemoneye-agent process architecture. The broker operates independently
//! of the IPC server for CLI communication and provides topic-based pub/sub messaging
//! for collector-core component coordination.

use anyhow::{Context, Result};
use daemoneye_eventbus::{DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventBusStatistics};
use daemoneye_lib::config::BrokerConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Health status of the embedded broker
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerHealth {
    /// Broker is healthy and operational
    Healthy,
    /// Broker is starting up
    Starting,
    /// Broker is shutting down
    ShuttingDown,
    /// Broker has encountered an error
    Unhealthy(String),
    /// Broker is stopped
    Stopped,
}

/// Embedded broker manager that coordinates the DaemoneyeBroker lifecycle
/// within the daemoneye-agent process architecture.
pub struct BrokerManager {
    /// Configuration for the broker
    config: BrokerConfig,
    /// The embedded broker instance
    broker: Arc<RwLock<Option<Arc<DaemoneyeBroker>>>>,
    /// EventBus client for agent-side operations
    event_bus: Arc<Mutex<Option<DaemoneyeEventBus>>>,
    /// Current health status
    health_status: Arc<RwLock<BrokerHealth>>,
    /// Shutdown signal sender
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl BrokerManager {
    /// Create a new broker manager with the given configuration
    pub fn new(config: BrokerConfig) -> Self {
        Self {
            config,
            broker: Arc::new(RwLock::new(None)),
            event_bus: Arc::new(Mutex::new(None)),
            health_status: Arc::new(RwLock::new(BrokerHealth::Stopped)),
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Initialize and start the embedded broker
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Embedded broker is disabled in configuration");
            return Ok(());
        }

        // Update health status to starting
        {
            let mut health = self.health_status.write().await;
            *health = BrokerHealth::Starting;
        }

        info!(
            socket_path = %self.config.socket_path,
            max_connections = self.config.max_connections,
            "Starting embedded DaemonEye EventBus broker"
        );

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
        {
            let mut broker_guard = self.broker.write().await;
            *broker_guard = Some(Arc::clone(&broker_arc));
        }

        {
            let mut event_bus_guard = self.event_bus.lock().await;
            *event_bus_guard = Some(event_bus);
        }

        // Update health status to healthy
        {
            let mut health = self.health_status.write().await;
            *health = BrokerHealth::Healthy;
        }

        info!("Embedded DaemonEye EventBus broker started successfully");
        Ok(())
    }

    /// Gracefully shutdown the embedded broker
    pub async fn shutdown(&self) -> Result<()> {
        info!("Initiating graceful shutdown of embedded broker");

        // Update health status to shutting down
        {
            let mut health = self.health_status.write().await;
            *health = BrokerHealth::ShuttingDown;
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
        {
            let mut broker_guard = self.broker.write().await;
            *broker_guard = None;
        }

        // Update health status to stopped
        {
            let mut health = self.health_status.write().await;
            *health = BrokerHealth::Stopped;
        }

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

    /// Get a reference to the EventBus client for agent operations
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
                    BrokerHealth::Healthy
                } else {
                    warn!("Broker health check failed - unable to get statistics");
                    let unhealthy_status =
                        BrokerHealth::Unhealthy("Unable to get statistics".to_string());
                    let mut health = self.health_status.write().await;
                    *health = unhealthy_status.clone();
                    unhealthy_status
                }
            }
            other => other,
        }
    }

    /// Wait for the broker to become healthy with a timeout
    pub async fn wait_for_healthy(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let health = self.health_status().await;
            match health {
                BrokerHealth::Healthy => {
                    debug!("Broker is healthy");
                    return Ok(());
                }
                BrokerHealth::Starting => {
                    debug!("Broker is still starting, waiting...");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                BrokerHealth::Unhealthy(ref error) => {
                    return Err(anyhow::anyhow!("Broker is unhealthy: {}", error));
                }
                BrokerHealth::ShuttingDown | BrokerHealth::Stopped => {
                    return Err(anyhow::anyhow!("Broker is not running"));
                }
            }
        }

        Err(anyhow::anyhow!(
            "Timeout waiting for broker to become healthy after {:?}",
            timeout
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemoneye_lib::config::BrokerConfig;

    #[tokio::test]
    async fn test_broker_manager_creation() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let health = manager.health_status().await;
        assert_eq!(health, BrokerHealth::Stopped);
        assert!(!manager.is_running().await);
    }

    #[tokio::test]
    async fn test_broker_manager_disabled() {
        let config = BrokerConfig {
            enabled: false,
            ..Default::default()
        };

        let manager = BrokerManager::new(config);
        let result = manager.start().await;

        assert!(result.is_ok());
        let health = manager.health_status().await;
        assert_eq!(health, BrokerHealth::Stopped);
    }

    #[tokio::test]
    async fn test_broker_manager_socket_path() {
        let config = BrokerConfig {
            socket_path: "/tmp/test-broker.sock".to_string(),
            ..Default::default()
        };

        let manager = BrokerManager::new(config);
        assert_eq!(manager.socket_path(), "/tmp/test-broker.sock");
    }

    #[tokio::test]
    async fn test_broker_manager_statistics_when_stopped() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let stats = manager.statistics().await;
        assert!(stats.is_none());
    }

    #[tokio::test]
    async fn test_broker_manager_health_check_when_stopped() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let health = manager.health_check().await;
        assert_eq!(health, BrokerHealth::Stopped);
    }

    #[tokio::test]
    async fn test_broker_manager_wait_for_healthy_timeout() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let result = manager.wait_for_healthy(Duration::from_millis(100)).await;
        assert!(result.is_err());
    }
}
