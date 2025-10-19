//! Graceful shutdown coordination system for collector lifecycle management.
//!
//! This module provides coordinated shutdown capabilities for collector components
//! using RPC calls instead of signal handling, enabling proper resource cleanup
//! and state preservation during system shutdown.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{Mutex, Notify, broadcast},
    task::JoinHandle,
    time::{Instant, sleep},
};
use tracing::{debug, error, info, instrument};

/// Shutdown coordination configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownConfig {
    /// Graceful shutdown timeout
    pub graceful_timeout: Duration,
    /// Forced shutdown timeout
    pub forced_timeout: Duration,
    /// Shutdown sequence delay between collectors
    pub sequence_delay: Duration,
    /// Enable parallel shutdown
    pub enable_parallel_shutdown: bool,
    /// Maximum concurrent shutdowns
    pub max_concurrent_shutdowns: usize,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            graceful_timeout: Duration::from_secs(60),
            forced_timeout: Duration::from_secs(10),
            sequence_delay: Duration::from_millis(500),
            enable_parallel_shutdown: true,
            max_concurrent_shutdowns: 5,
        }
    }
}
/// Shutdown coordinator for managing collector lifecycle
pub struct ShutdownCoordinator {
    /// Configuration
    config: ShutdownConfig,
    /// Shutdown state tracking
    shutdown_states: Arc<RwLock<HashMap<String, CollectorShutdownState>>>,
    /// Shutdown event broadcaster
    shutdown_events: broadcast::Sender<ShutdownEvent>,
    /// Global shutdown signal
    global_shutdown: Arc<AtomicBool>,
    /// Shutdown sequence counter
    sequence_counter: Arc<AtomicU32>,
    /// Shutdown completion notifier
    completion_notify: Arc<Notify>,
    /// Active shutdown tasks
    active_tasks: Arc<Mutex<HashMap<String, JoinHandle<Result<()>>>>>,
}

/// Collector shutdown state tracking
#[derive(Debug, Clone)]
#[allow(dead_code)] // API design - fields used in future shutdown tracking features
struct CollectorShutdownState {
    /// Collector identifier
    collector_id: String,
    /// Current shutdown phase
    phase: ShutdownPhase,
    /// Shutdown start time
    start_time: SystemTime,
    /// Last update time
    last_update: SystemTime,
    /// Shutdown timeout
    timeout: Duration,
    /// Force shutdown flag
    force_shutdown: bool,
    /// Shutdown reason
    reason: String,
    /// Cleanup tasks completed
    cleanup_completed: bool,
}

/// Shutdown phases
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShutdownPhase {
    /// Shutdown initiated
    Initiated,
    /// Preparing for shutdown
    Preparing,
    /// Stopping active operations
    Stopping,
    /// Cleaning up resources
    Cleanup,
    /// Shutdown completed
    Completed,
    /// Shutdown failed
    Failed,
    /// Forced shutdown
    Forced,
}

/// Shutdown events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShutdownEvent {
    /// Shutdown initiated for collector
    ShutdownInitiated {
        collector_id: String,
        reason: String,
        timeout: Duration,
        timestamp: SystemTime,
    },
    /// Shutdown phase changed
    PhaseChanged {
        collector_id: String,
        old_phase: ShutdownPhase,
        new_phase: ShutdownPhase,
        timestamp: SystemTime,
    },
    /// Shutdown completed successfully
    ShutdownCompleted {
        collector_id: String,
        duration: Duration,
        timestamp: SystemTime,
    },
    /// Shutdown failed
    ShutdownFailed {
        collector_id: String,
        error: String,
        phase: ShutdownPhase,
        timestamp: SystemTime,
    },
    /// Forced shutdown executed
    ForcedShutdown {
        collector_id: String,
        reason: String,
        timestamp: SystemTime,
    },
    /// Global shutdown initiated
    GlobalShutdownInitiated {
        reason: String,
        collector_count: usize,
        timestamp: SystemTime,
    },
    /// Global shutdown completed
    GlobalShutdownCompleted {
        duration: Duration,
        successful_shutdowns: usize,
        failed_shutdowns: usize,
        timestamp: SystemTime,
    },
}

