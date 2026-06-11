//! Process control and inspection operations: configuration access,
//! pause/resume, health checks, status reporting, and bulk shutdown.

use super::CollectorProcessManager;
#[cfg(all(unix, feature = "freebsd"))]
use super::send_signal;
use super::types::{
    CollectorState, CollectorStatus, HealthStatus, ProcessManagerConfig, ProcessManagerError,
};
#[cfg(all(unix, feature = "freebsd"))]
use nix::sys::signal::Signal;
use std::time::SystemTime;
use tracing::{info, warn};

impl CollectorProcessManager {
    /// Get the process manager configuration
    pub const fn config(&self) -> &ProcessManagerConfig {
        &self.config
    }

    /// Pause a collector process (Unix only)
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to pause
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `InvalidState` if collector is not in Running state
    /// - `PlatformNotSupported` on Windows
    // SAFETY: Lock guard is held throughout the unix block to atomically check + update process state.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn pause_collector(&self, collector_id: &str) -> Result<(), ProcessManagerError> {
        #[cfg(unix)]
        {
            let mut processes = self.processes.lock().await;
            let process = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

            // Validate state before pausing
            if process.state != CollectorState::Running {
                return Err(ProcessManagerError::InvalidState(format!(
                    "Collector {} is not running (state: {:?}), cannot pause",
                    collector_id, process.state
                )));
            }

            info!("Pausing collector {} (PID: {})", collector_id, process.pid);

            #[cfg(feature = "freebsd")]
            {
                send_signal(process.pid, Signal::SIGSTOP)?;
                process.state = CollectorState::Paused;
                Ok(())
            }
            #[cfg(not(feature = "freebsd"))]
            {
                Err(ProcessManagerError::SpawnFailed(
                    "Pause operation requires freebsd feature".to_owned(),
                ))
            }
        }

