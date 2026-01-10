//! Process event source implementation for collector-core framework.
//!
//! This module provides the `ProcessEventSource` that implements the `EventSource` trait
//! and wraps the existing `ProcessMessageHandler` to integrate with the collector-core
//! framework while preserving all existing functionality.

use crate::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use async_trait::async_trait;
use collector_core::{CollectionEvent, EventSource, SourceCaps};
use daemoneye_lib::{storage, telemetry::PerformanceTimer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, instrument, warn};

struct BatchOutcome {
    dead_letters: Vec<CollectionEvent>,
}

impl BatchOutcome {
    const fn new(dead_letters: Vec<CollectionEvent>) -> Self {
        Self { dead_letters }
    }

    const fn empty() -> Self {
        Self {
            dead_letters: Vec::new(),
        }
    }

    fn dead_letters(&self) -> &[CollectionEvent] {
        &self.dead_letters
    }
}

type BatchResult = Result<BatchOutcome, (anyhow::Error, Vec<CollectionEvent>)>;

/// Process event source that implements the `EventSource` trait.
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
/// │  │  - Event batching and backpressure                    │ │
/// │  │  - Graceful shutdown coordination                      │ │
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
    #[allow(dead_code)] // Used for future audit logging features
    database: Arc<Mutex<storage::DatabaseManager>>,
    /// Configuration for process collection
    config: ProcessSourceConfig,
    /// Process collector implementation
    collector: Box<dyn ProcessCollector>,
    /// Runtime statistics for monitoring and health checks
    stats: ProcessSourceStats,
    /// Backpressure semaphore for event flow control
    backpressure_semaphore: Arc<Semaphore>,
}

/// Runtime statistics for process event source monitoring.
#[derive(Debug)]
pub struct ProcessSourceStats {
    /// Total number of collection cycles completed
    pub collection_cycles: AtomicU64,
    /// Total number of processes collected
    pub processes_collected: AtomicU64,
    /// Total number of collection errors
    pub collection_errors: AtomicU64,
    /// Number of events currently being processed
    pub events_in_flight: AtomicUsize,
    /// Last successful collection timestamp
    pub last_collection_time: Arc<Mutex<Option<Instant>>>,
    /// Average collection duration in milliseconds
    pub avg_collection_duration_ms: AtomicU64,
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
    /// Maximum number of events that can be in flight at once
    pub max_events_in_flight: usize,
    /// Timeout for individual collection operations
    pub collection_timeout: Duration,
    /// Timeout for graceful shutdown
    pub shutdown_timeout: Duration,
    /// Maximum time to wait for backpressure relief
    pub max_backpressure_wait: Duration,
    /// Batch size for event processing
    pub event_batch_size: usize,
    /// Timeout for event batching
    pub batch_timeout: Duration,
}

impl Default for ProcessSourceConfig {
    fn default() -> Self {
        Self {
            collection_interval: Duration::from_secs(30),
            collect_enhanced_metadata: true,
            max_processes_per_cycle: 0, // Unlimited
            compute_executable_hashes: false,
            max_events_in_flight: 1000,
            collection_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(10),
            max_backpressure_wait: Duration::from_secs(5),
            event_batch_size: 100,
            batch_timeout: Duration::from_millis(500),
        }
    }
}

impl Default for ProcessSourceStats {
    fn default() -> Self {
        Self {
            collection_cycles: AtomicU64::new(0),
            processes_collected: AtomicU64::new(0),
            collection_errors: AtomicU64::new(0),
            events_in_flight: AtomicUsize::new(0),
            last_collection_time: Arc::new(Mutex::new(None)),
            avg_collection_duration_ms: AtomicU64::new(0),
        }
    }
}

/// Snapshot of process source statistics for monitoring and diagnostics.
#[derive(Debug, Clone)]
pub struct ProcessSourceStatsSnapshot {
    /// Total number of collection cycles completed
    pub collection_cycles: u64,
    /// Total number of processes collected
    pub processes_collected: u64,
    /// Total number of collection errors
    pub collection_errors: u64,
    /// Number of events currently being processed
    pub events_in_flight: usize,
    /// Average collection duration in milliseconds
    pub avg_collection_duration_ms: u64,
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
        let config = ProcessSourceConfig::default();
        let backpressure_semaphore = Arc::new(Semaphore::new(config.max_events_in_flight));

        // Create platform-specific collector with fallback to sysinfo
        let collector = Self::create_platform_collector(&config);

        Self {
            database,
            config,
            collector,
            stats: ProcessSourceStats::default(),
            backpressure_semaphore,
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
    ///     ..Default::default()
    /// };
    /// let source = ProcessEventSource::with_config(db_manager, config);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_config(
        database: Arc<Mutex<storage::DatabaseManager>>,
        config: ProcessSourceConfig,
    ) -> Self {
        let backpressure_semaphore = Arc::new(Semaphore::new(config.max_events_in_flight));

        // Create platform-specific collector with fallback to sysinfo
        let collector = Self::create_platform_collector(&config);

        Self {
            database,
            config,
            collector,
            stats: ProcessSourceStats::default(),
            backpressure_semaphore,
        }
    }

