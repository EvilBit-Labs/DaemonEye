//! Collector runtime and management infrastructure.
//!
//! This module provides the main `Collector` struct that manages event sources,
//! handles event aggregation, and provides shared operational infrastructure.

use crate::{
    config::CollectorConfig,
    daemoneye_event_bus::DaemoneyeEventBus,
    event::CollectionEvent,
    event_bus::{EventBus, EventBusConfig, LocalEventBus},
    ipc::CollectorIpcServer,
    performance::{PerformanceConfig, PerformanceMonitor},
    source::{EventSource, SourceCaps},
};
use anyhow::{Context, Result};
use daemoneye_lib::telemetry::{HealthStatus, PerformanceTimer, TelemetryCollector};
use futures::future::try_join_all;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{RwLock, Semaphore, mpsc},
    task::JoinHandle,
    time::{interval, timeout},
};
use tracing::{debug, error, info, instrument, warn};

/// Main collector runtime for managing event sources and processing events.
///
/// The `Collector` provides a unified runtime for multiple event sources,
/// handling registration, lifecycle management, event aggregation, and
/// shared infrastructure services.
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │                    Collector                                │
/// │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐ │
/// │  │ EventSource │  │ EventSource │  │   CollectorRuntime  │ │
/// │  │   (Process) │  │  (Network)  │  │                     │ │
/// │  └─────────────┘  └─────────────┘  │  - Event Aggregation│ │
/// │         │                │         │  - Health Monitoring│ │
/// │         └────────────────┼─────────│  - Graceful Shutdown│ │
/// │                          │         │  - Capability Mgmt  │ │
/// │                          │         └─────────────────────┘ │
/// │                          │                   │             │
/// │                          └───────────────────┘             │
/// └─────────────────────────────────────────────────────────────┘
/// ```
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::{Collector, CollectorConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = CollectorConfig::default();
///     let mut collector = Collector::new(config);
///
///     // Register event sources would go here
///
///     collector.run().await
/// }
/// ```
pub struct Collector {
    config: CollectorConfig,
    sources: Vec<Box<dyn EventSource>>,
}

/// Internal runtime state for the collector.
///
/// This structure manages the operational state of the collector including
/// event processing, health monitoring, graceful shutdown coordination,
/// telemetry collection, and backpressure handling.
pub struct CollectorRuntime {
    config: CollectorConfig,
    event_rx: mpsc::Receiver<CollectionEvent>,
    event_tx: mpsc::Sender<CollectionEvent>,
    source_handles: HashMap<String, JoinHandle<Result<()>>>,
    health_handles: HashMap<String, JoinHandle<Result<()>>>,
    shutdown_signal: Arc<AtomicBool>,
    capabilities: Arc<RwLock<HashMap<String, SourceCaps>>>,
    ipc_server: Option<CollectorIpcServer>,
    telemetry_collector: Arc<RwLock<TelemetryCollector>>,
    event_counter: Arc<AtomicUsize>,
    error_counter: Arc<AtomicUsize>,
    backpressure_semaphore: Arc<Semaphore>,
    event_batch: Vec<CollectionEvent>,
    last_batch_flush: Instant,
    performance_monitor: Arc<PerformanceMonitor>,
    event_bus: Option<Box<dyn EventBus>>,
}

impl Collector {
    /// Creates a new collector with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the collector runtime
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::{Collector, CollectorConfig};
    ///
    /// let config = CollectorConfig::default();
    /// let collector = Collector::new(config);
    /// ```
    pub fn new(config: CollectorConfig) -> Self {
        Self {
            config,
            sources: Vec::new(),
        }
    }

    /// Configures a collector to use DaemoneyeEventBus integration.
    ///
    /// This method only configures the socket path for later initialization.
    /// The EventBus will be initialized when the collector starts running.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the collector runtime
    /// * `socket_path` - Socket path for the embedded broker
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = CollectorConfig::default();
    ///     let collector = Collector::configure_daemoneye_eventbus(config, "/tmp/daemoneye.sock")?;
    ///     Ok(())
    /// }
    /// ```
    pub fn configure_daemoneye_eventbus(
        config: CollectorConfig,
        socket_path: &str,
    ) -> Result<Self> {
        info!(
            socket_path = socket_path,
            "Creating collector with DaemoneyeEventBus"
        );

        let mut collector = Self::new(config);

        // Store the socket path in the config for later use during runtime creation
        // This ensures the EventBus will be properly initialized when the collector runs
        collector.config.daemoneye_socket_path = Some(socket_path.to_string());

        info!("Collector created with DaemoneyeEventBus configuration");
        Ok(collector)
    }

    /// Registers an event source with the collector.
    ///
    /// Event sources must be registered before calling `run()`. The collector
    /// will manage the lifecycle of all registered sources.
    ///
    /// # Arguments
    ///
    /// * `source` - Event source to register
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum number of event sources has been reached
    /// or if a source with the same name is already registered.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig, EventSource, SourceCaps, CollectionEvent};
    /// use async_trait::async_trait;
    /// use std::sync::{Arc, atomic::AtomicBool};
    /// use tokio::sync::mpsc;
    ///
    /// struct MySource;
    ///
    /// #[async_trait]
    /// impl EventSource for MySource {
    ///     fn name(&self) -> &'static str { "my-source" }
    ///     fn capabilities(&self) -> SourceCaps { SourceCaps::PROCESS }
    ///     async fn start(&self, _tx: mpsc::Sender<CollectionEvent>, _shutdown_signal: Arc<AtomicBool>) -> anyhow::Result<()> { Ok(()) }
    ///     async fn stop(&self) -> anyhow::Result<()> { Ok(()) }
    /// }
    ///
    /// let mut collector = Collector::new(CollectorConfig::default());
    /// collector.register(Box::new(MySource))?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn register(&mut self, source: Box<dyn EventSource>) -> anyhow::Result<()> {
        if self.sources.len() >= self.config.max_event_sources {
            anyhow::bail!(
                "Cannot register more than {} event sources",
                self.config.max_event_sources
            );
        }

        let source_name = source.name();

        // Check for duplicate source names to prevent confusion
        if self.sources.iter().any(|s| s.name() == source_name) {
            anyhow::bail!(
                "Event source with name '{}' is already registered",
                source_name
            );
        }

        info!(
            source_name = source_name,
            capabilities = ?source.capabilities(),
            "Registering event source"
        );

        self.sources.push(source);
        Ok(())
    }

    /// Returns the capabilities of all registered event sources.
    ///
    /// This method aggregates the capabilities of all registered sources,
    /// providing a unified view of what the collector can monitor.
    pub fn capabilities(&self) -> SourceCaps {
        self.sources
            .iter()
            .map(|source| source.capabilities())
            .fold(SourceCaps::empty(), |acc, caps| acc | caps)
    }

    /// Returns the number of registered event sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Returns a reference to the performance monitor for external access.
    ///
    /// # Deprecation Notice
    ///
    /// This is a temporary placeholder that currently always returns None.
    /// The performance monitor is only available in the runtime and is not
    /// stored on the Collector struct. This method will be implemented when
    /// the PerformanceMonitor is properly stored on the Collector.
    ///
    /// # Planned Behavior
    ///
    /// When implemented, this method will return a reference to the performance
    /// monitor for external access to metrics and monitoring capabilities.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig};
    ///
    /// let collector = Collector::new(CollectorConfig::default());
    /// if let Some(monitor) = collector.performance_monitor() {
    ///     // Access performance metrics
    ///     let stats = monitor.get_statistics();
    /// }
    /// ```
    #[deprecated(
        note = "temporary placeholder; always returns None until PerformanceMonitor is stored on Collector"
    )]
    pub fn performance_monitor(&self) -> Option<Arc<PerformanceMonitor>> {
        // This would require storing the performance monitor in the Collector struct
        // For now, return None as the monitor is only available in the runtime
        None
    }

    /// Runs the collector with all registered event sources.
    ///
    /// This method starts all registered event sources, begins event processing,
    /// and runs until a shutdown signal is received or an unrecoverable error occurs.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Configuration validation fails
    /// - Event sources fail to start
    /// - Critical runtime errors occur
    ///
    /// # Examples
    ///
    /// Running with the in-process event bus:
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let mut collector = Collector::new(CollectorConfig::default());
    ///     // Register sources...
    ///     collector.run().await
    /// }
    /// ```
    ///
    /// Running with the Daemoneye event bus enabled:
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let mut collector = Collector::configure_daemoneye_eventbus(
    ///         CollectorConfig::default(),
    ///         "/var/run/daemoneye.sock",
    ///     )?;
    ///     // Register sources...
    ///     collector.run().await
    /// }
    /// ```
    pub async fn run(self) -> Result<()> {
        // Validate configuration
        self.config
            .validate()
            .context("Invalid collector configuration")?;

        info!(
            source_count = self.sources.len(),
            capabilities = ?self.capabilities(),
            "Starting collector runtime"
        );

        // Create event channel
        let (event_tx, event_rx) = mpsc::channel(self.config.event_buffer_size);

        // Create runtime
        let mut runtime = CollectorRuntime::new(self.config.clone(), event_tx.clone(), event_rx);

        // Initialize EventBus based on configuration
        let socket_path = runtime.config.daemoneye_socket_path.clone();
        if let Some(socket_path) = socket_path {
            runtime.initialize_daemoneye_eventbus(&socket_path).await?;
        } else {
            runtime.initialize_local_eventbus().await?;
        }

        // Start IPC server
        runtime.start_ipc_server().await?;

        // Start all event sources
        for source in self.sources {
            let source_name = source.name().to_string();
            info!("Starting source: {}", source_name);
            runtime.start_source(source).await?;
            info!("Source started successfully: {}", source_name);
        }

        // Start health monitoring
        runtime.start_health_monitoring().await;

        // Start performance monitoring
        runtime.start_performance_monitoring().await;

        // Start telemetry collection
        if runtime.config.enable_telemetry {
            runtime.start_telemetry_collection().await;
        }

        // Start event processing
        runtime.start_event_processing().await;

        // Run until shutdown
        runtime.run_until_shutdown().await?;

        // Ensure the configured EventBus is shut down gracefully
        runtime.shutdown_eventbus().await?;

        info!("Collector runtime stopped");
        Ok(())
    }
}

