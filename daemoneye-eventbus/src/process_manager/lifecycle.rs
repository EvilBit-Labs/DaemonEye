//! Lifecycle operations for collector processes: spawning, monitoring,
//! heartbeats, and restart.

use super::types::{CollectorConfig, CollectorProcess, CollectorState, ProcessManagerError};
use super::{CollectorProcessManager, RestartRequest};
use crate::Message;
use crate::rpc::CollectorRpcClient;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

impl CollectorProcessManager {
    pub(super) async fn spawn_process_with_retries(
        binary_path: &PathBuf,
        config: &CollectorConfig,
    ) -> io::Result<Child> {
        const MAX_ATTEMPTS: usize = 3;
        let mut attempt = 0_usize;

        loop {
            let mut command = Command::new(binary_path);
            command.args(&config.args);

            for (key, value) in &config.env {
                command.env(key, value);
            }

            if let Some(ref working_dir) = config.working_dir {
                command.current_dir(working_dir);
            }

            command.stdin(Stdio::null());
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());

            match command.spawn() {
                Ok(child) => return Ok(child),
                Err(e)
                    if Self::is_text_file_busy(&e) && attempt.saturating_add(1) < MAX_ATTEMPTS =>
                {
                    attempt = attempt.saturating_add(1);
                    // SAFETY: attempt is bounded by MAX_ATTEMPTS (a small constant); cast to u64 is lossless.
                    #[allow(clippy::as_conversions)]
                    let backoff_ms = 25_u64.saturating_mul(attempt as u64);
                    warn!(
                        attempt,
                        backoff_ms,
                        path = %binary_path.display(),
                        error = %e,
                        "Collector spawn returned ETXTBUSY, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn is_text_file_busy(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(code) if code == 26)
    }

    /// Start a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Unique identifier for the collector
    /// * `collector_type` - Type of collector (procmond, netmond, etc.)
    /// * `config` - Configuration for spawning the process
    ///
    /// # Returns
    ///
    /// Process ID on success
    ///
    /// # Errors
    ///
    /// - `AlreadyRunning` if collector is already running
    /// - `SpawnFailed` if process spawn fails
    pub async fn start_collector(
        &self,
        collector_id: &str,
        collector_type: &str,
        config: CollectorConfig,
    ) -> Result<u32, ProcessManagerError> {
        {
            let processes = self.processes.lock().await;
            if processes.contains_key(collector_id) {
                return Err(ProcessManagerError::AlreadyRunning(collector_id.to_owned()));
            }
        }

        let binary_path = if config.binary_path.as_os_str().is_empty() {
            self.config
                .collector_binaries
                .get(collector_type)
                .cloned()
                .ok_or_else(|| {
                    ProcessManagerError::SpawnFailed(format!(
                        "No binary path configured for collector type: {collector_type}"
                    ))
                })?
        } else {
            config.binary_path.clone()
        };

        info!(
            "Starting collector: {} (type: {}) with binary: {}",
            collector_id,
            collector_type,
            binary_path.display()
        );

        let mut child = Self::spawn_process_with_retries(&binary_path, &config)
            .await
            .map_err(|e| {
                ProcessManagerError::SpawnFailed(format!(
                    "Failed to spawn {}: {}",
                    binary_path.display(),
                    e
                ))
            })?;

        let pid = child
            .id()
            .ok_or_else(|| ProcessManagerError::SpawnFailed("Failed to get PID".to_owned()))?;

        info!("Spawned collector {} with PID: {}", collector_id, pid);

        let mut processes = self.processes.lock().await;
        if processes.contains_key(collector_id) {
            drop(processes);
            warn!(
                collector_id,
                pid,
                "Collector registered while spawn was in progress; terminating duplicate instance"
            );
            if let Err(e) = child.start_kill() {
                warn!(pid, error = %e, "Failed to signal duplicate collector for termination");
            }
            drop(child.wait().await);
            return Err(ProcessManagerError::AlreadyRunning(collector_id.to_owned()));
        }

        let now = SystemTime::now();
        let process = CollectorProcess {
            collector_id: collector_id.to_owned(),
            collector_type: collector_type.to_owned(),
            child: Some(child),
            pid,
            state: CollectorState::Starting,
            start_time: now,
            config: config.clone(),
            restart_count: 0,
            last_health_check: None,
            last_heartbeat: now,
            missed_heartbeats: 0,
            heartbeat_enabled: true,
            heartbeat_sequence: 0,
            rpc_client: None,
        };

        processes.insert(collector_id.to_owned(), process);
        drop(processes);

        // Create RPC client if broker is available
        if let Some(ref broker) = self.broker {
            let target_topic = format!("control.collector.{collector_id}");
            match CollectorRpcClient::new(&target_topic, Arc::clone(broker)).await {
                Ok(client) => {
                    if let Some(proc_entry) = self.processes.lock().await.get_mut(collector_id) {
                        proc_entry.rpc_client = Some(Arc::new(client));
                    }
                    info!(
                        collector_id = %collector_id,
                        "Created RPC client for collector"
                    );
                }
                Err(e) => {
                    warn!(
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to create RPC client, will use signal-based operations"
                    );
                }
            }
        }

        // Spawn monitoring task
        self.spawn_process_monitor(collector_id.to_owned()).await;

        // Spawn heartbeat task
        self.spawn_heartbeat_task(collector_id.to_owned()).await;

        // Update state to Running
        if let Some(proc_entry) = self.processes.lock().await.get_mut(collector_id) {
            proc_entry.state = CollectorState::Running;
        }

        Ok(pid)
    }

    /// Spawn a monitoring task for a collector process
    ///
    /// This task waits for the process to exit and handles auto-restart if configured.
    /// Auto-restart is performed if:
    /// - Global `enable_auto_restart` is true
    /// - Process-level `auto_restart` is true
    /// - `restart_count < max_restarts`
    ///
    /// On exit without restart, the collector is removed from the process map to allow subsequent start calls.
    #[allow(clippy::unused_async)]
    async fn spawn_process_monitor(&self, collector_id: String) {
        let processes = Arc::clone(&self.processes);
        let process_manager_config = self.config.clone();
        let restart_tx = self.restart_tx.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Check if process exited and decide on restart
                let restart_request = {
                    let mut procs = processes.lock().await;

                    let Some(proc) = procs.get_mut(&collector_id) else {
                        debug!(
                            "Collector {} no longer in process map, ending monitor",
                            collector_id
                        );
                        return;
                    };

                    let child_pid = proc.pid;

                    match proc
                        .child
                        .as_mut()
                        .and_then(|c| c.try_wait().ok())
                        .flatten()
                    {
                        Some(status) => {
                            info!(
                                "Collector {} (PID: {}) exited with status: {:?}",
                                collector_id, child_pid, status
                            );

                            // Determine if should auto-restart
                            let should_restart = process_manager_config.enable_auto_restart
                                && proc.config.auto_restart
                                && proc.restart_count < proc.config.max_restarts;

                            if should_restart {
                                let restart_count = proc.restart_count;
                                let collector_type = proc.collector_type.clone();
                                let config = proc.config.clone();

                                info!(
                                    "Collector {} will be auto-restarted (attempt {}/{})",
                                    collector_id,
                                    restart_count.saturating_add(1),
                                    config.max_restarts
                                );

                                // Remove from map to allow start_collector to succeed
                                procs.remove(&collector_id);

                                Some(RestartRequest {
                                    collector_id: collector_id.clone(),
                                    collector_type,
                                    config,
                                    restart_count,
                                })
                            } else {
                                // Not restarting, remove from map
                                if status.success() {
                                    info!(
                                        "Collector {} stopped cleanly, removing from map",
                                        collector_id
                                    );
                                } else {
                                    warn!(
                                        "Collector {} failed with exit code: {:?}, removing from map",
                                        collector_id,
                                        status.code()
                                    );
                                }
                                procs.remove(&collector_id);
                                None
                            }
                        }
                        None => {
                            // Still running, continue monitoring
                            None
                        }
                    }
                };
                // Lock dropped here

                // Send restart request if needed, then exit monitor
                if let Some(req) = restart_request {
                    if let Err(e) = restart_tx.send(req) {
                        error!("Failed to send restart request: {}", e);
                    }
                    return;
                }

                // If no restart but we removed the process, exit monitor
                if restart_request.is_none() {
                    let procs = processes.lock().await;
                    if !procs.contains_key(&collector_id) {
                        debug!("Monitor exiting for collector {}", collector_id);
                        return;
                    }
                }
            }
        });
    }

