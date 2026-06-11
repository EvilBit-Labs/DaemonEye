//! Agent loading state machine methods.
//!
//! These methods will be used when main.rs integrates the state machine (Task #15).

use super::BrokerManager;
use super::state::AgentState;
use crate::collector_config::CollectorsConfig;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, error, info, warn};

impl BrokerManager {
    /// Get the current agent state.
    #[allow(dead_code)]
    pub async fn agent_state(&self) -> AgentState {
        self.agent_state.read().await.clone()
    }

    /// Set the collectors configuration and update the readiness tracker.
    ///
    /// This should be called during agent startup after loading the configuration file.
    /// Only enabled collectors are added to the expected collectors set.
    #[allow(dead_code)]
    pub async fn set_collectors_config(&self, config: CollectorsConfig) {
        // Extract enabled collector IDs
        let expected: HashSet<String> = config
            .collectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.clone())
            .collect();

        let expected_count = expected.len();

        // Update readiness tracker with expected collectors
        self.readiness_tracker.write().await.set_expected(expected);

        // Store the configuration
        *self.collectors_config.write().await = config;

        info!(
            expected_collectors = expected_count,
            "Configured collector readiness tracking"
        );
    }

    /// Get the current collectors configuration.
    #[allow(dead_code)]
    pub async fn collectors_config(&self) -> CollectorsConfig {
        self.collectors_config.read().await.clone()
    }

    /// Mark a collector as ready and check if all collectors are ready.
    ///
    /// Returns `true` if all expected collectors are now ready.
    /// This method is typically called when a collector sends a "ready" status
    /// during its registration handshake.
    #[allow(dead_code)]
    pub async fn mark_collector_ready(&self, collector_id: &str) -> bool {
        let mut tracker = self.readiness_tracker.write().await;
        tracker.mark_ready(collector_id);
        let all_ready = tracker.all_ready();

        info!(
            collector_id = %collector_id,
            ready_count = tracker.ready_count(),
            expected_count = tracker.expected_count(),
            all_ready = all_ready,
            "Collector marked as ready"
        );

        all_ready
    }

    /// Get the list of collectors that haven't reported ready yet.
    #[allow(dead_code)]
    pub async fn pending_collectors(&self) -> Vec<String> {
        let tracker = self.readiness_tracker.read().await;
        tracker
            .pending_collectors()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Check if all expected collectors have reported ready.
    #[allow(dead_code)]
    pub async fn all_collectors_ready(&self) -> bool {
        let tracker = self.readiness_tracker.read().await;
        tracker.all_ready()
    }

    /// Transition from Loading to Ready state.
    ///
    /// This transition occurs when all expected collectors have registered and
    /// reported "ready" status. Returns `Ok(())` if the transition is valid,
    /// or `Err` if the current state doesn't allow this transition.
    #[allow(dead_code)]
    pub async fn transition_to_ready(&self) -> Result<()> {
        let mut state = self.agent_state.write().await;

        match *state {
            AgentState::Loading => {
                // Verify all collectors are ready before transitioning
                let tracker = self.readiness_tracker.read().await;
                if !tracker.all_ready() {
                    let pending: Vec<_> = tracker.pending_collectors().into_iter().collect();
                    anyhow::bail!(
                        "Cannot transition to Ready: collectors still pending: {pending:?}"
                    );
                }
                drop(tracker);

                info!(
                    previous_state = %*state,
                    "Agent transitioning to Ready state"
                );
                *state = AgentState::Ready;
                Ok(())
            }
            AgentState::Ready | AgentState::SteadyState => {
                // Already in Ready or beyond - no-op
                debug!(current_state = %*state, "Agent already in Ready or SteadyState");
                Ok(())
            }
            AgentState::StartupFailed { ref reason } => {
                anyhow::bail!("Cannot transition to Ready: startup failed: {reason}");
            }
            AgentState::ShuttingDown => {
                anyhow::bail!("Cannot transition to Ready: agent is shutting down");
            }
        }
    }

    /// Transition from Ready to `SteadyState` and broadcast "begin monitoring".
    ///
    /// This transition occurs after privilege dropping (if configured) and
    /// signals all collectors to begin their monitoring operations.
    #[allow(dead_code)]
    pub async fn transition_to_steady_state(&self) -> Result<()> {
        let mut state = self.agent_state.write().await;

        match *state {
            AgentState::Ready => {
                info!(
                    previous_state = %*state,
                    "Agent transitioning to SteadyState"
                );
                *state = AgentState::SteadyState;

                // Broadcast "begin monitoring" to all collectors
                drop(state); // Release lock before async operations
                if let Err(e) = self.broadcast_begin_monitoring().await {
                    // Rollback state on broadcast failure
                    warn!(
                        error = %e,
                        "Failed to broadcast 'begin monitoring', rolling back to Ready state"
                    );
                    *self.agent_state.write().await = AgentState::Ready;
                    return Err(e);
                }

                Ok(())
            }
            AgentState::SteadyState => {
                // Already in SteadyState - no-op
                debug!("Agent already in SteadyState");
                Ok(())
            }
            AgentState::Loading => {
                anyhow::bail!(
                    "Cannot transition to SteadyState: must be in Ready state first (currently Loading)"
                );
            }
            AgentState::StartupFailed { ref reason } => {
                anyhow::bail!("Cannot transition to SteadyState: startup failed: {reason}");
            }
            AgentState::ShuttingDown => {
                anyhow::bail!("Cannot transition to SteadyState: agent is shutting down");
            }
        }
    }

    /// Mark the agent startup as failed.
    ///
    /// This is called when the startup timeout expires before all collectors
    /// have reported ready.
    #[allow(dead_code)]
    pub async fn mark_startup_failed(&self, reason: String) {
        let mut state = self.agent_state.write().await;

        // Only transition to failed if still in Loading state
        if matches!(*state, AgentState::Loading) {
            let pending = {
                let tracker = self.readiness_tracker.read().await;
                tracker
                    .pending_collectors()
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            };

            error!(
                reason = %reason,
                pending_collectors = ?pending,
                "Agent startup failed"
            );

            *state = AgentState::StartupFailed {
                reason: reason.clone(),
            };
        }
    }

    /// Transition to `ShuttingDown` state.
    ///
    /// This should be called before initiating the shutdown sequence.
    #[allow(dead_code)]
    pub async fn transition_to_shutting_down(&self) {
        let mut state = self.agent_state.write().await;
        let previous = state.clone();

        *state = AgentState::ShuttingDown;
        drop(state);
        info!(previous_state = %previous, "Agent transitioning to ShuttingDown state");
    }

    /// Wait for all expected collectors to be ready within the given timeout.
    ///
    /// This method polls the readiness tracker at intervals until all collectors
    /// are ready or the timeout expires. If the timeout expires, the startup is
    /// marked as failed.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for all collectors to be ready
    /// * `poll_interval` - How often to check readiness status
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - All collectors are ready
    /// * `Ok(false)` - Timeout expired before all collectors were ready (startup marked as failed)
    #[allow(dead_code)]
    pub async fn wait_for_collectors_ready(
        &self,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<bool> {
        let start = std::time::Instant::now();

        info!(
            timeout_secs = timeout.as_secs(),
            "Waiting for all collectors to be ready"
        );

        loop {
            // Check if all collectors are ready
            if self.all_collectors_ready().await {
                let elapsed = start.elapsed();
                info!(elapsed_secs = elapsed.as_secs(), "All collectors are ready");
                return Ok(true);
            }

            // Check if we've timed out
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                let pending = self.pending_collectors().await;
                warn!(
                    timeout_secs = timeout.as_secs(),
                    pending_collectors = ?pending,
                    "Startup timeout expired waiting for collectors"
                );

                // Mark startup as failed
                self.mark_startup_failed(format!(
                    "Timeout after {} seconds waiting for collectors: {:?}",
                    timeout.as_secs(),
                    pending
                ))
                .await;

                return Ok(false);
            }

            // Log progress periodically
            let pending = self.pending_collectors().await;
            debug!(
                elapsed_secs = elapsed.as_secs(),
                pending_count = pending.len(),
                pending_collectors = ?pending,
                "Still waiting for collectors to be ready"
            );

            // Wait before polling again
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Drop privileges after all collectors have registered and reported ready.
    ///
    /// This method is a stub that will be implemented with platform-specific
    /// privilege dropping logic in the future. Currently it just logs that
    /// privilege dropping would occur.
    ///
    /// # Platform-specific behavior (future)
    ///
    /// - **Unix**: Drop to a non-root user (e.g., `daemoneye`)
    /// - **Windows**: Reduce process token privileges
    /// - **macOS**: Drop supplementary groups and effective UID
    ///
    /// # Safety
    ///
    /// This should only be called after all collectors have been spawned and
    /// have completed their privileged initialization (e.g., binding to
    /// privileged ports, accessing protected resources).
    #[allow(dead_code)]
    #[allow(clippy::unused_async)] // Future implementation will use async for platform-specific privilege dropping
    pub async fn drop_privileges(&self) -> Result<()> {
        info!("Privilege dropping requested (stub - not yet implemented)");

        // Future implementation will:
        // 1. Check if running as root/elevated
        // 2. Drop supplementary groups
        // 3. Set effective UID/GID to unprivileged user
        // 4. Verify privileges were dropped successfully

        // For now, just log and succeed
        debug!("Privilege dropping is not implemented - running with current privileges");

        Ok(())
    }

    /// Get the default startup timeout from the collectors configuration.
    ///
    /// Returns the maximum startup timeout among all enabled collectors,
    /// or a default of 60 seconds if no collectors are configured.
    #[allow(dead_code)]
    pub async fn get_startup_timeout(&self) -> Duration {
        let max_timeout = self
            .collectors_config
            .read()
            .await
            .collectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.startup_timeout_secs)
            .max()
            .unwrap_or(60);

        Duration::from_secs(max_timeout)
    }

    /// Broadcast "begin monitoring" message to all collectors.
    ///
    /// This is sent on the `control.collector.lifecycle` topic to signal
    /// all collectors that they should begin their monitoring operations.
    #[allow(dead_code)]
    #[allow(clippy::significant_drop_tightening)] // Guard must be held while broker is used
    pub async fn broadcast_begin_monitoring(&self) -> Result<()> {
        let broker_guard = self.broker.read().await;
        let Some(broker) = broker_guard.as_ref() else {
            warn!("Cannot broadcast 'begin monitoring': broker not available");
            return Ok(());
        };

        let topic = "control.collector.lifecycle";

        // Create the lifecycle message
        let message = serde_json::json!({
            "type": "BeginMonitoring",
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            "source": "daemoneye-agent",
        });

        let payload =
            serde_json::to_vec(&message).context("Failed to serialize BeginMonitoring message")?;

        broker
            .publish(topic, "begin-monitoring", payload)
            .await
            .context("Failed to publish BeginMonitoring message")?;

        info!(
            topic = %topic,
            "Broadcast 'begin monitoring' to all collectors"
        );

        Ok(())
    }
}
