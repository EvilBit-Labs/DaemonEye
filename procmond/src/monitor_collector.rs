//! Procmond-specific Monitor Collector implementation.
//!
//! This module provides a concrete implementation of the Monitor Collector framework
//! specifically for procmond, integrating process lifecycle tracking with the
//! collector-core EventSource trait.

use crate::{
    lifecycle::{LifecycleTrackingConfig, ProcessLifecycleTracker},
    process_collector::{ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector},
};
use anyhow::Context;
use async_trait::async_trait;
use collector_core::{
    AnalysisChainCoordinator, CollectionEvent, EventBus, EventSource, LocalEventBus,
    MonitorCollector as MonitorCollectorTrait, MonitorCollectorConfig, MonitorCollectorStats,
    MonitorCollectorStatsSnapshot, SourceCaps, TriggerManager,
};
use daemoneye_lib::{storage, telemetry::PerformanceTimer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, RwLock, Semaphore, mpsc},
    task::JoinHandle,
    time::{interval, timeout},
};
use tracing::{debug, error, info, instrument, warn};

/// Procmond-specific Monitor Collector configuration.
///
/// This extends the base MonitorCollectorConfig with procmond-specific
/// configuration for process collection and lifecycle tracking.
#[derive(Debug, Clone, Default)]
pub struct ProcmondMonitorConfig {
    /// Base monitor collector configuration
    pub base_config: MonitorCollectorConfig,
    /// Process collection configuration
    pub process_config: ProcessCollectionConfig,
    /// Lifecycle tracking configuration
    pub lifecycle_config: LifecycleTrackingConfig,
}

impl ProcmondMonitorConfig {
    /// Validates the configuration parameters.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.base_config.validate()
    }
}

/// Procmond Monitor Collector implementation.
///
/// This collector integrates process lifecycle tracking with the collector-core
/// framework, providing event-driven process monitoring capabilities.
#[allow(dead_code)]
pub struct ProcmondMonitorCollector {
    /// Configuration
    config: ProcmondMonitorConfig,
    /// Database manager for audit logging
    database: Arc<Mutex<storage::DatabaseManager>>,
    /// Process collector implementation
    process_collector: Box<dyn ProcessCollector>,
    /// Process lifecycle tracker
    lifecycle_tracker: Arc<Mutex<ProcessLifecycleTracker>>,
    /// Trigger manager for analysis coordination
    trigger_manager: Arc<TriggerManager>,
    /// Analysis chain coordinator
    analysis_coordinator: Arc<AnalysisChainCoordinator>,
    /// Event bus for inter-collector communication
    event_bus: Arc<RwLock<Option<Arc<dyn EventBus + Send + Sync>>>>,
    /// Runtime statistics
    stats: Arc<MonitorCollectorStats>,
    /// Backpressure semaphore
    backpressure_semaphore: Arc<Semaphore>,
    /// Background task handles
    task_handles: Arc<Mutex<Vec<JoinHandle<anyhow::Result<()>>>>>,
    /// Shutdown coordination
    shutdown_signal: Arc<AtomicBool>,
}

