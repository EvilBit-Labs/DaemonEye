//! DaemonEye EventBus integration for collector-core framework.
//!
//! This module provides integration between the collector-core framework and the
//! daemoneye-eventbus message broker, enabling high-performance pub/sub messaging
//! with topic-based routing and embedded broker functionality.

use crate::{
    event::CollectionEvent,
    event_bus::{BusEvent, EventBus, EventBusConfig, EventBusStatistics, EventSubscription},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use daemoneye_eventbus::{
    DaemoneyeBroker, DaemoneyeEventBus as DaemoneyeEventBusImpl,
    EventBus as DaemoneyeEventBusTrait, EventBusStatistics as DaemoneyeEventBusStatistics,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info};
use uuid::Uuid;

/// DaemonEye EventBus implementation for collector-core compatibility.
///
/// This struct wraps the daemoneye-eventbus broker functionality and implements
/// the collector-core EventBus trait for seamless integration with existing
/// collector-core components.
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │                DaemoneyeEventBus (collector-core)               │
/// ├─────────────────────────────────────────────────────────────────┤
/// │  ┌─────────────────┐  ┌─────────────────────────────────────┐   │
/// │  │ EventBus Trait  │  │     Embedded Broker                 │   │
/// │  │ Implementation  │  │  - Topic routing                    │   │
/// │  │                 │  │  - Subscription management          │   │
/// │  │ - publish()     │  │  - Message delivery                 │   │
/// │  │ - subscribe()   │  │  - Statistics collection           │   │
/// │  │ - unsubscribe() │  │                                     │   │
/// │  │ - statistics()  │  │                                     │   │
/// │  └─────────────────┘  └─────────────────────────────────────┘   │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
pub struct DaemoneyeEventBus {
    /// Embedded daemoneye-eventbus implementation
    inner: Arc<Mutex<DaemoneyeEventBusImpl>>,
    /// Broker reference for direct access
    broker: Arc<DaemoneyeBroker>,
    /// Start time for uptime calculation
    start_time: Instant,
    /// Subscriber mapping for compatibility
    subscriber_mapping: Arc<RwLock<HashMap<String, Uuid>>>,
}

impl DaemoneyeEventBus {
    /// Create a new DaemoneyeEventBus with embedded broker.
    ///
    /// # Arguments
    ///
    /// * `config` - EventBus configuration
    /// * `socket_path` - Socket path for the embedded broker
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{EventBusConfig, DaemoneyeEventBus};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = EventBusConfig::default();
    ///     let event_bus = DaemoneyeEventBus::new(config, "/tmp/daemoneye.sock").await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new(config: EventBusConfig, socket_path: &str) -> Result<Self> {
        info!(
            socket_path = socket_path,
            max_subscribers = config.max_subscribers,
            buffer_size = config.buffer_size,
            "Creating DaemoneyeEventBus with embedded broker"
        );

        // Create embedded broker
        let broker = DaemoneyeBroker::new(socket_path)
            .await
            .context("Failed to create embedded broker")?;

        // Create EventBus implementation
        let event_bus_impl = DaemoneyeEventBusImpl::from_broker(broker)
            .await
            .context("Failed to create EventBus from broker")?;

        // Get broker reference from the event_bus_impl
        let broker_ref = event_bus_impl.broker().clone();

        let event_bus = Self {
            inner: Arc::new(Mutex::new(event_bus_impl)),
            broker: broker_ref,
            start_time: Instant::now(),
            subscriber_mapping: Arc::new(RwLock::new(HashMap::new())),
        };

        info!("DaemoneyeEventBus created successfully");
        Ok(event_bus)
    }

    /// Create a new DaemoneyeEventBus from an existing broker.
    ///
    /// This method is useful when you want to share a broker instance
    /// across multiple EventBus instances.
    ///
    /// # Arguments
    ///
    /// * `config` - EventBus configuration
    /// * `broker` - Existing broker instance
    pub async fn from_broker(config: EventBusConfig, broker: DaemoneyeBroker) -> Result<Self> {
        info!(
            max_subscribers = config.max_subscribers,
            buffer_size = config.buffer_size,
            "Creating DaemoneyeEventBus from existing broker"
        );

        let event_bus_impl = DaemoneyeEventBusImpl::from_broker(broker)
            .await
            .context("Failed to create EventBus from broker")?;

        // Get broker reference from the event_bus_impl
        let broker_ref = event_bus_impl.broker().clone();

        let event_bus = Self {
            inner: Arc::new(Mutex::new(event_bus_impl)),
            broker: broker_ref,
            start_time: Instant::now(),
            subscriber_mapping: Arc::new(RwLock::new(HashMap::new())),
        };

        info!("DaemoneyeEventBus created from existing broker");
        Ok(event_bus)
    }

    /// Get a reference to the embedded broker.
    ///
    /// This method provides direct access to the broker for advanced
    /// operations that are not exposed through the EventBus trait.
    pub fn broker(&self) -> &Arc<DaemoneyeBroker> {
        &self.broker
    }

    /// Start the embedded broker.
    ///
    /// This method starts the broker's internal transport and message
    /// routing systems. It should be called before using the EventBus.
    pub async fn start(&self) -> Result<()> {
        info!("Starting embedded broker");
        self.broker
            .start()
            .await
            .context("Failed to start embedded broker")?;
        info!("Embedded broker started successfully");
        Ok(())
    }

    /// Shutdown the EventBus and embedded broker.
    ///
    /// This method performs a graceful shutdown of the EventBus and
    /// its embedded broker, ensuring all pending messages are processed.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down DaemoneyeEventBus");

        // Shutdown the inner EventBus implementation
        {
            let mut inner = self.inner.lock().await;
            if let Err(e) = inner.shutdown().await {
                error!(error = %e, "Failed to shutdown inner EventBus");
            }
        }

        // Shutdown the broker
        if let Err(e) = self.broker.shutdown().await {
            error!(error = %e, "Failed to shutdown broker");
        }

        info!("DaemoneyeEventBus shutdown completed");
        Ok(())
    }

    /// Convert collector-core CollectionEvent to daemoneye-eventbus CollectionEvent.
    fn convert_collection_event(
        event: &crate::event::CollectionEvent,
    ) -> Result<daemoneye_eventbus::CollectionEvent, anyhow::Error> {
        match event {
            crate::event::CollectionEvent::Process(process_event) => Ok(
                daemoneye_eventbus::CollectionEvent::Process(daemoneye_eventbus::ProcessEvent {
                    pid: process_event.pid,
                    name: process_event.name.clone(),
                    command_line: if process_event.command_line.is_empty() {
                        None
                    } else {
                        Some(process_event.command_line.join(" "))
                    },
                    executable_path: process_event.executable_path.clone(),
                    ppid: process_event.ppid,
                    start_time: process_event.start_time,
                    metadata: std::collections::HashMap::new(),
                }),
            ),
            crate::event::CollectionEvent::Network(network_event) => Ok(
                daemoneye_eventbus::CollectionEvent::Network(daemoneye_eventbus::NetworkEvent {
                    connection_id: network_event.connection_id.clone(),
                    source_address: network_event.source_addr.clone(),
                    destination_address: network_event.dest_addr.clone(),
                    protocol: network_event.protocol.clone(),
                    metadata: std::collections::HashMap::new(),
                }),
            ),
            crate::event::CollectionEvent::Filesystem(fs_event) => {
                Ok(daemoneye_eventbus::CollectionEvent::Filesystem(
                    daemoneye_eventbus::FilesystemEvent {
                        path: fs_event.path.clone(),
                        event_type: fs_event.operation.clone(),
                        size: fs_event.size,
                        metadata: std::collections::HashMap::new(),
                    },
                ))
            }
            crate::event::CollectionEvent::Performance(perf_event) => {
                Ok(daemoneye_eventbus::CollectionEvent::Performance(
                    daemoneye_eventbus::PerformanceEvent {
                        metric_name: perf_event.metric_name.clone(),
                        value: perf_event.value,
                        unit: perf_event.unit.clone(),
                        metadata: std::collections::HashMap::new(),
                    },
                ))
            }
            crate::event::CollectionEvent::TriggerRequest(trigger_request) => {
                let analysis_type_str = match trigger_request.analysis_type {
                    crate::event::AnalysisType::BinaryHash => "binary_hash",
                    crate::event::AnalysisType::MemoryAnalysis => "memory_analysis",
                    crate::event::AnalysisType::YaraScan => "yara_scan",
                    crate::event::AnalysisType::NetworkAnalysis => "network_analysis",
                    crate::event::AnalysisType::BehavioralAnalysis => "behavioral_analysis",
                    crate::event::AnalysisType::Custom(ref custom) => custom,
                };

                // Serialize the full trigger request into payload so downstream can reconstruct
                let payload = serde_json::to_vec(trigger_request)
                    .map_err(|e| anyhow::anyhow!("Failed to serialize trigger request: {}", e))?;

                Ok(daemoneye_eventbus::CollectionEvent::TriggerRequest(
                    daemoneye_eventbus::TriggerRequest {
                        request_id: trigger_request.trigger_id.clone(),
                        collector_type: analysis_type_str.to_string(),
                        priority: match trigger_request.priority {
                            crate::event::TriggerPriority::Low => 1,
                            crate::event::TriggerPriority::Normal => 2,
                            crate::event::TriggerPriority::High => 3,
                            crate::event::TriggerPriority::Critical => 4,
                        },
                        payload,
                        metadata: std::collections::HashMap::new(),
                    },
                ))
            }
        }
    }

    /// Convert collector-core EventSubscription to daemoneye-eventbus EventSubscription.
    fn convert_subscription(
        subscription: &crate::event_bus::EventSubscription,
    ) -> daemoneye_eventbus::EventSubscription {
        // Accept all event types by default to maximize compatibility with broker routing
        let event_types: Vec<String> = Vec::new();

        daemoneye_eventbus::EventSubscription {
            subscriber_id: subscription.subscriber_id.clone(),
            capabilities: daemoneye_eventbus::SourceCaps {
                event_types,
                collectors: Vec::new(), // accept events from any collector
                max_priority: 10,
            },
            event_filter: subscription.event_filter.as_ref().map(|filter| {
                daemoneye_eventbus::EventFilter {
                    event_types: filter.event_types.clone(),
                    pids: filter.pids.clone(),
                    min_priority: filter.min_priority,
                    metadata_filters: filter.metadata_filters.clone(),
                    topic_filters: filter.topic_filters.clone(),
                    source_collectors: filter.source_collectors.clone(),
                }
            }),
            correlation_filter: subscription.correlation_filter.as_ref().map(|filter| {
                daemoneye_eventbus::CorrelationFilter {
                    correlation_ids: if let Some(id) = &filter.correlation_id {
                        vec![id.clone()]
                    } else {
                        vec![]
                    },
                    correlation_patterns: vec![],
                }
            }),
            topic_patterns: subscription.topic_patterns.clone(),
            enable_wildcards: subscription.enable_wildcards,
        }
    }

    /// Convert daemoneye-eventbus BusEvent to collector-core BusEvent.
    fn convert_bus_event(event: &daemoneye_eventbus::BusEvent) -> crate::event_bus::BusEvent {
        // Convert daemoneye-eventbus CollectionEvent to collector-core CollectionEvent
        let collection_event = match &event.event {
            daemoneye_eventbus::CollectionEvent::Process(process_event) => {
                crate::event::CollectionEvent::Process(crate::event::ProcessEvent {
                    pid: process_event.pid,
                    ppid: process_event.ppid,
                    name: process_event.name.clone(),
                    executable_path: process_event.executable_path.clone(),
                    command_line: process_event
                        .command_line
                        .clone()
                        .map_or_else(Vec::new, |cmd| vec![cmd]),
                    start_time: process_event.start_time,
                    cpu_usage: None,
                    memory_usage: None,
                    executable_hash: None,
                    user_id: None,
                    accessible: true,
                    file_exists: true,
                    timestamp: std::time::SystemTime::now(),
                    platform_metadata: None,
                })
            }
            daemoneye_eventbus::CollectionEvent::Network(network_event) => {
                crate::event::CollectionEvent::Network(crate::event::NetworkEvent {
                    connection_id: network_event.connection_id.clone(),
                    source_addr: network_event.source_address.clone(),
                    dest_addr: network_event.destination_address.clone(),
                    protocol: network_event.protocol.clone(),
                    state: "unknown".to_string(),
                    pid: None,
                    bytes_sent: 0,
                    bytes_received: 0,
                    timestamp: std::time::SystemTime::now(),
                })
            }
            daemoneye_eventbus::CollectionEvent::Filesystem(fs_event) => {
                crate::event::CollectionEvent::Filesystem(crate::event::FilesystemEvent {
                    path: fs_event.path.clone(),
                    operation: fs_event.event_type.clone(),
                    pid: None,
                    size: fs_event.size,
                    permissions: None,
                    file_hash: None,
                    timestamp: std::time::SystemTime::now(),
                })
            }
            daemoneye_eventbus::CollectionEvent::Performance(perf_event) => {
                crate::event::CollectionEvent::Performance(crate::event::PerformanceEvent {
                    metric_name: perf_event.metric_name.clone(),
                    value: perf_event.value,
                    unit: perf_event.unit.clone(),
                    pid: None,
                    component: "unknown".to_string(),
                    timestamp: std::time::SystemTime::now(),
                })
            }
            daemoneye_eventbus::CollectionEvent::TriggerRequest(trigger_request) => {
                // Try to reconstruct the original trigger request from payload
                if let Ok(req) =
                    serde_json::from_slice::<crate::event::TriggerRequest>(&trigger_request.payload)
                {
                    crate::event::CollectionEvent::TriggerRequest(req)
                } else {
                    // Fallback to minimal mapping if payload is unavailable
                    crate::event::CollectionEvent::TriggerRequest(crate::event::TriggerRequest {
                        trigger_id: trigger_request.request_id.clone(),
                        target_collector: trigger_request.collector_type.clone(),
                        analysis_type: crate::event::AnalysisType::YaraScan, // Default
                        priority: crate::event::TriggerPriority::Normal,
                        target_pid: None,
                        target_path: None,
                        correlation_id: "".to_string(),
                        metadata: std::collections::HashMap::new(),
                        timestamp: std::time::SystemTime::now(),
                    })
                }
            }
        };

        crate::event_bus::BusEvent {
            id: event.event_id.parse().unwrap_or_else(|_| Uuid::new_v4()),
            timestamp: event
                .bus_timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            event: collection_event,
            correlation_metadata: crate::event_bus::CorrelationMetadata::new(
                event.correlation_id.clone(),
            ),
            routing_metadata: std::collections::HashMap::new(),
        }
    }

    /// Convert daemoneye-eventbus statistics to collector-core statistics.
    fn convert_statistics(stats: &DaemoneyeEventBusStatistics) -> EventBusStatistics {
        EventBusStatistics {
            events_published: stats.messages_published,
            events_delivered: stats.messages_delivered,
            active_subscribers: stats.active_subscribers,
            uptime: Duration::from_secs(stats.uptime_seconds),
        }
    }
}

