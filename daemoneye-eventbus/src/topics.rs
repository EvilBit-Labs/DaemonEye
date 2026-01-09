//! Complete topic hierarchy for DaemonEye EventBus
//!
//! This module defines the complete topic hierarchy as designed in the architecture,
//! including event topics, control topics, and health monitoring topics.
//!
//! # Topic Hierarchy
//!
//! ## Event Topics (Data Flow)
//! - `events.process.*` - Process monitoring events
//! - `events.network.*` - Network monitoring events (future)
//! - `events.filesystem.*` - Filesystem monitoring events (future)
//! - `events.performance.*` - Performance monitoring events (future)
//!
//! ## Control Topics (Management Flow)
//! - `control.collector.*` - Collector lifecycle and configuration
//! - `control.agent.*` - Agent orchestration
//! - `control.health.*` - Health monitoring and diagnostics
//!
//! # Wildcard Support
//!
//! Topics support two types of wildcards:
//! - `+` - Single-level wildcard (matches exactly one segment)
//! - `#` - Multi-level wildcard (matches zero or more segments)
//!
//! # Access Control
//!
//! Topics have three access levels:
//! - Public: Accessible to all components
//! - Restricted: Component-specific access
//! - Privileged: Requires authentication

use crate::topic::{Topic, TopicAccessLevel, TopicError, TopicPattern, TopicRegistry};

/// Event topics for process monitoring
pub mod process {
    /// Process lifecycle events (start, stop, exit)
    pub const LIFECYCLE: &str = "events.process.lifecycle";

    /// Process metadata updates (CPU, memory, etc.)
    pub const METADATA: &str = "events.process.metadata";

    /// Parent-child relationship changes
    pub const TREE: &str = "events.process.tree";

    /// Hash verification and integrity checks
    pub const INTEGRITY: &str = "events.process.integrity";

    /// Behavioral anomalies and suspicious patterns
    pub const ANOMALY: &str = "events.process.anomaly";

    /// Bulk process enumeration results
    pub const BATCH: &str = "events.process.batch";

    /// Wildcard pattern for all process events
    pub const ALL: &str = "events.process.#";

    /// Get all process event topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![LIFECYCLE, METADATA, TREE, INTEGRITY, ANOMALY, BATCH]
    }
}

/// Event topics for network monitoring (future extension)
pub mod network {
    /// Network connection events
    pub const CONNECTIONS: &str = "events.network.connections";

    /// DNS query events
    pub const DNS: &str = "events.network.dns";

    /// Network traffic analysis
    pub const TRAFFIC: &str = "events.network.traffic";

    /// Network anomaly detection
    pub const ANOMALY: &str = "events.network.anomaly";

    /// Wildcard pattern for all network events
    pub const ALL: &str = "events.network.#";

    /// Get all network event topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![CONNECTIONS, DNS, TRAFFIC, ANOMALY]
    }
}

/// Event topics for filesystem monitoring (future extension)
pub mod filesystem {
    /// File creation, modification, deletion events
    pub const OPERATIONS: &str = "events.filesystem.operations";

    /// File access pattern tracking
    pub const ACCESS: &str = "events.filesystem.access";

    /// Bulk file operation detection
    pub const BULK: &str = "events.filesystem.bulk";

    /// Filesystem anomaly detection
    pub const ANOMALY: &str = "events.filesystem.anomaly";

    /// Wildcard pattern for all filesystem events
    pub const ALL: &str = "events.filesystem.#";

    /// Get all filesystem event topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![OPERATIONS, ACCESS, BULK, ANOMALY]
    }
}

/// Event topics for performance monitoring (future extension)
pub mod performance {
    /// Resource utilization metrics
    pub const UTILIZATION: &str = "events.performance.utilization";

    /// System-wide performance metrics
    pub const SYSTEM: &str = "events.performance.system";

    /// Performance anomaly detection
    pub const ANOMALY: &str = "events.performance.anomaly";

    /// Wildcard pattern for all performance events
    pub const ALL: &str = "events.performance.#";

    /// Get all performance event topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![UTILIZATION, SYSTEM, ANOMALY]
    }
}