/// Shutdown request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownRequest {
    /// Collector identifier
    pub collector_id: String,
    /// Shutdown reason
    pub reason: String,
    /// Force shutdown flag
    pub force: bool,
    /// Custom timeout (overrides default)
    pub timeout: Option<Duration>,
}

/// Shutdown response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownResponse {
    /// Success flag
    pub success: bool,
    /// Error message if failed
    pub error_message: Option<String>,
    /// Shutdown duration
    pub duration: Duration,
    /// Final phase reached
    pub final_phase: ShutdownPhase,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator
    pub fn new(config: ShutdownConfig) -> Self {
        let (shutdown_events, _) = broadcast::channel(1000);

        Self {
            config,
            shutdown_states: Arc::new(RwLock::new(HashMap::new())),
            shutdown_events,
            global_shutdown: Arc::new(AtomicBool::new(false)),
            sequence_counter: Arc::new(AtomicU32::new(0)),
            completion_notify: Arc::new(Notify::new()),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Initiate shutdown for a specific collector
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    pub async fn shutdown_collector(&self, request: ShutdownRequest) -> Result<ShutdownResponse> {
        info!(
            collector_id = %request.collector_id,
            reason = %request.reason,
            force = request.force,
            "Initiating collector shutdown"
        );

        let start_time = Instant::now();
        let timeout = request.timeout.unwrap_or(if request.force {
            self.config.forced_timeout
        } else {
            self.config.graceful_timeout
        });

        // Create shutdown state
        let shutdown_state = CollectorShutdownState {
            collector_id: request.collector_id.clone(),
            phase: ShutdownPhase::Initiated,
            start_time: SystemTime::now(),
            last_update: SystemTime::now(),
            timeout,
            force_shutdown: request.force,
            reason: request.reason.clone(),
            cleanup_completed: false,
        };

        // Register shutdown state
        {
            let mut states = self.shutdown_states.write().unwrap();
            states.insert(request.collector_id.clone(), shutdown_state);
        }

        // Send shutdown initiated event
        let event = ShutdownEvent::ShutdownInitiated {
            collector_id: request.collector_id.clone(),
            reason: request.reason.clone(),
            timeout,
            timestamp: SystemTime::now(),
        };
        let _ = self.shutdown_events.send(event);

        // Execute shutdown sequence
        let result = if request.force {
            self.execute_forced_shutdown(&request.collector_id).await
        } else {
            self.execute_graceful_shutdown(&request.collector_id).await
        };

        let duration = start_time.elapsed();
        let final_phase = {
            let states = self.shutdown_states.read().unwrap();
            states
                .get(&request.collector_id)
                .map(|s| s.phase.clone())
                .unwrap_or(ShutdownPhase::Failed)
        };

        match result {
            Ok(()) => {
                info!(
                    collector_id = %request.collector_id,
                    duration = ?duration,
                    "Collector shutdown completed successfully"
                );

                let event = ShutdownEvent::ShutdownCompleted {
                    collector_id: request.collector_id.clone(),
                    duration,
                    timestamp: SystemTime::now(),
                };
                let _ = self.shutdown_events.send(event);

                Ok(ShutdownResponse {
                    success: true,
                    error_message: None,
                    duration,
                    final_phase,
                })
            }
            Err(e) => {
                error!(
                    collector_id = %request.collector_id,
                    error = %e,
                    duration = ?duration,
                    "Collector shutdown failed"
                );

                let event = ShutdownEvent::ShutdownFailed {
                    collector_id: request.collector_id.clone(),
                    error: e.to_string(),
                    phase: final_phase.clone(),
                    timestamp: SystemTime::now(),
                };
                let _ = self.shutdown_events.send(event);

                Ok(ShutdownResponse {
                    success: false,
                    error_message: Some(e.to_string()),
                    duration,
                    final_phase,
                })
            }
        }
    }

    /// Initiate global shutdown for all collectors
    #[instrument(skip(self))]
    pub async fn shutdown_all_collectors(&self, reason: String) -> Result<Vec<ShutdownResponse>> {
        info!(reason = %reason, "Initiating global shutdown");

        // RAII guard to ensure flag is reset on all exit paths
        struct ShutdownGuard<'a>(&'a AtomicBool);
        impl Drop for ShutdownGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }

        // Set global shutdown flag
        self.global_shutdown.store(true, Ordering::Relaxed);
        let _guard = ShutdownGuard(&self.global_shutdown);

        // Get list of active collectors
        let collector_ids: Vec<String> = {
            let states = self.shutdown_states.read().unwrap();
            states.keys().cloned().collect()
        };

        let collector_count = collector_ids.len();

        // Send global shutdown initiated event
        let event = ShutdownEvent::GlobalShutdownInitiated {
            reason: reason.clone(),
            collector_count,
            timestamp: SystemTime::now(),
        };
        let _ = self.shutdown_events.send(event);

        let start_time = Instant::now();
        let responses = if self.config.enable_parallel_shutdown {
            // Parallel shutdown with concurrency limit
            self.shutdown_collectors_parallel(collector_ids, reason)
                .await?
        } else {
            // Sequential shutdown
            self.shutdown_collectors_sequential(collector_ids, reason)
                .await?
        };

        let duration = start_time.elapsed();
        let successful_shutdowns = responses.iter().filter(|r| r.success).count();
        let failed_shutdowns = responses.len() - successful_shutdowns;

        info!(
            duration = ?duration,
            successful = successful_shutdowns,
            failed = failed_shutdowns,
            "Global shutdown completed"
        );

        // Send global shutdown completed event
        let event = ShutdownEvent::GlobalShutdownCompleted {
            duration,
            successful_shutdowns,
            failed_shutdowns,
            timestamp: SystemTime::now(),
        };
        let _ = self.shutdown_events.send(event);

        // Notify all waiters that shutdown is complete
        self.completion_notify.notify_waiters();

        Ok(responses)
    }

    /// Execute graceful shutdown for a collector
    async fn execute_graceful_shutdown(&self, collector_id: &str) -> Result<()> {
        // Phase 1: Preparing
        self.update_shutdown_phase(collector_id, ShutdownPhase::Preparing)
            .await?;

        // Allow collector to prepare for shutdown
        sleep(Duration::from_millis(100)).await;

        // Phase 2: Stopping
        self.update_shutdown_phase(collector_id, ShutdownPhase::Stopping)
            .await?;

        // Stop active operations
        self.stop_collector_operations(collector_id).await?;

        // Phase 3: Cleanup
        self.update_shutdown_phase(collector_id, ShutdownPhase::Cleanup)
            .await?;

        // Perform cleanup
        self.cleanup_collector_resources(collector_id).await?;

        // Phase 4: Completed
        self.update_shutdown_phase(collector_id, ShutdownPhase::Completed)
            .await?;

        Ok(())
    }

    /// Execute forced shutdown for a collector
    async fn execute_forced_shutdown(&self, collector_id: &str) -> Result<()> {
        info!(collector_id = %collector_id, "Executing forced shutdown");

        // Update to forced phase
        self.update_shutdown_phase(collector_id, ShutdownPhase::Forced)
            .await?;

        // Send forced shutdown event
        let event = ShutdownEvent::ForcedShutdown {
            collector_id: collector_id.to_string(),
            reason: "Forced shutdown requested".to_string(),
            timestamp: SystemTime::now(),
        };
        let _ = self.shutdown_events.send(event);

        // Immediate resource cleanup
        self.cleanup_collector_resources(collector_id).await?;

        // Mark as completed
        self.update_shutdown_phase(collector_id, ShutdownPhase::Completed)
            .await?;

        Ok(())
    }

    /// Update shutdown phase for a collector
    async fn update_shutdown_phase(
        &self,
        collector_id: &str,
        new_phase: ShutdownPhase,
    ) -> Result<()> {
        let old_phase = {
            let mut states = self.shutdown_states.write().unwrap();
            if let Some(state) = states.get_mut(collector_id) {
                let old_phase = state.phase.clone();
                state.phase = new_phase.clone();
                state.last_update = SystemTime::now();
                old_phase
            } else {
                return Err(anyhow::anyhow!("Collector not found: {}", collector_id));
            }
        };

        debug!(
            collector_id = %collector_id,
            old_phase = ?old_phase,
            new_phase = ?new_phase,
            "Shutdown phase updated"
        );

        // Send phase change event
        let event = ShutdownEvent::PhaseChanged {
            collector_id: collector_id.to_string(),
            old_phase,
            new_phase,
            timestamp: SystemTime::now(),
        };
        let _ = self.shutdown_events.send(event);

        Ok(())
    }

    /// Stop collector operations
    async fn stop_collector_operations(&self, collector_id: &str) -> Result<()> {
        debug!(collector_id = %collector_id, "Stopping collector operations");

        // Simulate stopping operations
        sleep(Duration::from_millis(200)).await;

        Ok(())
    }

    /// Cleanup collector resources
    async fn cleanup_collector_resources(&self, collector_id: &str) -> Result<()> {
        debug!(collector_id = %collector_id, "Cleaning up collector resources");

        // Mark cleanup as completed
        {
            let mut states = self.shutdown_states.write().unwrap();
            if let Some(state) = states.get_mut(collector_id) {
                state.cleanup_completed = true;
            }
        }

        // Simulate cleanup operations
        sleep(Duration::from_millis(100)).await;

        Ok(())
    }

    /// Shutdown collectors in parallel
    async fn shutdown_collectors_parallel(
        &self,
        collector_ids: Vec<String>,
        reason: String,
    ) -> Result<Vec<ShutdownResponse>> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            self.config.max_concurrent_shutdowns,
        ));
        let mut tasks = Vec::new();

        for collector_id in collector_ids {
            let semaphore = Arc::clone(&semaphore);
            let reason = reason.clone();
            let coordinator = self.clone_for_task();

            let task = tokio::spawn(async move {
                let permit = match semaphore.acquire().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        error!(
                            collector_id = %collector_id,
                            error = %err,
                            "Failed to acquire shutdown semaphore permit"
                        );
                        return Err(anyhow::anyhow!("Failed to acquire shutdown permit: {err}"));
                    }
                };

                let request = ShutdownRequest {
                    collector_id: collector_id.clone(),
                    reason,
                    force: false,
                    timeout: None,
                };

                let result = coordinator.shutdown_collector(request).await;
                drop(permit);
                result
            });

            tasks.push(task);
        }

        let mut responses = Vec::new();
        for task in tasks {
            match task.await {
                Ok(Ok(response)) => responses.push(response),
                Ok(Err(e)) => {
                    error!(error = %e, "Shutdown task failed");
                    responses.push(ShutdownResponse {
                        success: false,
                        error_message: Some(e.to_string()),
                        duration: Duration::from_secs(0),
                        final_phase: ShutdownPhase::Failed,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Shutdown task join failed");
                    responses.push(ShutdownResponse {
                        success: false,
                        error_message: Some(format!("Task join failed: {}", e)),
                        duration: Duration::from_secs(0),
                        final_phase: ShutdownPhase::Failed,
                    });
                }
            }
        }

        Ok(responses)
    }

    /// Shutdown collectors sequentially
    async fn shutdown_collectors_sequential(
        &self,
        collector_ids: Vec<String>,
        reason: String,
    ) -> Result<Vec<ShutdownResponse>> {
        let mut responses = Vec::new();

        for collector_id in collector_ids {
            let request = ShutdownRequest {
                collector_id: collector_id.clone(),
                reason: reason.clone(),
                force: false,
                timeout: None,
            };

            let response = self.shutdown_collector(request).await?;
            responses.push(response);

            // Add delay between shutdowns
            if !self.config.sequence_delay.is_zero() {
                sleep(self.config.sequence_delay).await;
            }
        }

        Ok(responses)
    }

    /// Clone coordinator for async tasks (simplified version)
    fn clone_for_task(&self) -> Self {
        Self {
            config: self.config.clone(),
            shutdown_states: Arc::clone(&self.shutdown_states),
            shutdown_events: self.shutdown_events.clone(),
            global_shutdown: Arc::clone(&self.global_shutdown),
            sequence_counter: Arc::clone(&self.sequence_counter),
            completion_notify: Arc::clone(&self.completion_notify),
            active_tasks: Arc::clone(&self.active_tasks),
        }
    }

    /// Subscribe to shutdown events
    pub fn subscribe_to_shutdown_events(&self) -> broadcast::Receiver<ShutdownEvent> {
        self.shutdown_events.subscribe()
    }

    /// Check if global shutdown is active
    pub fn is_global_shutdown_active(&self) -> bool {
        self.global_shutdown.load(Ordering::Relaxed)
    }

    /// Get shutdown status for a collector
    pub async fn get_shutdown_status(&self, collector_id: &str) -> Option<ShutdownPhase> {
        let states = self.shutdown_states.read().unwrap();
        states.get(collector_id).map(|s| s.phase.clone())
    }

    /// Wait for all shutdowns to complete
    pub async fn wait_for_completion(&self, timeout: Duration) -> Result<()> {
        // Use a loop to handle race conditions between checking the flag and waiting
        loop {
            if !self.global_shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }

            // Wait for notification with timeout
            match tokio::time::timeout(timeout, self.completion_notify.notified()).await {
                Ok(_) => {
                    // Notification received, check if shutdown is still active
                    if !self.global_shutdown.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    // If still active, continue waiting (spurious wakeup)
                }
                Err(_) => {
                    return Err(anyhow::anyhow!("Timeout waiting for shutdown completion"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let config = ShutdownConfig {
            graceful_timeout: Duration::from_secs(5),
            forced_timeout: Duration::from_secs(1),
            sequence_delay: Duration::from_millis(100),
            enable_parallel_shutdown: false,
            max_concurrent_shutdowns: 1,
        };

        let coordinator = ShutdownCoordinator::new(config);

        let request = ShutdownRequest {
            collector_id: "test-collector".to_string(),
            reason: "Test shutdown".to_string(),
            force: false,
            timeout: None,
        };

        let response = coordinator.shutdown_collector(request).await.unwrap();
        assert!(response.success);
        assert_eq!(response.final_phase, ShutdownPhase::Completed);
    }

    #[tokio::test]
    async fn test_forced_shutdown() {
        let config = ShutdownConfig::default();
        let coordinator = ShutdownCoordinator::new(config);

        let request = ShutdownRequest {
            collector_id: "test-collector".to_string(),
            reason: "Test forced shutdown".to_string(),
            force: true,
            timeout: None,
        };

        let response = coordinator.shutdown_collector(request).await.unwrap();
        assert!(response.success);
        assert_eq!(response.final_phase, ShutdownPhase::Completed);
    }

    #[tokio::test]
    async fn test_shutdown_events() {
        let config = ShutdownConfig::default();
        let coordinator = ShutdownCoordinator::new(config);

        let mut event_rx = coordinator.subscribe_to_shutdown_events();

        let request = ShutdownRequest {
            collector_id: "event-test".to_string(),
            reason: "Test events".to_string(),
            force: false,
            timeout: None,
        };

        // Start shutdown in background
        let coordinator_clone = coordinator.clone_for_task();
        tokio::spawn(async move { coordinator_clone.shutdown_collector(request).await });

        // Check for shutdown initiated event
        if let Ok(event) = event_rx.recv().await {
            match event {
                ShutdownEvent::ShutdownInitiated { collector_id, .. } => {
                    assert_eq!(collector_id, "event-test");
                }
                _ => panic!("Expected ShutdownInitiated event"),
            }
        }
    }
}
