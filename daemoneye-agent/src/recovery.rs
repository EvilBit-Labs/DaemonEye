//! Escalating recovery actions for failed collectors.
//!
//! This module implements a progressive recovery strategy for collectors that
//! have missed heartbeats or become unresponsive:
//!
//! 1. **Health Check**: Verify collector state via RPC
//! 2. **Graceful Shutdown**: Request clean shutdown via RPC
//! 3. **Force Kill**: Terminate process via signal
//! 4. **Restart**: Spawn new collector process
//!
//! Each stage escalates if the previous action fails or times out.

use crate::broker_manager::BrokerManager;
use crate::collector_registry::HeartbeatStatus;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

/// Maximum number of recovery attempts before giving up.
pub const MAX_RECOVERY_ATTEMPTS: u32 = 3;

/// Default timeout for health check RPC.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for graceful shutdown.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for force kill.
const FORCE_KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for restart.
const RESTART_TIMEOUT: Duration = Duration::from_secs(60);

/// Recovery actions in escalating order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
#[allow(dead_code)] // Variants used by recovery orchestration
pub enum RecoveryAction {
    /// Send health check RPC to verify collector state.
    HealthCheck,
    /// Request graceful shutdown via RPC.
    GracefulShutdown,
    /// Force kill the collector process.
    ForceKill,
    /// Restart the collector process.
    Restart,
}

impl std::fmt::Display for RecoveryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::HealthCheck => write!(f, "HealthCheck"),
            Self::GracefulShutdown => write!(f, "GracefulShutdown"),
            Self::ForceKill => write!(f, "ForceKill"),
            Self::Restart => write!(f, "Restart"),
        }
    }
}

impl RecoveryAction {
    /// Get the next escalation action.
    #[must_use]
    #[allow(dead_code)] // Used by escalation logic
    pub const fn escalate(self) -> Option<Self> {
        match self {
            Self::HealthCheck => Some(Self::GracefulShutdown),
            Self::GracefulShutdown => Some(Self::ForceKill),
            Self::ForceKill => Some(Self::Restart),
            Self::Restart => None,
        }
    }

    /// Get the appropriate action based on heartbeat status.
    #[must_use]
    #[allow(dead_code)] // Used by recovery determination logic
    pub const fn from_heartbeat_status(status: &HeartbeatStatus) -> Option<Self> {
        match *status {
            HeartbeatStatus::Healthy => None,
            HeartbeatStatus::Degraded { missed_count } => {
                if missed_count >= 2 {
                    Some(Self::HealthCheck)
                } else {
                    None
                }
            }
            HeartbeatStatus::Failed { .. } => Some(Self::GracefulShutdown),
        }
    }
}

/// Result of a recovery attempt.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[allow(dead_code)] // Variants used by recovery orchestration
pub enum RecoveryResult {
    /// Recovery action succeeded, collector is healthy.
    Success {
        /// The action that succeeded.
        action: RecoveryAction,
        /// Human-readable message.
        message: String,
    },
    /// Recovery action failed, may need escalation.
    Failed {
        /// The action that failed.
        action: RecoveryAction,
        /// Error message.
        error: String,
        /// Whether to escalate to the next action.
        should_escalate: bool,
    },
    /// Recovery was skipped (e.g., collector already healthy).
    Skipped {
        /// Reason for skipping.
        reason: String,
    },
    /// Maximum recovery attempts exceeded.
    Exhausted {
        /// Total attempts made.
        attempts: u32,
        /// Last action attempted.
        last_action: RecoveryAction,
    },
}

/// Errors that can occur during recovery operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RecoveryError {
    /// Health check failed.
    #[error("health check failed: {0}")]
    HealthCheckFailed(String),

    /// Graceful shutdown failed.
    #[error("graceful shutdown failed: {0}")]
    GracefulShutdownFailed(String),

    /// Force kill failed.
    #[error("force kill failed: {0}")]
    ForceKillFailed(String),

    /// Restart failed.
    #[error("restart failed: {0}")]
    RestartFailed(String),

    /// Operation timed out.
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),

    /// Collector not found.
    #[error("collector `{0}` not found")]
    CollectorNotFound(String),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Tracks recovery state for a collector.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used by recovery orchestration