/// Control topics for collector management
pub mod collector {
    /// Collector lifecycle management (start, stop, restart)
    pub const LIFECYCLE: &str = "control.collector.lifecycle";

    /// Configuration updates and reloads
    pub const CONFIG: &str = "control.collector.config";

    /// Task assignment and distribution
    pub const TASK: &str = "control.collector.task";

    /// Collector registration and capability advertisement
    pub const REGISTRATION: &str = "control.collector.registration";

    /// Wildcard pattern for all collector control messages
    pub const ALL: &str = "control.collector.#";

    /// Get all collector control topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![LIFECYCLE, CONFIG, TASK, REGISTRATION]
    }

    /// Build a collector-specific task topic
    ///
    /// # Arguments
    ///
    /// * `collector_type` - The type of collector (e.g., "procmond", "netmond")
    /// * `collector_id` - The unique identifier for the collector instance
    ///
    /// # Returns
    ///
    /// A topic string in the format: `control.collector.task.{collector_type}.{collector_id}`
    ///
    /// # Example
    ///
    /// ```
    /// use daemoneye_eventbus::topics::collector;
    ///
    /// let topic = collector::task_topic("procmond", "procmond-1");
    /// assert_eq!(topic, "control.collector.task.procmond.procmond-1");
    /// ```
    pub fn task_topic(collector_type: &str, collector_id: &str) -> String {
        format!("{}.{}.{}", TASK, collector_type, collector_id)
    }

    /// Build a collector-specific lifecycle topic for RPC operations
    ///
    /// # Arguments
    ///
    /// * `collector_id` - The unique identifier for the collector instance
    ///
    /// # Returns
    ///
    /// A topic string in the format: `control.collector.{collector_id}`
    ///
    /// # Example
    ///
    /// ```
    /// use daemoneye_eventbus::topics::collector;
    ///
    /// let topic = collector::lifecycle_topic("procmond-1");
    /// assert_eq!(topic, "control.collector.procmond-1");
    /// ```
    pub fn lifecycle_topic(collector_id: &str) -> String {
        format!("control.collector.{}", collector_id)
    }

    /// Build a collector-specific config topic for RPC operations
    ///
    /// # Arguments
    ///
    /// * `collector_id` - The unique identifier for the collector instance
    ///
    /// # Returns
    ///
    /// A topic string in the format: `control.collector.config.{collector_id}`
    ///
    /// # Example
    ///
    /// ```
    /// use daemoneye_eventbus::topics::collector;
    ///
    /// let topic = collector::config_topic("procmond-1");
    /// assert_eq!(topic, "control.collector.config.procmond-1");
    /// ```
    pub fn config_topic(collector_id: &str) -> String {
        format!("{}.{}", CONFIG, collector_id)
    }
}

/// Control topics for agent orchestration
pub mod agent {
    /// Agent orchestration and coordination
    pub const ORCHESTRATION: &str = "control.agent.orchestration";

    /// Policy updates and enforcement
    pub const POLICY: &str = "control.agent.policy";

    /// Wildcard pattern for all agent control messages
    pub const ALL: &str = "control.agent.#";

    /// Get all agent control topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![ORCHESTRATION, POLICY]
    }
}

/// Control topics for health monitoring
pub mod health {
    /// Heartbeat messages for liveness checks
    pub const HEARTBEAT: &str = "control.health.heartbeat";

    /// Component status updates
    pub const STATUS: &str = "control.health.status";

    /// Diagnostic information exchange
    pub const DIAGNOSTICS: &str = "control.health.diagnostics";

    /// Wildcard pattern for all health messages
    pub const ALL: &str = "control.health.#";

    /// Get all health control topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![HEARTBEAT, STATUS, DIAGNOSTICS]
    }

    /// Build a collector-specific heartbeat topic
    ///
    /// # Arguments
    ///
    /// * `collector_id` - The unique identifier for the collector instance
    ///
    /// # Returns
    ///
    /// A topic string in the format: `control.health.heartbeat.{collector_id}`
    ///
    /// # Example
    ///
    /// ```
    /// use daemoneye_eventbus::topics::health;
    ///
    /// let topic = health::heartbeat_topic("procmond-1");
    /// assert_eq!(topic, "control.health.heartbeat.procmond-1");
    /// ```
    pub fn heartbeat_topic(collector_id: &str) -> String {
        format!("{}.{}", HEARTBEAT, collector_id)
    }
}

