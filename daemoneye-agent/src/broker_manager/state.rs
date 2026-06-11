//! Agent loading state machine types and collector readiness tracking.

use std::collections::HashSet;

/// Agent loading state machine.
///
/// The agent progresses through these states during startup:
/// - Loading → Ready → `SteadyState`
///
/// This ensures coordinated startup where all collectors are ready before
/// the agent drops privileges and enters normal operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)] // Variants constructed by state machine methods, used in Task #15
pub enum AgentState {
    /// Agent is starting up, broker initializing, spawning collectors.
    /// The agent waits for all expected collectors to register and report "ready".
    Loading,

    /// All collectors have registered and reported "ready".
    /// The caller should drop privileges (if configured) before transitioning to `SteadyState`.
    Ready,

    /// Normal operation. Collectors are actively monitoring.
    /// The "begin monitoring" message is broadcast during `transition_to_steady_state()`.
    SteadyState,

    /// Agent failed to start (collectors didn't report ready within timeout).
    StartupFailed { reason: String },

    /// Agent is shutting down.
    ShuttingDown,
}

impl AgentState {
    /// Returns true if the agent is in a state where it's accepting collector registrations.
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn accepts_registrations(&self) -> bool {
        matches!(self, Self::Loading | Self::Ready | Self::SteadyState)
    }

    /// Returns true if the agent is in a running state (not failed or shutting down).
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Loading | Self::Ready | Self::SteadyState)
    }

    /// Returns true if the agent startup has failed.
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::StartupFailed { .. })
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Loading => write!(f, "Loading"),
            Self::Ready => write!(f, "Ready"),
            Self::SteadyState => write!(f, "SteadyState"),
            Self::StartupFailed { ref reason } => write!(f, "StartupFailed: {reason}"),
            Self::ShuttingDown => write!(f, "ShuttingDown"),
        }
    }
}

/// Tracks which collectors have reported ready status.
#[derive(Debug)]
#[allow(dead_code)] // Methods used by BrokerManager state machine, integrated in Task #15
pub(super) struct CollectorReadinessTracker {
    /// Set of collector IDs that are expected to report ready.
    expected_collectors: HashSet<String>,
    /// Set of collector IDs that have reported ready.
    ready_collectors: HashSet<String>,
}

#[allow(dead_code)] // Methods used by BrokerManager state machine, integrated in Task #15
impl CollectorReadinessTracker {
    /// Create a new readiness tracker with expected collector IDs.
    #[allow(dead_code)] // May be used in future for direct initialization
    pub(super) fn new(expected_collectors: HashSet<String>) -> Self {
        Self {
            expected_collectors,
            ready_collectors: HashSet::new(),
        }
    }

    /// Create an empty tracker (no collectors expected).
    pub(super) fn empty() -> Self {
        Self {
            expected_collectors: HashSet::new(),
            ready_collectors: HashSet::new(),
        }
    }

    /// Mark a collector as ready.
    pub(super) fn mark_ready(&mut self, collector_id: &str) {
        self.ready_collectors.insert(collector_id.to_owned());
    }

    /// Check if all expected collectors are ready.
    pub(super) fn all_ready(&self) -> bool {
        if self.expected_collectors.is_empty() {
            // No collectors expected, consider ready
            return true;
        }
        self.expected_collectors
            .iter()
            .all(|id| self.ready_collectors.contains(id))
    }

    /// Get the list of collectors that haven't reported ready yet.
    pub(super) fn pending_collectors(&self) -> Vec<&str> {
        self.expected_collectors
            .iter()
            .filter(|id| !self.ready_collectors.contains(*id))
            .map(String::as_str)
            .collect()
    }

    /// Get the number of expected collectors.
    pub(super) fn expected_count(&self) -> usize {
        self.expected_collectors.len()
    }

    /// Get the number of ready collectors.
    pub(super) fn ready_count(&self) -> usize {
        self.ready_collectors.len()
    }

    /// Set expected collectors from configuration.
    pub(super) fn set_expected(&mut self, expected: HashSet<String>) {
        self.expected_collectors = expected;
    }

    /// Reset ready status for all collectors.
    #[allow(dead_code)] // May be used in future for restart scenarios
    pub(super) fn reset(&mut self) {
        self.ready_collectors.clear();
    }
}