pub struct CollectorRecoveryState {
    /// Collector ID.
    pub collector_id: String,
    /// Current recovery action being attempted.
    pub current_action: Option<RecoveryAction>,
    /// Number of recovery attempts.
    pub attempt_count: u32,
    /// Last recovery result.
    pub last_result: Option<RecoveryResult>,
    /// Whether recovery is in progress.
    pub in_progress: bool,
}

impl CollectorRecoveryState {
    /// Create a new recovery state for a collector.
    #[must_use]
    #[allow(dead_code)]
    pub const fn new(collector_id: String) -> Self {
        Self {
            collector_id,
            current_action: None,
            attempt_count: 0,
            last_result: None,
            in_progress: false,
        }
    }

    /// Check if recovery has been exhausted.
    #[must_use]
    #[allow(dead_code)]
    pub const fn is_exhausted(&self) -> bool {
        self.attempt_count >= MAX_RECOVERY_ATTEMPTS
    }

    /// Mark recovery as starting.
    #[allow(dead_code)]
    pub const fn start_recovery(&mut self, action: RecoveryAction) {
        self.current_action = Some(action);
        self.in_progress = true;
        self.attempt_count = self.attempt_count.saturating_add(1);
    }

    /// Mark recovery as complete with result.
    #[allow(dead_code)]
    pub fn complete_recovery(&mut self, result: RecoveryResult) {
        self.last_result = Some(result);
        self.in_progress = false;
    }

    /// Reset recovery state (e.g., after successful recovery).
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.current_action = None;
        self.attempt_count = 0;
        self.last_result = None;
        self.in_progress = false;
    }
}

/// Execute escalating recovery actions for a collector.
///
/// This function attempts recovery actions in escalating order:
/// 1. Health check to verify state
/// 2. Graceful shutdown via RPC
/// 3. Force kill via process manager
/// 4. Restart via process manager
///
/// Returns when the collector is recovered or all actions are exhausted.
#[allow(dead_code)]
pub async fn execute_recovery(
    manager: &BrokerManager,
    collector_id: &str,
    starting_action: RecoveryAction,
) -> RecoveryResult {
    let mut current_action = starting_action;
    let mut attempts = 0_u32;

    loop {
        attempts = attempts.saturating_add(1);

        if attempts > MAX_RECOVERY_ATTEMPTS {
            warn!(
                collector_id = %collector_id,
                attempts = attempts,
                last_action = %current_action,
                "Recovery attempts exhausted"
            );
            return RecoveryResult::Exhausted {
                attempts,
                last_action: current_action,
            };
        }

        info!(
            collector_id = %collector_id,
            action = %current_action,
            attempt = attempts,
            "Executing recovery action"
        );

        let result = execute_action(manager, collector_id, current_action).await;

        match result {
            Ok(()) => {
                info!(
                    collector_id = %collector_id,
                    action = %current_action,
                    "Recovery action succeeded"
                );
                return RecoveryResult::Success {
                    action: current_action,
                    message: format!("{current_action} completed successfully"),
                };
            }
            Err(e) => {
                warn!(
                    collector_id = %collector_id,
                    action = %current_action,
                    error = %e,
                    "Recovery action failed"
                );

                // Try to escalate to next action
                if let Some(next_action) = current_action.escalate() {
                    info!(
                        collector_id = %collector_id,
                        from = %current_action,
                        to = %next_action,
                        "Escalating recovery action"
                    );
                    current_action = next_action;
                } else {
                    // No more escalation options
                    error!(
                        collector_id = %collector_id,
                        action = %current_action,
                        error = %e,
                        "Recovery failed with no more escalation options"
                    );
                    return RecoveryResult::Failed {
                        action: current_action,
                        error: e.to_string(),
                        should_escalate: false,
                    };
                }
            }
        }
    }
}