/// Control topics for shutdown coordination
pub mod shutdown {
    /// Base shutdown topic for graceful shutdown coordination
    pub const SHUTDOWN: &str = "control.shutdown";

    /// Wildcard pattern for all shutdown messages
    pub const ALL: &str = "control.shutdown.#";

    /// Get all shutdown control topics
    pub fn all_topics() -> Vec<&'static str> {
        vec![SHUTDOWN]
    }

    /// Build a collector-specific shutdown topic
    ///
    /// # Arguments
    ///
    /// * `collector_id` - The unique identifier for the collector instance
    ///
    /// # Returns
    ///
    /// A topic string in the format: `control.shutdown.{collector_id}`
    ///
    /// # Example
    ///
    /// ```
    /// use daemoneye_eventbus::topics::shutdown;
    ///
    /// let topic = shutdown::shutdown_topic("procmond-1");
    /// assert_eq!(topic, "control.shutdown.procmond-1");
    /// ```
    pub fn shutdown_topic(collector_id: &str) -> String {
        format!("{}.{}", SHUTDOWN, collector_id)
    }
}

/// Topic hierarchy utilities
pub struct TopicHierarchy;

impl TopicHierarchy {
    /// Get all event topics across all domains
    pub fn all_event_topics() -> Vec<&'static str> {
        let mut topics = Vec::with_capacity(17); // 6+4+4+3
        topics.extend(process::all_topics());
        topics.extend(network::all_topics());
        topics.extend(filesystem::all_topics());
        topics.extend(performance::all_topics());
        topics
    }

    /// Get all control topics across all domains
    pub fn all_control_topics() -> Vec<&'static str> {
        let mut topics = Vec::with_capacity(10); // 4+2+3+1
        topics.extend(collector::all_topics());
        topics.extend(agent::all_topics());
        topics.extend(health::all_topics());
        topics.extend(shutdown::all_topics());
        topics
    }

    /// Get all topics in the hierarchy
    pub fn all_topics() -> Vec<&'static str> {
        let mut topics = Self::all_event_topics();
        topics.extend(Self::all_control_topics());
        topics
    }

    /// Validate a topic against the hierarchy
    pub fn validate_topic(topic: &str) -> Result<Topic, TopicError> {
        Topic::new(topic)
    }

    /// Create a topic pattern with wildcard support
    pub fn create_pattern(pattern: &str) -> Result<TopicPattern, TopicError> {
        TopicPattern::new(pattern)
    }

    /// Initialize a topic registry with default access control
    pub fn initialize_registry() -> TopicRegistry {
        let mut registry = TopicRegistry::new();

        // Register public topics (broad access)
        for topic in health::all_topics() {
            let _ = registry.register_publisher("*", topic);
        }

        // Register anomaly topics as public (accessible to all components)
        let _ = registry.register_publisher("*", process::ANOMALY);
        let _ = registry.register_publisher("*", network::ANOMALY);
        let _ = registry.register_publisher("*", filesystem::ANOMALY);
        let _ = registry.register_publisher("*", performance::ANOMALY);

        // Register shutdown topics (privileged - daemoneye-agent only)
        // Base shutdown topic
        let _ = registry.register_publisher("daemoneye-agent", shutdown::SHUTDOWN);
        // Register wildcard pattern for collector-specific shutdown topics
        let _ = registry.register_publisher("daemoneye-agent", shutdown::ALL);

        // Register restricted topics (component-specific)
        for topic in process::all_topics() {
            let _ = registry.register_publisher("procmond", topic);
        }
        for topic in network::all_topics() {
            let _ = registry.register_publisher("netmond", topic);
        }
        for topic in filesystem::all_topics() {
            let _ = registry.register_publisher("fsmond", topic);
        }
        for topic in performance::all_topics() {
            let _ = registry.register_publisher("perfmond", topic);
        }

        // Register privileged topics (authenticated only)
        // Base collector control topics (lifecycle, config, registration)
        let _ = registry.register_publisher("daemoneye-agent", collector::LIFECYCLE);
        let _ = registry.register_publisher("daemoneye-agent", collector::CONFIG);
        let _ = registry.register_publisher("daemoneye-agent", collector::REGISTRATION);

        // Task distribution topics - daemoneye-agent can publish to base task topic
        // and to specific collector task topics (control.collector.task.*.*)
        let _ = registry.register_publisher("daemoneye-agent", collector::TASK);
        // Register wildcard pattern for collector-specific task topics
        // This allows daemoneye-agent to publish to control.collector.task.{type}.{id}
        let _ = registry.register_publisher("daemoneye-agent", "control.collector.task.#");

        for topic in agent::all_topics() {
            let _ = registry.register_publisher("daemoneye-agent", topic);
        }

        registry
    }

    /// Get access level for a topic
    pub fn get_access_level(topic: &str) -> TopicAccessLevel {
        // Health topics are public
        if topic.starts_with("control.health.") {
            return TopicAccessLevel::Public;
        }

        // Anomaly event topics are public (accessible to all components)
        if topic.ends_with(".anomaly") && topic.starts_with("events.") {
            return TopicAccessLevel::Public;
        }

        // Event topics are restricted (component-specific)
        if topic.starts_with("events.") {
            return TopicAccessLevel::Restricted;
        }

        // Collector lifecycle and config are privileged
        if topic.starts_with("control.collector.lifecycle")
            || topic.starts_with("control.collector.config")
            || topic.starts_with("control.agent.")
        {
            return TopicAccessLevel::Privileged;
        }

        // Shutdown topics are privileged (daemoneye-agent only)
        if topic.starts_with("control.shutdown") {
            return TopicAccessLevel::Privileged;
        }

        // Task distribution topics (base and collector-specific) are restricted
        // Base: control.collector.task
        // Collector-specific: control.collector.task.{collector_type}.{collector_id}
        if topic.starts_with("control.collector.task") {
            return TopicAccessLevel::Restricted;
        }

        // Collector registration is restricted
        if topic.starts_with("control.collector.registration") {
            return TopicAccessLevel::Restricted;
        }

        // Default to restricted access
        TopicAccessLevel::Restricted
    }

    /// Check if a topic is an event topic
    pub fn is_event_topic(topic: &str) -> bool {
        topic.starts_with("events.")
    }

    /// Check if a topic is a control topic
    pub fn is_control_topic(topic: &str) -> bool {
        topic.starts_with("control.")
    }

    /// Check if a topic is a health topic
    pub fn is_health_topic(topic: &str) -> bool {
        topic.starts_with("control.health.")
    }

    /// Get the domain of a topic (events, control, etc.)
    pub fn get_domain(topic: &str) -> Option<&str> {
        topic.split('.').next()
    }

    /// Get the subdomain of a topic (process, network, collector, etc.)
    pub fn get_subdomain(topic: &str) -> Option<&str> {
        let mut parts = topic.split('.');
        parts.next()?; // Skip domain
        parts.next()
    }

    /// Get the specific topic type (lifecycle, metadata, etc.)
    pub fn get_topic_type(topic: &str) -> Option<&str> {
        let mut parts = topic.split('.');
        parts.next()?; // Skip domain
        parts.next()?; // Skip subdomain
        parts.next()
    }
}