impl CollectorRuntime {
    /// Creates a new collector runtime.
    fn new(
        config: CollectorConfig,
        event_tx: mpsc::Sender<CollectionEvent>,
        event_rx: mpsc::Receiver<CollectionEvent>,
    ) -> Self {
        let telemetry_collector = Arc::new(RwLock::new(TelemetryCollector::new(
            config.component_name.clone(),
        )));

        let semaphore_permits = config
            .event_buffer_size
            .saturating_sub(config.backpressure_threshold);
        if semaphore_permits == 0 && config.backpressure_threshold >= config.event_buffer_size {
            warn!(
                backpressure_threshold = config.backpressure_threshold,
                event_buffer_size = config.event_buffer_size,
                "Backpressure threshold >= event buffer size, semaphore permits set to 0"
            );
        }

        let backpressure_semaphore = Arc::new(Semaphore::new(semaphore_permits));

        let max_batch_size = config.max_batch_size;

        // Initialize performance monitor with default configuration
        let performance_config = PerformanceConfig::default();
        let performance_monitor = Arc::new(PerformanceMonitor::new(performance_config));

        Self {
            config,
            event_rx,
            event_tx,
            source_handles: HashMap::new(),
            health_handles: HashMap::new(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            capabilities: Arc::new(RwLock::new(HashMap::new())),
            ipc_server: None,
            telemetry_collector,
            event_counter: Arc::new(AtomicUsize::new(0)),
            error_counter: Arc::new(AtomicUsize::new(0)),
            backpressure_semaphore,
            event_batch: Vec::with_capacity(max_batch_size),
            last_batch_flush: Instant::now(),
            performance_monitor,
            event_bus: None,
        }
    }

    /// Starts an event source and manages its lifecycle.
    async fn start_source(&mut self, source: Box<dyn EventSource>) -> Result<()> {
        let source_name = source.name().to_string();
        let capabilities = source.capabilities();
        let event_tx = self.event_tx.clone();
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let startup_timeout = self.config.startup_timeout;

        info!(source_name = %source_name, "Starting event source");

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            caps.insert(source_name.clone(), capabilities);
        }

        // Start the source with configurable timeout and shutdown signal
        let source_name_clone = source_name.clone();
        let handle = tokio::spawn(async move {
            // Pass the shutdown signal directly to the event source
            let result = timeout(startup_timeout, source.start(event_tx, shutdown_signal)).await;

            match result {
                Ok(Ok(())) => {
                    info!(source_name = %source_name_clone, "Event source started successfully");
                    Ok(())
                }
                Ok(Err(e)) => {
                    error!(source_name = %source_name_clone, error = %e, "Event source failed to start");
                    Err(e)
                }
                Err(_) => {
                    error!(source_name = %source_name_clone, "Event source startup timed out");
                    Err(anyhow::anyhow!("Source startup timeout"))
                }
            }
        });

        self.source_handles.insert(source_name, handle);

        // Update IPC server capabilities
        self.update_ipc_capabilities().await?;

        Ok(())
    }