        #[cfg(windows)]
        {
            let _ = collector_id; // Parameter required for API consistency
            Err(ProcessManagerError::PlatformNotSupported(
                "Pause operation not supported on Windows".to_string(),
            ))
        }
    }

    /// Resume a paused collector process (Unix only)
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to resume
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `InvalidState` if collector is not paused
    /// - `PlatformNotSupported` on Windows
    // SAFETY: Lock guard is held throughout the unix block to atomically check + update process state.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn resume_collector(&self, collector_id: &str) -> Result<(), ProcessManagerError> {
        #[cfg(unix)]
        {
            let mut processes = self.processes.lock().await;
            let process = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

            if process.state != CollectorState::Paused {
                return Err(ProcessManagerError::InvalidState(format!(
                    "Collector {} is not paused (state: {:?})",
                    collector_id, process.state
                )));
            }

            info!("Resuming collector {} (PID: {})", collector_id, process.pid);

            #[cfg(feature = "freebsd")]
            {
                send_signal(process.pid, Signal::SIGCONT)?;
                process.state = CollectorState::Running;
                Ok(())
            }
            #[cfg(not(feature = "freebsd"))]
            {
                Err(ProcessManagerError::SpawnFailed(
                    "Resume operation requires freebsd feature".to_owned(),
                ))
            }
        }

        #[cfg(windows)]
        {
            let _ = collector_id; // Parameter required for API consistency
            Err(ProcessManagerError::PlatformNotSupported(
                "Resume operation not supported on Windows".to_string(),
            ))
        }
    }

    /// Check collector health
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to check
    ///
    /// # Returns
    ///
    /// Health status
    // SAFETY: Lock guard held throughout to allow mutable access to child process handle.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn check_collector_health(
        &self,
        collector_id: &str,
    ) -> Result<HealthStatus, ProcessManagerError> {
        let mut processes = self.processes.lock().await;
        let process = processes
            .get_mut(collector_id)
            .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

        process.last_health_check = Some(SystemTime::now());

        // Check heartbeat status with graduated response
        let heartbeat_threshold = self.config.heartbeat_timeout_multiplier;
        let health_from_heartbeat = if process.missed_heartbeats >= heartbeat_threshold {
            HealthStatus::Unhealthy
        } else if process.missed_heartbeats > 0 {
            HealthStatus::Degraded // Some heartbeats missed but not yet critical
        } else {
            HealthStatus::Healthy
        };

        // Check if process is still running
        let health_from_process = match process
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .flatten()
        {
            Some(_) => HealthStatus::Unhealthy, // Process exited
            None => HealthStatus::Healthy,      // Still running or no child handle
        };

        // Return worst status (Unhealthy > Degraded > Healthy)
        Ok(match (health_from_process, health_from_heartbeat) {
            (HealthStatus::Unhealthy, _) | (_, HealthStatus::Unhealthy) => HealthStatus::Unhealthy,
            (HealthStatus::Degraded, _) | (_, HealthStatus::Degraded) => HealthStatus::Degraded,
            _ => HealthStatus::Healthy,
        })
    }

    /// Get collector status
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector
    ///
    /// # Returns
    ///
    /// Collector status
    // SAFETY: Lock guard held throughout to allow mutable access to child process handle.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn get_collector_status(
        &self,
        collector_id: &str,
    ) -> Result<CollectorStatus, ProcessManagerError> {
        let mut processes = self.processes.lock().await;
        let process = processes
            .get_mut(collector_id)
            .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

        let uptime = SystemTime::now()
            .duration_since(process.start_time)
            .unwrap_or_default();

        let health = match process
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .flatten()
        {
            Some(_) => HealthStatus::Unhealthy, // Process exited
            None => HealthStatus::Healthy,      // Still running or no child handle
        };

        let heartbeat_healthy = process.missed_heartbeats == 0;

        Ok(CollectorStatus {
            collector_id: process.collector_id.clone(),
            state: process.state.clone(),
            pid: process.pid,
            start_time: process.start_time,
            restart_count: process.restart_count,
            uptime,
            health,
            last_heartbeat: process.last_heartbeat,
            missed_heartbeats: process.missed_heartbeats,
            heartbeat_healthy,
        })
    }

    /// Shutdown all collectors
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub async fn shutdown_all(&self) -> Result<(), ProcessManagerError> {
        info!("Shutting down all collectors");

        // Send shutdown signal
        if self.shutdown_tx.send(()).is_err() {
            warn!(
                "Shutdown signal channel closed - some collectors may not receive shutdown notification"
            );
        }

        // Get list of collector IDs
        let collector_ids: Vec<String> = {
            let processes = self.processes.lock().await;
            processes.keys().cloned().collect()
        };

        // First pass: attempt graceful stop for each collector
        for collector_id in &collector_ids {
            match self
                .stop_collector(collector_id, true, self.config.default_graceful_timeout)
                .await
            {
                Ok(_) => {
                    info!("Successfully stopped collector: {}", collector_id);
                }
                Err(e) => {
                    warn!(
                        "Failed to gracefully stop collector {}: {}. Will attempt force stop.",
                        collector_id, e
                    );
                }
            }
        }

        // Second pass: force kill any remaining collectors
        let remaining_ids: Vec<String> = {
            let processes = self.processes.lock().await;
            processes.keys().cloned().collect()
        };

        for collector_id in &remaining_ids {
            match self
                .stop_collector(collector_id, false, self.config.default_force_timeout)
                .await
            {
                Ok(_) => info!("Force-stopped collector: {}", collector_id),
                Err(e) => warn!(
                    "Collector {} did not terminate after force stop: {}. Keeping entry for visibility.",
                    collector_id, e
                ),
            }
        }

        info!("All collectors shut down");
        Ok(())
    }

    /// List the IDs of all known collectors
    pub async fn list_collector_ids(&self) -> Vec<String> {
        let processes = self.processes.lock().await;
        processes.keys().cloned().collect()
    }
}