/// Topic builder for constructing topics programmatically
pub struct TopicBuilder {
    segments: Vec<String>,
}

impl TopicBuilder {
    /// Create a new topic builder
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Start building an event topic
    pub fn events() -> Self {
        Self {
            segments: vec!["events".to_string()],
        }
    }

    /// Start building a control topic
    pub fn control() -> Self {
        Self {
            segments: vec!["control".to_string()],
        }
    }

    /// Add a process subdomain
    pub fn process(mut self) -> Self {
        self.segments.push("process".to_string());
        self
    }

    /// Add a network subdomain
    pub fn network(mut self) -> Self {
        self.segments.push("network".to_string());
        self
    }

    /// Add a filesystem subdomain
    pub fn filesystem(mut self) -> Self {
        self.segments.push("filesystem".to_string());
        self
    }

    /// Add a performance subdomain
    pub fn performance(mut self) -> Self {
        self.segments.push("performance".to_string());
        self
    }

    /// Add a collector subdomain
    pub fn collector(mut self) -> Self {
        self.segments.push("collector".to_string());
        self
    }

    /// Add an agent subdomain
    pub fn agent(mut self) -> Self {
        self.segments.push("agent".to_string());
        self
    }

    /// Add a health subdomain
    pub fn health(mut self) -> Self {
        self.segments.push("health".to_string());
        self
    }

