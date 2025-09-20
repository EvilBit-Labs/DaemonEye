//! Collector runtime and management infrastructure.
//!
//! This module provides the main `Collector` struct that manages event sources,
//! handles event aggregation, and provides shared operational infrastructure.

use crate::{
    config::CollectorConfig,
    event::CollectionEvent,
    source::{EventSource, SourceCaps},
};
use anyhow::{Context, Result};
use futures::future::try_join_all;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{RwLock, mpsc},
    task::JoinHandle,
    time::{interval, timeout},
};
use tracing::{debug, error, info, warn};

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
/// event processing, health monitoring, and graceful shutdown coordination.
pub struct CollectorRuntime {
    config: CollectorConfig,
    event_rx: mpsc::Receiver<CollectionEvent>,
    event_tx: mpsc::Sender<CollectionEvent>,
    source_handles: HashMap<String, JoinHandle<Result<()>>>,
    health_handles: HashMap<String, JoinHandle<Result<()>>>,
    shutdown_signal: Arc<AtomicBool>,
    capabilities: Arc<RwLock<HashMap<String, SourceCaps>>>,
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

    /// Registers an event source with the collector.
    ///
    /// Event sources must be registered before calling `run()`. The collector
    /// will manage the lifecycle of all registered sources.
    ///
    /// # Arguments
    ///
    /// * `source` - Event source to register
    ///
    /// # Panics
    ///
    /// Panics if the maximum number of event sources has been reached.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{Collector, CollectorConfig, EventSource, SourceCaps, CollectionEvent};
    /// use async_trait::async_trait;
    /// use tokio::sync::mpsc;
    ///
    /// struct MySource;
    ///
    /// #[async_trait]
    /// impl EventSource for MySource {
    ///     fn name(&self) -> &'static str { "my-source" }
    ///     fn capabilities(&self) -> SourceCaps { SourceCaps::PROCESS }
    ///     async fn start(&self, _tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> { Ok(()) }
    ///     async fn stop(&self) -> anyhow::Result<()> { Ok(()) }
    /// }
    ///
    /// let mut collector = Collector::new(CollectorConfig::default());
    /// collector.register(Box::new(MySource));
    /// ```
    pub fn register(&mut self, source: Box<dyn EventSource>) {
        if self.sources.len() >= self.config.max_event_sources {
            panic!(
                "Cannot register more than {} event sources",
                self.config.max_event_sources
            );
        }

        info!(
            source_name = source.name(),
            capabilities = ?source.capabilities(),
            "Registering event source"
        );

        self.sources.push(source);
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

        // Start all event sources
        for source in self.sources {
            runtime.start_source(source).await?;
        }

        // Start health monitoring
        runtime.start_health_monitoring().await;

        // Start event processing
        runtime.start_event_processing().await;

        // Run until shutdown
        runtime.run_until_shutdown().await?;

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
        Self {
            config,
            event_rx,
            event_tx,
            source_handles: HashMap::new(),
            health_handles: HashMap::new(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            capabilities: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Starts an event source and manages its lifecycle.
    async fn start_source(&mut self, source: Box<dyn EventSource>) -> Result<()> {
        let source_name = source.name().to_string();
        let capabilities = source.capabilities();
        let event_tx = self.event_tx.clone();
        let _shutdown_signal = Arc::clone(&self.shutdown_signal);

        debug!(source_name = %source_name, "Starting event source");

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            caps.insert(source_name.clone(), capabilities);
        }

        // Start the source with timeout
        let source_name_clone = source_name.clone();
        let handle = tokio::spawn(async move {
            let result = timeout(Duration::from_secs(10), source.start(event_tx)).await;

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
                    anyhow::bail!("Source startup timeout")
                }
            }
        });

        self.source_handles.insert(source_name, handle);
        Ok(())
    }

    /// Starts health monitoring for all event sources.
    async fn start_health_monitoring(&mut self) {
        let interval_duration = self.config.health_check_interval;
        let capabilities = Arc::clone(&self.capabilities);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let handle = tokio::spawn(async move {
            let mut health_interval = interval(interval_duration);

            while !shutdown_signal.load(Ordering::Relaxed) {
                health_interval.tick().await;

                let caps = capabilities.read().await;
                for (source_name, _caps) in caps.iter() {
                    debug!(source_name = %source_name, "Performing health check");
                    // Health check implementation would go here
                }
            }

            Ok(())
        });

        self.health_handles
            .insert("health_monitor".to_string(), handle);
    }

    /// Starts event processing loop.
    async fn start_event_processing(&mut self) {
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let enable_debug = self.config.enable_debug_logging;

        // Move the receiver into the processing task
        let mut event_rx = std::mem::replace(
            &mut self.event_rx,
            mpsc::channel(1).1, // Dummy receiver
        );

        let handle = tokio::spawn(async move {
            let mut event_count = 0u64;

            while !shutdown_signal.load(Ordering::Relaxed) {
                match event_rx.recv().await {
                    Some(event) => {
                        event_count += 1;

                        if enable_debug {
                            debug!(
                                event_type = event.event_type(),
                                pid = event.pid(),
                                timestamp = ?event.timestamp(),
                                "Received collection event"
                            );
                        }

                        // Event processing logic would go here
                        // For now, we just count events

                        if event_count % 1000 == 0 {
                            info!(events_processed = event_count, "Event processing milestone");
                        }
                    }
                    None => {
                        debug!("Event channel closed, stopping event processing");
                        break;
                    }
                }
            }

            info!(total_events = event_count, "Event processing stopped");
            Ok(())
        });

        self.health_handles
            .insert("event_processor".to_string(), handle);
    }

    /// Runs the runtime until shutdown is signaled.
    async fn run_until_shutdown(&mut self) -> Result<()> {
        // Set up signal handling
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = signal(SignalKind::terminate())?;
            let mut sigint = signal(SignalKind::interrupt())?;

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, initiating graceful shutdown");
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT, initiating graceful shutdown");
                }
            }
        }

        #[cfg(windows)]
        {
            use tokio::signal::windows::{ctrl_break, ctrl_c};
            let mut ctrl_c = ctrl_c()?;
            let mut ctrl_break = ctrl_break()?;

            tokio::select! {
                _ = ctrl_c.recv() => {
                    info!("Received Ctrl+C, initiating graceful shutdown");
                }
                _ = ctrl_break.recv() => {
                    info!("Received Ctrl+Break, initiating graceful shutdown");
                }
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

        async fn start(&self, _tx: mpsc::Sender<CollectionEvent>) -> Result<()> {
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

        collector.register(Box::new(source1));
        collector.register(Box::new(source2));

        assert_eq!(collector.source_count(), 2);
        assert!(collector.capabilities().contains(SourceCaps::PROCESS));
        assert!(collector.capabilities().contains(SourceCaps::NETWORK));
    }

    #[tokio::test]
    #[should_panic(expected = "Cannot register more than")]
    async fn test_max_sources_limit() {
        let config = CollectorConfig::default().with_max_event_sources(1);
        let mut collector = Collector::new(config);

        let source1 = TestEventSource::new("test1", SourceCaps::PROCESS);
        let source2 = TestEventSource::new("test2", SourceCaps::NETWORK);

        collector.register(Box::new(source1));
        collector.register(Box::new(source2)); // Should panic
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
    }
}
