//! Process event source implementation for collector-core framework.
//!
//! This module provides the `ProcessEventSource` that implements the `EventSource` trait
//! and wraps the existing `ProcessMessageHandler` to integrate with the collector-core
//! framework while preserving all existing functionality.

// ProcessMessageHandler is no longer directly used but kept for reference
use async_trait::async_trait;
use collector_core::{CollectionEvent, EventSource, ProcessEvent, SourceCaps};
use daemoneye_lib::storage;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};
use sysinfo::System;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

/// Process event source that implements the EventSource trait.
///
/// This struct wraps the existing `ProcessMessageHandler` and provides a bridge
/// between the collector-core framework and the existing process collection logic.
/// It maintains all existing functionality while enabling integration with the
/// unified collection system.
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │                ProcessEventSource                           │
/// │  ┌─────────────────────────────────────────────────────────┐ │
/// │  │  ProcessMessageHandler (existing logic)                │ │
/// │  │  - Process enumeration via sysinfo                     │ │
/// │  │  - Database operations                                 │ │
/// │  │  - Process record conversion                           │ │
/// │  └─────────────────────────────────────────────────────────┘ │
/// │  ┌─────────────────────────────────────────────────────────┐ │
/// │  │  EventSource trait implementation                      │ │
/// │  │  - Async event streaming                               │ │
/// │  │  - Capability negotiation                              │ │
/// │  │  - Health monitoring                                   │ │
/// │  └─────────────────────────────────────────────────────────┘ │
/// └─────────────────────────────────────────────────────────────┘
/// ```
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::event_source::ProcessEventSource;
/// use daemoneye_lib::storage::DatabaseManager;
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
///
/// // Create database manager
/// let db_manager = Arc::new(Mutex::new(
///     DatabaseManager::new("/var/lib/daemoneye/processes.db")
///         .expect("Failed to create database manager")
/// ));
///
/// // Create process event source
/// let process_source = ProcessEventSource::new(db_manager);
///
/// // The source can now be registered with collector-core
/// ```
pub struct ProcessEventSource {
    /// Database manager for audit logging and data storage
    #[allow(dead_code)] // Database is used for future audit logging features
    database: Arc<Mutex<storage::DatabaseManager>>,
    /// Configuration for process collection
    config: ProcessSourceConfig,
}

/// Configuration for the process event source.
#[derive(Debug, Clone)]
pub struct ProcessSourceConfig {
    /// Interval between process collection cycles
    pub collection_interval: Duration,
    /// Whether to collect enhanced metadata (requires privileges)
    pub collect_enhanced_metadata: bool,
    /// Maximum number of processes to collect per cycle (0 = unlimited)
    pub max_processes_per_cycle: usize,
    /// Whether to compute executable hashes
    pub compute_executable_hashes: bool,
}

impl Default for ProcessSourceConfig {
    fn default() -> Self {
        Self {
            collection_interval: Duration::from_secs(30),
            collect_enhanced_metadata: true,
            max_processes_per_cycle: 0, // Unlimited
            compute_executable_hashes: false,
        }
    }
}

