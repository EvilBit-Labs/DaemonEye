//! Registration state machine for the registration manager.

/// Registration state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistrationState {
    /// Not yet registered with the agent.
    Unregistered,
    /// Registration in progress.
    Registering,
    /// Successfully registered and receiving commands.
    Registered,
    /// Deregistration in progress.
    Deregistering,
    /// Registration failed after retries.
    Failed,
}

impl std::fmt::Display for RegistrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Unregistered => write!(f, "unregistered"),
            Self::Registering => write!(f, "registering"),
            Self::Registered => write!(f, "registered"),
            Self::Deregistering => write!(f, "deregistering"),
            Self::Failed => write!(f, "failed"),
        }
    }
}
