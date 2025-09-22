//! Advanced IPC client implementation with connection management and resilience.
//!
//! This module provides a robust client implementation for daemoneye-agent that includes
//! automatic reconnection, connection pooling, circuit breaker patterns, load balancing,
//! failover capabilities, and comprehensive metrics collection for reliable communication
//! with multiple collector-core components.
//!
//! # Architecture
//!
//! The client uses a multi-endpoint architecture with:
//! - Load balancing strategies (`RoundRobin`, `Weighted`, `Priority`)
//! - Circuit breaker patterns for fault tolerance
//! - Connection pooling for resource efficiency
//! - Comprehensive metrics collection
//!
//! # Examples
//!
//! ```no_run
//! use daemoneye_lib::ipc::{IpcConfig, TransportType};
//! use daemoneye_lib::ipc::client::{ResilientIpcClient, LoadBalancingStrategy};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = IpcConfig {
//!     transport: TransportType::Interprocess,
//!     endpoint_path: "/tmp/collector.sock".to_owned(),
//!     max_frame_bytes: 1024 * 1024,
//!     accept_timeout_ms: 5000,
//!     read_timeout_ms: 30000,
//!     write_timeout_ms: 10000,
//!     max_connections: 32,
//!     panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
//! };
//!
//! let client = ResilientIpcClient::new(&config);
//! let stats = client.get_stats().await;
//! # Ok(())
//! # }
//! ```

use crate::ipc::IpcConfig;
use crate::ipc::codec::{IpcCodec, IpcError, IpcResult};
use crate::proto::{
    CollectionCapabilities, DetectionResult, DetectionTask, MonitoringDomain, TaskType,
};
use interprocess::local_socket::Name;
use interprocess::local_socket::tokio::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Connection state for the IPC client
///
/// Represents the current state of an IPC connection, used for monitoring
/// and health checks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionState {
    /// Client is disconnected
    Disconnected,
    /// Client is attempting to connect
    Connecting,
    /// Client is connected and ready
    Connected,
    /// Client is in a failed state and will retry
    Failed,
    /// Client is in circuit breaker state (temporarily disabled)
    CircuitBreakerOpen,
}

/// Comprehensive metrics for IPC client performance monitoring
///
/// Provides detailed metrics for monitoring IPC client performance,
/// including task execution, connection management, and throughput statistics.
///
/// # Thread Safety
///
/// All metrics use atomic operations and are safe to access from multiple threads.
#[derive(Debug, Default)]
pub struct IpcClientMetrics {
    /// Total number of tasks sent
    pub tasks_sent_total: AtomicU64,
    /// Total number of successful task completions
    pub tasks_completed_total: AtomicU64,
    /// Total number of task failures
    pub tasks_failed_total: AtomicU64,
    /// Total number of connection attempts
    pub connection_attempts_total: AtomicU64,
    /// Total number of successful connections
    pub connections_established_total: AtomicU64,
    /// Total number of connection failures
    pub connection_failures_total: AtomicU64,
    /// Total number of circuit breaker activations
    pub circuit_breaker_activations_total: AtomicU64,
    /// Total bytes sent
    pub bytes_sent_total: AtomicU64,
    /// Total bytes received
    pub bytes_received_total: AtomicU64,
    /// Current number of active connections
    pub active_connections_current: AtomicU64,
    /// Current number of pooled connections
    pub pooled_connections_current: AtomicU64,
    /// Total task execution time in milliseconds
    pub task_duration_ms_total: AtomicU64,
    /// Total connection establishment time in milliseconds
    pub connection_duration_ms_total: AtomicU64,
}