impl ProcmondMonitorCollector {
    /// Creates a new Procmond Monitor Collector.
    pub async fn new(
        database: Arc<Mutex<storage::DatabaseManager>>,
        config: ProcmondMonitorConfig,
    ) -> anyhow::Result<Self> {
        // Validate configuration first
        config
            .validate()
            .with_context(|| "Procmond Monitor Collector configuration validation failed")?;

        info!(
            collection_interval_secs = config.base_config.collection_interval.as_secs(),
            max_events_in_flight = config.base_config.max_events_in_flight,
            event_driven = config.base_config.enable_event_driven,
            "Creating Procmond Monitor Collector"
        );

        // Create process collector
        let process_collector =
            Box::new(SysinfoProcessCollector::new(config.process_config.clone()));

        // Create lifecycle tracker
        let lifecycle_tracker = Arc::new(Mutex::new(ProcessLifecycleTracker::new(
            config.lifecycle_config.clone(),
        )));

        // Create trigger manager
        let trigger_manager = Arc::new(TriggerManager::new(
            config.base_config.trigger_config.clone(),
        ));

        // Create analysis chain coordinator
        let analysis_coordinator = Arc::new(AnalysisChainCoordinator::new(
            config.base_config.analysis_config.clone(),
        ));

        // Create event bus if event-driven architecture is enabled
        let event_bus = if config.base_config.enable_event_driven {
            let bus_config = collector_core::EventBusConfig::default();
            let local_bus = LocalEventBus::new(bus_config)
                .await
                .with_context(|| "Failed to create event bus for Procmond Monitor Collector")?;
            Arc::new(RwLock::new(Some(
                Arc::new(local_bus) as Arc<dyn EventBus + Send + Sync>
            )))
        } else {
            Arc::new(RwLock::new(None))
        };

        // Create backpressure semaphore with validated capacity
        let backpressure_semaphore =
            Arc::new(Semaphore::new(config.base_config.max_events_in_flight));

        Ok(Self {
            config,
            database,
            process_collector,
            lifecycle_tracker,
            trigger_manager,
            analysis_coordinator,
            event_bus,
            stats: Arc::new(MonitorCollectorStats::default()),
            backpressure_semaphore,
            task_handles: Arc::new(Mutex::new(Vec::new())),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Performs process collection and lifecycle analysis.
    #[instrument(
        skip(self, tx, shutdown_signal),
        fields(source = "procmond-monitor-collector")
    )]
    async fn collect_and_analyze(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        shutdown_signal: &Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        let timer = PerformanceTimer::start("procmond_monitor_collection".to_string());
        let collection_start = Instant::now();

        // Check for shutdown before starting
        if shutdown_signal.load(Ordering::Relaxed) {
            debug!("Shutdown signal detected, skipping collection");
            return Ok(());
        }

        // Collect process data with timeout
        let collection_result = timeout(
            Duration::from_secs(30),
            self.process_collector.collect_processes(),
        )
        .await;

        let (process_events, _collection_stats) = match collection_result {
            Ok(Ok((events, stats))) => (events, stats),
            Ok(Err(e)) => {
                error!(error = %e, "Process collection failed");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(e).with_context(|| "Process collection failed");
            }
            Err(_) => {
                error!("Process collection timed out");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(anyhow::anyhow!("Process collection timeout"));
            }
        };

        // Perform lifecycle analysis
        let lifecycle_events = {
            let mut tracker = self.lifecycle_tracker.lock().await;
            match tracker.update_and_detect_changes(process_events.clone()) {
                Ok(events) => events,
                Err(e) => {
                    error!(error = %e, "Lifecycle tracking failed");
                    self.stats.analysis_errors.fetch_add(1, Ordering::Relaxed);
                    Vec::new()
                }
            }
        };

        // Update statistics
        self.stats.collection_cycles.fetch_add(1, Ordering::Relaxed);
        self.stats
            .lifecycle_events
            .fetch_add(lifecycle_events.len() as u64, Ordering::Relaxed);

        // Send process events with backpressure handling
        for process_event in process_events {
            if shutdown_signal.load(Ordering::Relaxed) {
                debug!("Shutdown signal detected during event emission");
                break;
            }

            if let Err(e) = self
                .send_event_with_backpressure(
                    tx,
                    CollectionEvent::Process(process_event),
                    shutdown_signal,
                )
                .await
            {
                error!(error = %e, "Failed to send process event");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(e).with_context(|| "Failed to send process event with backpressure");
            }
        }

        let _duration = timer.finish();
        let collection_duration = collection_start.elapsed();

        debug!(
            lifecycle_events = lifecycle_events.len(),
            collection_duration_ms = collection_duration.as_millis(),
            "Procmond monitor collection cycle completed"
        );

        Ok(())
    }

    /// Sends an event with backpressure handling.
    async fn send_event_with_backpressure(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        event: CollectionEvent,
        shutdown_signal: &Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        // Acquire backpressure permit with timeout
        let permit = timeout(
            Duration::from_secs(5),
            self.backpressure_semaphore.acquire(),
        )
        .await
        .with_context(|| "Backpressure timeout while acquiring permit")?
        .with_context(|| "Failed to acquire backpressure permit")?;

        // Update in-flight counter
        self.stats.events_in_flight.fetch_add(1, Ordering::Relaxed);

        // Send event with timeout
        let send_result = timeout(Duration::from_secs(5), tx.send(event)).await;

        // Update in-flight counter and release permit
        self.stats.events_in_flight.fetch_sub(1, Ordering::Relaxed);
        drop(permit);

        match send_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => {
                if !shutdown_signal.load(Ordering::Relaxed) {
                    warn!("Event channel closed during send");
                }
                Err(anyhow::anyhow!("Event channel closed"))
            }
            Err(_) => {
                warn!("Event send timed out");
                Err(anyhow::anyhow!("Event send timeout"))
            }
        }
    }
}