impl ProcessEventSource {
    /// Creates a new process event source with the specified database manager.
    ///
    /// # Arguments
    ///
    /// * `database` - Thread-safe database manager for audit logging
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_source::ProcessEventSource;
    /// use daemoneye_lib::storage::DatabaseManager;
    /// use std::sync::Arc;
    /// use tokio::sync::Mutex;
    ///
    /// let db_manager = Arc::new(Mutex::new(
    ///     DatabaseManager::new("/var/lib/daemoneye/processes.db")?
    /// ));
    /// let source = ProcessEventSource::new(db_manager);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(database: Arc<Mutex<storage::DatabaseManager>>) -> Self {
        Self {
            database,
            config: ProcessSourceConfig::default(),
        }
    }

    /// Creates a new process event source with custom configuration.
    ///
    /// # Arguments
    ///
    /// * `database` - Thread-safe database manager for audit logging
    /// * `config` - Custom configuration for the process source
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_source::{ProcessEventSource, ProcessSourceConfig};
    /// use daemoneye_lib::storage::DatabaseManager;
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use tokio::sync::Mutex;
    ///
    /// let db_manager = Arc::new(Mutex::new(
    ///     DatabaseManager::new("/var/lib/daemoneye/processes.db")?
    /// ));
    /// let config = ProcessSourceConfig {
    ///     collection_interval: Duration::from_secs(60),
    ///     collect_enhanced_metadata: false,
    ///     max_processes_per_cycle: 1000,
    ///     compute_executable_hashes: true,
    /// };
    /// let source = ProcessEventSource::with_config(db_manager, config);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_config(
        database: Arc<Mutex<storage::DatabaseManager>>,
        config: ProcessSourceConfig,
    ) -> Self {
        Self { database, config }
    }

    /// Collects process information and converts it to collection events.
    ///
    /// This method performs the actual process enumeration using the existing
    /// `ProcessMessageHandler` logic and converts the results to `CollectionEvent`s
    /// for the collector-core framework.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel sender for emitting collection events
    ///
    /// # Errors
    ///
    /// Returns an error if process enumeration fails or if the event channel is closed.
    async fn collect_processes(&self, tx: &mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        let mut system = System::new_all();
        system.refresh_all();

        let processes = system.processes();
        let mut collected_count = 0;

        for (pid, process) in processes.iter() {
            // Check if we've hit the maximum process limit
            if self.config.max_processes_per_cycle > 0
                && collected_count >= self.config.max_processes_per_cycle
            {
                debug!(
                    max_processes = self.config.max_processes_per_cycle,
                    "Reached maximum process collection limit"
                );
                break;
            }

            // Convert sysinfo process to our event format
            let process_event = self.convert_to_process_event(pid, process);

            // Send the event
            if tx
                .send(CollectionEvent::Process(process_event))
                .await
                .is_err()
            {
                warn!("Event channel closed, stopping process collection");
                return Err(anyhow::anyhow!("Event channel closed"));
            }

            collected_count += 1;
        }

        info!(
            processes_collected = collected_count,
            "Process collection cycle completed"
        );

        Ok(())
    }

    /// Converts a sysinfo process to a ProcessEvent.
    ///
    /// This method reuses the existing conversion logic from `ProcessMessageHandler`
    /// but adapts it to work with the collector-core event model.
    fn convert_to_process_event(
        &self,
        pid: &sysinfo::Pid,
        process: &sysinfo::Process,
    ) -> ProcessEvent {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(|p| p.as_u32());
        let name = process.name().to_string_lossy().to_string();
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        // Convert start time from sysinfo format to SystemTime
        let start_time = if self.config.collect_enhanced_metadata {
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(process.start_time()))
        } else {
            None
        };

        // Collect CPU and memory usage if enhanced metadata is enabled
        let (cpu_usage, memory_usage) = if self.config.collect_enhanced_metadata {
            (Some(process.cpu_usage() as f64), Some(process.memory()))
        } else {
            (None, None)
        };

        // Compute executable hash if requested
        let executable_hash = if self.config.compute_executable_hashes {
            // TODO: Implement executable hashing issue #40
            // For now, we'll leave this as None until the hashing implementation is added
            None
        } else {
            None
        };

        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true; // Process is accessible if we can enumerate it
        let file_exists = executable_path.is_some();

        ProcessEvent {
            pid: pid_u32,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            user_id,
            accessible,
            file_exists,
            timestamp: SystemTime::now(),
        }
    }
}

