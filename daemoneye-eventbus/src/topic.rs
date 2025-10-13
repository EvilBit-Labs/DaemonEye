use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

/// Topic hierarchy management for DaemonEye event bus
///
/// Provides structured topic naming, validation, and wildcard matching
/// for multi-collector communication patterns.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Topic {
    /// Full topic name (e.g., "events.process.lifecycle")
    name: String,
    /// Topic segments split by '.' (e.g., ["events", "process", "lifecycle"])
    segments: Vec<String>,
    /// Topic domain (events, control, system, debug)
    domain: TopicDomain,
    /// Topic type classification
    topic_type: TopicType,
}

/// Topic domain classification
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicDomain {
    /// Data flow topics (events.*)
    Events,
    /// Management flow topics (control.*)
    Control,
    /// System-level topics (system.*) - reserved for future use
    System,
    /// Debug and development topics (debug.*) - reserved for development
    Debug,
}

/// Topic type classification for access control and routing
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicType {
    /// Process-related topics
    Process,
    /// Network-related topics (future)
    Network,
    /// Filesystem-related topics (future)
    Filesystem,
    /// Performance-related topics (future)
    Performance,
    /// Collector management topics
    Collector,
    /// Agent orchestration topics
    Agent,
    /// Health monitoring topics
    Health,
    /// Generic/unknown topic type
    Generic,
}

/// Topic pattern for subscription matching with wildcards
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicPattern {
    /// Original pattern string
    pattern: String,
    /// Pattern segments with wildcard indicators
    segments: Vec<PatternSegment>,
}

/// Pattern segment types for wildcard matching
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PatternSegment {
    /// Exact match segment
    Literal(String),
    /// Single-level wildcard (+)
    SingleWildcard,
    /// Multi-level wildcard (#)
    MultiWildcard,
}

/// Topic validation and parsing errors
#[derive(Debug, thiserror::Error)]
pub enum TopicError {
    #[error("Invalid topic format: {0}")]
    InvalidFormat(String),
    #[error("Topic too deep: {0} levels (max 4)")]
    TooDeep(usize),
    #[error("Empty topic segment at position {0}")]
    EmptySegment(usize),
    #[error("Invalid character in topic: {0}")]
    InvalidCharacter(char),
    #[error("Reserved topic prefix: {0}")]
    ReservedPrefix(String),
    #[error("Unknown topic domain: {0}")]
    UnknownDomain(String),
}

impl Topic {
    /// Maximum topic depth (number of segments)
    pub const MAX_DEPTH: usize = 4;