#[async_trait]
impl EventSource for ProcmondMonitorCollector {
    fn name(&self) -> &'static str {
        "procmond-monitor-collector"
    }

    fn capabilities(&self) -> SourceCaps {
        let mut caps = SourceCaps::PROCESS | SourceCaps::SYSTEM_WIDE;

        // Add real-time capability if collecting frequently
        if self.config.base_config.collection_interval <= Duration::from_secs(10) {
            caps |= SourceCaps::REALTIME;
        }

        caps
    }

    #[instrument(skip(self, tx), fields(source = "procmond-monitor-collector"))]
    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        _shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        info!(
            collection_interval_secs = self.config.base_config.collection_interval.as_secs(),
            max_events_in_flight = self.config.base_config.max_events_in_flight,
            event_driven = self.config.base_config.enable_event_driven,
            "Starting Procmond Monitor Collector"
        );

        // Main collection loop
        let mut collection_interval = interval(self.config.base_config.collection_interval);
        let mut consecutive_failures = 0u32;
        const MAX_CONSECUTIVE_FAILURES: u32 = 5;

        // Skip first tick to avoid immediate collection
        collection_interval.tick().await;

        loop {
            tokio::select! {
                _ = collection_interval.tick() => {
                    // Check for shutdown
                    if self.shutdown_signal.load(Ordering::Relaxed) {
                        info!("Shutdown signal received, stopping Procmond Monitor Collector");
                        break;
                    }

                    // Perform collection and analysis
                    match self.collect_and_analyze(&tx, &self.shutdown_signal).await {
                        Ok(()) => {
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            error!(error = %e, "Procmond monitor collection failed");
                            consecutive_failures += 1;

                            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                error!(
                                    consecutive_failures = consecutive_failures,
                                    "Too many consecutive failures, stopping Procmond Monitor Collector"
                                );
                                return Err(anyhow::anyhow!(
                                    "Procmond Monitor Collector failed {} consecutive times",
                                    consecutive_failures
                                ));
                            }

                            // Exponential backoff for failures
                            let backoff_duration = Duration::from_secs(2_u64.pow(consecutive_failures.min(6)));
                            warn!(
                                backoff_seconds = backoff_duration.as_secs(),
                                "Applying backoff after collection failure"
                            );
                            tokio::time::sleep(backoff_duration).await;
                        }
                    }
                }

                _ = async {
                    while !self.shutdown_signal.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                } => {
                    info!("Shutdown signal received in monitoring loop");
                    break;
                }
            }
        }

        info!("Procmond Monitor Collector stopped successfully");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("Stopping Procmond Monitor Collector");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        // Use the monitor collector trait health check
        self.monitor_health_check().await
    }
}

impl MonitorCollectorTrait for ProcmondMonitorCollector {
    fn stats(&self) -> MonitorCollectorStatsSnapshot {
        self.stats.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemoneye_lib::storage::DatabaseManager;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn create_test_database() -> Arc<Mutex<DatabaseManager>> {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = DatabaseManager::new(db_path.to_str().unwrap())
            .expect("Failed to create database manager");
        Arc::new(Mutex::new(db_manager))
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_creation() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let collector = ProcmondMonitorCollector::new(db_manager, config).await;
        assert!(collector.is_ok());

        let collector = collector.unwrap();
        assert_eq!(collector.name(), "procmond-monitor-collector");

        let caps = collector.capabilities();
        assert!(caps.contains(SourceCaps::PROCESS));
        assert!(caps.contains(SourceCaps::SYSTEM_WIDE));
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_capabilities() {
        let db_manager = create_test_database().await;

        // Test real-time capability with fast collection interval
        let fast_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_secs(5),
                ..Default::default()
            },
            ..Default::default()
        };

        let collector = ProcmondMonitorCollector::new(db_manager.clone(), fast_config)
            .await
            .unwrap();
        let caps = collector.capabilities();
        assert!(caps.contains(SourceCaps::REALTIME));

        // Test without real-time capability with slow collection interval
        let slow_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_secs(60),
                ..Default::default()
            },
            ..Default::default()
        };

        let collector = ProcmondMonitorCollector::new(db_manager, slow_config)
            .await
            .unwrap();
        let caps = collector.capabilities();
        assert!(!caps.contains(SourceCaps::REALTIME));
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_health_check() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let collector = ProcmondMonitorCollector::new(db_manager, config)
            .await
            .unwrap();

        // Initial health check should pass
        let health_result = collector.health_check().await;
        assert!(health_result.is_ok());
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_statistics() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let collector = ProcmondMonitorCollector::new(db_manager, config)
            .await
            .unwrap();

        // Initial statistics should be zero
        let stats = collector.stats();
        assert_eq!(stats.collection_cycles, 0);
        assert_eq!(stats.lifecycle_events, 0);
        assert_eq!(stats.trigger_requests, 0);
        assert_eq!(stats.analysis_workflows, 0);
        assert_eq!(stats.events_in_flight, 0);
        assert_eq!(stats.collection_errors, 0);
        assert_eq!(stats.trigger_errors, 0);
        assert_eq!(stats.analysis_errors, 0);
    }

    #[test]
    fn test_config_validation() {
        // Test valid configuration
        let valid_config = ProcmondMonitorConfig::default();
        assert!(valid_config.validate().is_ok());

        // Test invalid collection interval
        let invalid_interval_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_millis(500),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(invalid_interval_config.validate().is_err());
    }
}
