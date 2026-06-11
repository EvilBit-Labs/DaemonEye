//! Error types for the registration manager.

use super::RegistrationState;
use thiserror::Error;

/// Errors that can occur during registration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// Registration request failed.
    #[error("Registration failed: {0}")]
    RegistrationFailed(String),

    /// Registration was rejected by the agent.
    #[error("Registration rejected: {0}")]
    RegistrationRejected(String),

    /// Registration timed out.
    #[error("Registration timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    /// Failed to publish heartbeat.
    #[error("Failed to publish heartbeat: {0}")]
    HeartbeatFailed(String),

    /// Deregistration failed.
    #[error("Deregistration failed: {0}")]
    DeregistrationFailed(String),

    /// Event bus error.
    #[error("Event bus error: {0}")]
    EventBusError(String),

    /// Invalid state transition.
    #[error("Invalid state transition from {from} to {to}")]
    InvalidStateTransition {
        from: RegistrationState,
        to: RegistrationState,
    },
}

/// Result type for registration operations.
pub type RegistrationResult<T> = Result<T, RegistrationError>;