    /// Reserved topic prefixes that cannot be used by applications
    pub const RESERVED_PREFIXES: &'static [&'static str] = &["system", "debug"];

    /// Create a new topic from a string
    pub fn new(name: &str) -> Result<Self, TopicError> {
        Self::validate_topic_name(name)?;

        let segments: Vec<String> = name.split('.').map(|s| s.to_string()).collect();

        if segments.len() > Self::MAX_DEPTH {
            return Err(TopicError::TooDeep(segments.len()));
        }

        let domain = Self::parse_domain(&segments[0])?;
        let topic_type = Self::parse_topic_type(&segments);

        Ok(Topic {
            name: name.to_string(),
            segments,
            domain,
            topic_type,
        })
    }

    /// Validate topic name format and characters
    fn validate_topic_name(name: &str) -> Result<(), TopicError> {
        if name.is_empty() {
            return Err(TopicError::InvalidFormat("empty topic name".to_string()));
        }

        // Check for reserved prefixes
        for prefix in Self::RESERVED_PREFIXES {
            if name.starts_with(prefix) {
                return Err(TopicError::ReservedPrefix(prefix.to_string()));
            }
        }

        // Validate characters and segments
        for (i, segment) in name.split('.').enumerate() {
            if segment.is_empty() {
                return Err(TopicError::EmptySegment(i));
            }

            // Allow lowercase letters, numbers, hyphens, and underscores
            for ch in segment.chars() {
                if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' && ch != '_' {
                    return Err(TopicError::InvalidCharacter(ch));
                }
            }
        }

        Ok(())
    }

    /// Parse topic domain from first segment
    fn parse_domain(first_segment: &str) -> Result<TopicDomain, TopicError> {
        match first_segment {
            "events" => Ok(TopicDomain::Events),
            "control" => Ok(TopicDomain::Control),
            "system" => Ok(TopicDomain::System),
            "debug" => Ok(TopicDomain::Debug),
            _ => Err(TopicError::UnknownDomain(first_segment.to_string())),
        }
    }

    /// Parse topic type from segments
    fn parse_topic_type(segments: &[String]) -> TopicType {
        if segments.len() < 2 {
            return TopicType::Generic;
        }

        match segments[1].as_str() {
            "process" => TopicType::Process,
            "network" => TopicType::Network,
            "filesystem" => TopicType::Filesystem,
            "performance" => TopicType::Performance,
            "collector" => TopicType::Collector,
            "agent" => TopicType::Agent,
            "health" => TopicType::Health,
            _ => TopicType::Generic,
        }
    }

    /// Get the full topic name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get topic segments
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Get topic domain
    pub fn domain(&self) -> &TopicDomain {
        &self.domain
    }

    /// Get topic type
    pub fn topic_type(&self) -> &TopicType {
        &self.topic_type
    }

    /// Check if this is an event topic
    pub fn is_event_topic(&self) -> bool {
        matches!(self.domain, TopicDomain::Events)
    }

    /// Check if this is a control topic
    pub fn is_control_topic(&self) -> bool {
        matches!(self.domain, TopicDomain::Control)
    }

    /// Get the topic depth (number of segments)
    pub fn depth(&self) -> usize {
        self.segments.len()
    }
}

impl TopicPattern {
    /// Create a new topic pattern from a string with wildcards
    pub fn new(pattern: &str) -> Result<Self, TopicError> {
        let segments: Vec<PatternSegment> = pattern
            .split('.')
            .map(|segment| match segment {
                "+" => PatternSegment::SingleWildcard,
                "#" => PatternSegment::MultiWildcard,
                s => PatternSegment::Literal(s.to_string()),
            })
            .collect();

        // Validate that multi-level wildcard is only at the end
        for (i, segment) in segments.iter().enumerate() {
            if matches!(segment, PatternSegment::MultiWildcard) && i != segments.len() - 1 {
                return Err(TopicError::InvalidFormat(
                    "Multi-level wildcard (#) must be at the end".to_string(),
                ));
            }
        }

        Ok(TopicPattern {
            pattern: pattern.to_string(),
            segments,
        })
    }

    /// Check if this pattern matches a topic
    pub fn matches(&self, topic: &Topic) -> bool {
        self.matches_segments(&topic.segments)
    }

    /// Check if pattern matches topic segments
    fn matches_segments(&self, topic_segments: &[String]) -> bool {
        let mut pattern_idx = 0;
        let mut topic_idx = 0;

        while pattern_idx < self.segments.len() && topic_idx < topic_segments.len() {
            match &self.segments[pattern_idx] {
                PatternSegment::Literal(literal) => {
                    if literal != &topic_segments[topic_idx] {
                        return false;
                    }
                    pattern_idx += 1;
                    topic_idx += 1;
                }
                PatternSegment::SingleWildcard => {
                    // Single wildcard matches exactly one segment
                    pattern_idx += 1;
                    topic_idx += 1;
                }
                PatternSegment::MultiWildcard => {
                    // Multi-level wildcard matches remaining segments
                    return true;
                }
            }
        }

        // Check if we consumed all pattern segments and topic segments
        pattern_idx == self.segments.len() && topic_idx == topic_segments.len()
    }

    /// Get the original pattern string
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl FromStr for Topic {
    type Err = TopicError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Topic::new(s)
    }
}

impl FromStr for TopicPattern {
    type Err = TopicError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TopicPattern::new(s)
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl fmt::Display for TopicPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pattern)
    }
}

/// Topic matcher for routing messages to subscribers
#[derive(Debug, Default)]
pub struct TopicMatcher {
    /// Subscriptions mapped by subscriber ID
    subscriptions: HashMap<String, Vec<TopicPattern>>,
    /// Topic statistics
    topic_stats: HashMap<String, TopicStats>,
}