    /// Starts health monitoring for all event sources.
    async fn start_health_monitoring(&mut self) {
        let interval_duration = self.config.health_check_interval;
        let capabilities = Arc::clone(&self.capabilities);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let telemetry_collector = Arc::clone(&self.telemetry_collector);
        let event_counter = Arc::clone(&self.event_counter);
        let error_counter = Arc::clone(&self.error_counter);

        let handle = tokio::spawn(async move {
            let mut health_interval = interval(interval_duration);

            while !shutdown_signal.load(Ordering::Relaxed) {
                health_interval.tick().await;

                let timer = PerformanceTimer::start("health_check".to_string());

                // Perform health checks on registered sources
                let caps = capabilities.read().await;
                let mut overall_health = HealthStatus::Healthy;

                for (source_name, _caps) in caps.iter() {
                    debug!(source_name = %source_name, "Performing health check");

                    // In a full implementation, this would call source.health_check()
                    // For now, we'll check basic metrics
                    let event_count = event_counter.load(Ordering::Relaxed);
                    let error_count = error_counter.load(Ordering::Relaxed);

                    if error_count > 0 && (error_count as f64 / event_count.max(1) as f64) > 0.1 {
                        overall_health = HealthStatus::Degraded;
                        warn!(
                            source_name = %source_name,
                            error_rate = error_count as f64 / event_count.max(1) as f64,
                            "High error rate detected"
                        );
                    }
                }

                // Update telemetry with health check results
                {
                    let mut telemetry = telemetry_collector.write().await;
                    let duration = timer.finish();
                    telemetry.record_operation(duration);

                    if overall_health != HealthStatus::Healthy {
                        telemetry.record_error();
                    }
                }

                debug!(health_status = ?overall_health, "Health check completed");
            }

            Ok(())
        });

        self.health_handles
            .insert("health_monitor".to_string(), handle);
    }

    /// Starts performance monitoring for comprehensive metrics collection.
    async fn start_performance_monitoring(&mut self) {
        let performance_monitor = Arc::clone(&self.performance_monitor);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let telemetry_collector = Arc::clone(&self.telemetry_collector);
        let collection_interval = Duration::from_secs(10); // Default collection interval

        let handle = tokio::spawn(async move {
            let mut monitoring_interval = interval(collection_interval);

            while !shutdown_signal.load(Ordering::Relaxed) {
                monitoring_interval.tick().await;

                // Collect resource metrics
                let metrics = performance_monitor.collect_resource_metrics().await;

                // Update system resource usage in performance monitor
                let cpu_usage = daemoneye_lib::telemetry::ResourceMonitor::get_cpu_usage();
                let memory_usage = daemoneye_lib::telemetry::ResourceMonitor::get_memory_usage();

                performance_monitor.update_cpu_usage(cpu_usage);
                performance_monitor.update_memory_usage(memory_usage);

                // Log performance metrics periodically
                debug!(
                    throughput_eps = metrics.throughput.events_per_second,
                    cpu_percent = metrics.cpu.current_cpu_percent,
                    memory_mb = metrics.memory.current_memory_bytes / (1024 * 1024),
                    "Performance metrics collected"
                );

                // Update telemetry collector with performance data
                {
                    let mut telemetry = telemetry_collector.write().await;
                    telemetry.add_custom_metric(
                        "events_per_second".to_string(),
                        metrics.throughput.events_per_second,
                    );
                    telemetry.add_custom_metric(
                        "peak_cpu_percent".to_string(),
                        metrics.cpu.peak_cpu_percent,
                    );
                    telemetry.add_custom_metric(
                        "peak_memory_mb".to_string(),
                        metrics.memory.peak_memory_bytes as f64 / (1024.0 * 1024.0),
                    );

                    if let Some(trigger_metrics) = &metrics.trigger_latency {
                        telemetry.add_custom_metric(
                            "avg_trigger_latency_ms".to_string(),
                            trigger_metrics.avg_latency_ms,
                        );
                        telemetry.add_custom_metric(
                            "triggers_per_second".to_string(),
                            trigger_metrics.triggers_per_second,
                        );
                    }
                }

                // Check for performance degradation
                if let Some(degradation) = performance_monitor.check_performance_degradation().await
                {
                    warn!(
                        degradation_count = degradation.degradations.len(),
                        "Performance degradation detected"
                    );

                    for degradation_type in &degradation.degradations {
                        match degradation_type {
                            crate::performance::DegradationType::ThroughputDegradation {
                                current_ratio,
                                threshold,
                            } => {
                                warn!(
                                    current_ratio = current_ratio,
                                    threshold = threshold,
                                    "Throughput degradation detected"
                                );
                            }
                            crate::performance::DegradationType::CpuUsageIncrease {
                                current_ratio,
                                threshold,
                            } => {
                                warn!(
                                    current_ratio = current_ratio,
                                    threshold = threshold,
                                    "CPU usage increase detected"
                                );
                            }
                            crate::performance::DegradationType::MemoryUsageIncrease {
                                current_ratio,
                                threshold,
                            } => {
                                warn!(
                                    current_ratio = current_ratio,
                                    threshold = threshold,
                                    "Memory usage increase detected"
                                );
                            }
                            crate::performance::DegradationType::TriggerLatencyIncrease {
                                current_ratio,
                                threshold,
                            } => {
                                warn!(
                                    current_ratio = current_ratio,
                                    threshold = threshold,
                                    "Trigger latency increase detected"
                                );
                            }
                        }
                    }
                }
            }

            Ok(())
        });

        self.health_handles
            .insert("performance_monitor".to_string(), handle);
    }

