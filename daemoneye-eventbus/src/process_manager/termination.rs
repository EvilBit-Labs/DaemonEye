//! Collector termination operations: graceful and forced shutdown,
//! including RPC-based shutdown coordination.

use super::CollectorProcessManager;
#[cfg(all(unix, feature = "freebsd"))]
use super::send_signal;
use super::types::{CollectorState, ProcessManagerError};
use crate::rpc::CollectorRpcClient;
#[cfg(all(unix, feature = "freebsd"))]
use nix::sys::signal::Signal;
use std::time::Duration;
use tokio::process::Child;
use tracing::{error, info, warn};

impl CollectorProcessManager {
    /// Stop a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to stop
    /// * `graceful` - Whether to attempt graceful shutdown first
    /// * `timeout` - Timeout for graceful shutdown
    ///
    /// # Returns
    ///
    /// Exit status on success
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `TerminateFailed` if termination fails
    /// - `Timeout` if graceful shutdown times out
    ///
    /// # Platform Notes
    ///
    /// - **Unix**: Graceful stop sends SIGTERM, force kill sends SIGKILL
    /// - **Windows**: Both graceful and force kill use `TerminateProcess` (immediate termination)
    ///   Windows does not support graceful process termination; applications must implement
    ///   their own shutdown coordination (e.g., via named events or window messages).
    pub async fn stop_collector(
        &self,
        collector_id: &str,
        graceful: bool,
        timeout: Duration,
    ) -> Result<Option<i32>, ProcessManagerError> {
        // Extract child handle and RPC client before awaiting
        #[allow(unused_variables, clippy::significant_drop_tightening)]
        let (mut child, pid, collector_id_owned, rpc_client) = {
            let mut processes = self.processes.lock().await;
            let proc = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_owned()))?;

            // If child is None, process already stopped or being stopped
            if proc.child.is_none() {
                info!(
                    "Collector {} already stopped (no child handle)",
                    collector_id
                );
                processes.remove(collector_id);
                drop(processes);
                return Ok(None);
            }

            info!(
                "Stopping collector {} (PID: {}, graceful: {})",
                collector_id, proc.pid, graceful
            );

            // Mark as stopping
            proc.state = CollectorState::Stopping;
            let pid = proc.pid;
            let collector_id_owned = collector_id.to_owned();

            // Extract child and RPC client, leaving None in their places
            let child = proc.child.take().ok_or_else(|| {
                ProcessManagerError::InvalidState(format!(
                    "Collector {collector_id} has no child process to stop (may have already been stopped)"
                ))
            })?;
            let rpc_client = proc.rpc_client.take();

