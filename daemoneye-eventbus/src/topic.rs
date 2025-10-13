//! Topic routing and pattern matching for the EventBus

use crate::error::Result;
use std::collections::HashMap;
use uuid::Uuid;

/// A topic pattern for subscription matching
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicPattern {
    /// The pattern string (e.g., "events.process.*")
    pub pattern: String,
    /// Whether this is an exact match or wildcard
    pub is_wildcard: bool,
}

impl TopicPattern {
    /// Create a new topic pattern
    pub fn new(pattern: String) -> Self {
        let is_wildcard = pattern.contains('*');
        Self {
            pattern,
            is_wildcard,
        }
    }

    /// Check if this pattern matches a topic
    pub fn matches(&self, topic: &str) -> bool {
        if self.is_wildcard {
            self.matches_wildcard(topic)
        } else {
            self.pattern == topic
        }
    }

    /// Wildcard matching logic
    fn matches_wildcard(&self, topic: &str) -> bool {
        let pattern_parts: Vec<&str> = self.pattern.split('.').collect();
        let topic_parts: Vec<&str> = topic.split('.').collect();

        // If pattern has more parts than topic, no match
        if pattern_parts.len() > topic_parts.len() {
            return false;
        }

        // Check each part
        for (i, pattern_part) in pattern_parts.iter().enumerate() {
            if *pattern_part == "*" {
                // Wildcard matches any part
                continue;
            } else if i >= topic_parts.len() || pattern_part != &topic_parts[i] {
                // No match
                return false;
            }
        }

        // If pattern is shorter, it should end with * to match remaining parts
        if pattern_parts.len() < topic_parts.len() {
            return pattern_parts.last() == Some(&"*");
        }

        true
    }
}

/// A topic matcher that manages subscriptions and routing
#[derive(Debug)]
pub struct TopicMatcher {
    /// Map of topic patterns to subscriber IDs
    subscriptions: HashMap<TopicPattern, Vec<Uuid>>,
    /// Map of subscriber IDs to their patterns
    subscriber_patterns: HashMap<Uuid, Vec<TopicPattern>>,
}

impl TopicMatcher {
    /// Create a new topic matcher
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            subscriber_patterns: HashMap::new(),
        }
    }

    /// Subscribe to a topic pattern
    pub fn subscribe(&mut self, pattern: &str, subscriber_id: Uuid) -> Result<()> {
        let topic_pattern = TopicPattern::new(pattern.to_string());

        // Add to subscriptions
        self.subscriptions
            .entry(topic_pattern.clone())
            .or_insert_with(Vec::new)
            .push(subscriber_id);

        // Add to subscriber patterns
        self.subscriber_patterns
            .entry(subscriber_id)
            .or_insert_with(Vec::new)
            .push(topic_pattern);

        Ok(())
    }

    /// Unsubscribe from all patterns for a subscriber
    pub fn unsubscribe(&mut self, subscriber_id: Uuid) -> Result<()> {
        // Remove from subscriber patterns
        if let Some(patterns) = self.subscriber_patterns.remove(&subscriber_id) {
            // Remove from subscriptions
            for pattern in patterns {
                if let Some(subscribers) = self.subscriptions.get_mut(&pattern) {
                    subscribers.retain(|&id| id != subscriber_id);
                    if subscribers.is_empty() {
                        self.subscriptions.remove(&pattern);
                    }
                }
            }
        }

        Ok(())
    }

    /// Find all subscribers for a topic
    pub fn find_subscribers(&self, topic: &str) -> Vec<Uuid> {
        let mut subscribers = Vec::new();

        for (pattern, subscriber_list) in &self.subscriptions {
            if pattern.matches(topic) {
                subscribers.extend(subscriber_list.iter());
            }
        }

        // Remove duplicates while preserving order
        let mut unique_subscribers = Vec::new();
        for subscriber in subscribers {
            if !unique_subscribers.contains(&subscriber) {
                unique_subscribers.push(subscriber);
            }
        }

        unique_subscribers
    }

    /// Get all topic patterns
    pub fn get_patterns(&self) -> Vec<String> {
        self.subscriptions
            .keys()
            .map(|p| p.pattern.clone())
            .collect()
    }

    /// Get subscriber count
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_patterns.len()
    }

    /// Get pattern count
    pub fn pattern_count(&self) -> usize {
        self.subscriptions.len()
    }
}

impl Default for TopicMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let pattern = TopicPattern::new("events.process.new".to_string());
        assert!(pattern.matches("events.process.new"));
        assert!(!pattern.matches("events.process.old"));
        assert!(!pattern.matches("events.network.new"));
    }

    #[test]
    fn test_wildcard_match() {
        let pattern = TopicPattern::new("events.process.*".to_string());
        assert!(pattern.matches("events.process.new"));
        assert!(pattern.matches("events.process.old"));
        assert!(!pattern.matches("events.network.new"));
        assert!(!pattern.matches("events.process"));
    }

    #[test]
    fn test_multi_level_wildcard() {
        let pattern = TopicPattern::new("events.*".to_string());
        assert!(pattern.matches("events.process.new"));
        assert!(pattern.matches("events.network.connections"));
        assert!(!pattern.matches("control.collector.start"));
    }

    #[test]
    fn test_topic_matcher() {
        let mut matcher = TopicMatcher::new();
        let subscriber1 = Uuid::new_v4();
        let subscriber2 = Uuid::new_v4();

        // Subscribe to patterns
        matcher.subscribe("events.process.*", subscriber1).unwrap();
        matcher.subscribe("events.*", subscriber2).unwrap();

        // Find subscribers
        let subscribers = matcher.find_subscribers("events.process.new");
        assert_eq!(subscribers.len(), 2);
        assert!(subscribers.contains(&subscriber1));
        assert!(subscribers.contains(&subscriber2));

        // Test exact topic
        let subscribers = matcher.find_subscribers("events.network.connections");
        assert_eq!(subscribers.len(), 1);
        assert!(subscribers.contains(&subscriber2));

        // Unsubscribe
        matcher.unsubscribe(subscriber1).unwrap();
        let subscribers = matcher.find_subscribers("events.process.new");
        assert_eq!(subscribers.len(), 1);
        assert!(subscribers.contains(&subscriber2));
    }
}