    /// Starts telemetry collection for performance monitoring.
    async fn start_telemetry_collection(&mut self) {
        let interval_duration = self.config.telemetry_interval;
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let telemetry_collector = Arc::clone(&self.telemetry_collector);
        let event_counter = Arc::clone(&self.event_counter);
        let error_counter = Arc::clone(&self.error_counter);

        let handle = tokio::spawn(async move {
            let mut telemetry_interval = interval(interval_duration);

            while !shutdown_signal.load(Ordering::Relaxed) {
                telemetry_interval.tick().await;

                let timer = PerformanceTimer::start("telemetry_collection".to_string());

                // Collect system resource usage
                let cpu_usage = daemoneye_lib::telemetry::ResourceMonitor::get_cpu_usage();
                let memory_usage = daemoneye_lib::telemetry::ResourceMonitor::get_memory_usage();

                // Update telemetry collector
                {
                    let mut telemetry = telemetry_collector.write().await;
                    telemetry.update_resource_usage(cpu_usage, memory_usage);

                    // Add custom metrics
                    let event_count = event_counter.load(Ordering::Relaxed);
                    let error_count = error_counter.load(Ordering::Relaxed);

                    telemetry.add_custom_metric("events_processed".to_string(), event_count as f64);
                    telemetry.add_custom_metric("errors_total".to_string(), error_count as f64);

                    if event_count > 0 {
                        let error_rate = error_count as f64 / event_count as f64;
                        telemetry.add_custom_metric("error_rate".to_string(), error_rate);
                    }

                    let duration = timer.finish();
                    telemetry.record_operation(duration);
                }

                debug!(
                    events_processed = event_counter.load(Ordering::Relaxed),
                    errors_total = error_counter.load(Ordering::Relaxed),
                    cpu_usage = cpu_usage,
                    memory_usage = memory_usage,
                    "Telemetry collected"
                );
            }

            Ok(())
        });

        self.health_handles
            .insert("telemetry_collector".to_string(), handle);
    }