    /// Add a lifecycle topic type
    pub fn lifecycle(mut self) -> Self {
        self.segments.push("lifecycle".to_string());
        self
    }

    /// Add a metadata topic type
    pub fn metadata(mut self) -> Self {
        self.segments.push("metadata".to_string());
        self
    }

    /// Add a config topic type
    pub fn config(mut self) -> Self {
        self.segments.push("config".to_string());
        self
    }

    /// Add a task topic type
    pub fn task(mut self) -> Self {
        self.segments.push("task".to_string());
        self
    }

    /// Add a heartbeat topic type
    pub fn heartbeat(mut self) -> Self {
        self.segments.push("heartbeat".to_string());
        self
    }

    /// Add a status topic type
    pub fn status(mut self) -> Self {
        self.segments.push("status".to_string());
        self
    }

    /// Add a diagnostics topic type
    pub fn diagnostics(mut self) -> Self {
        self.segments.push("diagnostics".to_string());
        self
    }

    /// Add a tree topic type
    pub fn tree(mut self) -> Self {
        self.segments.push("tree".to_string());
        self
    }

    /// Add an integrity topic type
    pub fn integrity(mut self) -> Self {
        self.segments.push("integrity".to_string());
        self
    }

    /// Add an anomaly topic type
    pub fn anomaly(mut self) -> Self {
        self.segments.push("anomaly".to_string());
        self
    }

    /// Add a batch topic type
    pub fn batch(mut self) -> Self {
        self.segments.push("batch".to_string());
        self
    }

    /// Add a custom segment
    pub fn segment(mut self, segment: impl Into<String>) -> Self {
        self.segments.push(segment.into());
        self
    }

    /// Build the topic string
    pub fn build(self) -> String {
        self.segments.join(".")
    }

    /// Build and validate the topic
    pub fn build_validated(self) -> Result<Topic, TopicError> {
        let topic_str = self.build();
        Topic::new(&topic_str)
    }
}

impl Default for TopicBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_topics() {
        assert_eq!(process::LIFECYCLE, "events.process.lifecycle");
        assert_eq!(process::METADATA, "events.process.metadata");
        assert_eq!(process::TREE, "events.process.tree");
        assert_eq!(process::INTEGRITY, "events.process.integrity");
        assert_eq!(process::ANOMALY, "events.process.anomaly");
        assert_eq!(process::BATCH, "events.process.batch");
        assert_eq!(process::ALL, "events.process.#");