#[async_trait]
impl EventSource for ProcessEventSource {
    fn name(&self) -> &'static str {
        "process-monitor"
    }

    fn capabilities(&self) -> SourceCaps {
        let mut caps = SourceCaps::PROCESS | SourceCaps::SYSTEM_WIDE;

        // Add real-time capability if we're collecting frequently
        if self.config.collection_interval <= Duration::from_secs(10) {
            caps |= SourceCaps::REALTIME;
        }

        caps
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        info!(
            collection_interval_secs = self.config.collection_interval.as_secs(),
            enhanced_metadata = self.config.collect_enhanced_metadata,
            max_processes = self.config.max_processes_per_cycle,
            "Starting process event source"
        );

        let mut collection_interval = tokio::time::interval(self.config.collection_interval);

        // Initial collection
        if let Err(e) = self.collect_processes(&tx).await {
            error!(error = %e, "Failed during initial process collection");
            return Err(e);
        }

        // Main collection loop
        loop {
            tokio::select! {
                _ = collection_interval.tick() => {
                    if let Err(e) = self.collect_processes(&tx).await {
                        error!(error = %e, "Process collection failed");
                        // Continue running even if individual collections fail
                    }
                }
                _ = async {
                    while !shutdown_signal.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                } => {
                    info!("Shutdown signal received, stopping process collection");
                    break;
                }
            }
        }

        info!("Process event source stopped");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("Stopping process event source");
        // No cleanup needed for process collection
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        // Perform a quick health check by trying to enumerate a few processes
        let mut system = System::new_all();
        system.refresh_all();

        let process_count = system.processes().len();
        if process_count == 0 {
            return Err(anyhow::anyhow!("No processes found during health check"));
        }

        debug!(
            process_count = process_count,
            "Process event source health check passed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_database() -> Arc<Mutex<storage::DatabaseManager>> {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("test.db");
        Arc::new(Mutex::new(
            storage::DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for test"),
        ))
    }

    #[tokio::test]
    async fn test_process_event_source_creation() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        assert_eq!(source.name(), "process-monitor");
        assert!(source.capabilities().contains(SourceCaps::PROCESS));
        assert!(source.capabilities().contains(SourceCaps::SYSTEM_WIDE));
    }

    #[tokio::test]
    async fn test_process_event_source_with_config() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            collection_interval: Duration::from_secs(5),
            collect_enhanced_metadata: false,
            max_processes_per_cycle: 100,
            compute_executable_hashes: true,
        };
        let source = ProcessEventSource::with_config(db_manager, config);

        assert_eq!(source.name(), "process-monitor");
        assert!(source.capabilities().contains(SourceCaps::PROCESS));
        assert!(source.capabilities().contains(SourceCaps::REALTIME)); // Due to 5s interval
    }

    #[tokio::test]
    async fn test_convert_to_process_event() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        let mut system = System::new_all();
        system.refresh_all();

        if let Some((pid, process)) = system.processes().iter().next() {
            let event = source.convert_to_process_event(pid, process);

            assert_eq!(event.pid, pid.as_u32());
            assert_eq!(event.name, process.name().to_string_lossy().to_string());
            assert!(event.accessible);
            assert!(event.timestamp <= SystemTime::now());
        }
    }

    #[tokio::test]
    async fn test_health_check() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        let result = source.health_check().await;
        assert!(
            result.is_ok(),
            "Health check should pass on a system with processes"
        );
    }

    #[tokio::test]
    async fn test_capabilities_with_different_intervals() {
        let db_manager = create_test_database();

        // Fast collection should include REALTIME capability
        let fast_config = ProcessSourceConfig {
            collection_interval: Duration::from_secs(5),
            ..Default::default()
        };
        let fast_source = ProcessEventSource::with_config(db_manager.clone(), fast_config);
        assert!(fast_source.capabilities().contains(SourceCaps::REALTIME));

        // Slow collection should not include REALTIME capability
        let slow_config = ProcessSourceConfig {
            collection_interval: Duration::from_secs(60),
            ..Default::default()
        };
        let slow_source = ProcessEventSource::with_config(db_manager, slow_config);
        assert!(!slow_source.capabilities().contains(SourceCaps::REALTIME));
    }
}