/// Statistics for topic usage
#[derive(Debug, Default, Clone)]
pub struct TopicStats {
    /// Number of messages published to this topic
    pub messages_published: u64,
    /// Number of messages delivered from this topic
    pub messages_delivered: u64,
    /// Number of active subscribers
    pub active_subscribers: usize,
}

impl TopicMatcher {
    /// Create a new topic matcher
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a subscription for a subscriber
    pub fn add_subscription(
        &mut self,
        subscriber_id: &str,
        pattern: &str,
    ) -> Result<(), TopicError> {
        let topic_pattern = TopicPattern::new(pattern)?;
        self.subscriptions
            .entry(subscriber_id.to_string())
            .or_default()
            .push(topic_pattern);
        Ok(())
    }

    /// Remove all subscriptions for a subscriber
    pub fn remove_subscriber(&mut self, subscriber_id: &str) {
        self.subscriptions.remove(subscriber_id);
    }

    /// Get all subscribers that match a topic
    pub fn get_matching_subscribers(&self, topic: &str) -> Result<Vec<String>, TopicError> {
        let topic_obj = Topic::new(topic)?;
        let mut matching_subscribers = Vec::new();

        for (subscriber_id, patterns) in &self.subscriptions {
            for pattern in patterns {
                if pattern.matches(&topic_obj) {
                    matching_subscribers.push(subscriber_id.clone());
                    break; // One match per subscriber is enough
                }
            }
        }

        Ok(matching_subscribers)
    }

    /// Record a message publication
    pub fn record_publication(&mut self, topic: &str) {
        let stats = self.topic_stats.entry(topic.to_string()).or_default();
        stats.messages_published += 1;
    }

    /// Record a message delivery
    pub fn record_delivery(&mut self, topic: &str) {
        let stats = self.topic_stats.entry(topic.to_string()).or_default();
        stats.messages_delivered += 1;
    }

    /// Get statistics for a topic
    pub fn get_topic_stats(&self, topic: &str) -> Option<&TopicStats> {
        self.topic_stats.get(topic)
    }

    /// Get all topic statistics
    pub fn get_all_stats(&self) -> &HashMap<String, TopicStats> {
        &self.topic_stats
    }

    /// Get number of active subscribers
    pub fn get_subscriber_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Get number of active topics
    pub fn get_topic_count(&self) -> usize {
        self.topic_stats.len()
    }

    /// Find subscribers for a topic (alias for get_matching_subscribers)
    pub fn find_subscribers(&self, topic: &str) -> Result<Vec<String>, TopicError> {
        self.get_matching_subscribers(topic)
    }

    /// Subscribe a subscriber to a pattern (alias for add_subscription)
    pub fn subscribe(
        &mut self,
        pattern: &str,
        subscriber_id: impl ToString,
    ) -> Result<(), TopicError> {
        self.add_subscription(&subscriber_id.to_string(), pattern)
    }

    /// Unsubscribe a subscriber (alias for remove_subscriber)
    pub fn unsubscribe(&mut self, subscriber_id: impl ToString) -> Result<(), TopicError> {
        self.remove_subscriber(&subscriber_id.to_string());
        Ok(())
    }

    /// Get subscriber count (alias for get_subscriber_count)
    pub fn subscriber_count(&self) -> usize {
        self.get_subscriber_count()
    }

    /// Get pattern count (alias for get_topic_count)
    pub fn pattern_count(&self) -> usize {
        self.get_topic_count()
    }
}

/// Topic registry for managing component topic usage
#[derive(Debug, Default)]
pub struct TopicRegistry {
    /// Topics that components can publish to
    publishers: HashMap<String, HashSet<String>>,
    /// Topic patterns that components subscribe to
    subscribers: HashMap<String, HashSet<TopicPattern>>,
    /// Security boundaries for topic access
    access_control: HashMap<String, TopicAccessLevel>,
}

/// Access control levels for topics
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicAccessLevel {
    /// Public topics accessible to all components
    Public,
    /// Restricted topics for specific components
    Restricted,
    /// Privileged topics requiring authentication
    Privileged,
}