            (child, pid, collector_id_owned, rpc_client)
        };
        // Lock dropped here

        // Try RPC-based shutdown first if client is available
        if graceful && let Some(client) = rpc_client {
            match self
                .shutdown_collector_rpc(&collector_id_owned, &client, true, timeout, &mut child)
                .await
            {
                Ok(exit_code) => {
                    info!(
                        collector_id = %collector_id_owned,
                        "Collector stopped via RPC"
                    );
                    // Remove from process map
                    self.processes.lock().await.remove(&collector_id_owned);
                    return Ok(exit_code);
                }
                Err(e) => {
                    warn!(
                        collector_id = %collector_id_owned,
                        error = %e,
                        "RPC shutdown failed, falling back to signal-based shutdown"
                    );
                    // Fall through to signal-based shutdown
                }
            }
        }

        let exit_code = if graceful {
            // Send termination signal
            #[cfg(all(unix, feature = "freebsd"))]
            {
                // Treat ESRCH (no such process) as benign: the process likely exited between checks
                match send_signal(pid, Signal::SIGTERM) {
                    Ok(()) => {}
                    Err(ProcessManagerError::TerminateFailed(msg))
                        if msg.contains("ESRCH") || msg.contains("No such process") =>
                    {
                        warn!(
                            "SIGTERM returned ESRCH for collector {} (PID: {}), proceeding as already exited",
                            collector_id_owned, pid
                        );
                        // Continue to wait() which should return immediately if already exited
                    }
                    Err(e) => {
                        // Re-acquire lock to set Failed state and restore child
                        if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned)
                        {
                            proc.state = CollectorState::Failed(format!("Signal failed: {e}"));
                            proc.child = Some(child);
                        }
                        return Err(e);
                    }
                }
            }
            #[cfg(windows)]
            {
                // Windows: start_kill() uses TerminateProcess (immediate termination)
                // No graceful shutdown mechanism available at OS level
                if let Err(e) = child.start_kill() {
                    let err =
                        ProcessManagerError::TerminateFailed(format!("Failed to terminate: {}", e));
                    // Re-acquire lock to set Failed state and restore child
                    if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Terminate failed: {}", e));
                        proc.child = Some(child);
                    }
                    return Err(err);
                }
            }

            // Wait with timeout (lock not held)
            let wait_result = tokio::time::timeout(timeout, child.wait()).await;

            match wait_result {
                Ok(Ok(status)) => {
                    info!(
                        "Collector {} exited gracefully with status: {:?}",
                        collector_id_owned, status
                    );
                    // Re-acquire lock to remove entry on successful termination
                    self.processes.lock().await.remove(&collector_id_owned);
                    status.code()
                }
                Ok(Err(e)) => {
                    warn!("Error waiting for collector {}: {}", collector_id_owned, e);
                    // Re-acquire lock to set Failed state and keep entry for visibility
                    if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Wait failed: {e}"));
                        // Don't restore child as it's consumed by wait()
                    }
                    // Fall through to force kill
                    None
                }
                Err(_) => {
                    warn!(
                        "Graceful shutdown timeout for collector {}, escalating to force kill",
                        collector_id_owned
                    );
                    // Keep entry for force kill attempt
                    // Fall through to force kill
                    None
                }
            }
        } else {
            None
        };

        // If graceful succeeded, return immediately
        if exit_code.is_some() {
            return Ok(exit_code);
        }

        // Force kill path
        #[cfg(all(unix, feature = "freebsd"))]
        {
            // Treat ESRCH (no such process) as benign and proceed to wait
            match send_signal(pid, Signal::SIGKILL) {
                Ok(()) => {}
                Err(ProcessManagerError::TerminateFailed(msg))
                    if msg.contains("ESRCH") || msg.contains("No such process") =>
                {
                    warn!(
                        "SIGKILL returned ESRCH for collector {} (PID: {}), assuming already exited",
                        collector_id_owned, pid
                    );
                }
                Err(e) => {
                    // Re-acquire lock to set Failed state
                    if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Force signal failed: {e}"));
                    }
                    return Err(e);
                }
            }
        }
        #[cfg(windows)]
        {
            if let Err(e) = child.start_kill() {
                let err = ProcessManagerError::TerminateFailed(format!("Failed to kill: {}", e));
                // Re-acquire lock to set Failed state
                if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed(format!("Force kill failed: {}", e));
                }
                return Err(err);
            }
        }

        // Wait with force timeout (lock not held)
        let force_timeout = self.config.default_force_timeout;
        match tokio::time::timeout(force_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                info!(
                    "Collector {} force killed: {:?}",
                    collector_id_owned, status
                );
                // Re-acquire lock to remove entry on successful termination
                self.processes.lock().await.remove(&collector_id_owned);
                Ok(status.code())
            }
            Ok(Err(e)) => {
                // Re-acquire lock to set Failed state and keep entry for visibility
                if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed(format!("Force wait failed: {e}"));
                }
                Err(ProcessManagerError::TerminateFailed(format!(
                    "Force wait failed: {e}"
                )))
            }
            Err(_) => {
                error!("Force kill timeout for collector {}", collector_id_owned);
                // Re-acquire lock to set Failed state and keep entry for visibility
                if let Some(proc) = self.processes.lock().await.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed("Force kill timeout".to_owned());
                }
                Err(ProcessManagerError::Timeout(format!(
                    "Force kill timeout for {collector_id_owned}"
                )))
            }
        }
    }

    /// Shutdown a collector via RPC
    async fn shutdown_collector_rpc(
        &self,
        collector_id: &str,
        client: &CollectorRpcClient,
        graceful: bool,
        timeout: Duration,
        child: &mut Child,
    ) -> Result<Option<i32>, ProcessManagerError> {
        use crate::rpc::{
            CollectorLifecycleRequest, CollectorOperation, RpcRequest, RpcStatus, ShutdownRequest,
            ShutdownType,
        };

        if graceful {
            let shutdown_request = ShutdownRequest {
                collector_id: collector_id.to_owned(),
                shutdown_type: ShutdownType::Graceful,
                // SAFETY: Duration millis fit in u64 for any reasonable timeout value.
                #[allow(clippy::as_conversions)]
                graceful_timeout_ms: timeout.as_millis() as u64,
                force_after_timeout: true,
                reason: Some("Process manager initiated graceful shutdown".to_owned()),
            };
            let request = RpcRequest::shutdown(
                client.client_id.clone(),
                client.target_topic.clone(),
                shutdown_request,
                timeout,
            );

            match tokio::time::timeout(timeout, client.call(request, timeout)).await {
                Ok(Ok(response)) => {
                    if response.status == RpcStatus::Success {
                        // Wait for process to exit
                        match tokio::time::timeout(timeout, child.wait()).await {
                            Ok(Ok(status)) => Ok(status.code()),
                            Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                                "Wait failed: {e}"
                            ))),
                            Err(_) => Err(ProcessManagerError::Timeout(
                                "Process wait timeout".to_owned(),
                            )),
                        }
                    } else {
                        Err(ProcessManagerError::TerminateFailed(
                            response
                                .error_details
                                .map_or_else(|| "RPC shutdown failed".to_owned(), |e| e.message),
                        ))
                    }
                }
                Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                    "RPC call failed: {e}"
                ))),
                Err(_) => Err(ProcessManagerError::Timeout(
                    "RPC shutdown timeout".to_owned(),
                )),
            }
        } else {
            let lifecycle_request = CollectorLifecycleRequest::stop(collector_id);
            let request = RpcRequest::lifecycle(
                client.client_id.clone(),
                client.target_topic.clone(),
                CollectorOperation::Stop,
                lifecycle_request,
                timeout,
            );

            match tokio::time::timeout(timeout, client.call(request, timeout)).await {
                Ok(Ok(response)) => {
                    if response.status == RpcStatus::Success {
                        // Wait for process to exit
                        match tokio::time::timeout(timeout, child.wait()).await {
                            Ok(Ok(status)) => Ok(status.code()),
                            Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                                "Wait failed: {e}"
                            ))),
                            Err(_) => Err(ProcessManagerError::Timeout(
                                "Process wait timeout".to_owned(),
                            )),
                        }
                    } else {
                        Err(ProcessManagerError::TerminateFailed(
                            response
                                .error_details
                                .map_or_else(|| "RPC stop failed".to_owned(), |e| e.message),
                        ))
                    }
                }
                Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                    "RPC call failed: {e}"
                ))),
                Err(_) => Err(ProcessManagerError::Timeout("RPC stop timeout".to_owned())),
            }
        }
    }
}
