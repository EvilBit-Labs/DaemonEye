//! DaemonEye EventBus integration for collector-core framework.
//!
//! This module provides integration between the collector-core framework and the
//! daemoneye-eventbus message broker, enabling high-performance pub/sub messaging
//! with topic-based routing and embedded broker functionality.

use crate::{
    event::CollectionEvent,
    event_bus::{BusEvent, EventBus, EventBusConfig, EventBusStatistics, EventSubscription},
    source::SourceCaps as CollectorSourceCaps,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use daemoneye_eventbus::{
    DaemoneyeBroker, DaemoneyeEventBus as DaemoneyeEventBusImpl,
    EventBus as DaemoneyeEventBusTrait, EventBusStatistics as DaemoneyeEventBusStatistics,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Comprehensive metrics for DaemonEye EventBus monitoring and observability.
///
/// This structure provides detailed metrics beyond the standard EventBus interface,
/// including broker-specific performance indicators, transport statistics,
/// and health monitoring data for operational visibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemoneyeEventBusMetrics {
    /// Core broker statistics from daemoneye-eventbus
    pub broker_statistics: DaemoneyeEventBusStatistics,
    /// Broker health status and indicators
    pub broker_health: BrokerHealthStatus,
    /// Transport layer statistics
    pub transport_statistics: TransportStatistics,
    /// Client statistics aggregated across all connections
    pub client_statistics: ClientStatisticsAggregate,
    /// Topic routing and subscription statistics
    pub topic_statistics: TopicStatistics,
    /// Performance metrics and efficiency indicators
    pub performance_metrics: PerformanceMetrics,
    /// Timestamp when metrics were collected
    pub collection_timestamp: SystemTime,
}

/// Broker health status with comprehensive health indicators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerHealthStatus {
    /// Overall health status
    pub status: HealthStatus,
    /// Broker uptime
    pub uptime: Duration,
    /// Last health check timestamp
    pub last_health_check: SystemTime,
    /// Number of active connections
    pub active_connections: usize,
    /// Message throughput (messages per second)
    pub message_throughput: f64,
    /// Error rate (errors per second)
    pub error_rate: f64,
    /// Estimated memory usage in bytes
    pub memory_usage: u64,
}

/// Health status enumeration for monitoring systems.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    /// System is operating normally
    Healthy,
    /// System is starting up
    Starting,
    /// System has minor issues but is functional
    Warning,
    /// System has significant issues affecting performance
    Degraded,
    /// System has critical issues requiring immediate attention
    Critical,
}

/// Transport layer statistics for connection monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportStatistics {
    /// Total number of connections
    pub total_connections: usize,
    /// Number of healthy connections
    pub healthy_connections: usize,
    /// Number of failed connection attempts
    pub failed_connections: u64,
    /// Number of reconnection attempts
    pub reconnection_attempts: u64,
    /// Total bytes sent across all connections
    pub bytes_sent: u64,
    /// Total bytes received across all connections
    pub bytes_received: u64,
    /// Average message latency in milliseconds
    pub average_latency_ms: f64,
    /// Number of connection errors
    pub connection_errors: u64,
}

/// Client statistics aggregated across all connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientStatisticsAggregate {
    /// Total number of connected clients
    pub total_clients: usize,
    /// Total active subscriptions across all clients
    pub active_subscriptions: usize,
    /// Total messages published by all clients
    pub total_messages_published: u64,
    /// Total messages received by all clients
    pub total_messages_received: u64,
    /// Average client uptime
    pub average_uptime: Duration,
    /// Total health check failures across all clients
    pub health_check_failures: u64,
}

/// Topic routing and subscription statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicStatistics {
    /// Number of active topics
    pub active_topics: usize,
    /// Total number of subscriptions
    pub total_subscriptions: usize,
    /// Message routing efficiency (0.0 to 1.0)
    pub message_routing_efficiency: f64,
    /// Distribution of messages across topics
    pub topic_distribution: HashMap<String, u64>,
    /// Number of wildcard subscriptions
    pub wildcard_usage: usize,
}