impl IpcClientMetrics {
    /// Create new metrics instance
    ///
    /// All counters are initialized to zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a task attempt
    pub fn record_task_sent(&self) {
        self.tasks_sent_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful task completion
    pub fn record_task_completed(&self, duration_ms: u64) {
        self.tasks_completed_total.fetch_add(1, Ordering::Relaxed);
        self.task_duration_ms_total
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Record a task failure
    pub fn record_task_failed(&self) {
        self.tasks_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a connection attempt
    pub fn record_connection_attempt(&self) {
        self.connection_attempts_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful connection
    pub fn record_connection_established(&self, duration_ms: u64) {
        self.connections_established_total
            .fetch_add(1, Ordering::Relaxed);
        self.connection_duration_ms_total
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Record a connection failure
    pub fn record_connection_failed(&self) {
        self.connection_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record circuit breaker activation
    pub fn record_circuit_breaker_activation(&self) {
        self.circuit_breaker_activations_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record bytes sent
    pub fn record_bytes_sent(&self, bytes: u64) {
        self.bytes_sent_total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes received
    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received_total
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Update active connections count
    pub fn set_active_connections(&self, count: u64) {
        self.active_connections_current
            .store(count, Ordering::Relaxed);
    }

    /// Update pooled connections count
    pub fn set_pooled_connections(&self, count: u64) {
        self.pooled_connections_current
            .store(count, Ordering::Relaxed);
    }

    /// Get current success rate (0.0 to 1.0)
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn success_rate(&self) -> f64 {
        let total_tasks = self.tasks_sent_total.load(Ordering::Relaxed);
        if total_tasks == 0 {
            return 1.0; // No tasks sent yet, assume healthy
        }

        let completed_tasks = self.tasks_completed_total.load(Ordering::Relaxed);
        completed_tasks as f64 / total_tasks as f64
    }

    /// Get average task duration in milliseconds
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn average_task_duration_ms(&self) -> f64 {
        let total_duration = self.task_duration_ms_total.load(Ordering::Relaxed);
        let completed_tasks = self.tasks_completed_total.load(Ordering::Relaxed);

        if completed_tasks == 0 {
            return 0.0;
        }

        total_duration as f64 / completed_tasks as f64
    }

    /// Get average connection establishment time in milliseconds
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn average_connection_duration_ms(&self) -> f64 {
        let total_duration = self.connection_duration_ms_total.load(Ordering::Relaxed);
        let established_connections = self.connections_established_total.load(Ordering::Relaxed);

        if established_connections == 0 {
            return 0.0;
        }

        total_duration as f64 / established_connections as f64
    }

    /// Get connection success rate (0.0 to 1.0)
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn connection_success_rate(&self) -> f64 {
        let total_attempts = self.connection_attempts_total.load(Ordering::Relaxed);
        if total_attempts == 0 {
            return 1.0; // No attempts yet, assume healthy
        }

        let successful_connections = self.connections_established_total.load(Ordering::Relaxed);
        successful_connections as f64 / total_attempts as f64
    }
}

/// Circuit breaker state for failure handling with enhanced metrics
///
/// Implements the circuit breaker pattern to prevent cascading failures
/// by temporarily disabling connections that are experiencing issues.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Work in progress - some methods not yet used
struct CircuitBreaker {
    failure_count: u32,
    last_failure_time: Option<Instant>,
    failure_threshold: u32,
    recovery_timeout: Duration,
    is_open: bool,
    /// Half-open state for testing recovery
    is_half_open: bool,
    /// Number of successful calls in half-open state needed to close
    half_open_success_threshold: u32,
    /// Current success count in half-open state
    half_open_success_count: u32,
    /// Metrics reference for recording activations
    metrics: Option<Arc<IpcClientMetrics>>,
}

#[allow(dead_code)] // Work in progress - some methods not yet used
impl CircuitBreaker {
    const fn new(
        failure_threshold: u32,
        recovery_timeout: Duration,
        metrics: Option<Arc<IpcClientMetrics>>,
    ) -> Self {
        Self {
            failure_count: 0,
            last_failure_time: None,
            failure_threshold,
            recovery_timeout,
            is_open: false,
            is_half_open: false,
            half_open_success_threshold: 3, // Need 3 successes to close
            half_open_success_count: 0,
            metrics,
        }
    }

    fn record_success(&mut self) {
        if self.is_half_open {
            self.half_open_success_count = self.half_open_success_count.saturating_add(1);

            if self.half_open_success_count >= self.half_open_success_threshold {
                // Close the circuit breaker
                self.failure_count = 0;
                self.last_failure_time = None;
                self.is_open = false;
                self.is_half_open = false;
                self.half_open_success_count = 0;

                info!("Circuit breaker closed after successful recovery");
            }
        } else if self.is_open {
            // Shouldn't happen, but reset if it does
            self.failure_count = 0;
            self.last_failure_time = None;
            self.is_open = false;
        } else {
            // Normal operation - reset failure count
            self.failure_count = 0;
            self.last_failure_time = None;
        }
    }

    fn record_failure(&mut self) {
        if self.is_half_open {
            // Failure in half-open state - go back to open
            self.is_half_open = false;
            self.half_open_success_count = 0;
            self.last_failure_time = Some(Instant::now());

            warn!("Circuit breaker reopened due to failure during recovery");
        } else {
            self.failure_count = self.failure_count.saturating_add(1);
            self.last_failure_time = Some(Instant::now());

            if self.failure_count >= self.failure_threshold && !self.is_open {
                self.is_open = true;

                // Record metrics
                if let Some(ref metrics) = self.metrics {
                    metrics.record_circuit_breaker_activation();
                }

                warn!(
                    failure_count = self.failure_count,
                    threshold = self.failure_threshold,
                    "Circuit breaker opened due to failure threshold"
                );
            }
        }
    }

    fn should_attempt_connection(&mut self) -> bool {
        if !self.is_open {
            return true;
        }

        if let Some(last_failure) = self.last_failure_time {
            if last_failure.elapsed() >= self.recovery_timeout && !self.is_half_open {
                // Enter half-open state
                self.is_half_open = true;
                self.half_open_success_count = 0;

                debug!("Circuit breaker entering half-open state for recovery testing");
                return true;
            }
        }

        false
    }

    #[allow(dead_code)]
    const fn is_open(&self) -> bool {
        self.is_open
    }

    #[allow(dead_code)]
    const fn is_half_open(&self) -> bool {
        self.is_half_open
    }

    /// Get circuit breaker state for monitoring
    pub const fn state(&self) -> CircuitBreakerState {
        if self.is_open {
            if self.is_half_open {
                CircuitBreakerState::HalfOpen
            } else {
                CircuitBreakerState::Open
            }
        } else {
            CircuitBreakerState::Closed
        }
    }
}

/// Circuit breaker state for external monitoring
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitBreakerState {
    /// Circuit breaker is closed (normal operation)
    Closed,
    /// Circuit breaker is open (blocking requests)
    Open,
    /// Circuit breaker is half-open (testing recovery)
    HalfOpen,
}

/// Statistics for a specific endpoint
#[derive(Debug, Clone)]
pub struct EndpointStats {
    /// Endpoint identifier
    pub endpoint_id: String,
    /// Endpoint priority
    pub priority: u32,
    /// Whether endpoint is healthy
    pub is_healthy: bool,
    /// Current health score
    pub health_score: f64,
    /// Circuit breaker state
    pub circuit_breaker_state: CircuitBreakerState,
    /// Endpoint capabilities
    pub capabilities: Option<CollectionCapabilities>,
    /// Last successful operation
    pub last_success: Option<Instant>,
    /// Last failure
    pub last_failure: Option<Instant>,
}

/// Comprehensive client statistics
#[derive(Debug, Clone)]
pub struct ClientStats {
    /// Current load balancing strategy
    pub load_balancing_strategy: LoadBalancingStrategy,
    /// Total pooled connections
    pub total_pooled_connections: usize,
    /// Per-endpoint statistics
    pub endpoint_stats: Vec<EndpointStats>,
}

/// Collector endpoint configuration for load balancing and failover
///
/// Represents a single collector endpoint with health tracking and capability information.
/// Used by the resilient client for load balancing and failover decisions.
#[derive(Debug, Clone)]
pub struct CollectorEndpoint {
    /// Unique identifier for this collector
    pub id: String,
    /// Endpoint path (Unix socket path or Windows pipe name)
    pub endpoint_path: String,
    /// Priority for load balancing (lower = higher priority)
    pub priority: u32,
    /// Whether this endpoint is currently healthy
    pub is_healthy: bool,
    /// Last successful connection time
    pub last_success: Option<Instant>,
    /// Last failure time
    pub last_failure: Option<Instant>,
    /// Capabilities reported by this collector
    pub capabilities: Option<CollectionCapabilities>,
    /// Last time capabilities were negotiated
    pub last_capability_check: Option<Instant>,
}

impl CollectorEndpoint {
    /// Create a new collector endpoint
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this endpoint
    /// * `endpoint_path` - Path to the socket or pipe
    /// * `priority` - Priority for load balancing (lower = higher priority)
    pub const fn new(id: String, endpoint_path: String, priority: u32) -> Self {
        Self {
            id,
            endpoint_path,
            priority,
            is_healthy: true,
            last_success: None,
            last_failure: None,
            capabilities: None,
            last_capability_check: None,
        }
    }

    /// Mark endpoint as healthy after successful operation
    pub fn mark_healthy(&mut self) {
        self.is_healthy = true;
        self.last_success = Some(Instant::now());
    }

    /// Mark endpoint as unhealthy after failure
    pub fn mark_unhealthy(&mut self) {
        self.is_healthy = false;
        self.last_failure = Some(Instant::now());
    }

    /// Update endpoint capabilities
    pub fn update_capabilities(&mut self, capabilities: CollectionCapabilities) {
        self.capabilities = Some(capabilities);
        self.last_capability_check = Some(Instant::now());
    }

    /// Check if capabilities need to be refreshed
    pub fn needs_capability_refresh(&self, max_age: Duration) -> bool {
        self.last_capability_check
            .is_none_or(|last_check| last_check.elapsed() > max_age)
    }

    /// Check if this endpoint supports a specific task type
    pub fn supports_task_type(&self, task_type: TaskType) -> bool {
        let Some(ref caps) = self.capabilities else {
            return false; // Unknown capabilities, assume not supported
        };

        match task_type {
            TaskType::EnumerateProcesses
            | TaskType::CheckProcessHash
            | TaskType::MonitorProcessTree
            | TaskType::VerifyExecutable => caps
                .supported_domains
                .contains(&i32::from(MonitoringDomain::Process)),
            TaskType::MonitorNetworkConnections => caps
                .supported_domains
                .contains(&i32::from(MonitoringDomain::Network)),
            TaskType::TrackFileOperations => caps
                .supported_domains
                .contains(&i32::from(MonitoringDomain::Filesystem)),
            TaskType::CollectPerformanceMetrics => caps
                .supported_domains
                .contains(&i32::from(MonitoringDomain::Performance)),
        }
    }

    /// Check if this endpoint supports advanced capabilities
    pub fn supports_advanced_capability(&self, capability: &str) -> bool {
        let Some(ref caps) = self.capabilities else {
            return false;
        };

        let Some(ref advanced) = caps.advanced else {
            return false;
        };

        match capability {
            "kernel_level" => advanced.kernel_level,
            "realtime" => advanced.realtime,
            "system_wide" => advanced.system_wide,
            _ => false,
        }
    }

    /// Calculate health score for load balancing (higher = better)
    pub fn health_score(&self) -> f64 {
        if !self.is_healthy {
            return 0.0;
        }

        let base_score = 1.0 / f64::from(self.priority.max(1));

        // Boost score based on recent success
        self.last_success.map_or(base_score, |last_success| {
            let seconds_since_success = last_success.elapsed().as_secs_f64();
            let recency_boost = (-seconds_since_success / 300.0).exp(); // 5-minute decay
            base_score * (1.0 + recency_boost)
        })
    }
}

/// Connection pool entry with metadata
#[derive(Debug)]
#[allow(dead_code)] // Work in progress - some fields not yet used
struct PooledConnection {
    stream: LocalSocketStream,
    endpoint_id: String,
    created_at: Instant,
    last_used: Instant,
    use_count: u64,
}

#[allow(dead_code)] // Work in progress - some methods not yet used
impl PooledConnection {
    fn new(stream: LocalSocketStream, endpoint_id: String) -> Self {
        let now = Instant::now();
        Self {
            stream,
            endpoint_id,
            created_at: now,
            last_used: now,
            use_count: 0,
        }
    }

    fn mark_used(&mut self) {
        self.last_used = Instant::now();
        self.use_count = self.use_count.saturating_add(1);
    }

    fn is_stale(&self, max_idle_time: Duration, max_lifetime: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time || self.created_at.elapsed() > max_lifetime
    }
}

/// Advanced connection pool for managing multiple collector connections
#[derive(Debug)]
#[allow(dead_code)] // Work in progress - some fields not yet used
struct ConnectionPool {
    /// Available connections by endpoint ID
    connections: HashMap<String, VecDeque<PooledConnection>>,
    /// Maximum connections per endpoint
    max_connections_per_endpoint: usize,
    /// Maximum total connections
    #[allow(dead_code)]
    max_total_connections: usize,
    /// Connection idle timeout
    max_idle_time: Duration,
    /// Connection maximum lifetime
    max_lifetime: Duration,
    /// Semaphore to limit total connections
    connection_semaphore: Arc<Semaphore>,
}

#[allow(dead_code)] // Work in progress - some methods not yet used
impl ConnectionPool {
    fn new(max_connections_per_endpoint: usize, max_total_connections: usize) -> Self {
        Self {
            connections: HashMap::new(),
            max_connections_per_endpoint,
            max_total_connections,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            max_lifetime: Duration::from_secs(3600), // 1 hour
            connection_semaphore: Arc::new(Semaphore::new(max_total_connections)),
        }
    }

    /// Get a connection from the pool or None if not available
    fn get_connection(&mut self, endpoint_id: &str) -> Option<PooledConnection> {
        let endpoint_connections = self.connections.get_mut(endpoint_id)?;

        // Remove stale connections
        endpoint_connections.retain(|conn| !conn.is_stale(self.max_idle_time, self.max_lifetime));

        // Get the most recently used connection
        endpoint_connections.pop_back()
    }

    /// Return a connection to the pool
    fn return_connection(&mut self, mut connection: PooledConnection) {
        // Check if connection is still valid
        if connection.is_stale(self.max_idle_time, self.max_lifetime) {
            debug!(
                endpoint_id = %connection.endpoint_id,
                age_secs = connection.created_at.elapsed().as_secs(),
                idle_secs = connection.last_used.elapsed().as_secs(),
                "Discarding stale connection"
            );
            return;
        }

        let endpoint_connections = self
            .connections
            .entry(connection.endpoint_id.clone())
            .or_default();

        // Limit connections per endpoint
        if endpoint_connections.len() >= self.max_connections_per_endpoint {
            debug!(
                endpoint_id = %connection.endpoint_id,
                pool_size = endpoint_connections.len(),
                "Connection pool full for endpoint, discarding connection"
            );
            return;
        }

        connection.mark_used();
        endpoint_connections.push_back(connection);
    }

    /// Get total number of pooled connections
    fn total_connections(&self) -> usize {
        self.connections
            .values()
            .map(std::collections::VecDeque::len)
            .sum()
    }

    /// Clean up stale connections across all endpoints
    fn cleanup_stale_connections(&mut self) {
        let mut total_removed: usize = 0;

        for (endpoint_id, connections) in &mut self.connections {
            let initial_count = connections.len();
            connections.retain(|conn| !conn.is_stale(self.max_idle_time, self.max_lifetime));
            let removed_count = initial_count.saturating_sub(connections.len());

            if removed_count > 0 {
                debug!(
                    endpoint_id = %endpoint_id,
                    removed = removed_count,
                    remaining = connections.len(),
                    "Cleaned up stale connections"
                );
                total_removed = total_removed.saturating_add(removed_count);
            }
        }

        // Remove empty endpoint entries
        self.connections
            .retain(|_, connections| !connections.is_empty());

        if total_removed > 0 {
            info!(
                total_removed = total_removed,
                total_remaining = self.total_connections(),
                "Connection pool cleanup completed"
            );
        }
    }

    /// Get connection semaphore for limiting total connections
    fn connection_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.connection_semaphore)
    }
}

/// Load balancing strategy for multiple collectors
///
/// Defines how the client selects endpoints when multiple collectors are available.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadBalancingStrategy {
    /// Round-robin selection - cycles through endpoints in order
    RoundRobin,
    /// Weighted selection based on priority and health scores
    Weighted,
    /// Always prefer the highest priority healthy endpoint
    Priority,
}

/// Advanced IPC client with connection management, load balancing, and resilience
#[allow(dead_code)] // Work in progress - some fields not yet used
pub struct ResilientIpcClient {
    config: IpcConfig,
    codec: IpcCodec,
    /// Available collector endpoints
    endpoints: Arc<RwLock<Vec<CollectorEndpoint>>>,
    /// Per-endpoint circuit breakers
    circuit_breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
    /// Connection pool for all endpoints
    connection_pool: Arc<Mutex<ConnectionPool>>,
    /// Load balancing strategy
    load_balancing_strategy: LoadBalancingStrategy,
    /// Round-robin counter for load balancing
    round_robin_counter: Arc<AtomicU64>,
    /// Comprehensive metrics
    metrics: Arc<IpcClientMetrics>,
    /// Reconnection configuration
    max_reconnect_attempts: u32,
    base_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

#[allow(dead_code)] // Work in progress - some methods not yet used
impl ResilientIpcClient {
    /// Create a new resilient IPC client with a single endpoint
    pub fn new(config: &IpcConfig) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes);
        let metrics = Arc::new(IpcClientMetrics::new());

        // Create default endpoint from config (initially unhealthy until tested)
        let mut default_endpoint = CollectorEndpoint::new(
            "default".to_owned(),
            config.endpoint_path.clone(),
            1, // Default priority
        );
        default_endpoint.is_healthy = false; // Start as unhealthy until proven otherwise

        Self {
            config: config.clone(),
            codec,
            endpoints: Arc::new(RwLock::new(vec![default_endpoint])),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            connection_pool: Arc::new(Mutex::new(ConnectionPool::new(
                4,                      // max connections per endpoint
                config.max_connections, // max total connections
            ))),
            load_balancing_strategy: LoadBalancingStrategy::Weighted,
            round_robin_counter: Arc::new(AtomicU64::new(0)),
            metrics,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }

    /// Create a new resilient IPC client with multiple collector endpoints
    pub fn new_with_endpoints(
        config: &IpcConfig,
        endpoints: Vec<CollectorEndpoint>,
        load_balancing_strategy: LoadBalancingStrategy,
    ) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes);
        let metrics = Arc::new(IpcClientMetrics::new());

        assert!(
            !endpoints.is_empty(),
            "At least one collector endpoint must be provided"
        );

        Self {
            config: config.clone(),
            codec,
            endpoints: Arc::new(RwLock::new(endpoints)),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            connection_pool: Arc::new(Mutex::new(ConnectionPool::new(
                4,                      // max connections per endpoint
                config.max_connections, // max total connections
            ))),
            load_balancing_strategy,
            round_robin_counter: Arc::new(AtomicU64::new(0)),
            metrics,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }

    /// Add a new collector endpoint for load balancing and failover
    pub async fn add_endpoint(&self, endpoint: CollectorEndpoint) {
        {
            let mut endpoints = self.endpoints.write().await;

            // Check if endpoint already exists
            if endpoints.iter().any(|e| e.id == endpoint.id) {
                warn!(endpoint_id = %endpoint.id, "Endpoint already exists, skipping");
                return;
            }

            endpoints.push(endpoint.clone());
        };
        info!(
            endpoint_id = %endpoint.id,
            endpoint_path = %endpoint.endpoint_path,
            priority = endpoint.priority,
            "Added new collector endpoint"
        );
    }

    /// Remove a collector endpoint
    pub async fn remove_endpoint(&self, endpoint_id: &str) -> bool {
        let mut endpoints = self.endpoints.write().await;
        let initial_len = endpoints.len();

        endpoints.retain(|e| e.id != endpoint_id);

        if endpoints.len() < initial_len {
            info!(endpoint_id = %endpoint_id, "Removed collector endpoint");

            // Clean up circuit breaker
            let mut circuit_breakers = self.circuit_breakers.write().await;
            circuit_breakers.remove(endpoint_id);

            true
        } else {
            false
        }
    }

    /// Get metrics for monitoring
    pub fn metrics(&self) -> Arc<IpcClientMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Negotiate capabilities with a specific collector endpoint
    #[allow(clippy::wildcard_enum_match_arm)]
    pub async fn negotiate_capabilities(
        &self,
        endpoint_id: &str,
    ) -> IpcResult<CollectionCapabilities> {
        let endpoint = {
            let endpoints = self.endpoints.read().await;
            endpoints
                .iter()
                .find(|e| e.id == endpoint_id)
                .cloned()
                .ok_or_else(|| {
                    IpcError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Endpoint not found: {endpoint_id}"),
                    ))
                })?
        };

        // Establish connection for capability negotiation
        let name = Self::create_socket_name_for_endpoint(&endpoint.endpoint_path)?;
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);

        let stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .map_err(|_err| IpcError::ConnectionTimeout {
                endpoint: endpoint.endpoint_path.clone(),
            })?
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => IpcError::ServerNotFound {
                    endpoint: endpoint.endpoint_path.clone(),
                },
                std::io::ErrorKind::ConnectionRefused => IpcError::ConnectionRefused {
                    endpoint: endpoint.endpoint_path.clone(),
                },
                _ => IpcError::Io(err),
            })?;