/// Execute a single recovery action.
async fn execute_action(
    manager: &BrokerManager,
    collector_id: &str,
    action: RecoveryAction,
) -> Result<(), RecoveryError> {
    match action {
        RecoveryAction::HealthCheck => execute_health_check(manager, collector_id).await,
        RecoveryAction::GracefulShutdown => execute_graceful_shutdown(manager, collector_id).await,
        RecoveryAction::ForceKill => execute_force_kill(manager, collector_id).await,
        RecoveryAction::Restart => execute_restart(manager, collector_id).await,
    }
}

/// Execute health check via RPC.
async fn execute_health_check(
    manager: &BrokerManager,
    collector_id: &str,
) -> Result<(), RecoveryError> {
    debug!(collector_id = %collector_id, "Executing health check");

    let result = tokio::time::timeout(HEALTH_CHECK_TIMEOUT, async {
        manager.health_check_rpc(collector_id).await
    })
    .await;

    match result {
        Ok(Ok(health_data)) => {
            if health_data.status.is_healthy() {
                debug!(
                    collector_id = %collector_id,
                    status = ?health_data.status,
                    "Health check passed"
                );
                Ok(())
            } else {
                Err(RecoveryError::HealthCheckFailed(format!(
                    "collector unhealthy: {:?}",
                    health_data.status
                )))
            }
        }
        Ok(Err(e)) => Err(RecoveryError::HealthCheckFailed(e.to_string())),
        Err(_) => Err(RecoveryError::Timeout(HEALTH_CHECK_TIMEOUT)),
    }
}

/// Execute graceful shutdown via RPC.
async fn execute_graceful_shutdown(
    manager: &BrokerManager,
    collector_id: &str,
) -> Result<(), RecoveryError> {
    debug!(collector_id = %collector_id, "Executing graceful shutdown");

    let result = tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, async {
        manager.stop_collector_rpc(collector_id, true).await
    })
    .await;

    match result {
        Ok(Ok(())) => {
            debug!(collector_id = %collector_id, "Graceful shutdown succeeded");
            Ok(())
        }
        Ok(Err(e)) => Err(RecoveryError::GracefulShutdownFailed(e.to_string())),
        Err(_) => Err(RecoveryError::Timeout(GRACEFUL_SHUTDOWN_TIMEOUT)),
    }
}

/// Execute force kill via process manager.
async fn execute_force_kill(
    manager: &BrokerManager,
    collector_id: &str,
) -> Result<(), RecoveryError> {
    debug!(collector_id = %collector_id, "Executing force kill");

    let process_manager = manager.process_manager();

    let result = tokio::time::timeout(FORCE_KILL_TIMEOUT, async {
        process_manager
            .stop_collector(collector_id, true, FORCE_KILL_TIMEOUT)
            .await
    })
    .await;

    match result {
        Ok(Ok(_exit_code)) => {
            debug!(collector_id = %collector_id, "Force kill succeeded");
            Ok(())
        }
        Ok(Err(e)) => Err(RecoveryError::ForceKillFailed(e.to_string())),
        Err(_) => Err(RecoveryError::Timeout(FORCE_KILL_TIMEOUT)),
    }
}

/// Execute restart via process manager.
async fn execute_restart(manager: &BrokerManager, collector_id: &str) -> Result<(), RecoveryError> {
    debug!(collector_id = %collector_id, "Executing restart");

    let process_manager = manager.process_manager();

    let result = tokio::time::timeout(RESTART_TIMEOUT, async {
        process_manager
            .restart_collector(collector_id, RESTART_TIMEOUT)
            .await
    })
    .await;

    match result {
        Ok(Ok(_new_pid)) => {
            debug!(collector_id = %collector_id, "Restart succeeded");
            Ok(())
        }
        Ok(Err(e)) => Err(RecoveryError::RestartFailed(e.to_string())),
        Err(_) => Err(RecoveryError::Timeout(RESTART_TIMEOUT)),
    }
}

/// Extension trait for `HealthStatus` to check if healthy.
trait HealthStatusExt {
    fn is_healthy(&self) -> bool;
}