#[async_trait]
impl EventBus for DaemoneyeEventBus {
    async fn publish(
        &self,
        event: CollectionEvent,
        correlation_metadata: crate::event_bus::CorrelationMetadata,
    ) -> Result<()> {
        let daemoneye_event = Self::convert_collection_event(&event)?;
        let correlation = correlation_metadata.correlation_id.clone();

        debug!(
            event_type = event.event_type(),
            correlation_id = %correlation,
            "Publishing event to DaemoneyeEventBus"
        );

        let mut inner = self.inner.lock().await;
        inner
            .publish(daemoneye_event, correlation)
            .await
            .context("Failed to publish event to daemoneye-eventbus")?;

        Ok(())
    }

    async fn subscribe(
        &mut self,
        subscription: EventSubscription,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<BusEvent>> {
        let daemoneye_subscription = Self::convert_subscription(&subscription);
        let subscriber_id = subscription.subscriber_id.clone();

        debug!(
            subscriber_id = %subscriber_id,
            capabilities = ?subscription.capabilities,
            topic_patterns = ?subscription.topic_patterns,
            "Subscribing to DaemoneyeEventBus"
        );

        let mut inner = self.inner.lock().await;
        let daemoneye_receiver = inner
            .subscribe(daemoneye_subscription)
            .await
            .context("Failed to subscribe to daemoneye-eventbus")?;

        // Create a new channel for collector-core compatibility
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Store subscriber mapping
        {
            let mut mapping = self.subscriber_mapping.write().await;
            mapping.insert(subscriber_id.clone(), Uuid::new_v4());
        }

        // Spawn a task to convert and forward events
        let subscriber_id_clone = subscriber_id.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            // Signal readiness before entering the loop to avoid race with immediate publish
            let _ = ready_tx.send(());
            let mut daemoneye_rx = daemoneye_receiver;
            while let Some(daemoneye_event) = daemoneye_rx.recv().await {
                let collector_event = Self::convert_bus_event(&daemoneye_event);
                if tx.send(collector_event).is_err() {
                    debug!(
                        subscriber_id = %subscriber_id_clone,
                        "Subscriber channel closed, stopping event forwarding"
                    );
                    break;
                }
            }
        });