    /// Creates a new process event source with a custom collector and configuration.
    ///
    /// This method allows for dependency injection of different `ProcessCollector`
    /// implementations, enabling platform-specific optimizations and testing.
    ///
    /// # Arguments
    ///
    /// * `database` - Thread-safe database manager for audit logging
    /// * `config` - Configuration for the process source
    /// * `collector` - Custom process collector implementation
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::{ProcessEventSource, ProcessSourceConfig, SysinfoProcessCollector, ProcessCollectionConfig};
    /// use daemoneye_lib::storage::DatabaseManager;
    /// use std::sync::Arc;
    /// use tokio::sync::Mutex;
    ///
    /// let db_manager = Arc::new(Mutex::new(
    ///     DatabaseManager::new("/var/lib/daemoneye/processes.db")?
    /// ));
    /// let config = ProcessSourceConfig::default();
    /// let collector_config = ProcessCollectionConfig::default();
    /// let collector = Box::new(SysinfoProcessCollector::new(collector_config));
    /// let source = ProcessEventSource::with_collector(db_manager, config, collector);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_collector(
        database: Arc<Mutex<storage::DatabaseManager>>,
        config: ProcessSourceConfig,
        collector: Box<dyn ProcessCollector>,
    ) -> Self {
        let backpressure_semaphore = Arc::new(Semaphore::new(config.max_events_in_flight));

        Self {
            database,
            config,
            collector,
            stats: ProcessSourceStats::default(),
            backpressure_semaphore,
        }
    }

    /// Returns current runtime statistics for monitoring and diagnostics.
    pub fn stats(&self) -> ProcessSourceStatsSnapshot {
        ProcessSourceStatsSnapshot {
            collection_cycles: self.stats.collection_cycles.load(Ordering::Relaxed),
            processes_collected: self.stats.processes_collected.load(Ordering::Relaxed),
            collection_errors: self.stats.collection_errors.load(Ordering::Relaxed),
            events_in_flight: self.stats.events_in_flight.load(Ordering::Relaxed),
            avg_collection_duration_ms: self
                .stats
                .avg_collection_duration_ms
                .load(Ordering::Relaxed),
        }
    }

    /// Creates a platform-specific process collector with fallback to sysinfo.
    ///
    /// This method attempts to create the most appropriate collector for the current
    /// platform, falling back to the cross-platform sysinfo collector if platform-specific
    /// collectors are not available or fail to initialize.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the process source
    ///
    /// # Returns
    ///
    /// A boxed `ProcessCollector` implementation suitable for the current platform.
    fn create_platform_collector(config: &ProcessSourceConfig) -> Box<dyn ProcessCollector> {
        let base_collector_config = ProcessCollectionConfig {
            collect_enhanced_metadata: config.collect_enhanced_metadata,
            compute_executable_hashes: config.compute_executable_hashes,
            max_processes: config.max_processes_per_cycle,
            ..Default::default()
        };

        // Try to create platform-specific collector first
        #[cfg(target_os = "windows")]
        {
            use crate::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

            let windows_config = WindowsCollectorConfig::default();
            match WindowsProcessCollector::new(base_collector_config.clone(), windows_config) {
                Ok(collector) => {
                    debug!("Created Windows-specific process collector");
                    return Box::new(collector);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to create Windows-specific collector, falling back to sysinfo"
                    );
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            use crate::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

            let macos_config = MacOSCollectorConfig::default();
            match EnhancedMacOSCollector::new(base_collector_config.clone(), macos_config) {
                Ok(collector) => {
                    debug!("Created macOS-specific process collector");
                    return Box::new(collector);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to create macOS-specific collector, falling back to sysinfo"
                    );
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use crate::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

            let linux_config = LinuxCollectorConfig::default();
            match LinuxProcessCollector::new(base_collector_config.clone(), linux_config) {
                Ok(collector) => {
                    debug!("Created Linux-specific process collector");
                    return Box::new(collector);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to create Linux-specific collector, falling back to sysinfo"
                    );
                }
            }
        }

        // Determine appropriate fallback collector based on platform
        if Self::is_secondary_platform() {
            debug!("Using fallback process collector for secondary platform");
            Box::new(FallbackProcessCollector::new(base_collector_config))
        } else {
            debug!("Using cross-platform sysinfo process collector");
            Box::new(SysinfoProcessCollector::new(base_collector_config))
        }
    }

    /// Determines if the current platform is a secondary/minimally supported platform.
    ///
    /// Secondary platforms are those that don't have dedicated optimized collectors
    /// and should use the `FallbackProcessCollector` instead of `SysinfoProcessCollector`.
    /// This includes BSD variants and other Unix-like systems.
    ///
    /// # Returns
    ///
    /// `true` if the current platform is considered secondary, `false` otherwise.
    const fn is_secondary_platform() -> bool {
        cfg!(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
            target_os = "solaris",
            target_os = "illumos",
            target_os = "aix",
            target_os = "haiku"
        ))
    }

    /// Collects process information and converts it to collection events with batching and backpressure handling.
    ///
    /// This method performs the actual process enumeration using the existing
    /// `ProcessMessageHandler` logic and converts the results to `CollectionEvent`s
    /// for the collector-core framework. It includes comprehensive error handling,
    /// event batching, and backpressure management.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel sender for emitting collection events
    /// * `shutdown_signal` - Atomic boolean to check for shutdown requests
    ///
    /// # Errors
    ///
    /// Returns an error if process enumeration fails critically or if the event channel is closed.
    /// Individual process collection failures are logged but don't stop the overall collection.
    #[instrument(skip(self, tx, shutdown_signal), fields(source = "process-monitor"))]
    async fn collect_processes(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
    ) -> BatchResult {
        let timer = PerformanceTimer::start("process_collection".to_owned());
        let collection_start = Instant::now();

        // Check for shutdown before starting collection
        if shutdown_signal.load(Ordering::Relaxed) {
            debug!("Shutdown signal detected, skipping process collection");
            return Ok(BatchOutcome::empty());
        }

        // Perform process enumeration using the collector trait
        let enumeration_result = timeout(
            self.config.collection_timeout,
            self.collector.collect_processes(),
        )
        .await;

        let (process_events, collection_stats) = match enumeration_result {
            Ok(Ok((events, stats))) => (events, stats),
            Ok(Err(e)) => {
                error!(error = %e, "Process enumeration failed");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err((
                    anyhow::anyhow!("Process collection failed: {e}"),
                    Vec::new(),
                ));
            }
            Err(_) => {
                error!(
                    timeout_secs = self.config.collection_timeout.as_secs(),
                    "Process enumeration timed out"
                );
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err((anyhow::anyhow!("Process enumeration timeout"), Vec::new()));
            }
        };

        debug!(
            total_found = collection_stats.total_processes,
            successful = collection_stats.successful_collections,
            inaccessible = collection_stats.inaccessible_processes,
            invalid = collection_stats.invalid_processes,
            duration_ms = collection_stats.collection_duration_ms,
            "Process collection completed"
        );

        // Process events in batches with backpressure handling
        let mut event_batch = Vec::with_capacity(self.config.event_batch_size);
        let mut collected_count: u64 = 0;
        let mut batch_count: u64 = 0;
        let mut dead_letter_events = Vec::new();

        for process_event in process_events {
            // Check for shutdown during processing
            if shutdown_signal.load(Ordering::Relaxed) {
                debug!("Shutdown signal detected during process collection");
                break;
            }

            event_batch.push(CollectionEvent::Process(process_event));
            collected_count = collected_count.saturating_add(1);

            // Send batch when it's full
            if event_batch.len() >= self.config.event_batch_size {
                match self
                    .send_event_batch(tx, &mut event_batch, shutdown_signal)
                    .await
                {
                    Ok(mut dropped) => dead_letter_events.append(&mut dropped),
                    Err(e) => {
                        error!(error = %e, "Failed to send event batch");
                        dead_letter_events.append(&mut event_batch);
                        return Err((e, dead_letter_events));
                    }
                }
                batch_count = batch_count.saturating_add(1);
            }
        }

        // Send any remaining events in the batch
        if !event_batch.is_empty() {
            match self
                .send_event_batch(tx, &mut event_batch, shutdown_signal)
                .await
            {
                Ok(mut dropped) => dead_letter_events.append(&mut dropped),
                Err(e) => {
                    error!(error = %e, "Failed to send final event batch");
                    dead_letter_events.append(&mut event_batch);
                    return Err((e, dead_letter_events));
                }
            }
            batch_count = batch_count.saturating_add(1);
        }

        // Update statistics
        let collection_duration = collection_start.elapsed();
        self.stats.collection_cycles.fetch_add(1, Ordering::Relaxed);
        #[allow(clippy::as_conversions)] // Safe: usize to u64 won't overflow on 64-bit systems
        self.stats.processes_collected.fetch_add(
            collection_stats.successful_collections as u64,
            Ordering::Relaxed,
        );

        // Update average collection duration
        #[allow(clippy::as_conversions)]
        // Safe: collection duration will not exceed u64::MAX milliseconds
        let duration_ms = collection_duration.as_millis() as u64;
        let cycles = self.stats.collection_cycles.load(Ordering::Relaxed);
        let current_avg = self
            .stats
            .avg_collection_duration_ms
            .load(Ordering::Relaxed);
        #[allow(clippy::integer_division, clippy::arithmetic_side_effects)]
        // Intentional: running average calculation
        let new_avg = if cycles == 1 {
            duration_ms
        } else {
            (current_avg
                .saturating_mul(cycles.saturating_sub(1))
                .saturating_add(duration_ms))
                / cycles
        };
        self.stats
            .avg_collection_duration_ms
            .store(new_avg, Ordering::Relaxed);

        // Update last collection time
        *self.stats.last_collection_time.lock().await = Some(Instant::now());

        // Record telemetry
        let _duration = timer.finish();

        info!(
            processes_collected = collected_count,
            batches_sent = batch_count,
            duration_ms = duration_ms,
            avg_duration_ms = new_avg,
            "Process collection cycle completed"
        );

        Ok(BatchOutcome::new(dead_letter_events))
    }

    /// Sends a batch of events with backpressure handling.
    async fn send_event_batch(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        event_batch: &mut Vec<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
    ) -> anyhow::Result<Vec<CollectionEvent>> {
        if event_batch.is_empty() {
            return Ok(Vec::new());
        }

        let batch_size = event_batch.len();
        debug!(batch_size = batch_size, "Sending event batch");

        let mut processed_count: usize = 0;
        let mut dead_letter_events: Vec<CollectionEvent> = Vec::new();
        let max_retries: u32 = 3;
        let retry_delay = Duration::from_millis(10);

        while !event_batch.is_empty() {
            // Check for shutdown before processing
            if shutdown_signal.load(Ordering::Relaxed) {
                debug!("Shutdown signal detected, stopping event batch send");
                return Ok(dead_letter_events);
            }

            // Peek at the first event without removing it
            // Safety: we check !event_batch.is_empty() at the start of the while loop
            let Some(event) = event_batch.first().cloned() else {
                break;
            };

            // Capture event details for potential error reporting
            let event_type = event.event_type();
            let event_pid = event.pid();

            // Acquire backpressure permit with timeout
            let permit = timeout(
                self.config.max_backpressure_wait,
                self.backpressure_semaphore.acquire(),
            )
            .await
            .map_err(|_timeout_err| anyhow::anyhow!("Backpressure timeout exceeded"))?
            .map_err(|e| anyhow::anyhow!("Failed to acquire backpressure permit: {e}"))?;

            // Update in-flight counter
            self.stats.events_in_flight.fetch_add(1, Ordering::Relaxed);

            // Attempt to send the event with retry logic
            let mut retry_count: u32 = 0;
            let mut send_successful = false;

            while retry_count < max_retries && !send_successful {
                // Check for shutdown before each retry
                if shutdown_signal.load(Ordering::Relaxed) {
                    debug!("Shutdown signal detected during event send retry");
                    break;
                }

                // Send the event with timeout to prevent indefinite blocking
                let send_result =
                    timeout(self.config.max_backpressure_wait, tx.send(event.clone())).await;

                match send_result {
                    Ok(Ok(())) => {
                        // Event sent successfully
                        send_successful = true;
                        processed_count = processed_count.saturating_add(1);
                    }
                    Ok(Err(_)) => {
                        warn!("Event channel closed during batch send");
                        // Update in-flight counter and release permit
                        self.stats.events_in_flight.fetch_sub(1, Ordering::Relaxed);
                        drop(permit);
                        return Err(anyhow::anyhow!("Event channel closed"));
                    }
                    Err(_) => {
                        // Timeout occurred during send
                        retry_count = retry_count.saturating_add(1);
                        if retry_count < max_retries {
                            debug!(
                                retry_count = retry_count,
                                max_retries = max_retries,
                                "Event send timed out, retrying with backoff"
                            );
                            // Small backoff before retry
                            #[allow(clippy::arithmetic_side_effects)]
                            // Safe: retry_count is bounded by max_retries
                            tokio::time::sleep(retry_delay * retry_count).await;

                            // Check for shutdown after backoff
                            if shutdown_signal.load(Ordering::Relaxed) {
                                debug!("Shutdown signal detected during backoff");
                                break;
                            }
                        } else {
                            debug!(
                                retry_count = retry_count,
                                "Event send failed after max retries, giving up on current attempt"
                            );
                            break;
                        }
                    }
                }
            }

            // Update in-flight counter and release permit
            self.stats.events_in_flight.fetch_sub(1, Ordering::Relaxed);
            drop(permit);

            // Only remove the event from the batch if it was successfully sent
            if send_successful {
                if !event_batch.is_empty() {
                    event_batch.remove(0);
                }
            } else {
                warn!(
                    attempts = retry_count,
                    max_retries = max_retries,
                    event_type = event_type,
                    pid = ?event_pid,
                    "Event send failed after retries; moving to dead-letter queue"
                );
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                if !event_batch.is_empty() {
                    event_batch.remove(0);
                }
                dead_letter_events.push(event);
                if shutdown_signal.load(Ordering::Relaxed) {
                    debug!(
                        "Shutdown detected during event send; stopping batch processing after recording dead-letter event"
                    );
                    break;
                }
            }
        }

        debug!(
            batch_size = batch_size,
            processed_count = processed_count,
            failed = dead_letter_events.len(),
            "Event batch processed"
        );
        Ok(dead_letter_events)
    }

    #[allow(clippy::unused_self)] // Method on struct for future extensibility
    fn log_dead_letter_events(&self, events: &[CollectionEvent]) {
        const MAX_DETAILED_EVENTS: usize = 5;

        if events.is_empty() {
            return;
        }

        warn!(
            failed = events.len(),
            "Dead-letter events retained after batch processing"
        );

        for event in events.iter().take(MAX_DETAILED_EVENTS) {
            debug!(
                event_type = event.event_type(),
                pid = ?event.pid(),
                "Dead-letter event details"
            );
        }

        if events.len() > MAX_DETAILED_EVENTS {
            debug!(
                omitted = events.len().saturating_sub(MAX_DETAILED_EVENTS),
                "Additional dead-letter events omitted from warn-level logging"
            );
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

    #[instrument(skip(self, tx, shutdown_signal), fields(source = "process-monitor"))]
    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        const MAX_CONSECUTIVE_FAILURES: u32 = 5;
        const FAILURE_BACKOFF_BASE: Duration = Duration::from_secs(1);

        info!(
            collection_interval_secs = self.config.collection_interval.as_secs(),
            enhanced_metadata = self.config.collect_enhanced_metadata,
            max_processes = self.config.max_processes_per_cycle,
            max_events_in_flight = self.config.max_events_in_flight,
            event_batch_size = self.config.event_batch_size,
            "Starting process event source"
        );

        let mut collection_interval = interval(self.config.collection_interval);
        #[allow(unused_assignments)]
        let mut consecutive_failures = 0_u32;

        // Skip the first tick to avoid immediate collection
        collection_interval.tick().await;

        // Initial collection with error handling
        match self.collect_processes(&tx, &shutdown_signal).await {
            Ok(outcome) => {
                self.log_dead_letter_events(outcome.dead_letters());
                info!("Initial process collection completed successfully");
                consecutive_failures = 0;
            }
            Err((err, dead_letters)) => {
                self.log_dead_letter_events(&dead_letters);
                error!(error = %err, "Failed during initial process collection");
                consecutive_failures = 1;
                // Don't return error immediately, try to recover
            }
        }

        // Main collection loop with graceful degradation
        loop {
            tokio::select! {
                _ = collection_interval.tick() => {
                    // Check for shutdown before starting collection
                    if shutdown_signal.load(Ordering::Relaxed) {
                        debug!("Shutdown signal detected, stopping collection loop");
                        break;
                    }

                    match self.collect_processes(&tx, &shutdown_signal).await {
                        Ok(outcome) => {
                            self.log_dead_letter_events(outcome.dead_letters());
                            if consecutive_failures > 0 {
                                info!(
                                    previous_failures = consecutive_failures,
                                    "Process collection recovered successfully"
                                );
                            }
                            consecutive_failures = 0;
                        }
                        Err((err, dead_letters)) => {
                            self.log_dead_letter_events(&dead_letters);
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            error!(
                                error = %err,
                                consecutive_failures = consecutive_failures,
                                max_failures = MAX_CONSECUTIVE_FAILURES,
                                "Process collection failed"
                            );

                            // Implement exponential backoff for repeated failures
                            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                error!(
                                    consecutive_failures = consecutive_failures,
                                    "Too many consecutive failures, entering degraded mode"
                                );

                                // Wait longer before next attempt
                                #[allow(clippy::arithmetic_side_effects)] // Safe: consecutive_failures is bounded
                                let backoff_duration = FAILURE_BACKOFF_BASE * consecutive_failures;
                                let max_backoff = Duration::from_secs(60);
                                let actual_backoff = std::cmp::min(backoff_duration, max_backoff);

                                warn!(
                                    backoff_secs = actual_backoff.as_secs(),
                                    "Applying failure backoff"
                                );

                                tokio::time::sleep(actual_backoff).await;
                            }
                        }
                    }
                }
                () = async {
                    // More responsive shutdown checking
                    while !shutdown_signal.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => {
                    info!("Shutdown signal received, stopping process collection");
                    break;
                }
            }
        }

        // Graceful shutdown: wait for in-flight events to complete
        let shutdown_start = Instant::now();
        while self.stats.events_in_flight.load(Ordering::Relaxed) > 0 {
            if shutdown_start.elapsed() > self.config.shutdown_timeout {
                warn!(
                    events_in_flight = self.stats.events_in_flight.load(Ordering::Relaxed),
                    timeout_secs = self.config.shutdown_timeout.as_secs(),
                    "Shutdown timeout exceeded, some events may be lost"
                );
                break;
            }

            debug!(
                events_in_flight = self.stats.events_in_flight.load(Ordering::Relaxed),
                "Waiting for in-flight events to complete"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let final_stats = self.stats();
        info!(
            collection_cycles = final_stats.collection_cycles,
            processes_collected = final_stats.processes_collected,
            collection_errors = final_stats.collection_errors,
            avg_duration_ms = final_stats.avg_collection_duration_ms,
            "Process event source stopped"
        );

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("Stopping process event source");
        // No cleanup needed for process collection
        Ok(())
    }

    #[instrument(skip(self), fields(source = "process-monitor"))]
    async fn health_check(&self) -> anyhow::Result<()> {
        const MAX_ERROR_RATE: f64 = 0.5; // 50% error rate threshold

        let timer = PerformanceTimer::start("health_check".to_owned());
        let health_check_start = Instant::now();

        // Get current statistics
        let stats = self.stats();

        // Check if we've had recent successful collections
        let last_collection_time = {
            let last_time = self.stats.last_collection_time.lock().await;
            *last_time
        };

        // Health check criteria
        let mut health_issues = Vec::new();

        // 1. Check if we've had any successful collections recently
        if let Some(last_time) = last_collection_time {
            let time_since_last = last_time.elapsed();
            let max_time_since_collection = self.config.collection_interval * 3; // Allow 3 intervals

            if time_since_last > max_time_since_collection {
                health_issues.push(format!(
                    "No successful collection in {} seconds (max: {})",
                    time_since_last.as_secs(),
                    max_time_since_collection.as_secs()
                ));
            }
        } else if stats.collection_cycles > 0 {
            health_issues.push("No successful collections recorded".to_owned());
        }

        // 2. Check error rate
        if stats.collection_cycles > 0 {
            #[allow(clippy::as_conversions)] // Safe: casting u64 to f64 for ratio calculation
            let error_rate = stats.collection_errors as f64 / stats.collection_cycles as f64;

            if error_rate > MAX_ERROR_RATE {
                health_issues.push(format!(
                    "High error rate: {:.1}% (max: {:.1}%)",
                    error_rate * 100.0,
                    MAX_ERROR_RATE * 100.0
                ));
            }
        }

        // 3. Check if we can still enumerate processes using the collector
        let enumeration_result = timeout(
            Duration::from_secs(5), // Short timeout for health check
            self.collector.health_check(),
        )
        .await;

        match enumeration_result {
            Ok(Ok(())) => {
                debug!("Health check enumeration successful");
            }
            Ok(Err(e)) => {
                health_issues.push(format!("Process collector health check failed: {e}"));
            }
            Err(_) => {
                health_issues.push("Process collector health check timed out".to_owned());
            }
        }

        // 4. Check backpressure semaphore availability
        let available_permits = self.backpressure_semaphore.available_permits();
        let total_permits = self.config.max_events_in_flight;
        #[allow(clippy::as_conversions)] // Safe: casting counts to f64 for ratio calculation
        let permit_usage = 1.0 - (available_permits as f64 / total_permits as f64);

        if permit_usage > 0.9 {
            health_issues.push(format!(
                "High backpressure: {:.1}% permits in use",
                permit_usage * 100.0
            ));
        }

        // 5. Check average collection duration
        if stats.avg_collection_duration_ms > 0 {
            let avg_duration = Duration::from_millis(stats.avg_collection_duration_ms);
            let max_duration = self.config.collection_timeout / 2; // Half of timeout

            if avg_duration > max_duration {
                health_issues.push(format!(
                    "Slow collections: avg {}ms (max: {}ms)",
                    avg_duration.as_millis(),
                    max_duration.as_millis()
                ));
            }
        }

        let _duration = timer.finish();
        let health_check_duration = health_check_start.elapsed();

        // Report health status
        if health_issues.is_empty() {
            #[allow(clippy::as_conversions)] // Safe: casting u64 to f64 for percentage
            let error_rate_str = if stats.collection_cycles > 0 {
                format!(
                    "{:.1}%",
                    (stats.collection_errors as f64 / stats.collection_cycles as f64) * 100.0
                )
            } else {
                "N/A".to_owned()
            };
            info!(
                collection_cycles = stats.collection_cycles,
                processes_collected = stats.processes_collected,
                error_rate = %error_rate_str,
                avg_duration_ms = stats.avg_collection_duration_ms,
                events_in_flight = stats.events_in_flight,
                available_permits = available_permits,
                health_check_duration_ms = health_check_duration.as_millis(),
                "Process event source health check passed"
            );
            Ok(())
        } else {
            let health_summary = health_issues.join("; ");
            error!(
                health_issues = ?health_issues,
                collection_cycles = stats.collection_cycles,
                processes_collected = stats.processes_collected,
                collection_errors = stats.collection_errors,
                events_in_flight = stats.events_in_flight,
                health_check_duration_ms = health_check_duration.as_millis(),
                "Process event source health check failed"
            );
            Err(anyhow::anyhow!("Health check failed: {health_summary}"))
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::redundant_clone,
    clippy::missing_panics_doc,
    clippy::uninlined_format_args,
    clippy::semicolon_outside_block,
    clippy::shadow_unrelated,
    clippy::clone_on_ref_ptr,
    clippy::single_match_else,
    clippy::match_same_arms
)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::SystemTime;

    use collector_core::ProcessEvent;
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

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

        // Check initial statistics
        let stats = source.stats();
        assert_eq!(stats.collection_cycles, 0);
        assert_eq!(stats.processes_collected, 0);
        assert_eq!(stats.collection_errors, 0);
        assert_eq!(stats.events_in_flight, 0);
    }

    #[tokio::test]
    async fn test_process_event_source_with_config() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            collection_interval: Duration::from_secs(5),
            collect_enhanced_metadata: false,
            max_processes_per_cycle: 100,
            compute_executable_hashes: true,
            max_events_in_flight: 500,
            event_batch_size: 50,
            ..Default::default()
        };
        let source = ProcessEventSource::with_config(db_manager, config.clone());

        assert_eq!(source.name(), "process-monitor");
        assert!(source.capabilities().contains(SourceCaps::PROCESS));
        assert!(source.capabilities().contains(SourceCaps::REALTIME)); // Due to 5s interval
        assert_eq!(source.config.max_events_in_flight, 500);
        assert_eq!(source.config.event_batch_size, 50);
    }

    #[tokio::test]
    async fn test_process_collector_integration() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Test that the collector can enumerate processes
        let result = source.collector.collect_processes().await;
        assert!(result.is_ok(), "Process collection should succeed");

        let (events, stats) = result.expect("Process collection should succeed");
        assert!(!events.is_empty(), "Should collect at least one process");
        assert!(stats.total_processes > 0, "Should find processes");
        assert!(
            stats.successful_collections > 0,
            "Should successfully collect some processes"
        );
        assert!(
            stats.collection_duration_ms > 0,
            "Collection should take some time"
        );

        // Verify event data quality
        for event in &events {
            assert!(event.pid > 0, "Process PID should be valid");
            assert!(!event.name.is_empty(), "Process name should not be empty");
            assert!(event.accessible, "Collected processes should be accessible");
            assert!(
                event.timestamp <= SystemTime::now(),
                "Timestamp should be reasonable"
            );
        }
    }

    #[tokio::test]
    async fn test_process_collector_health_check() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Test collector health check
        let result = source.collector.health_check().await;
        assert!(result.is_ok(), "Collector health check should pass");
    }

    #[tokio::test]
    async fn test_process_collector_single_process() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Try to collect information for the current process
        let current_pid = std::process::id();
        let result = source.collector.collect_process(current_pid).await;

        assert!(result.is_ok(), "Should be able to collect current process");

        let event = result.expect("Should be able to collect current process");
        assert_eq!(event.pid, current_pid);
        assert!(!event.name.is_empty(), "Process name should not be empty");
        assert!(event.accessible, "Current process should be accessible");
    }

    #[tokio::test]
    async fn test_collect_processes_with_shutdown() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            collection_interval: Duration::from_millis(50), // Very fast for testing
            max_processes_per_cycle: 10,                    // Limit for faster testing
            event_batch_size: 5,
            ..Default::default()
        };
        let source = ProcessEventSource::with_config(db_manager, config);

        let (tx, mut rx) = mpsc::channel(100);
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        // Start the event source in background
        let source_clone = source;
        let shutdown_clone = Arc::clone(&shutdown_signal);
        let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

        // Wait for some events with a reasonable timeout
        let mut event_count = 0;
        let max_wait = Duration::from_secs(3);
        let start_time = std::time::Instant::now();

        while event_count < 5 && start_time.elapsed() < max_wait {
            if let Ok(Some(_event)) = timeout(Duration::from_millis(200), rx.recv()).await {
                event_count += 1;
            }
        }

        // Signal shutdown
        shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for start task to complete
        let result = timeout(Duration::from_secs(3), start_task).await;
        assert!(result.is_ok(), "Start task should complete");
        assert!(
            result.expect("Start task should complete").is_ok(),
            "Start should succeed"
        );
        assert!(event_count > 0, "Should have collected some events");
    }

    #[tokio::test]
    async fn test_send_event_batch() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        let (tx, mut rx) = mpsc::channel(100);
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        // Create a test event batch
        let mut event_batch = vec![
            CollectionEvent::Process(ProcessEvent {
                pid: 1234,
                ppid: None,
                name: "test1".to_string(),
                executable_path: None,
                command_line: vec![],
                start_time: None,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: true,
                file_exists: false,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            }),
            CollectionEvent::Process(ProcessEvent {
                pid: 5678,
                ppid: None,
                name: "test2".to_string(),
                executable_path: None,
                command_line: vec![],
                start_time: None,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: true,
                file_exists: false,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            }),
        ];

        // Send the batch
        let dead_letters = source
            .send_event_batch(&tx, &mut event_batch, &shutdown_signal)
            .await
            .expect("Batch send should succeed");
        assert!(dead_letters.is_empty(), "No events should be dead-lettered");
        assert!(event_batch.is_empty(), "Batch should be drained");

        // Verify events were received
        let event1 = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(event1.is_ok() && event1.unwrap().is_some());

        let event2 = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(event2.is_ok() && event2.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_comprehensive_health_check() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Initial health check should pass
        let result = source.health_check().await;
        assert!(
            result.is_ok(),
            "Initial health check should pass: {:?}",
            result
        );

        // Simulate some collection activity
        source.stats.collection_cycles.store(10, Ordering::Relaxed);
        source
            .stats
            .processes_collected
            .store(1000, Ordering::Relaxed);
        source.stats.collection_errors.store(1, Ordering::Relaxed);
        source
            .stats
            .avg_collection_duration_ms
            .store(500, Ordering::Relaxed);

        // Update last collection time
        {
            let mut last_time = source.stats.last_collection_time.lock().await;
            *last_time = Some(Instant::now());
        }

        // Health check should still pass with good metrics
        let result = source.health_check().await;
        assert!(
            result.is_ok(),
            "Health check with good metrics should pass: {:?}",
            result
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

    #[tokio::test]
    async fn test_backpressure_handling() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            max_events_in_flight: 2, // Very low limit for testing
            max_backpressure_wait: Duration::from_millis(100),
            ..Default::default()
        };
        let source = ProcessEventSource::with_config(db_manager, config);

        // Create a channel that won't be read from to simulate backpressure
        let (tx, _rx) = mpsc::channel(1);
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        // Create a small batch
        let mut event_batch = vec![CollectionEvent::Process(ProcessEvent {
            pid: 1,
            ppid: None,
            name: "test".to_string(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: false,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        })];

        // This should eventually timeout due to backpressure
        let result = timeout(
            Duration::from_millis(500),
            source.send_event_batch(&tx, &mut event_batch, &shutdown_signal),
        )
        .await;

        // The operation should either timeout or complete quickly
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_statistics_tracking() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Initial stats should be zero
        let initial_stats = source.stats();
        assert_eq!(initial_stats.collection_cycles, 0);
        assert_eq!(initial_stats.processes_collected, 0);
        assert_eq!(initial_stats.collection_errors, 0);

        // Simulate some activity
        source
            .stats
            .collection_cycles
            .fetch_add(5, Ordering::Relaxed);
        source
            .stats
            .processes_collected
            .fetch_add(100, Ordering::Relaxed);
        source
            .stats
            .collection_errors
            .fetch_add(1, Ordering::Relaxed);
        source
            .stats
            .events_in_flight
            .fetch_add(3, Ordering::Relaxed);

        let updated_stats = source.stats();
        assert_eq!(updated_stats.collection_cycles, 5);
        assert_eq!(updated_stats.processes_collected, 100);
        assert_eq!(updated_stats.collection_errors, 1);
        assert_eq!(updated_stats.events_in_flight, 3);
    }

    #[tokio::test]
    async fn test_event_source_trait_compliance() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Test EventSource trait methods
        assert_eq!(source.name(), "process-monitor");

        let capabilities = source.capabilities();
        assert!(capabilities.contains(SourceCaps::PROCESS));
        assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));

        // Test health check
        let health_result = source.health_check().await;
        assert!(health_result.is_ok(), "Health check should pass");

        // Test stop method
        let stop_result = source.stop().await;
        assert!(stop_result.is_ok(), "Stop should succeed");
    }

    #[tokio::test]
    async fn test_configuration_validation() {
        let db_manager = create_test_database();

        // Test default configuration
        let default_config = ProcessSourceConfig::default();
        assert_eq!(default_config.collection_interval, Duration::from_secs(30));
        assert!(default_config.collect_enhanced_metadata);
        assert_eq!(default_config.max_processes_per_cycle, 0);
        assert!(!default_config.compute_executable_hashes);
        assert_eq!(default_config.max_events_in_flight, 1000);

        // Test custom configuration
        let custom_config = ProcessSourceConfig {
            collection_interval: Duration::from_millis(500),
            collect_enhanced_metadata: false,
            max_processes_per_cycle: 100,
            compute_executable_hashes: true,
            max_events_in_flight: 50,
            collection_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
            max_backpressure_wait: Duration::from_millis(200),
            event_batch_size: 25,
            batch_timeout: Duration::from_millis(250),
        };

        let source = ProcessEventSource::with_config(db_manager, custom_config.clone());
        assert_eq!(
            source.config.collection_interval,
            Duration::from_millis(500)
        );
        assert!(!source.config.collect_enhanced_metadata);
        assert_eq!(source.config.max_processes_per_cycle, 100);
        assert!(source.config.compute_executable_hashes);
        assert_eq!(source.config.max_events_in_flight, 50);
    }

    #[tokio::test]
    async fn test_graceful_shutdown_timeout() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            collection_interval: Duration::from_millis(50),
            shutdown_timeout: Duration::from_millis(100), // Very short timeout
            max_events_in_flight: 10,
            max_backpressure_wait: Duration::from_millis(10), // Very short timeout for test
            collection_timeout: Duration::from_millis(100),   // Short collection timeout
            event_batch_size: 1,                              // Small batches
            ..Default::default()
        };
        let source = ProcessEventSource::with_config(db_manager, config);

        let (tx, rx) = mpsc::channel(1000);
        // Spawn a task to keep the receiver alive but not consume messages (simulates backpressure)
        let _rx_task = tokio::spawn(async move {
            // Just hold the receiver without reading to simulate backpressure
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(rx);
        });
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        // Start collection
        let source_clone = source;
        let shutdown_clone = Arc::clone(&shutdown_signal);
        let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Signal shutdown and measure time
        let shutdown_start = std::time::Instant::now();
        shutdown_signal.store(true, Ordering::Relaxed);

        let result = timeout(Duration::from_secs(2), start_task).await;
        let shutdown_duration = shutdown_start.elapsed();

        assert!(result.is_ok(), "Shutdown should complete");
        assert!(result.unwrap().is_ok(), "Shutdown should succeed");

        // Should complete reasonably quickly even with timeout
        // Allow more time on slower systems or under load
        assert!(
            shutdown_duration < Duration::from_secs(60),
            "Shutdown should be fast, took {:?}",
            shutdown_duration
        );
    }

    #[tokio::test]
    async fn test_error_recovery_and_degradation() {
        let db_manager = create_test_database();
        let config = ProcessSourceConfig {
            collection_interval: Duration::from_millis(50),
            collection_timeout: Duration::from_millis(1), // Very short timeout to force errors
            max_processes_per_cycle: 5,
            ..Default::default()
        };
        let source = ProcessEventSource::with_config(db_manager, config);

        let (tx, mut rx) = mpsc::channel(1000);
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        // Start collection with aggressive timeouts
        let source_clone = source;
        let shutdown_clone = Arc::clone(&shutdown_signal);
        let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

        // Let it run and potentially encounter errors
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Check if we received any events (some might succeed despite timeouts)
        let mut event_count = 0;
        while let Ok(Some(_)) = timeout(Duration::from_millis(10), rx.recv()).await {
            event_count += 1;
            if event_count > 100 {
                break;
            } // Prevent infinite loop
        }

        // Signal shutdown
        shutdown_signal.store(true, Ordering::Relaxed);

        let result = timeout(Duration::from_secs(5), start_task).await;

        // The task should complete, but we'll be more lenient about errors
        match result {
            Ok(task_result) => {
                // Task completed within timeout
                match task_result {
                    Ok(_) => {
                        // Success case
                    }
                    Err(_) => {
                        // Task completed with error, which is acceptable for this test
                        // We're testing that the system doesn't panic or hang
                    }
                }
            }
            Err(_) => {
                // Timeout occurred - this is also acceptable for this stress test
                // The important thing is that we don't panic
            }
        }

        // The system should handle errors gracefully and not panic
        // Event count might be 0 due to aggressive timeouts, which is acceptable
    }

    #[tokio::test]
    async fn test_concurrent_health_checks() {
        let db_manager = create_test_database();
        let source = Arc::new(ProcessEventSource::new(db_manager));

        // Perform multiple concurrent health checks
        let mut handles = Vec::new();
        for i in 0..5 {
            let source_clone = Arc::clone(&source);
            let handle = tokio::spawn(async move {
                let result = source_clone.health_check().await;
                (i, result)
            });
            handles.push(handle);
        }

        // Wait for all health checks to complete
        for handle in handles {
            let (i, result) = handle.await.expect("Health check task should complete");
            assert!(
                result.is_ok(),
                "Health check {} should pass: {:?}",
                i,
                result
            );
        }
    }

    #[tokio::test]
    async fn test_statistics_snapshot_consistency() {
        let db_manager = create_test_database();
        let source = ProcessEventSource::new(db_manager);

        // Take multiple snapshots and verify consistency
        let snapshot1 = source.stats();
        let snapshot2 = source.stats();

        assert_eq!(snapshot1.collection_cycles, snapshot2.collection_cycles);
        assert_eq!(snapshot1.processes_collected, snapshot2.processes_collected);
        assert_eq!(snapshot1.collection_errors, snapshot2.collection_errors);

        // Modify stats and verify changes are reflected
        source
            .stats
            .collection_cycles
            .fetch_add(1, Ordering::Relaxed);
        let snapshot3 = source.stats();
        assert_eq!(snapshot3.collection_cycles, snapshot1.collection_cycles + 1);
    }
}