impl HealthStatusExt for daemoneye_eventbus::rpc::HealthStatus {
    fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::semicolon_outside_block,
    clippy::semicolon_inside_block
)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_action_escalation() {
        assert_eq!(
            RecoveryAction::HealthCheck.escalate(),
            Some(RecoveryAction::GracefulShutdown)
        );
        assert_eq!(
            RecoveryAction::GracefulShutdown.escalate(),
            Some(RecoveryAction::ForceKill)
        );
        assert_eq!(
            RecoveryAction::ForceKill.escalate(),
            Some(RecoveryAction::Restart)
        );
        assert_eq!(RecoveryAction::Restart.escalate(), None);
    }

    #[test]
    fn test_recovery_action_from_heartbeat_status_healthy() {
        let status = HeartbeatStatus::Healthy;
        assert!(RecoveryAction::from_heartbeat_status(&status).is_none());
    }

    #[test]
    fn test_recovery_action_from_heartbeat_status_degraded_low() {
        let status = HeartbeatStatus::Degraded { missed_count: 1 };
        assert!(RecoveryAction::from_heartbeat_status(&status).is_none());
    }

    #[test]
    fn test_recovery_action_from_heartbeat_status_degraded_high() {
        let status = HeartbeatStatus::Degraded { missed_count: 2 };
        assert_eq!(
            RecoveryAction::from_heartbeat_status(&status),
            Some(RecoveryAction::HealthCheck)
        );
    }

    #[test]
    fn test_recovery_action_from_heartbeat_status_failed() {
        let status = HeartbeatStatus::Failed {
            missed_count: 3,
            time_since_last: Duration::from_secs(90),
        };
        assert_eq!(
            RecoveryAction::from_heartbeat_status(&status),
            Some(RecoveryAction::GracefulShutdown)
        );
    }

    #[test]
    fn test_recovery_action_display() {
        assert_eq!(format!("{}", RecoveryAction::HealthCheck), "HealthCheck");
        assert_eq!(
            format!("{}", RecoveryAction::GracefulShutdown),
            "GracefulShutdown"
        );
        assert_eq!(format!("{}", RecoveryAction::ForceKill), "ForceKill");
        assert_eq!(format!("{}", RecoveryAction::Restart), "Restart");
    }

    #[test]
    fn test_recovery_action_ordering() {
        // Actions should be ordered by escalation level
        assert!(RecoveryAction::HealthCheck < RecoveryAction::GracefulShutdown);
        assert!(RecoveryAction::GracefulShutdown < RecoveryAction::ForceKill);
        assert!(RecoveryAction::ForceKill < RecoveryAction::Restart);
    }

    #[test]
    fn test_collector_recovery_state_new() {
        let state = CollectorRecoveryState::new("test-collector".to_owned());
        assert_eq!(state.collector_id, "test-collector");
        assert!(state.current_action.is_none());
        assert_eq!(state.attempt_count, 0);
        assert!(state.last_result.is_none());
        assert!(!state.in_progress);
    }

    #[test]
    fn test_collector_recovery_state_exhaustion() {
        let mut state = CollectorRecoveryState::new("test-collector".to_owned());
        assert!(!state.is_exhausted());

        // Simulate max attempts
        state.attempt_count = MAX_RECOVERY_ATTEMPTS;
        assert!(state.is_exhausted());
    }

    #[test]
    fn test_collector_recovery_state_lifecycle() {
        let mut state = CollectorRecoveryState::new("test-collector".to_owned());

        // Start recovery
        state.start_recovery(RecoveryAction::HealthCheck);
        assert!(state.in_progress);
        assert_eq!(state.current_action, Some(RecoveryAction::HealthCheck));
        assert_eq!(state.attempt_count, 1);

        // Complete recovery
        state.complete_recovery(RecoveryResult::Success {
            action: RecoveryAction::HealthCheck,
            message: "OK".to_owned(),
        });
        assert!(!state.in_progress);
        assert!(matches!(
            state.last_result,
            Some(RecoveryResult::Success { .. })
        ));

        // Reset
        state.reset();
        assert!(state.current_action.is_none());
        assert_eq!(state.attempt_count, 0);
        assert!(state.last_result.is_none());
        assert!(!state.in_progress);
    }
}