impl TopicRegistry {
    /// Create a new topic registry
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.initialize_default_access_control();
        registry
    }

    /// Initialize default access control rules
    fn initialize_default_access_control(&mut self) {
        // Public topics (broad access)
        self.access_control
            .insert("control.health.+".to_string(), TopicAccessLevel::Public);
        self.access_control
            .insert("events.+.anomaly".to_string(), TopicAccessLevel::Public);
        self.access_control
            .insert("events.+.security".to_string(), TopicAccessLevel::Public);

        // Restricted topics (component-specific)
        self.access_control.insert(
            "control.collector.config".to_string(),
            TopicAccessLevel::Restricted,
        );
        self.access_control.insert(
            "control.agent.orchestration".to_string(),
            TopicAccessLevel::Restricted,
        );
        self.access_control.insert(
            "events.+.metadata".to_string(),
            TopicAccessLevel::Restricted,
        );

        // Privileged topics (authenticated only)
        self.access_control.insert(
            "control.collector.lifecycle".to_string(),
            TopicAccessLevel::Privileged,
        );
        self.access_control.insert(
            "control.agent.policy".to_string(),
            TopicAccessLevel::Privileged,
        );
        self.access_control
            .insert("control.+.config".to_string(), TopicAccessLevel::Privileged);
    }

    /// Register a component as a publisher for a topic
    pub fn register_publisher(&mut self, component: &str, topic: &str) -> Result<(), TopicError> {
        let topic = Topic::new(topic)?;
        self.publishers
            .entry(component.to_string())
            .or_default()
            .insert(topic.name().to_string());
        Ok(())
    }

    /// Register a component as a subscriber for a topic pattern
    pub fn register_subscriber(
        &mut self,
        component: &str,
        pattern: &str,
    ) -> Result<(), TopicError> {
        let pattern = TopicPattern::new(pattern)?;
        self.subscribers
            .entry(component.to_string())
            .or_default()
            .insert(pattern);
        Ok(())
    }

    /// Check if a component can publish to a topic
    pub fn can_publish(&self, component: &str, topic: &str) -> bool {
        self.publishers
            .get(component)
            .map(|topics| topics.contains(topic))
            .unwrap_or(false)
    }

    /// Check if a component can subscribe to a topic
    pub fn can_subscribe(&self, component: &str, topic: &Topic) -> bool {
        if let Some(patterns) = self.subscribers.get(component) {
            patterns.iter().any(|pattern| pattern.matches(topic))
        } else {
            false
        }
    }

    /// Get access level for a topic
    pub fn get_access_level(&self, topic: &str) -> TopicAccessLevel {
        // Check for exact matches first
        if let Some(level) = self.access_control.get(topic) {
            return level.clone();
        }

        // Check for pattern matches
        for (pattern_str, level) in &self.access_control {
            if let Ok(pattern) = TopicPattern::new(pattern_str)
                && let Ok(topic_obj) = Topic::new(topic)
                && pattern.matches(&topic_obj)
            {
                return level.clone();
            }
        }

        // Default to restricted access
        TopicAccessLevel::Restricted
    }

    /// Get all publishers for a component
    pub fn get_publishers(&self, component: &str) -> Option<&HashSet<String>> {
        self.publishers.get(component)
    }

    /// Get all subscribers for a component
    pub fn get_subscribers(&self, component: &str) -> Option<&HashSet<TopicPattern>> {
        self.subscribers.get(component)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_creation() {
        let topic = Topic::new("events.process.lifecycle").unwrap();
        assert_eq!(topic.name(), "events.process.lifecycle");
        assert_eq!(topic.segments(), &["events", "process", "lifecycle"]);
        assert_eq!(topic.domain(), &TopicDomain::Events);
        assert_eq!(topic.topic_type(), &TopicType::Process);
        assert!(topic.is_event_topic());
        assert!(!topic.is_control_topic());
    }

    #[test]
    fn test_topic_validation() {
        // Valid topics
        assert!(Topic::new("events.process.lifecycle").is_ok());
        assert!(Topic::new("control.collector.status").is_ok());
        assert!(Topic::new("events.network.connections").is_ok());

        // Invalid topics
        assert!(Topic::new("").is_err());
        assert!(Topic::new("events..lifecycle").is_err());
        assert!(Topic::new("events.Process.lifecycle").is_err()); // uppercase
        assert!(Topic::new("system.reserved").is_err()); // reserved prefix
        assert!(Topic::new("events.process.lifecycle.too.deep").is_err()); // too deep
    }

    #[test]
    fn test_topic_pattern_matching() {
        let pattern = TopicPattern::new("events.process.+").unwrap();

        let topic1 = Topic::new("events.process.lifecycle").unwrap();
        let topic2 = Topic::new("events.process.metadata").unwrap();
        let topic3 = Topic::new("events.network.connections").unwrap();
        let topic4 = Topic::new("events.process.lifecycle.extra").unwrap();

        assert!(pattern.matches(&topic1));
        assert!(pattern.matches(&topic2));
        assert!(!pattern.matches(&topic3));
        assert!(!pattern.matches(&topic4));
    }

    #[test]
    fn test_multi_level_wildcard() {
        let pattern = TopicPattern::new("events.#").unwrap();

        let topic1 = Topic::new("events.process.lifecycle").unwrap();
        let topic2 = Topic::new("events.network.connections").unwrap();
        let topic3 = Topic::new("control.collector.status").unwrap();

        assert!(pattern.matches(&topic1));
        assert!(pattern.matches(&topic2));
        assert!(!pattern.matches(&topic3));
    }

    #[test]
    fn test_topic_registry() {
        let mut registry = TopicRegistry::new();

        // Register publishers and subscribers
        registry
            .register_publisher("procmond", "events.process.lifecycle")
            .unwrap();
        registry
            .register_subscriber("daemoneye-agent", "events.#")
            .unwrap();

        // Test permissions
        assert!(registry.can_publish("procmond", "events.process.lifecycle"));
        assert!(!registry.can_publish("procmond", "events.network.connections"));

        let topic = Topic::new("events.process.lifecycle").unwrap();
        assert!(registry.can_subscribe("daemoneye-agent", &topic));
    }

    #[test]
    fn test_access_control() {
        let registry = TopicRegistry::new();

        // Test default access levels
        assert_eq!(
            registry.get_access_level("control.health.status"),
            TopicAccessLevel::Public
        );
        assert_eq!(
            registry.get_access_level("control.collector.config"),
            TopicAccessLevel::Restricted
        );
        assert_eq!(
            registry.get_access_level("control.collector.lifecycle"),
            TopicAccessLevel::Privileged
        );
    }

    #[test]
    fn test_topic_matcher() {
        let mut matcher = TopicMatcher::new();

        // Add subscriptions
        matcher
            .add_subscription("subscriber1", "events.process.+")
            .unwrap();
        matcher.add_subscription("subscriber2", "events.#").unwrap();
        matcher
            .add_subscription("subscriber3", "control.collector.status")
            .unwrap();

        // Test matching
        let matches1 = matcher
            .get_matching_subscribers("events.process.lifecycle")
            .unwrap();
        assert!(matches1.contains(&"subscriber1".to_string()));
        assert!(matches1.contains(&"subscriber2".to_string()));
        assert!(!matches1.contains(&"subscriber3".to_string()));

        let matches2 = matcher
            .get_matching_subscribers("control.collector.status")
            .unwrap();
        assert!(!matches2.contains(&"subscriber1".to_string()));
        assert!(!matches2.contains(&"subscriber2".to_string()));
        assert!(matches2.contains(&"subscriber3".to_string()));

        // Test statistics
        matcher.record_publication("events.process.lifecycle");
        matcher.record_delivery("events.process.lifecycle");

        let stats = matcher.get_topic_stats("events.process.lifecycle").unwrap();
        assert_eq!(stats.messages_published, 1);
        assert_eq!(stats.messages_delivered, 1);

        // Test subscriber and topic counts
        assert_eq!(matcher.get_subscriber_count(), 3);
        assert_eq!(matcher.get_topic_count(), 1); // Only one topic has stats
    }
}