        let topics = process::all_topics();
        assert_eq!(topics.len(), 6);
    }

    #[test]
    fn test_network_topics() {
        assert_eq!(network::CONNECTIONS, "events.network.connections");
        assert_eq!(network::DNS, "events.network.dns");
        assert_eq!(network::TRAFFIC, "events.network.traffic");
        assert_eq!(network::ANOMALY, "events.network.anomaly");
        assert_eq!(network::ALL, "events.network.#");

        let topics = network::all_topics();
        assert_eq!(topics.len(), 4);
    }

    #[test]
    fn test_filesystem_topics() {
        assert_eq!(filesystem::OPERATIONS, "events.filesystem.operations");
        assert_eq!(filesystem::ACCESS, "events.filesystem.access");
        assert_eq!(filesystem::BULK, "events.filesystem.bulk");
        assert_eq!(filesystem::ANOMALY, "events.filesystem.anomaly");
        assert_eq!(filesystem::ALL, "events.filesystem.#");

        let topics = filesystem::all_topics();
        assert_eq!(topics.len(), 4);
    }

    #[test]
    fn test_performance_topics() {
        assert_eq!(performance::UTILIZATION, "events.performance.utilization");
        assert_eq!(performance::SYSTEM, "events.performance.system");
        assert_eq!(performance::ANOMALY, "events.performance.anomaly");
        assert_eq!(performance::ALL, "events.performance.#");

        let topics = performance::all_topics();
        assert_eq!(topics.len(), 3);
    }

    #[test]
    fn test_collector_topics() {
        assert_eq!(collector::LIFECYCLE, "control.collector.lifecycle");
        assert_eq!(collector::CONFIG, "control.collector.config");
        assert_eq!(collector::TASK, "control.collector.task");
        assert_eq!(collector::REGISTRATION, "control.collector.registration");
        assert_eq!(collector::ALL, "control.collector.#");

        let topics = collector::all_topics();
        assert_eq!(topics.len(), 4);
    }

    #[test]
    fn test_collector_task_topic() {
        // Test task topic builder with canonical namespace
        let topic = collector::task_topic("procmond", "procmond-1");
        assert_eq!(topic, "control.collector.task.procmond.procmond-1");

        let topic = collector::task_topic("netmond", "netmond-2");
        assert_eq!(topic, "control.collector.task.netmond.netmond-2");

        // Verify it uses the base TASK constant
        assert!(topic.starts_with(collector::TASK));
    }

    #[test]
    fn test_collector_lifecycle_topic() {
        // Test lifecycle topic builder for RPC operations
        let topic = collector::lifecycle_topic("procmond-1");
        assert_eq!(topic, "control.collector.procmond-1");

        let topic = collector::lifecycle_topic("netmond-2");
        assert_eq!(topic, "control.collector.netmond-2");
    }

    #[test]
    fn test_collector_config_topic() {
        // Test config topic builder for RPC operations
        let topic = collector::config_topic("procmond-1");
        assert_eq!(topic, "control.collector.config.procmond-1");

        let topic = collector::config_topic("netmond-2");
        assert_eq!(topic, "control.collector.config.netmond-2");

        // Verify it uses the base CONFIG constant
        assert!(topic.starts_with(collector::CONFIG));
    }

    #[test]
    fn test_agent_topics() {
        assert_eq!(agent::ORCHESTRATION, "control.agent.orchestration");
        assert_eq!(agent::POLICY, "control.agent.policy");
        assert_eq!(agent::ALL, "control.agent.#");

        let topics = agent::all_topics();
        assert_eq!(topics.len(), 2);
    }

    #[test]
    fn test_health_topics() {
        assert_eq!(health::HEARTBEAT, "control.health.heartbeat");
        assert_eq!(health::STATUS, "control.health.status");
        assert_eq!(health::DIAGNOSTICS, "control.health.diagnostics");
        assert_eq!(health::ALL, "control.health.#");

        let topics = health::all_topics();
        assert_eq!(topics.len(), 3);

        // Test heartbeat topic builder
        let topic = health::heartbeat_topic("procmond-1");
        assert_eq!(topic, "control.health.heartbeat.procmond-1");
    }

    #[test]
    fn test_shutdown_topics() {
        assert_eq!(shutdown::SHUTDOWN, "control.shutdown");
        assert_eq!(shutdown::ALL, "control.shutdown.#");

        let topics = shutdown::all_topics();
        assert_eq!(topics.len(), 1);

        // Test shutdown topic builder
        let topic = shutdown::shutdown_topic("procmond-1");
        assert_eq!(topic, "control.shutdown.procmond-1");
    }

    #[test]
    fn test_topic_hierarchy() {
        let event_topics = TopicHierarchy::all_event_topics();
        assert!(event_topics.len() >= 17); // At least 6+4+4+3

        let control_topics = TopicHierarchy::all_control_topics();
        assert!(control_topics.len() >= 9); // At least 4+2+3

        let all_topics = TopicHierarchy::all_topics();
        assert_eq!(all_topics.len(), event_topics.len() + control_topics.len());
    }

    #[test]
    fn test_topic_validation() {
        assert!(TopicHierarchy::validate_topic(process::LIFECYCLE).is_ok());
        assert!(TopicHierarchy::validate_topic(collector::CONFIG).is_ok());
        assert!(TopicHierarchy::validate_topic(health::HEARTBEAT).is_ok());

        // Invalid topics
        assert!(TopicHierarchy::validate_topic("").is_err());
        assert!(TopicHierarchy::validate_topic("invalid..topic").is_err());
    }

    #[test]
    fn test_topic_pattern_creation() {
        assert!(TopicHierarchy::create_pattern(process::ALL).is_ok());
        assert!(TopicHierarchy::create_pattern("events.+.lifecycle").is_ok());
        assert!(TopicHierarchy::create_pattern("control.#").is_ok());
    }

    #[test]
    fn test_access_levels() {
        assert_eq!(
            TopicHierarchy::get_access_level(health::HEARTBEAT),
            TopicAccessLevel::Public
        );
        assert_eq!(
            TopicHierarchy::get_access_level(process::LIFECYCLE),
            TopicAccessLevel::Restricted
        );
        assert_eq!(
            TopicHierarchy::get_access_level(collector::LIFECYCLE),
            TopicAccessLevel::Privileged
        );
        assert_eq!(
            TopicHierarchy::get_access_level(agent::POLICY),
            TopicAccessLevel::Privileged
        );
        // Test base task topic
        assert_eq!(
            TopicHierarchy::get_access_level(collector::TASK),
            TopicAccessLevel::Restricted
        );
        // Test collector-specific task topics
        assert_eq!(
            TopicHierarchy::get_access_level("control.collector.task.procmond.procmond-1"),
            TopicAccessLevel::Restricted
        );
        assert_eq!(
            TopicHierarchy::get_access_level("control.collector.task.netmond.netmond-2"),
            TopicAccessLevel::Restricted
        );
        // Test shutdown topics
        assert_eq!(
            TopicHierarchy::get_access_level(shutdown::SHUTDOWN),
            TopicAccessLevel::Privileged
        );
        assert_eq!(
            TopicHierarchy::get_access_level("control.shutdown.procmond-1"),
            TopicAccessLevel::Privileged
        );
        // Test anomaly topics are public
        assert_eq!(
            TopicHierarchy::get_access_level(process::ANOMALY),
            TopicAccessLevel::Public
        );
        assert_eq!(
            TopicHierarchy::get_access_level(network::ANOMALY),
            TopicAccessLevel::Public
        );
        assert_eq!(
            TopicHierarchy::get_access_level(filesystem::ANOMALY),
            TopicAccessLevel::Public
        );
        assert_eq!(
            TopicHierarchy::get_access_level(performance::ANOMALY),
            TopicAccessLevel::Public
        );
        // Test other event topics remain restricted
        assert_eq!(
            TopicHierarchy::get_access_level(process::METADATA),
            TopicAccessLevel::Restricted
        );
        assert_eq!(
            TopicHierarchy::get_access_level(network::CONNECTIONS),
            TopicAccessLevel::Restricted
        );
    }

    #[test]
    fn test_topic_classification() {
        assert!(TopicHierarchy::is_event_topic(process::LIFECYCLE));
        assert!(TopicHierarchy::is_event_topic(network::CONNECTIONS));
        assert!(!TopicHierarchy::is_event_topic(collector::LIFECYCLE));

        assert!(TopicHierarchy::is_control_topic(collector::LIFECYCLE));
        assert!(TopicHierarchy::is_control_topic(health::HEARTBEAT));
        assert!(!TopicHierarchy::is_control_topic(process::LIFECYCLE));

        assert!(TopicHierarchy::is_health_topic(health::HEARTBEAT));
        assert!(!TopicHierarchy::is_health_topic(collector::LIFECYCLE));
    }

    #[test]
    fn test_topic_parsing() {
        assert_eq!(
            TopicHierarchy::get_domain(process::LIFECYCLE),
            Some("events")
        );
        assert_eq!(
            TopicHierarchy::get_domain(collector::LIFECYCLE),
            Some("control")
        );

        assert_eq!(
            TopicHierarchy::get_subdomain(process::LIFECYCLE),
            Some("process")
        );
        assert_eq!(
            TopicHierarchy::get_subdomain(collector::LIFECYCLE),
            Some("collector")
        );

        assert_eq!(
            TopicHierarchy::get_topic_type(process::LIFECYCLE),
            Some("lifecycle")
        );
        assert_eq!(
            TopicHierarchy::get_topic_type(health::HEARTBEAT),
            Some("heartbeat")
        );
    }

    #[test]
    fn test_topic_builder() {
        let topic = TopicBuilder::events().process().lifecycle().build();
        assert_eq!(topic, process::LIFECYCLE);

        let topic = TopicBuilder::control().collector().config().build();
        assert_eq!(topic, collector::CONFIG);

        let topic = TopicBuilder::control().health().heartbeat().build();
        assert_eq!(topic, health::HEARTBEAT);

        let topic = TopicBuilder::events()
            .network()
            .segment("connections")
            .build();
        assert_eq!(topic, network::CONNECTIONS);
    }

    #[test]
    fn test_topic_builder_validation() {
        let result = TopicBuilder::events()
            .process()
            .lifecycle()
            .build_validated();
        assert!(result.is_ok());

        let result = TopicBuilder::control()
            .collector()
            .config()
            .build_validated();
        assert!(result.is_ok());
    }

    #[test]
    fn test_registry_initialization() {
        let registry = TopicHierarchy::initialize_registry();

        // Verify procmond can publish process events
        assert!(registry.can_publish("procmond", process::LIFECYCLE));
        assert!(registry.can_publish("procmond", process::METADATA));

        // Verify daemoneye-agent can publish control messages
        assert!(registry.can_publish("daemoneye-agent", collector::LIFECYCLE));
        assert!(registry.can_publish("daemoneye-agent", agent::ORCHESTRATION));

        // Verify daemoneye-agent can publish to base task topic
        assert!(registry.can_publish("daemoneye-agent", collector::TASK));

        // Verify daemoneye-agent can publish to collector-specific task topics
        assert!(registry.can_publish(
            "daemoneye-agent",
            "control.collector.task.procmond.procmond-1"
        ));
        assert!(registry.can_publish(
            "daemoneye-agent",
            "control.collector.task.netmond.netmond-2"
        ));

        // Verify anyone can publish health messages
        assert!(registry.can_publish("*", health::HEARTBEAT));

        // Verify daemoneye-agent can publish shutdown messages
        assert!(registry.can_publish("daemoneye-agent", shutdown::SHUTDOWN));
        assert!(registry.can_publish("daemoneye-agent", "control.shutdown.procmond-1"));

        // Verify anyone can publish anomaly events (public access)
        assert!(registry.can_publish("*", process::ANOMALY));
        assert!(registry.can_publish("*", network::ANOMALY));
        assert!(registry.can_publish("*", filesystem::ANOMALY));
        assert!(registry.can_publish("*", performance::ANOMALY));
        // Verify other event topics remain restricted
        assert!(!registry.can_publish("*", process::LIFECYCLE));
        assert!(!registry.can_publish("daemoneye-agent", process::METADATA));
    }

    #[test]
    fn test_wildcard_patterns() {
        let pattern = TopicHierarchy::create_pattern(process::ALL).unwrap();
        let topic = TopicHierarchy::validate_topic(process::LIFECYCLE).unwrap();
        assert!(pattern.matches(&topic));

        let pattern = TopicHierarchy::create_pattern("events.+.lifecycle").unwrap();
        let topic1 = TopicHierarchy::validate_topic(process::LIFECYCLE).unwrap();
        let topic2 = TopicHierarchy::validate_topic(network::CONNECTIONS).unwrap();
        assert!(pattern.matches(&topic1));
        assert!(!pattern.matches(&topic2));

        let pattern = TopicHierarchy::create_pattern("control.#").unwrap();
        let topic1 = TopicHierarchy::validate_topic(collector::LIFECYCLE).unwrap();
        let topic2 = TopicHierarchy::validate_topic(health::HEARTBEAT).unwrap();
        assert!(pattern.matches(&topic1));
        assert!(pattern.matches(&topic2));
    }
}