    /// Spawn a heartbeat task for a collector process
    ///
    /// This task periodically publishes heartbeat messages and monitors for timeouts.
    /// The task exits when the collector is removed from the process map.
    #[allow(clippy::unused_async)]
    async fn spawn_heartbeat_task(&self, collector_id: String) {
        let processes = Arc::clone(&self.processes);
        let broker = self.broker.clone();
        let heartbeat_interval = self.config.health_check_interval;
        let heartbeat_threshold = self.config.heartbeat_timeout_multiplier;

        // Guard against zero or extremely small intervals to avoid busy loops
        let min_interval = Duration::from_millis(50);
        let effective_interval = if heartbeat_interval.is_zero() {
            warn!(
                "Heartbeat interval is zero, using minimum of {:?}",
                min_interval
            );
            min_interval
        } else if heartbeat_interval < min_interval {
            warn!(
                "Heartbeat interval {:?} below minimum {:?}, clamping to minimum",
                heartbeat_interval, min_interval
            );
            min_interval
        } else {
            heartbeat_interval
        };

        tokio::spawn(async move {
            Self::run_heartbeat_loop(
                collector_id,
                processes,
                broker,
                effective_interval,
                heartbeat_threshold,
            )
            .await;
        });
    }

    // SAFETY: MutexGuard is always dropped before any await point inside the loop body.
    #[allow(clippy::significant_drop_tightening)]
    async fn run_heartbeat_loop(
        collector_id: String,
        processes: Arc<Mutex<HashMap<String, CollectorProcess>>>,
        broker: Option<Arc<crate::DaemoneyeBroker>>,
        heartbeat_interval: Duration,
        heartbeat_threshold: u32,
    ) {
        loop {
            tokio::time::sleep(heartbeat_interval).await;

            // Phase 1: Read state and compute work, then drop the lock before awaiting.
            let (sequence, intervals_missed, now, heartbeat_enabled) = {
                // SAFETY: MutexGuard is dropped at end of block before any await.
                #[allow(clippy::significant_drop_tightening)]
                let mut procs = processes.lock().await;

                let Some(proc) = procs.get_mut(&collector_id) else {
                    debug!(
                        "Collector {} no longer in process map, ending heartbeat task",
                        collector_id
                    );
                    return;
                };

                let now = SystemTime::now();
                let elapsed_since_heartbeat = now
                    .duration_since(proc.last_heartbeat)
                    .unwrap_or(Duration::from_secs(0));

                // Calculate expected heartbeats missed based on elapsed time.
                // `checked_div` returns None when the divisor is zero, which collapses
                // to the "no intervals missed yet" case via `unwrap_or(0)`.
                let intervals_missed: u64 = {
                    let interval_ms = heartbeat_interval.as_millis();
                    let elapsed_ms = elapsed_since_heartbeat.as_millis();
                    // SAFETY: both values are u128 millisecond counts; the quotient
                    // fits u64 in practice and we saturate on overflow.
                    let n = elapsed_ms.checked_div(interval_ms).unwrap_or(0);
                    u64::try_from(n).unwrap_or(u64::MAX)
                };

                // Increment heartbeat sequence
                proc.heartbeat_sequence = proc.heartbeat_sequence.wrapping_add(1);
                let sequence = proc.heartbeat_sequence;

                (sequence, intervals_missed, now, proc.heartbeat_enabled)
            };
            // Lock dropped here — safe to await.

            // Phase 2: Publish heartbeat (async, no lock held).
            let publish_result = if let Some(ref hb_broker) = broker {
                let topic = format!("control.health.heartbeat.{collector_id}");
                let message = Message::heartbeat(sequence);
                let correlation_id = format!("heartbeat-{collector_id}-{sequence}");

                match serde_json::to_vec(&message) {
                    Ok(payload) => {
                        match hb_broker.publish(&topic, &correlation_id, payload).await {
                            Ok(()) => {
                                debug!(
                                    "Published heartbeat {} for collector {}",
                                    sequence, collector_id
                                );
                                Some(true) // success
                            }
                            Err(e) => {
                                warn!("Failed to publish heartbeat for {}: {}", collector_id, e);
                                Some(false) // publish failure
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to serialize heartbeat message: {}", e);
                        Some(false) // serialize failure
                    }
                }
            } else {
                None // no broker
            };

            // Phase 3: Update state based on publish result.
            {
                // SAFETY: MutexGuard is dropped at end of block; no await follows within this block.
                #[allow(clippy::significant_drop_tightening)]
                let mut procs = processes.lock().await;

                let Some(proc) = procs.get_mut(&collector_id) else {
                    return; // Collector removed between phases
                };

                match publish_result {
                    Some(true) => {
                        // Successful publish: reset counters and update timestamp
                        proc.missed_heartbeats = 0;
                        proc.last_heartbeat = now;
                    }
                    Some(false) => {
                        // Publish or serialize failed
                        proc.missed_heartbeats = proc.missed_heartbeats.saturating_add(1);
                    }
                    None => {
                        // No broker: increment based on elapsed time if applicable
                        if intervals_missed > 0 {
                            // SAFETY: heartbeat_threshold is u32; adding 1 saturates safely.
                            #[allow(clippy::as_conversions)]
                            let cap = intervals_missed
                                .min(u64::from(heartbeat_threshold).saturating_add(1))
                                as u32;
                            proc.missed_heartbeats = cap;
                        }
                    }
                }
            }
            // Lock dropped here.

            let should_exit = heartbeat_enabled;

            if !should_exit {
                debug!(
                    "Heartbeat monitoring disabled for {}, exiting task",
                    collector_id
                );
                return;
            }
        }
    }

    /// Restart a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to restart
    /// * `timeout` - Timeout for stop operation
    ///
    /// # Returns
    ///
    /// New process ID on success
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `SpawnFailed` if restart fails
    // SAFETY: MutexGuard is dropped before any subsequent await in this function.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn restart_collector(
        &self,
        collector_id: &str,
        timeout: Duration,
    ) -> Result<u32, ProcessManagerError> {
        info!("Restarting collector: {}", collector_id);

        // Get current config
        let (collector_type, config, restart_count) = {
            // SAFETY: MutexGuard dropped at end of block before any await.
            #[allow(clippy::significant_drop_tightening)]
            let processes = self.processes.lock().await;
            let process = processes
                .get(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

            (
                process.collector_type.clone(),
                process.config.clone(),
                process.restart_count,
            )
        };

        // Stop the collector
        self.stop_collector(collector_id, true, timeout).await?;

        // Start with incremented restart count
        let pid = self
            .start_collector(collector_id, &collector_type, config)
            .await?;

        // Update restart count
        if let Some(process) = self.processes.lock().await.get_mut(collector_id) {
            process.restart_count = restart_count.saturating_add(1);
        }

        Ok(pid)
    }
}