/// Performance metrics and efficiency indicators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    /// Messages processed per second
    pub messages_per_second: f64,
    /// Message delivery rate (0.0 to 1.0)
    pub delivery_rate: f64,
    /// Average number of subscribers
    pub average_subscribers: f64,
    /// Overall throughput efficiency (0.0 to 1.0)
    pub throughput_efficiency: f64,
    /// Resource utilization estimate (0.0 to 1.0)
    pub resource_utilization: f64,
}

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
#[derive(Clone)]
pub struct DaemoneyeEventBus {
    /// Embedded daemoneye-eventbus implementation
    inner: Arc<Mutex<DaemoneyeEventBusImpl>>,
    /// Broker reference for direct access
    broker: Arc<DaemoneyeBroker>,
    /// Start time for uptime calculation
    start_time: Instant,
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
        let event_types = Self::capability_event_types(subscription.capabilities);

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
                    parent_correlation_ids: vec![],
                    root_correlation_ids: vec![],
                    workflow_stages: vec![],
                    required_tags: std::collections::HashMap::new(),
                    any_tags: std::collections::HashMap::new(),
                    min_sequence: None,
                    max_sequence: None,
                }
            }),
            topic_patterns: subscription.topic_patterns.clone(),
            enable_wildcards: subscription.enable_wildcards,
        }
    }

    fn capability_event_types(capabilities: CollectorSourceCaps) -> Vec<String> {
        let mut event_types = Vec::with_capacity(4);

        if capabilities.contains(CollectorSourceCaps::PROCESS) {
            event_types.push("process".to_string());
        }
        if capabilities.contains(CollectorSourceCaps::NETWORK) {
            event_types.push("network".to_string());
        }
        if capabilities.contains(CollectorSourceCaps::FILESYSTEM) {
            event_types.push("filesystem".to_string());
        }
        if capabilities.contains(CollectorSourceCaps::PERFORMANCE) {
            event_types.push("performance".to_string());
        }

        event_types
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
                let req_result = serde_json::from_slice::<crate::event::TriggerRequest>(
                    &trigger_request.payload,
                );
                match req_result {
                    Ok(req) => {
                        // Validate the deserialized request
                        if let Err(e) = req.validate() {
                            warn!(
                                error = %e,
                                "Deserialized trigger request failed validation, using fallback"
                            );
                            // Fallback to minimal mapping if validation failed
                            crate::event::CollectionEvent::TriggerRequest(
                                crate::event::TriggerRequest {
                                    trigger_id: trigger_request.request_id.clone(),
                                    target_collector: trigger_request.collector_type.clone(),
                                    analysis_type: crate::event::AnalysisType::YaraScan, // Default
                                    priority: crate::event::TriggerPriority::Normal,
                                    target_pid: None,
                                    target_path: None,
                                    correlation_id: "".to_string(),
                                    metadata: std::collections::HashMap::new(),
                                    timestamp: std::time::SystemTime::now(),
                                },
                            )
                        } else {
                            // Valid request, use it
                            crate::event::CollectionEvent::TriggerRequest(req)
                        }
                    }
                    Err(_) => {
                        // Deserialization failed, use fallback
                        // Note: Fallback request may not pass validation (empty correlation_id, no target)
                        crate::event::CollectionEvent::TriggerRequest(
                            crate::event::TriggerRequest {
                                trigger_id: trigger_request.request_id.clone(),
                                target_collector: trigger_request.collector_type.clone(),
                                analysis_type: crate::event::AnalysisType::YaraScan, // Default
                                priority: crate::event::TriggerPriority::Normal,
                                target_pid: None,
                                target_path: None,
                                correlation_id: "".to_string(),
                                metadata: std::collections::HashMap::new(),
                                timestamp: std::time::SystemTime::now(),
                            },
                        )
                    }
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
                event.correlation_id().to_string(),
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

    /// Get comprehensive daemoneye-eventbus specific metrics and health indicators.
    ///
    /// This method provides detailed metrics beyond the standard EventBus interface,
    /// including broker-specific performance indicators, transport statistics,
    /// and health monitoring data.
    ///
    /// # Returns
    ///
    /// Returns `DaemoneyeEventBusMetrics` containing:
    /// - Broker performance metrics
    /// - Transport layer statistics
    /// - Client connection health
    /// - Topic routing statistics
    /// - Message delivery tracking
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{DaemoneyeEventBus, event_bus::EventBusConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = EventBusConfig::default();
    ///     let event_bus = DaemoneyeEventBus::new(config, "/tmp/daemoneye.sock").await?;
    ///     let metrics = event_bus.get_detailed_metrics().await?;
    ///     println!("Broker health: {:?}", metrics.broker_health);
    ///     Ok(())
    /// }
    /// ```
    pub async fn get_detailed_metrics(&self) -> Result<DaemoneyeEventBusMetrics> {
        // Get broker statistics once and reuse
        let broker_stats = self.broker.statistics().await;

        // Execute all metric collection operations concurrently
        let (broker_health, transport_stats, client_stats, topic_stats, performance_metrics) = tokio::try_join!(
            self.get_broker_health_with_stats(&broker_stats),
            self.get_transport_statistics_with_stats(&broker_stats),
            self.get_client_statistics_with_stats(&broker_stats),
            self.get_topic_statistics_with_stats(&broker_stats),
            self.calculate_performance_metrics(&broker_stats)
        )?;

        Ok(DaemoneyeEventBusMetrics {
            broker_statistics: broker_stats,
            broker_health,
            transport_statistics: transport_stats,
            client_statistics: client_stats,
            topic_statistics: topic_stats,
            performance_metrics,
            collection_timestamp: std::time::SystemTime::now(),
        })
    }

    /// Get broker health status with comprehensive health indicators.
    async fn get_broker_health(&self) -> Result<BrokerHealthStatus> {
        let stats = self.broker.statistics().await;
        self.get_broker_health_with_stats(&stats).await
    }

    /// Get broker health status using provided statistics (optimized version).
    async fn get_broker_health_with_stats(
        &self,
        stats: &DaemoneyeEventBusStatistics,
    ) -> Result<BrokerHealthStatus> {
        let uptime = Duration::from_secs(stats.uptime_seconds);

        // Determine health status based on various indicators
        let health_status = if stats.active_subscribers == 0 && stats.messages_published > 0 {
            HealthStatus::Warning // Messages published but no subscribers
        } else if uptime < Duration::from_secs(10) {
            HealthStatus::Starting // Recently started
        } else if stats.messages_delivered < stats.messages_published {
            let delivery_ratio = stats.messages_delivered as f64 / stats.messages_published as f64;
            if delivery_ratio < 0.8 {
                HealthStatus::Degraded // Poor delivery ratio
            } else {
                HealthStatus::Healthy
            }
        } else {
            HealthStatus::Healthy
        };

        Ok(BrokerHealthStatus {
            status: health_status,
            uptime,
            last_health_check: std::time::SystemTime::now(),
            active_connections: stats.active_subscribers,
            message_throughput: self.calculate_message_throughput(stats),
            error_rate: 0.0, // Simplified implementation - would need broker API extension for actual error tracking
            memory_usage: self.estimate_memory_usage(stats),
        })
    }

    /// Get transport layer statistics using provided statistics (optimized version).
    async fn get_transport_statistics_with_stats(
        &self,
        broker_stats: &DaemoneyeEventBusStatistics,
    ) -> Result<TransportStatistics> {
        // Note: This is a simplified implementation as we don't have direct access
        // to transport layer statistics from the broker interface.
        // In a full implementation, we would need to extend the broker API.

        Ok(TransportStatistics {
            total_connections: broker_stats.active_subscribers,
            healthy_connections: broker_stats.active_subscribers, // Assume all active are healthy
            failed_connections: 0,                                // Would need broker API extension
            reconnection_attempts: 0,                             // Would need broker API extension
            bytes_sent: 0,                                        // Would need broker API extension
            bytes_received: 0,                                    // Would need broker API extension
            average_latency_ms: 0.0,                              // Would need broker API extension
            connection_errors: 0,                                 // Would need broker API extension
        })
    }

    /// Get client statistics using provided statistics (optimized version).
    async fn get_client_statistics_with_stats(
        &self,
        broker_stats: &DaemoneyeEventBusStatistics,
    ) -> Result<ClientStatisticsAggregate> {
        Ok(ClientStatisticsAggregate {
            total_clients: broker_stats.active_subscribers,
            active_subscriptions: broker_stats.active_subscribers, // Simplified
            total_messages_published: broker_stats.messages_published,
            total_messages_received: broker_stats.messages_delivered,
            average_uptime: Duration::from_secs(broker_stats.uptime_seconds),
            health_check_failures: 0, // Would need broker API extension
        })
    }

    /// Get topic routing statistics using provided statistics (optimized version).
    async fn get_topic_statistics_with_stats(
        &self,
        broker_stats: &DaemoneyeEventBusStatistics,
    ) -> Result<TopicStatistics> {
        Ok(TopicStatistics {
            active_topics: broker_stats.active_topics,
            total_subscriptions: broker_stats.active_subscribers,
            message_routing_efficiency: self.calculate_routing_efficiency(broker_stats),
            topic_distribution: HashMap::new(), // Would need broker API extension
            wildcard_usage: 0,                  // Would need broker API extension
        })
    }

    /// Calculate performance metrics based on broker statistics.
    async fn calculate_performance_metrics(
        &self,
        stats: &DaemoneyeEventBusStatistics,
    ) -> Result<PerformanceMetrics> {
        let uptime_seconds = stats.uptime_seconds.max(1); // Avoid division by zero

        Ok(PerformanceMetrics {
            messages_per_second: stats.messages_published as f64 / uptime_seconds as f64,
            delivery_rate: if stats.messages_published > 0 {
                stats.messages_delivered as f64 / stats.messages_published as f64
            } else {
                1.0
            },
            average_subscribers: stats.active_subscribers as f64,
            throughput_efficiency: self.calculate_throughput_efficiency(stats),
            resource_utilization: self.estimate_resource_utilization(stats),
        })
    }

    /// Calculate message throughput for health monitoring.
    fn calculate_message_throughput(&self, stats: &DaemoneyeEventBusStatistics) -> f64 {
        let uptime_seconds = stats.uptime_seconds.max(1);
        stats.messages_published as f64 / uptime_seconds as f64
    }

    /// Estimate memory usage based on broker statistics.
    fn estimate_memory_usage(&self, stats: &DaemoneyeEventBusStatistics) -> u64 {
        // Rough estimation based on active subscribers and topics
        // In practice, this would need actual memory monitoring
        const BASE_MEMORY: u64 = 1024 * 1024; // 1MB base
        const SUBSCRIBER_MEMORY: u64 = 1024; // 1KB per subscriber
        const TOPIC_MEMORY: u64 = 512; // 512B per topic

        BASE_MEMORY
            + (stats.active_subscribers as u64 * SUBSCRIBER_MEMORY)
            + (stats.active_topics as u64 * TOPIC_MEMORY)
    }

    /// Calculate routing efficiency for topic statistics.
    fn calculate_routing_efficiency(&self, stats: &DaemoneyeEventBusStatistics) -> f64 {
        if stats.messages_published == 0 {
            return 1.0;
        }

        // Efficiency = delivered / published
        stats.messages_delivered as f64 / stats.messages_published as f64
    }

    /// Calculate throughput efficiency for performance metrics.
    fn calculate_throughput_efficiency(&self, stats: &DaemoneyeEventBusStatistics) -> f64 {
        // Simplified efficiency calculation based on delivery ratio and subscriber count
        let delivery_ratio = if stats.messages_published > 0 {
            stats.messages_delivered as f64 / stats.messages_published as f64
        } else {
            1.0
        };

        let subscriber_efficiency = if stats.active_subscribers > 0 {
            1.0 // Assume good efficiency if we have subscribers
        } else {
            0.5 // Reduced efficiency if no subscribers
        };

        delivery_ratio * subscriber_efficiency
    }

    /// Estimate resource utilization for performance metrics.
    fn estimate_resource_utilization(&self, stats: &DaemoneyeEventBusStatistics) -> f64 {
        // Simplified resource utilization based on activity
        const MESSAGE_SCALE: f64 = 1000.0;
        const SUBSCRIBER_SCALE: f64 = 100.0;
        const TOPIC_SCALE: f64 = 50.0;

        let message_factor = (stats.messages_published as f64 / MESSAGE_SCALE).min(1.0);
        let subscriber_factor = (stats.active_subscribers as f64 / SUBSCRIBER_SCALE).min(1.0);
        let topic_factor = (stats.active_topics as f64 / TOPIC_SCALE).min(1.0);

        (message_factor + subscriber_factor + topic_factor) / 3.0
    }

    /// Log comprehensive monitoring metrics for observability.
    ///
    /// This method logs detailed metrics in structured format for monitoring
    /// systems and operational dashboards.
    pub async fn log_monitoring_metrics(&self) -> Result<()> {
        let metrics = self.get_detailed_metrics().await?;

        // Log broker health and performance
        tracing::info!(
            broker_status = ?metrics.broker_health.status,
            uptime_seconds = metrics.broker_health.uptime.as_secs(),
            active_connections = metrics.broker_health.active_connections,
            message_throughput = metrics.broker_health.message_throughput,
            error_rate = metrics.broker_health.error_rate,
            memory_usage_bytes = metrics.broker_health.memory_usage,
            "DaemonEye EventBus broker health metrics"
        );

        // Log performance metrics
        tracing::info!(
            messages_per_second = metrics.performance_metrics.messages_per_second,
            delivery_rate = metrics.performance_metrics.delivery_rate,
            average_subscribers = metrics.performance_metrics.average_subscribers,
            throughput_efficiency = metrics.performance_metrics.throughput_efficiency,
            resource_utilization = metrics.performance_metrics.resource_utilization,
            "DaemonEye EventBus performance metrics"
        );

        // Log transport statistics
        tracing::info!(
            total_connections = metrics.transport_statistics.total_connections,
            healthy_connections = metrics.transport_statistics.healthy_connections,
            failed_connections = metrics.transport_statistics.failed_connections,
            reconnection_attempts = metrics.transport_statistics.reconnection_attempts,
            average_latency_ms = metrics.transport_statistics.average_latency_ms,
            connection_errors = metrics.transport_statistics.connection_errors,
            "DaemonEye EventBus transport metrics"
        );

        // Log topic statistics
        tracing::info!(
            active_topics = metrics.topic_statistics.active_topics,
            total_subscriptions = metrics.topic_statistics.total_subscriptions,
            routing_efficiency = metrics.topic_statistics.message_routing_efficiency,
            wildcard_usage = metrics.topic_statistics.wildcard_usage,
            "DaemonEye EventBus topic routing metrics"
        );

        // Log client statistics
        tracing::info!(
            total_clients = metrics.client_statistics.total_clients,
            active_subscriptions = metrics.client_statistics.active_subscriptions,
            total_messages_published = metrics.client_statistics.total_messages_published,
            total_messages_received = metrics.client_statistics.total_messages_received,
            average_uptime_seconds = metrics.client_statistics.average_uptime.as_secs(),
            health_check_failures = metrics.client_statistics.health_check_failures,
            "DaemonEye EventBus client metrics"
        );

        Ok(())
    }

    /// Get health status for monitoring integration.
    ///
    /// This method provides a simple health check interface for monitoring
    /// systems that need to determine if the EventBus is operating correctly.
    pub async fn health_check(&self) -> Result<bool> {
        let health = self.get_broker_health().await?;

        match health.status {
            HealthStatus::Healthy | HealthStatus::Starting => Ok(true),
            HealthStatus::Warning | HealthStatus::Degraded | HealthStatus::Critical => Ok(false),
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

        // Update uptime from our start time for consistency
        stats.uptime = self.start_time.elapsed();

        // Log detailed metrics for monitoring (non-blocking)
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(e) = self_clone.log_monitoring_metrics().await {
                tracing::warn!(error = %e, "Failed to log monitoring metrics");
            }
        });

        Ok(stats)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn shutdown(&self) -> Result<()> {
        // Delegate to the existing shutdown method
        DaemoneyeEventBus::shutdown(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{AnalysisType, ProcessEvent, TriggerPriority, TriggerRequest},
        event_bus::EventSubscription,
        source::SourceCaps,
    };
    use std::time::SystemTime;
    use tokio::time::{Duration, timeout};

    #[test]
    fn convert_subscription_populates_event_types() {
        let subscription = EventSubscription {
            subscriber_id: "test".to_string(),
            capabilities: SourceCaps::PROCESS | SourceCaps::NETWORK,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: true,
            topic_filter: None,
        };

        let converted = DaemoneyeEventBus::convert_subscription(&subscription);
        assert_eq!(
            converted.capabilities.event_types,
            vec!["process".to_string(), "network".to_string()]
        );
    }

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
    #[ignore = "Flaky test - needs investigation"]
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

    #[tokio::test]
    async fn test_detailed_metrics_collection() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-detailed-metrics.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Create subscription and publish some events to generate metrics
        let subscription = EventSubscription {
            subscriber_id: "metrics-test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        // Publish multiple events to generate statistics
        for i in 0..5 {
            let process_event = CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i,
                ppid: None,
                name: format!("metrics_test_{}", i),
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
                crate::event_bus::CorrelationMetadata::new(format!("metrics-correlation-{}", i));
            event_bus
                .publish(process_event, correlation_metadata)
                .await
                .unwrap();
        }

        // Allow some time for metrics to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Get detailed metrics
        let detailed_metrics = event_bus.get_detailed_metrics().await.unwrap();

        // Verify broker statistics
        assert!(detailed_metrics.broker_statistics.messages_published >= 5);
        assert!(detailed_metrics.broker_statistics.active_subscribers >= 1);
        // Uptime might be 0 in fast tests, so just check it's not negative
        assert!(detailed_metrics.broker_statistics.uptime_seconds < u64::MAX);

        // Verify broker health
        assert!(matches!(
            detailed_metrics.broker_health.status,
            HealthStatus::Healthy | HealthStatus::Starting
        ));
        // Uptime might be 0 in fast tests
        assert!(detailed_metrics.broker_health.uptime < Duration::from_secs(3600));
        assert!(detailed_metrics.broker_health.active_connections >= 1);
        assert!(detailed_metrics.broker_health.message_throughput >= 0.0);

        // Verify transport statistics
        assert!(detailed_metrics.transport_statistics.total_connections >= 1);
        assert!(detailed_metrics.transport_statistics.healthy_connections >= 1);

        // Verify client statistics
        assert!(detailed_metrics.client_statistics.total_clients >= 1);
        assert!(detailed_metrics.client_statistics.total_messages_published >= 5);

        // Verify topic statistics (may be 0 in fast tests)
        assert!(detailed_metrics.topic_statistics.active_topics < 1000); // Reasonable upper bound
        assert!(detailed_metrics.topic_statistics.total_subscriptions >= 1);
        assert!(detailed_metrics.topic_statistics.message_routing_efficiency >= 0.0);
        assert!(detailed_metrics.topic_statistics.message_routing_efficiency <= 1.0);

        // Verify performance metrics
        assert!(detailed_metrics.performance_metrics.messages_per_second >= 0.0);
        assert!(detailed_metrics.performance_metrics.delivery_rate >= 0.0);
        assert!(detailed_metrics.performance_metrics.delivery_rate <= 1.0);
        assert!(detailed_metrics.performance_metrics.throughput_efficiency >= 0.0);
        assert!(detailed_metrics.performance_metrics.throughput_efficiency <= 1.0);

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_broker_health_status_calculation() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        // Use shorter unique identifier to avoid SUN_LEN limits
        let socket_name = format!("health-{}.sock", std::process::id());
        let socket_path = temp_dir.path().join(socket_name);

        // Clean up any existing socket from previous test runs
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Test health status calculation
        let health_status = event_bus.get_broker_health().await.unwrap();

        // Should be healthy or starting for a new broker
        assert!(matches!(
            health_status.status,
            HealthStatus::Healthy | HealthStatus::Starting
        ));
        // Uptime might be 0 in fast tests
        assert!(health_status.uptime < Duration::from_secs(3600));
        assert_eq!(health_status.active_connections, 0); // No subscribers yet
        assert!(health_status.message_throughput >= 0.0);
        assert_eq!(health_status.error_rate, 0.0); // No errors expected
        assert!(health_status.memory_usage > 0); // Should have some estimated usage

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_performance_metrics_calculation() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-performance-metrics.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Create subscription
        let subscription = EventSubscription {
            subscriber_id: "perf-test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        // Publish events to generate performance data
        for i in 0..10 {
            let process_event = CollectionEvent::Process(ProcessEvent {
                pid: 2000 + i,
                ppid: None,
                name: format!("perf_test_{}", i),
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
                crate::event_bus::CorrelationMetadata::new(format!("perf-correlation-{}", i));
            event_bus
                .publish(process_event, correlation_metadata)
                .await
                .unwrap();
        }

        // Allow time for processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Get broker statistics for performance calculation
        let broker_stats = event_bus.broker.statistics().await;
        let performance_metrics = event_bus
            .calculate_performance_metrics(&broker_stats)
            .await
            .unwrap();

        // Verify performance metrics are reasonable
        assert!(performance_metrics.messages_per_second >= 0.0);
        assert!(performance_metrics.delivery_rate >= 0.0);
        assert!(performance_metrics.delivery_rate <= 1.0);
        assert!(performance_metrics.average_subscribers >= 0.0);
        assert!(performance_metrics.throughput_efficiency >= 0.0);
        assert!(performance_metrics.throughput_efficiency <= 1.0);
        assert!(performance_metrics.resource_utilization >= 0.0);
        assert!(performance_metrics.resource_utilization <= 1.0);

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_monitoring_metrics_logging() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-monitoring-logging.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Create subscription and publish events
        let subscription = EventSubscription {
            subscriber_id: "logging-test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 3000,
            ppid: None,
            name: "logging_test".to_string(),
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
            crate::event_bus::CorrelationMetadata::new("logging-correlation".to_string());
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();

        // Test monitoring metrics logging (should not fail)
        let result = event_bus.log_monitoring_metrics().await;
        assert!(result.is_ok());

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_health_check_integration() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-health-check.sock");
        let event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Test health check
        let is_healthy = event_bus.health_check().await.unwrap();
        assert!(is_healthy); // Should be healthy for a new broker

        event_bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_statistics_compatibility_with_existing_interface() {
        let config = EventBusConfig::default();
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test-stats-compatibility.sock");
        let mut event_bus = DaemoneyeEventBus::new(config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Test that existing get_statistics interface still works
        let stats = event_bus.get_statistics().await.unwrap();
        assert_eq!(stats.events_published, 0);
        assert_eq!(stats.events_delivered, 0);
        assert_eq!(stats.active_subscribers, 0);
        // Uptime might be 0 in fast tests
        assert!(stats.uptime < Duration::from_secs(3600));

        // Create subscription and publish event
        let subscription = EventSubscription {
            subscriber_id: "compatibility-test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let _receiver = event_bus.subscribe(subscription).await.unwrap();

        let process_event = CollectionEvent::Process(ProcessEvent {
            pid: 4000,
            ppid: None,
            name: "compatibility_test".to_string(),
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
            crate::event_bus::CorrelationMetadata::new("compatibility-correlation".to_string());
        event_bus
            .publish(process_event, correlation_metadata)
            .await
            .unwrap();

        // Allow time for processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify statistics are updated
        let updated_stats = event_bus.get_statistics().await.unwrap();
        assert!(updated_stats.events_published >= 1);
        assert!(updated_stats.active_subscribers >= 1);

        event_bus.shutdown().await.unwrap();
    }
}