        // Create a special detection task for capability negotiation
        let capability_task = DetectionTask {
            task_id: format!("capability-negotiation-{}", uuid::Uuid::new_v4()),
            task_type: i32::from(TaskType::EnumerateProcesses), // Use a basic task type for negotiation
            process_filter: None,
            hash_check: None,
            metadata: Some("capability_negotiation".to_owned()), // Special marker
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        // Send capability negotiation request using existing task mechanism
        let result = self.send_task_on_stream(stream, capability_task).await?;

        // Extract capabilities from the result metadata
        let capabilities = if result.success {
            // Parse capabilities from result or use default process capabilities
            CollectionCapabilities {
                supported_domains: vec![i32::from(MonitoringDomain::Process)],
                advanced: Some(crate::proto::AdvancedCapabilities {
                    kernel_level: false,
                    realtime: true,
                    system_wide: true,
                }),
            }
        } else {
            return Err(IpcError::Io(std::io::Error::other(format!(
                "Capability negotiation failed: {}",
                result
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_owned())
            ))));
        };

        // Update endpoint capabilities
        {
            let mut endpoints = self.endpoints.write().await;
            if let Some(endpoint_mut) = endpoints.iter_mut().find(|e| e.id == endpoint_id) {
                endpoint_mut.update_capabilities(capabilities.clone());
                info!(
                    endpoint_id = %endpoint_id,
                    supported_domains = ?capabilities.supported_domains,
                    "Updated endpoint capabilities"
                );
            }
        }