        // Ensure the forwarding task is ready before returning
        let _ = ready_rx.await;

        info!(
            subscriber_id = %subscriber_id,
            "Successfully subscribed to DaemoneyeEventBus"
        );

        Ok(rx)
    }

    async fn unsubscribe(&mut self, subscriber_id: &str) -> Result<()> {
        debug!(
            subscriber_id = %subscriber_id,
            "Unsubscribing from DaemoneyeEventBus"
        );

        let mut inner = self.inner.lock().await;
        inner
            .unsubscribe(subscriber_id)
            .await
            .context("Failed to unsubscribe from daemoneye-eventbus")?;

        // Remove from subscriber mapping
        {
            let mut mapping = self.subscriber_mapping.write().await;
            mapping.remove(subscriber_id);
        }

        info!(
            subscriber_id = %subscriber_id,
            "Successfully unsubscribed from DaemoneyeEventBus"
        );

        Ok(())
    }

    async fn get_statistics(&self) -> Result<EventBusStatistics> {
        let inner = self.inner.lock().await;
        let daemoneye_stats = inner.statistics().await;
        let mut stats = Self::convert_statistics(&daemoneye_stats);

        // Update uptime from our start time
        stats.uptime = self.start_time.elapsed();

        Ok(stats)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn shutdown(&self) -> Result<()> {
        // Delegate to the existing shutdown method
        self.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{AnalysisType, ProcessEvent, TriggerPriority, TriggerRequest},
        source::SourceCaps,
    };
    use std::time::SystemTime;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn test_daemoneye_event_bus_creation() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-daemoneye-eventbus.sock");
        let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Start the event bus
        event_bus.start().await.unwrap();

        // Get statistics
        let stats = event_bus.get_statistics().await.unwrap();
        assert_eq!(stats.events_published, 0);
        assert_eq!(stats.active_subscribers, 0);

        // Shutdown
        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_publishing_and_subscription() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-pub-sub.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        // Start the event bus
        event_bus.start().await.unwrap();

        // Create subscription (tightened to process-specific pattern)
        let subscription = EventSubscription {
            subscriber_id: "test-subscriber".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = event_bus.subscribe(subscription).await.unwrap();

        // Create and publish a process event
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            ppid: Some(5678),
            name: "test_process".to_string(),
            executable_path: Some("/bin/test".to_string()),
            command_line: vec!["test".to_string(), "arg1".to_string()],
            start_time: Some(SystemTime::now()),
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        });

        let correlation_metadata =
            crate::event_bus::CorrelationMetadata::new("test-correlation".to_string());
        event_bus
            .publish(process_event.clone(), correlation_metadata)
            .await
            .unwrap();

        // Receive the event (allow a bit more time for async forwarding)
        let received_event = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("Timeout waiting for event")
            .unwrap();

        // Verify the event
        match received_event.event {
            CollectionEvent::Process(ref proc_event) => {
                assert_eq!(proc_event.pid, 1234);
                assert_eq!(proc_event.name, "test_process");
            }
            _ => panic!("Expected ProcessEvent"),
        }

        // Verify correlation ID
        assert_eq!(
            received_event.correlation_metadata.correlation_id,
            "test-correlation"
        );

        // Shutdown
        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_trigger_request_conversion() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-trigger.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Create subscription for trigger requests (tightened patterns)
        let subscription = EventSubscription {
            subscriber_id: "trigger-subscriber".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec![
                "events.process.+".to_string(),
                "control.trigger.+".to_string(),
            ]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut receiver = event_bus.subscribe(subscription).await.unwrap();

        // Create and publish a trigger request
        let trigger_request = CollectionEvent::TriggerRequest(TriggerRequest {
            trigger_id: "trigger-123".to_string(),
            target_collector: "yara-collector".to_string(),
            analysis_type: crate::event::AnalysisType::YaraScan,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: Some("/tmp/suspicious_file".to_string()),
            correlation_id: "test-correlation".to_string(),
            metadata: std::collections::HashMap::new(),
            timestamp: SystemTime::now(),
        });

        let trigger_correlation =
            crate::event_bus::CorrelationMetadata::new("trigger-correlation".to_string());
        event_bus
            .publish(trigger_request.clone(), trigger_correlation)
            .await
            .unwrap();

        // Receive the event (allow a bit more time for async forwarding)
        let received_event = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("Timeout waiting for trigger event")
            .unwrap();

        // Verify the trigger request (round-trip preservation)
        match received_event.event {
            CollectionEvent::TriggerRequest(ref trigger) => {
                assert_eq!(trigger.trigger_id, "trigger-123");
                assert_eq!(trigger.target_collector, "yara-collector");
                assert!(matches!(trigger.analysis_type, AnalysisType::YaraScan));
                assert_eq!(trigger.priority, TriggerPriority::High);
                assert_eq!(trigger.target_pid, Some(1234));
                assert_eq!(trigger.target_path.as_deref(), Some("/tmp/suspicious_file"));
                assert_eq!(trigger.correlation_id, "test-correlation");
            }
            _ => panic!("Expected TriggerRequest"),
        }

        // Bus correlation ID should match the one used in publish()
        assert_eq!(
            received_event.correlation_metadata.correlation_id,
            "trigger-correlation"
        );

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_statistics_collection() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-stats.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Get initial statistics
        let initial_stats = event_bus.get_statistics().await.unwrap();
        assert_eq!(initial_stats.events_published, 0);

        // Create subscription
        let subscription = EventSubscription {
            subscriber_id: "stats-subscriber".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        // Publish an event
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 9999,
            ppid: None,
            name: "stats_test".to_string(),
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
        });

        let correlation_metadata =
            crate::event_bus::CorrelationMetadata::new(uuid::Uuid::new_v4().to_string());
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();

        // Get updated statistics
        let updated_stats = event_bus.get_statistics().await.unwrap();
        assert!(updated_stats.events_published > initial_stats.events_published);
        assert!(updated_stats.active_subscribers > 0);

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-unsub.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Create subscription
        let subscription = EventSubscription {
            subscriber_id: "unsub-test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        // Verify subscription exists
        let stats_before = event_bus.get_statistics().await.unwrap();
        assert!(stats_before.active_subscribers > 0);

        // Unsubscribe
        event_bus.unsubscribe("unsub-test").await.unwrap();

        // Verify subscription removed
        let stats_after = event_bus.get_statistics().await.unwrap();
        assert_eq!(
            stats_after.active_subscribers,
            stats_before.active_subscribers - 1
        );

        event_bus.shutdown().await.unwrap();
    }

    // Regression: ensure immediate publish after subscribe does not lose the event
    #[tokio::test]
    async fn test_subscribe_then_immediate_publish_no_loss() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-immediate.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();
        event_bus.start().await.unwrap();

        let subscription = EventSubscription {
            subscriber_id: "immediate-sub".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut rx = event_bus.subscribe(subscription).await.unwrap();

        // Immediately publish after subscribe returns (regression for race)
        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 42,
            ppid: None,
            name: "immediate".to_string(),
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
        });

        let correlation_metadata =
            crate::event_bus::CorrelationMetadata::new("immediate-correlation".to_string());
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();

        let evt = timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("Timeout receiving immediate event")
            .unwrap();

        match evt.event {
            CollectionEvent::Process(ref p) => {
                assert_eq!(p.pid, 42);
                assert_eq!(p.name, "immediate");
            }
            _ => panic!("Expected Process event"),
        }

        assert_eq!(
            evt.correlation_metadata.correlation_id,
            "immediate-correlation"
        );
        event_bus.shutdown().await.unwrap();
    }

    // Regression: multiple subscribers should all receive the published event
    #[tokio::test]
    async fn test_multiple_subscribers_receive_all() {
        let config = EventBusConfig::default();
        let mut bus1 = DaemoneyeEventBus::new(config.clone(), "/tmp/test-multi-1.sock")
            .await
            .unwrap();
        bus1.start().await.unwrap();

        let sub = |id: &str| EventSubscription {
            subscriber_id: id.to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let mut rx1 = bus1.subscribe(sub("s1")).await.unwrap();
        let mut rx2 = bus1.subscribe(sub("s2")).await.unwrap();

        let event = CollectionEvent::Process(ProcessEvent {
            pid: 7,
            ppid: None,
            name: "multi".to_string(),
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
        });

        let correlation_metadata =
            crate::event_bus::CorrelationMetadata::new("multi-correlation".to_string());
        bus1.publish(event, correlation_metadata).await.unwrap();

        let e1 = timeout(Duration::from_secs(3), rx1.recv())
            .await
            .expect("Timeout receiving e1")
            .unwrap();
        let e2 = timeout(Duration::from_secs(3), rx2.recv())
            .await
            .expect("Timeout receiving e2")
            .unwrap();

        assert_eq!(e1.correlation_metadata.correlation_id, "multi-correlation");
        assert_eq!(e2.correlation_metadata.correlation_id, "multi-correlation");
        bus1.shutdown().await.unwrap();
    }
}