    /// Starts event processing loop with batching and backpressure handling.
    #[instrument(skip(self), fields(component = %self.config.component_name))]
    async fn start_event_processing(&mut self) {
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let enable_debug = self.config.enable_debug_logging;
        let max_batch_size = self.config.max_batch_size;
        let batch_timeout = self.config.batch_timeout;
        let event_counter = Arc::clone(&self.event_counter);
        let error_counter = Arc::clone(&self.error_counter);
        let telemetry_collector = Arc::clone(&self.telemetry_collector);
        let backpressure_semaphore = Arc::clone(&self.backpressure_semaphore);
        let max_backpressure_wait = self.config.max_backpressure_wait;
        let performance_monitor = Arc::clone(&self.performance_monitor);

        // Clone event_bus for the spawned task (we need to handle the Option<Box<dyn EventBus>>)
        // Since we can't clone Box<dyn EventBus>, we'll pass a flag to indicate if EventBus is available

        // Move the receiver into the processing task
        let mut event_rx = std::mem::replace(
            &mut self.event_rx,
            mpsc::channel(1).1, // Dummy receiver
        );

        // Move the batch into the processing task
        let mut event_batch = std::mem::take(&mut self.event_batch);
        let mut last_batch_flush = self.last_batch_flush;

        let handle = tokio::spawn(async move {
            let mut batch_flush_interval = interval(batch_timeout);
            let mut total_events = 0u64;
            let mut batches_processed = 0u64;

            info!("Starting event processing with batching and backpressure handling");

            loop {
                tokio::select! {
                    // Handle incoming events
                    event_result = event_rx.recv() => {
                        match event_result {
                            Some(event) => {
                                let timer = PerformanceTimer::start("event_processing".to_string());

                                // Handle backpressure
                                let permit = match timeout(max_backpressure_wait, backpressure_semaphore.acquire()).await {
                                    Ok(Ok(permit)) => permit,
                                    Ok(Err(_)) => {
                                        error!("Failed to acquire backpressure permit");
                                        error_counter.fetch_add(1, Ordering::Relaxed);
                                        continue;
                                    }
                                    Err(_) => {
                                        warn!("Backpressure timeout exceeded, dropping event");
                                        error_counter.fetch_add(1, Ordering::Relaxed);
                                        continue;
                                    }
                                };

                                total_events += 1;
                                event_counter.store(total_events as usize, Ordering::Relaxed);

                                if enable_debug {
                                    debug!(
                                        event_type = event.event_type(),
                                        pid = event.pid(),
                                        timestamp = ?event.timestamp(),
                                        batch_size = event_batch.len(),
                                        "Received collection event"
                                    );
                                }

                                // Add event to batch
                                event_batch.push(event);

                                // Process batch if it's full
                                if event_batch.len() >= max_batch_size {
                                    if let Err(e) = Self::process_event_batch(&mut event_batch, &telemetry_collector, &performance_monitor).await {
                                        error!(error = %e, "Failed to process event batch");
                                        error_counter.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        batches_processed += 1;
                                        last_batch_flush = Instant::now();
                                    }
                                }

                                // Record telemetry
                                {
                                    let mut telemetry = telemetry_collector.write().await;
                                    let duration = timer.finish();
                                    telemetry.record_operation(duration);
                                }

                                // Release backpressure permit
                                drop(permit);

                                // Log milestones
                                if total_events % 1000 == 0 {
                                    info!(
                                        events_processed = total_events,
                                        batches_processed = batches_processed,
                                        current_batch_size = event_batch.len(),
                                        "Event processing milestone"
                                    );
                                }
                            }
                            None => {
                                debug!("Event channel closed, processing remaining events");

                                // Process any remaining events in the batch
                                if !event_batch.is_empty() {
                                    if let Err(e) = Self::process_event_batch(&mut event_batch, &telemetry_collector, &performance_monitor).await {
                                        error!(error = %e, "Failed to process final event batch");
                                    }
                                }

                                break;
                            }
                        }
                    }

                    // Handle batch timeout
                    _ = batch_flush_interval.tick() => {
                        if !event_batch.is_empty() && last_batch_flush.elapsed() >= batch_timeout {
                            debug!(batch_size = event_batch.len(), "Flushing batch due to timeout");

                            if let Err(e) = Self::process_event_batch(&mut event_batch, &telemetry_collector, &performance_monitor).await {
                                error!(error = %e, "Failed to process timed-out event batch");
                                error_counter.fetch_add(1, Ordering::Relaxed);
                            } else {
                                batches_processed += 1;
                                last_batch_flush = Instant::now();
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = async {
                        while !shutdown_signal.load(Ordering::Relaxed) {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    } => {
                        info!("Shutdown signal received, processing remaining events");

                        // Process any remaining events in the batch
                        if !event_batch.is_empty() {
                            if let Err(e) = Self::process_event_batch(&mut event_batch, &telemetry_collector, &performance_monitor).await {
                                error!(error = %e, "Failed to process shutdown event batch");
                            }
                        }

                        break;
                    }
                }
            }

            info!(
                total_events = total_events,
                batches_processed = batches_processed,
                "Event processing stopped"
            );
            Ok(())
        });

        self.health_handles
            .insert("event_processor".to_string(), handle);
    }

    /// Processes a batch of events (legacy method for backward compatibility).
    ///
    /// This method handles the actual processing of collected events,
    /// including any necessary transformations, filtering, or forwarding.
    async fn process_event_batch(
        batch: &mut Vec<CollectionEvent>,
        telemetry_collector: &Arc<RwLock<TelemetryCollector>>,
        performance_monitor: &Arc<PerformanceMonitor>,
    ) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        let timer = PerformanceTimer::start("batch_processing".to_string());
        let batch_size = batch.len();

        debug!(batch_size = batch_size, "Processing event batch");

        // Record events in performance monitor
        for event in batch.iter() {
            performance_monitor.record_event(event);

            // Handle trigger requests for latency tracking
            if let CollectionEvent::TriggerRequest(trigger_request) = event {
                performance_monitor.record_trigger_start(&trigger_request.trigger_id);
                // In a real implementation, trigger completion would be recorded
                // when the trigger actually completes processing
                performance_monitor
                    .record_trigger_completion(&trigger_request.trigger_id, trigger_request);
            }
        }

        // In a full implementation, this would:
        // 1. Transform events as needed
        // 2. Apply any filtering rules
        // 3. Forward events to storage or other components
        // 4. Handle any errors gracefully

        // For now, we'll just simulate processing
        for event in batch.iter() {
            // Simulate event processing
            match event {
                CollectionEvent::Process(_) => {
                    // Process process events
                }
                CollectionEvent::Network(_) => {
                    // Process network events
                }
                CollectionEvent::Filesystem(_) => {
                    // Process filesystem events
                }
                CollectionEvent::Performance(_) => {
                    // Process performance events
                }
                CollectionEvent::TriggerRequest(_) => {
                    // Process trigger requests for analysis collector coordination
                }
            }
        }

        // Clear the batch
        batch.clear();

        // Record telemetry
        {
            let mut telemetry = telemetry_collector.write().await;
            let duration = timer.finish();
            telemetry.record_operation(duration);
            telemetry.add_custom_metric("batch_size".to_string(), batch_size as f64);
        }

        debug!(
            batch_size = batch_size,
            "Event batch processed successfully"
        );
        Ok(())
    }

    /// Runs the runtime until shutdown is signaled.
    async fn run_until_shutdown(&mut self) -> Result<()> {
        // Set up signal handling
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            // Try to set up signal handlers, but don't fail if it doesn't work (e.g., in tests)
            match (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) {
                (Ok(mut sigterm), Ok(mut sigint)) => {
                    tokio::select! {
                        _ = sigterm.recv() => {
                            info!("Received SIGTERM, initiating graceful shutdown");
                        }
                        _ = sigint.recv() => {
                            info!("Received SIGINT, initiating graceful shutdown");
                        }
                    }
                }
                _ => {
                    warn!("Failed to set up signal handlers, running without signal handling");
                    // In test environments or when signal handling fails, run with a timeout
                    // to allow tests to complete properly
                    let start_time = Instant::now();
                    let max_runtime = Duration::from_secs(30); // 30 second max runtime for tests

                    loop {
                        if shutdown_signal.load(Ordering::Relaxed) {
                            break;
                        }

                        // Check if we've exceeded the maximum runtime
                        if start_time.elapsed() > max_runtime {
                            info!("Maximum runtime exceeded, shutting down");
                            break;
                        }

                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }

        #[cfg(windows)]
        {
            use tokio::signal::windows::{ctrl_break, ctrl_c};

            // Try to set up signal handlers, but don't fail if it doesn't work (e.g., in tests)
            match (ctrl_c(), ctrl_break()) {
                (Ok(mut ctrl_c), Ok(mut ctrl_break)) => {
                    tokio::select! {
                        _ = ctrl_c.recv() => {
                            info!("Received Ctrl+C, initiating graceful shutdown");
                        }
                        _ = ctrl_break.recv() => {
                            info!("Received Ctrl+Break, initiating graceful shutdown");
                        }
                    }
                }
                _ => {
                    warn!("Failed to set up signal handlers, running without signal handling");
                    // In test environments or when signal handling fails, run with a timeout
                    // to allow tests to complete properly
                    let start_time = Instant::now();
                    let max_runtime = Duration::from_secs(30); // 30 second max runtime for tests

                    loop {
                        if shutdown_signal.load(Ordering::Relaxed) {
                            break;
                        }

                        // Check if we've exceeded the maximum runtime
                        if start_time.elapsed() > max_runtime {
                            info!("Maximum runtime exceeded, shutting down");
                            break;
                        }

                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            warn!("Signal handling not supported on this platform");
            // On unsupported platforms, just wait for shutdown signal
            loop {
                if shutdown_signal.load(Ordering::Relaxed) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        // Signal shutdown to all tasks
        shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for all tasks to complete with timeout
        self.shutdown_gracefully().await?;

        Ok(())
    }

    /// Performs graceful shutdown of all components.
    async fn shutdown_gracefully(&mut self) -> Result<()> {
        info!("Starting graceful shutdown");

        // Shutdown IPC server first
        if let Some(mut ipc_server) = self.ipc_server.take() {
            debug!("Shutting down IPC server");
            if let Err(e) = ipc_server.shutdown().await {
                error!(error = %e, "Failed to shutdown IPC server gracefully");
            }
        }

        // Collect all handles
        let mut all_handles = Vec::new();

        // Add source handles
        for (name, handle) in self.source_handles.drain() {
            debug!(source_name = %name, "Waiting for source to stop");
            all_handles.push(handle);
        }

        // Add health handles
        for (name, handle) in self.health_handles.drain() {
            debug!(component_name = %name, "Waiting for component to stop");
            all_handles.push(handle);
        }

        // Wait for all handles with timeout
        let shutdown_timeout = self.config.shutdown_timeout;
        match timeout(shutdown_timeout, try_join_all(all_handles)).await {
            Ok(Ok(_results)) => {
                info!("All components shut down cleanly");
            }
            Ok(Err(e)) => {
                error!(error = %e, "Component shutdown error");
            }
            Err(_) => {
                warn!(
                    timeout_secs = shutdown_timeout.as_secs(),
                    "Shutdown timeout exceeded, forcing exit"
                );
            }
        }

        Ok(())
    }

    /// Starts the IPC server for communication with daemoneye-agent.
    async fn start_ipc_server(&mut self) -> Result<()> {
        info!("Starting IPC server for collector-core");

        // Calculate aggregated capabilities
        let aggregated_caps = {
            let caps = self.capabilities.read().await;
            caps.values()
                .fold(SourceCaps::empty(), |acc, &cap| acc | cap)
        };

        // Create shared capabilities for IPC server
        let ipc_capabilities = Arc::new(RwLock::new(aggregated_caps));

        // Create IPC server
        let mut ipc_server =
            CollectorIpcServer::new(self.config.clone(), Arc::clone(&ipc_capabilities))
                .context("Failed to create IPC server")?;

        // Set up task handler that processes detection tasks
        let event_tx = self.event_tx.clone();
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        ipc_server.start(move |task| {
            let _tx = event_tx.clone();
            let shutdown = Arc::clone(&shutdown_signal);

            Box::pin(async move {
                use daemoneye_lib::proto::{DetectionResult, TaskType};

                // Check if we're shutting down
                if shutdown.load(Ordering::Relaxed) {
                    return Ok(DetectionResult::failure(
                        &task.task_id,
                        "Collector is shutting down",
                    ));
                }

                // For now, we'll implement a basic task handler
                // In a full implementation, this would route tasks to appropriate event sources
                match TaskType::try_from(task.task_type) {
                    Ok(TaskType::EnumerateProcesses) => {
                        debug!(task_id = %task.task_id, "Processing enumerate processes task");

                        // This is a placeholder - in the full implementation,
                        // this would trigger process enumeration via event sources
                        Ok(DetectionResult::success(&task.task_id, vec![]))
                    }
                    Ok(task_type) => {
                        warn!(task_id = %task.task_id, ?task_type, "Task type not yet implemented");
                        Ok(DetectionResult::failure(
                            &task.task_id,
                            format!("Task type {:?} not yet implemented", task_type),
                        ))
                    }
                    Err(_) => {
                        error!(task_id = %task.task_id, task_type = task.task_type, "Unknown task type");
                        Ok(DetectionResult::failure(
                            &task.task_id,
                            format!("Unknown task type: {}", task.task_type),
                        ))
                    }
                }
            })
        }).await?;

        self.ipc_server = Some(ipc_server);
        info!("IPC server started successfully");

        Ok(())
    }

    /// Updates IPC server capabilities when event sources change.
    async fn update_ipc_capabilities(&self) -> Result<()> {
        if let Some(ipc_server) = &self.ipc_server {
            let caps = self.capabilities.read().await;
            let aggregated_caps = caps
                .values()
                .fold(SourceCaps::empty(), |acc, &cap| acc | cap);
            ipc_server.update_capabilities(aggregated_caps).await;
        }
        Ok(())
    }

    /// Gets the current health status of the collector runtime.
    ///
    /// This method performs a comprehensive health check of all components
    /// and returns the overall health status.
    pub async fn get_health_status(&self) -> Result<daemoneye_lib::telemetry::HealthCheck> {
        let telemetry = self.telemetry_collector.read().await;
        Ok(telemetry.health_check())
    }

    /// Gets the current telemetry metrics from the collector runtime.
    ///
    /// This method returns performance metrics and operational statistics
    /// for monitoring and debugging purposes.
    pub async fn get_telemetry_metrics(&self) -> Result<daemoneye_lib::telemetry::Metrics> {
        let telemetry = self.telemetry_collector.read().await;
        Ok(telemetry.get_metrics().clone())
    }

    /// Gets runtime statistics for the collector.
    ///
    /// This method returns operational statistics including event counts,
    /// error rates, and performance metrics.
    pub fn get_runtime_stats(&self) -> RuntimeStats {
        RuntimeStats {
            events_processed: self.event_counter.load(Ordering::Relaxed),
            errors_total: self.error_counter.load(Ordering::Relaxed),
            registered_sources: self.source_handles.len(),
            active_components: self.health_handles.len(),
            backpressure_permits_available: self.backpressure_semaphore.available_permits(),
        }
    }

    /// Initialize DaemoneyeEventBus for the collector runtime.
    ///
    /// This method creates and configures a DaemoneyeEventBus instance
    /// for high-performance pub/sub messaging with topic-based routing.
    pub async fn initialize_daemoneye_eventbus(&mut self, socket_path: &str) -> Result<()> {
        info!(
            socket_path = socket_path,
            "Initializing DaemoneyeEventBus for collector runtime"
        );

        let event_bus_config = EventBusConfig {
            max_subscribers: self.config.max_event_sources * 10, // Allow multiple subscriptions per source
            buffer_size: self.config.event_buffer_size,
            enable_statistics: true,
        };

        let daemoneye_event_bus = DaemoneyeEventBus::new(event_bus_config, socket_path)
            .await
            .context("Failed to create DaemoneyeEventBus")?;

        // Start the embedded broker
        daemoneye_event_bus
            .start()
            .await
            .context("Failed to start DaemoneyeEventBus")?;

        self.event_bus = Some(Box::new(daemoneye_event_bus));

        info!("DaemoneyeEventBus initialized successfully");
        Ok(())
    }

    /// Initialize LocalEventBus for the collector runtime.
    ///
    /// This method creates and configures a LocalEventBus instance
    /// for in-process event distribution using crossbeam channels.
    pub async fn initialize_local_eventbus(&mut self) -> Result<()> {
        info!("Initializing LocalEventBus for collector runtime");

        let event_bus_config = EventBusConfig {
            max_subscribers: self.config.max_event_sources * 10,
            buffer_size: self.config.event_buffer_size,
            enable_statistics: true,
        };

        let local_event_bus = LocalEventBus::new(event_bus_config);
        self.event_bus = Some(Box::new(local_event_bus));

        info!("LocalEventBus initialized successfully");
        Ok(())
    }

    /// Get EventBus statistics if available.
    ///
    /// This method returns statistics from the configured EventBus,
    /// providing insights into message throughput and subscriber activity.
    pub async fn get_eventbus_statistics(
        &self,
    ) -> Result<Option<crate::event_bus::EventBusStatistics>> {
        if let Some(event_bus) = &self.event_bus {
            let stats = event_bus.get_statistics().await?;
            Ok(Some(stats))
        } else {
            Ok(None)
        }
    }

    /// Publish an event to the EventBus if configured.
    ///
    /// This method publishes events to the configured EventBus for
    /// distribution to subscribers with topic-based routing.
    pub async fn publish_to_eventbus(
        &self,
        event: CollectionEvent,
        correlation_id: Option<String>,
    ) -> Result<()> {
        if let Some(event_bus) = &self.event_bus {
            let correlation_metadata = match correlation_id {
                Some(id) => crate::event_bus::CorrelationMetadata::new(id),
                None => {
                    crate::event_bus::CorrelationMetadata::new(uuid::Uuid::new_v4().to_string())
                }
            };
            event_bus.publish(event, correlation_metadata).await?;
        } else {
            debug!("No EventBus configured, skipping event publication");
        }
        Ok(())
    }

    /// Shutdown the EventBus if configured.
    ///
    /// This method performs a graceful shutdown of the EventBus,
    /// ensuring all pending messages are processed.
    pub async fn shutdown_eventbus(&mut self) -> Result<()> {
        if let Some(event_bus) = self.event_bus.take() {
            info!("Shutting down EventBus");

            // Call shutdown on the EventBus trait
            event_bus.shutdown().await?;

            info!("EventBus shutdown completed");
        }
        Ok(())
    }
}

/// Runtime statistics for the collector.
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    /// Total number of events processed
    pub events_processed: usize,
    /// Total number of errors encountered
    pub errors_total: usize,
    /// Number of registered event sources
    pub registered_sources: usize,
    /// Number of active runtime components
    pub active_components: usize,
    /// Available backpressure permits
    pub backpressure_permits_available: usize,
}

impl RuntimeStats {
    /// Calculates the error rate as a percentage.
    pub fn error_rate(&self) -> f64 {
        if self.events_processed == 0 {
            0.0
        } else {
            (self.errors_total as f64 / self.events_processed as f64) * 100.0
        }
    }

    /// Checks if the runtime is experiencing backpressure.
    pub fn is_under_backpressure(&self) -> bool {
        self.backpressure_permits_available == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CollectionEvent, EventSource, SourceCaps};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    struct TestEventSource {
        name: &'static str,
        capabilities: SourceCaps,
        event_count: Arc<AtomicUsize>,
    }

    impl TestEventSource {
        fn new(name: &'static str, capabilities: SourceCaps) -> Self {
            Self {
                name,
                capabilities,
                event_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl EventSource for TestEventSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn capabilities(&self) -> SourceCaps {
            self.capabilities
        }

        async fn start(
            &self,
            _tx: mpsc::Sender<CollectionEvent>,
            _shutdown_signal: Arc<AtomicBool>,
        ) -> Result<()> {
            // Simulate some work
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.event_count.store(1, Ordering::Relaxed);
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_collector_creation() {
        let config = CollectorConfig::default();
        let collector = Collector::new(config);

        assert_eq!(collector.source_count(), 0);
        assert_eq!(collector.capabilities(), SourceCaps::empty());
    }

    #[tokio::test]
    async fn test_source_registration() {
        let mut collector = Collector::new(CollectorConfig::default());

        let source1 = TestEventSource::new("test1", SourceCaps::PROCESS);
        let source2 = TestEventSource::new("test2", SourceCaps::NETWORK);

        collector
            .register(Box::new(source1))
            .expect("Failed to register source1");
        collector
            .register(Box::new(source2))
            .expect("Failed to register source2");

        assert_eq!(collector.source_count(), 2);
        assert!(collector.capabilities().contains(SourceCaps::PROCESS));
        assert!(collector.capabilities().contains(SourceCaps::NETWORK));
    }

    #[tokio::test]
    async fn test_max_sources_limit() {
        let config = CollectorConfig::default().with_max_event_sources(1);
        let mut collector = Collector::new(config);

        let source1 = TestEventSource::new("test1", SourceCaps::PROCESS);
        let source2 = TestEventSource::new("test2", SourceCaps::NETWORK);

        collector
            .register(Box::new(source1))
            .expect("Failed to register first source");
        let result = collector.register(Box::new(source2));

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cannot register more than")
        );
    }

    #[tokio::test]
    async fn test_duplicate_source_names() {
        let mut collector = Collector::new(CollectorConfig::default());

        let source1 = TestEventSource::new("duplicate", SourceCaps::PROCESS);
        let source2 = TestEventSource::new("duplicate", SourceCaps::NETWORK);

        collector
            .register(Box::new(source1))
            .expect("Failed to register first source");
        let result = collector.register(Box::new(source2));

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already registered")
        );
    }

    #[tokio::test]
    async fn test_config_validation() {
        let mut config = CollectorConfig::default();
        assert!(config.validate().is_ok());

        config.max_event_sources = 0;
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_collector_runtime_creation() {
        let config = CollectorConfig::default();
        let (tx, rx) = mpsc::channel(100);

        let runtime = CollectorRuntime::new(config, tx, rx);
        assert_eq!(runtime.source_handles.len(), 0);
        assert_eq!(runtime.health_handles.len(), 0);
        assert_eq!(runtime.event_counter.load(Ordering::Relaxed), 0);
        assert_eq!(runtime.error_counter.load(Ordering::Relaxed), 0);
        assert!(runtime.backpressure_semaphore.available_permits() > 0);
    }

    #[tokio::test]
    async fn test_runtime_stats() {
        let config = CollectorConfig::default();
        let (tx, rx) = mpsc::channel(100);
        let runtime = CollectorRuntime::new(config, tx, rx);

        let stats = runtime.get_runtime_stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.errors_total, 0);
        assert_eq!(stats.error_rate(), 0.0);
        assert!(!stats.is_under_backpressure());
    }

    #[tokio::test]
    async fn test_event_batch_processing() {
        use crate::event::{CollectionEvent, ProcessEvent};
        use std::time::SystemTime;

        let telemetry_collector =
            Arc::new(RwLock::new(TelemetryCollector::new("test".to_string())));

        let mut batch = vec![CollectionEvent::Process(ProcessEvent {
            pid: 1234,
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
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        })];

        // Create a performance monitor for the test
        let performance_config = PerformanceConfig::default();
        let performance_monitor = Arc::new(PerformanceMonitor::new(performance_config));

        let result = CollectorRuntime::process_event_batch(
            &mut batch,
            &telemetry_collector,
            &performance_monitor,
        )
        .await;
        assert!(result.is_ok());
        assert!(batch.is_empty()); // Batch should be cleared after processing
    }

    #[tokio::test]
    async fn test_telemetry_integration() {
        let config = CollectorConfig::default().with_telemetry(true);
        let (tx, rx) = mpsc::channel(100);
        let runtime = CollectorRuntime::new(config, tx, rx);

        // Test health status
        let health = runtime.get_health_status().await;
        assert!(health.is_ok());

        // Test telemetry metrics
        let metrics = runtime.get_telemetry_metrics().await;
        assert!(metrics.is_ok());
    }
}