        Ok(capabilities)
    }

    /// Refresh capabilities for all endpoints that need it
    pub async fn refresh_capabilities(&self) -> IpcResult<()> {
        let capability_refresh_interval = Duration::from_secs(300); // 5 minutes
        let endpoints_to_refresh = {
            let endpoints = self.endpoints.read().await;
            endpoints
                .iter()
                .filter(|e| e.needs_capability_refresh(capability_refresh_interval))
                .map(|e| e.id.clone())
                .collect::<Vec<_>>()
        };

        for endpoint_id in endpoints_to_refresh {
            match self.negotiate_capabilities(&endpoint_id).await {
                Ok(capabilities) => {
                    debug!(
                        endpoint_id = %endpoint_id,
                        supported_domains = ?capabilities.supported_domains,
                        "Refreshed capabilities for endpoint"
                    );
                }
                Err(e) => {
                    warn!(
                        endpoint_id = %endpoint_id,
                        error = %e,
                        "Failed to refresh capabilities for endpoint"
                    );
                    // Mark endpoint as unhealthy if capability negotiation fails
                    self.update_endpoint_health(&endpoint_id, false).await;
                }
            }
        }

        Ok(())
    }

    /// Select the best endpoint for a specific task type
    pub async fn select_endpoint_for_task(&self, task_type: TaskType) -> Option<CollectorEndpoint> {
        let endpoints = self.endpoints.read().await;

        if endpoints.is_empty() {
            return None;
        }

        // Filter to endpoints that support the task type and are healthy
        let compatible_endpoints: Vec<&CollectorEndpoint> = endpoints
            .iter()
            .filter(|e| e.is_healthy && e.supports_task_type(task_type))
            .collect();

        if compatible_endpoints.is_empty() {
            // No compatible endpoints, try any healthy endpoint as fallback
            warn!(
                task_type = ?task_type,
                "No endpoints support task type, falling back to any healthy endpoint"
            );
            let result = endpoints
                .iter()
                .filter(|e| e.is_healthy)
                .min_by_key(|e| e.priority)
                .cloned();
            drop(endpoints);
            return result;
        }

        // Select based on load balancing strategy among compatible endpoints
        let result = match self.load_balancing_strategy {
            LoadBalancingStrategy::RoundRobin => {
                let counter = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
                #[allow(clippy::arithmetic_side_effects)]
                // Safe: wrapping_rem prevents overflow
                let index = usize::try_from(counter)
                    .unwrap_or(0)
                    .wrapping_rem(compatible_endpoints.len());
                compatible_endpoints.get(index).map(|&e| e.clone())
            }
            LoadBalancingStrategy::Weighted => {
                // Select based on health score (higher is better)
                compatible_endpoints
                    .iter()
                    .max_by(|a, b| {
                        a.health_score()
                            .partial_cmp(&b.health_score())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|&e| e.clone())
            }
            LoadBalancingStrategy::Priority => {
                // Select highest priority (lowest number) compatible endpoint
                compatible_endpoints
                    .iter()
                    .min_by_key(|e| e.priority)
                    .map(|&e| e.clone())
            }
        };
        drop(endpoints);
        result
    }

    /// Send a task with automatic endpoint selection based on task type
    pub async fn send_task_with_routing(&self, task: DetectionTask) -> IpcResult<DetectionResult> {
        // Refresh capabilities if needed
        if let Err(e) = self.refresh_capabilities().await {
            warn!(error = %e, "Failed to refresh capabilities, proceeding with cached data");
        }

        // Determine task type
        let task_type = TaskType::try_from(task.task_type).map_err(|_original_error| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid task type: {task_type}", task_type = task.task_type),
            ))
        })?;

        // Select appropriate endpoint
        let endpoint = self
            .select_endpoint_for_task(task_type)
            .await
            .ok_or_else(|| {
                IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("No endpoints available for task type: {task_type:?}"),
                ))
            })?;

        debug!(
            task_id = %task.task_id,
            task_type = ?task_type,
            endpoint_id = %endpoint.id,
            "Routing task to compatible endpoint"
        );

        // Send task to selected endpoint
        self.send_task_to_endpoint(task, &endpoint.id).await
    }

    /// Send a task to a specific endpoint
    pub async fn send_task_to_endpoint(
        &self,
        task: DetectionTask,
        endpoint_id: &str,
    ) -> IpcResult<DetectionResult> {
        let endpoint = {
            let endpoints = self.endpoints.read().await;
            endpoints
                .iter()
                .find(|e| e.id == endpoint_id)
                .cloned()
                .ok_or_else(|| {
                    IpcError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Endpoint not found: {endpoint_id}"),
                    ))
                })?
        };

        // Check circuit breaker
        let mut circuit_breaker = self.get_circuit_breaker(&endpoint.id).await;
        if !circuit_breaker.should_attempt_connection() {
            // Record task failure due to circuit breaker
            self.metrics.record_task_failed();
            return Err(IpcError::CircuitBreakerOpen);
        }

        // Record task attempt
        self.metrics.record_task_sent();

        // Try to get connection from pool first
        let pooled_stream = {
            let mut pool = self.connection_pool.lock().await;
            pool.get_connection(&endpoint.id)
        };

        let stream = if let Some(pooled_conn) = pooled_stream {
            debug!(endpoint_id = %endpoint.id, "Using pooled connection");
            pooled_conn.stream
        } else {
            debug!(endpoint_id = %endpoint.id, "Creating new connection");
            match self.establish_connection_to_endpoint(&endpoint).await {
                Ok(s) => s,
                Err(e) => {
                    // Connection failed: update breaker, health, and metrics
                    self.update_circuit_breaker(&endpoint.id, false).await;
                    self.update_endpoint_health(&endpoint.id, false).await;
                    self.metrics.record_task_failed();
                    return Err(e);
                }
            }
        };

        // Send task
        let result = self.send_task_on_stream(stream, task).await;
        let success = result.is_ok();

        // Update circuit breaker and endpoint health
        self.update_circuit_breaker(&endpoint.id, success).await;
        self.update_endpoint_health(&endpoint.id, success).await;

        // Record failure in metrics if the task failed at any point
        if !success {
            self.metrics.record_task_failed();
        }

        result
    }

    /// Send a task on an established stream
    async fn send_task_on_stream(
        &self,
        mut stream: LocalSocketStream,
        task: DetectionTask,
    ) -> IpcResult<DetectionResult> {
        let start_time = Instant::now();
        self.metrics.record_task_sent();

        // Send the task using the codec
        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);
        self.codec
            .write_message(&mut stream, &task, write_timeout)
            .await?;

        // Read the response using the codec
        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);
        let mut codec = IpcCodec::new(self.config.max_frame_bytes);
        let result: DetectionResult = codec.read_message(&mut stream, read_timeout).await?;

        let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.metrics.record_task_completed(duration_ms);

        debug!(
            task_id = %task.task_id,
            success = result.success,
            duration_ms = duration_ms,
            "Task completed"
        );

        Ok(result)
    }

    /// Get current endpoints (for monitoring and diagnostics)
    pub async fn get_endpoints(&self) -> Vec<CollectorEndpoint> {
        let endpoints = self.endpoints.read().await;
        endpoints.clone()
    }

    /// Update endpoint priority
    pub async fn update_endpoint_priority(&self, endpoint_id: &str, new_priority: u32) {
        let mut endpoints = self.endpoints.write().await;
        if let Some(endpoint) = endpoints.iter_mut().find(|e| e.id == endpoint_id) {
            endpoint.priority = new_priority;
            info!(
                endpoint_id = %endpoint_id,
                new_priority = new_priority,
                "Updated endpoint priority"
            );
        }
    }

    /// Get comprehensive client statistics
    pub async fn get_stats(&self) -> ClientStats {
        let endpoint_stats = {
            let endpoints = self.endpoints.read().await;
            endpoints
                .iter()
                .map(|e| EndpointStats {
                    endpoint_id: e.id.clone(),
                    priority: e.priority,
                    is_healthy: e.is_healthy,
                    health_score: e.health_score(),
                    circuit_breaker_state: {
                        // Simplified: Currently reporting Closed until full CB tracking is wired in stats
                        CircuitBreakerState::Closed
                    },
                    capabilities: e.capabilities.clone(),
                    last_success: e.last_success,
                    last_failure: e.last_failure,
                })
                .collect()
        };
        let pool = self.connection_pool.lock().await;

        ClientStats {
            load_balancing_strategy: self.load_balancing_strategy.clone(),
            total_pooled_connections: pool.total_connections(),
            endpoint_stats,
        }
    }

    /// Clean up stale connections
    pub async fn cleanup_connections(&self) {
        let mut pool = self.connection_pool.lock().await;
        pool.cleanup_stale_connections();
    }

    /// Send a task using the original method (for backward compatibility)
    pub async fn send_task(&self, task: DetectionTask) -> IpcResult<DetectionResult> {
        // Use the new routing method by default
        self.send_task_with_routing(task).await
    }

    /// Select the best endpoint based on load balancing strategy
    #[allow(clippy::significant_drop_tightening)] // More readable with current structure
    async fn select_endpoint(&self) -> Option<CollectorEndpoint> {
        let endpoints = self.endpoints.read().await;

        if endpoints.is_empty() {
            return None;
        }

        // Filter to healthy endpoints first
        let healthy_endpoints: Vec<&CollectorEndpoint> =
            endpoints.iter().filter(|e| e.is_healthy).collect();

        if healthy_endpoints.is_empty() {
            // No healthy endpoints, try the least recently failed one
            warn!("No healthy endpoints available, selecting least recently failed");
            return endpoints
                .iter()
                .min_by_key(|e| e.last_failure.unwrap_or_else(Instant::now))
                .cloned();
        }

        match self.load_balancing_strategy {
            LoadBalancingStrategy::RoundRobin => {
                let counter = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
                #[allow(clippy::arithmetic_side_effects)] // Safe: wrapping_rem prevents overflow
                let index = usize::try_from(counter)
                    .unwrap_or(0)
                    .wrapping_rem(healthy_endpoints.len());
                healthy_endpoints.get(index).map(|&e| e.clone())
            }
            LoadBalancingStrategy::Weighted => {
                // Select based on health score (higher is better)
                healthy_endpoints
                    .iter()
                    .max_by(|a, b| {
                        a.health_score()
                            .partial_cmp(&b.health_score())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|&e| e.clone())
            }
            LoadBalancingStrategy::Priority => {
                // Select highest priority (lowest number) healthy endpoint
                healthy_endpoints
                    .iter()
                    .min_by_key(|e| e.priority)
                    .map(|&e| e.clone())
            }
        }
    }

    /// Create socket name from endpoint path
    fn create_socket_name_for_endpoint(endpoint_path: &str) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            use std::path::Path;
            let path = Path::new(endpoint_path);
            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            endpoint_path
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Get or create circuit breaker for endpoint
    async fn get_circuit_breaker(&self, endpoint_id: &str) -> CircuitBreaker {
        let mut circuit_breakers = self.circuit_breakers.write().await;

        circuit_breakers
            .entry(endpoint_id.to_owned())
            .or_insert_with(|| {
                CircuitBreaker::new(
                    5,                       // failure threshold
                    Duration::from_secs(30), // recovery timeout
                    Some(Arc::clone(&self.metrics)),
                )
            })
            .clone()
    }

    /// Update circuit breaker state
    async fn update_circuit_breaker(&self, endpoint_id: &str, success: bool) {
        let mut circuit_breakers = self.circuit_breakers.write().await;

        if let Some(breaker) = circuit_breakers.get_mut(endpoint_id) {
            if success {
                breaker.record_success();
            } else {
                breaker.record_failure();
            }
        }
    }

    /// Mark endpoint as healthy or unhealthy
    async fn update_endpoint_health(&self, endpoint_id: &str, is_healthy: bool) {
        let mut endpoints = self.endpoints.write().await;

        if let Some(endpoint) = endpoints.iter_mut().find(|e| e.id == endpoint_id) {
            if is_healthy {
                endpoint.mark_healthy();
            } else {
                endpoint.mark_unhealthy();
            }
        }
    }

    /// Establish a new connection to a specific endpoint
    async fn establish_connection_to_endpoint(
        &self,
        endpoint: &CollectorEndpoint,
    ) -> IpcResult<LocalSocketStream> {
        let start_time = Instant::now();
        self.metrics.record_connection_attempt();

        // Check circuit breaker for this endpoint
        let mut circuit_breaker = self.get_circuit_breaker(&endpoint.id).await;
        if !circuit_breaker.should_attempt_connection() {
            return Err(IpcError::CircuitBreakerOpen);
        }

        // Acquire connection semaphore
        let semaphore = {
            let pool = self.connection_pool.lock().await;
            pool.connection_semaphore()
        };

        let _permit = semaphore.acquire().await.map_err(|_ignored| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "Connection pool exhausted",
            ))
        })?;

        let name = Self::create_socket_name_for_endpoint(&endpoint.endpoint_path)?;

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .map_err(|_err| IpcError::ConnectionTimeout {
                endpoint: endpoint.endpoint_path.clone(),
            })?
            .map_err(|err| {
                // Convert OS errors to user-friendly IPC errors
                #[allow(clippy::wildcard_enum_match_arm)]
                match err.kind() {
                    std::io::ErrorKind::NotFound => IpcError::ServerNotFound {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::ConnectionRefused => IpcError::ConnectionRefused {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    _ => IpcError::Io(err),
                }
            })?;

        let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.metrics.record_connection_established(duration_ms);

        debug!(
            endpoint_id = %endpoint.id,
            endpoint_path = %endpoint.endpoint_path,
            duration_ms = duration_ms,
            "Connection established to endpoint"
        );

        Ok(stream)
    }

    /// Establish a new connection to the selected endpoint
    async fn establish_connection(&self) -> IpcResult<(LocalSocketStream, String)> {
        let start_time = Instant::now();
        self.metrics.record_connection_attempt();

        // Select best endpoint
        let endpoint = self.select_endpoint().await.ok_or_else(|| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No collector endpoints available",
            ))
        })?;

        // Check circuit breaker for this endpoint
        let mut circuit_breaker = self.get_circuit_breaker(&endpoint.id).await;
        if !circuit_breaker.should_attempt_connection() {
            return Err(IpcError::CircuitBreakerOpen);
        }

        // Acquire connection semaphore
        let semaphore = {
            let pool = self.connection_pool.lock().await;
            pool.connection_semaphore()
        };

        let _permit = semaphore.acquire().await.map_err(|_ignored| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "Connection pool exhausted",
            ))
        })?;

        let name = Self::create_socket_name_for_endpoint(&endpoint.endpoint_path)?;

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .map_err(|_err| IpcError::ConnectionTimeout {
                endpoint: endpoint.endpoint_path.clone(),
            })?
            .map_err(|err| {
                // Convert OS errors to user-friendly IPC errors
                #[allow(clippy::wildcard_enum_match_arm)]
                match err.kind() {
                    std::io::ErrorKind::NotFound => IpcError::ServerNotFound {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::ConnectionRefused => IpcError::ConnectionRefused {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::PermissionDenied => IpcError::PermissionDenied {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::TimedOut => IpcError::ConnectionTimeout {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    _ => IpcError::Io(err),
                }
            })?;

        // Record successful connection
        let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(0);
        self.metrics.record_connection_established(duration_ms);
        self.update_circuit_breaker(&endpoint.id, true).await;
        self.update_endpoint_health(&endpoint.id, true).await;

        debug!(
            endpoint_id = %endpoint.id,
            endpoint_path = %endpoint.endpoint_path,
            duration_ms = duration_ms,
            "Successfully established IPC connection"
        );

        Ok((stream, endpoint.id))
    }

    /// Get a connection from the pool or create a new one
    async fn get_connection(&self) -> IpcResult<(LocalSocketStream, String)> {
        // Try to get from pool first
        let endpoint = self.select_endpoint().await.ok_or_else(|| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No collector endpoints available",
            ))
        })?;

        {
            let mut pool = self.connection_pool.lock().await;
            if let Some(pooled_conn) = pool.get_connection(&endpoint.id) {
                debug!(
                    endpoint_id = %endpoint.id,
                    use_count = pooled_conn.use_count,
                    "Reusing pooled connection"
                );
                return Ok((pooled_conn.stream, pooled_conn.endpoint_id));
            }
        }

        // No pooled connection available, create new one
        self.establish_connection().await
    }

    /// Release a connection back to the pool
    async fn release_connection(&self, stream: LocalSocketStream, endpoint_id: String) {
        let pooled_conn = PooledConnection::new(stream, endpoint_id);

        {
            let mut pool = self.connection_pool.lock().await;
            pool.return_connection(pooled_conn);

            // Update metrics
            self.metrics
                .set_pooled_connections(u64::try_from(pool.total_connections()).unwrap_or(0));
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::str_to_string)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (IpcConfig, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: crate::ipc::TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            panic_strategy: crate::ipc::PanicStrategy::Unwind,
        };

        (config, temp_dir)
    }

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-resilient-client.sock")
                .to_string_lossy()
                .to_string()
        }
        #[cfg(windows)]
        {
            let dir_name = temp_dir
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("test");
            format!(r"\\.\pipe\daemoneye\test-resilient-{dir_name}")
        }
    }

    #[test]
    fn test_client_creation() {
        let (config, _temp_dir) = create_test_config();
        let _client = ResilientIpcClient::new(&config);
        // Client creation always succeeds
    }

    #[test]
    fn test_endpoint_capability_support() {
        // Create an endpoint with process capabilities
        let mut endpoint = CollectorEndpoint::new(
            "process-collector".to_string(),
            "/tmp/process-collector.sock".to_string(),
            1,
        );

        // Initially, endpoint should not support any tasks (no capabilities)
        assert!(!endpoint.supports_task_type(TaskType::EnumerateProcesses));
        assert!(!endpoint.supports_task_type(TaskType::MonitorNetworkConnections));

        // Update with process capabilities
        let process_capabilities = CollectionCapabilities {
            supported_domains: vec![i32::from(MonitoringDomain::Process)],
            advanced: Some(crate::proto::AdvancedCapabilities {
                kernel_level: false,
                realtime: true,
                system_wide: true,
            }),
        };

        endpoint.update_capabilities(process_capabilities);

        // Now it should support process tasks but not network tasks
        assert!(endpoint.supports_task_type(TaskType::EnumerateProcesses));
        assert!(endpoint.supports_task_type(TaskType::CheckProcessHash));
        assert!(endpoint.supports_task_type(TaskType::MonitorProcessTree));
        assert!(endpoint.supports_task_type(TaskType::VerifyExecutable));
        assert!(!endpoint.supports_task_type(TaskType::MonitorNetworkConnections));
        assert!(!endpoint.supports_task_type(TaskType::TrackFileOperations));
        assert!(!endpoint.supports_task_type(TaskType::CollectPerformanceMetrics));

        // Test advanced capabilities
        assert!(!endpoint.supports_advanced_capability("kernel_level"));
        assert!(endpoint.supports_advanced_capability("realtime"));
        assert!(endpoint.supports_advanced_capability("system_wide"));
        assert!(!endpoint.supports_advanced_capability("unknown_capability"));
    }
}
