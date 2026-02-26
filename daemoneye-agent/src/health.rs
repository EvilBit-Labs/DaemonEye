//! Health state utilities for service managers.
//!
//! This module provides common health state abstractions and utilities
//! for waiting on service health transitions.

use anyhow::Result;
use std::time::Duration;
use tracing::debug;

/// Trait for health state enums that support the wait-for-healthy pattern.
///
/// Implement this trait for health state enums to enable use of
/// [`wait_for_healthy`] helper function.
pub trait HealthState: Clone + Send + Sync {
    /// Returns `true` if the state represents a healthy/operational service.
    fn is_healthy(&self) -> bool;

    /// Returns `true` if the state represents a starting/initializing service.
    fn is_starting(&self) -> bool;

    /// Returns an error message if the state represents an unhealthy service.
    ///
    /// Returns `None` for healthy, starting, or stopped states.
    fn unhealthy_message(&self) -> Option<&str>;

    /// Returns `true` if the state represents a stopped or shutting down service.
    fn is_stopped_or_shutting_down(&self) -> bool;

    /// Returns a descriptive name for the service type (e.g., "Broker", "IPC server").
    fn service_name() -> &'static str;
}

/// Wait for a service to become healthy with a timeout.
///
/// This is a generic implementation of the wait-for-healthy pattern that
/// polls the provided health status function until the service becomes
/// healthy, an error state is reached, or the timeout expires.
///
/// # Arguments
///
/// * `timeout` - Maximum time to wait for healthy state
/// * `get_health` - Async function that returns the current health state
///
/// # Returns
///
/// - `Ok(())` if service becomes healthy
/// - `Err` if service is unhealthy, stopped, or timeout expires
pub async fn wait_for_healthy<H, F, Fut>(timeout: Duration, get_health: F) -> Result<()>
where
    H: HealthState,
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = H>,
{
    let start = std::time::Instant::now();
    let service_name = H::service_name();

    while start.elapsed() < timeout {
        let health = get_health().await;

        if health.is_healthy() {
            debug!("{} is healthy", service_name);
            return Ok(());
        }

        if health.is_starting() {
            debug!("{} is still starting, waiting...", service_name);
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        if let Some(error) = health.unhealthy_message() {
            return Err(anyhow::anyhow!("{service_name} is unhealthy: {error}"));
        }

        if health.is_stopped_or_shutting_down() {
            return Err(anyhow::anyhow!("{service_name} is not running"));
        }
    }

    Err(anyhow::anyhow!(
        "Timeout waiting for {service_name} to become healthy after {timeout:?}"
    ))
}
