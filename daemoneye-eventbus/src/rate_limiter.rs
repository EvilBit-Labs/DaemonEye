//! Token bucket rate limiter with sub-second granularity
//!
//! Implements a proper token bucket algorithm that supports:
//! - Sub-second granularity for precise rate limiting
//! - Per-client and per-topic rate limits
//! - Configurable capacity and refill rate

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::debug;

/// Rate limit configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of tokens (burst capacity)
    pub capacity: usize,
    /// Number of tokens to add per second
    pub refill_rate: f64,
    /// Per-client rate limits (client_id -> config)
    pub per_client_limits: HashMap<String, RateLimitConfig>,
    /// Per-topic rate limits (topic_prefix -> config)
    pub per_topic_limits: HashMap<String, RateLimitConfig>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            capacity: 1000,
            refill_rate: 1000.0, // 1000 tokens per second
            per_client_limits: HashMap::new(),
            per_topic_limits: HashMap::new(),
        }
    }
}

impl RateLimitConfig {
    /// Create a new rate limit configuration
    pub fn new(capacity: usize, refill_rate: f64) -> Self {
        Self {
            capacity,
            refill_rate,
            per_client_limits: HashMap::new(),
            per_topic_limits: HashMap::new(),
        }
    }

    /// Set per-client rate limit
    pub fn with_client_limit(mut self, client_id: String, config: RateLimitConfig) -> Self {
        self.per_client_limits.insert(client_id, config);
        self
    }

    /// Set per-topic rate limit
    pub fn with_topic_limit(mut self, topic_prefix: String, config: RateLimitConfig) -> Self {
        self.per_topic_limits.insert(topic_prefix, config);
        self
    }
}

/// Token bucket state for a single rate limiter
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Current number of tokens available
    tokens: f64,
    /// Maximum capacity (burst limit)
    capacity: f64,
    /// Tokens added per second
    refill_rate: f64,
    /// Last time tokens were updated
    last_update: Instant,
}

impl TokenBucket {
    /// Create a new token bucket
    fn new(capacity: usize, refill_rate: f64) -> Self {
        Self {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_rate,
            last_update: Instant::now(),
        }
    }

    /// Try to consume tokens, returning true if successful
    fn try_consume(&mut self, tokens: usize) -> bool {
        self.refill();
        let tokens_needed = tokens as f64;
        if self.tokens >= tokens_needed {
            self.tokens -= tokens_needed;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time (sub-second precision)
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update);
        self.last_update = now;

        // Calculate tokens to add based on elapsed time with sub-second precision
        let tokens_to_add = self.refill_rate * elapsed.as_secs_f64();
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity);
    }

    /// Get current token count (for testing/debugging)
    #[cfg(test)]
    fn tokens(&self) -> f64 {
        self.tokens
    }
}

/// Rate limiter that manages multiple token buckets
pub struct RateLimiter {
    /// Default rate limit configuration
    default_config: RateLimitConfig,
    /// Per-client token buckets
    client_buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    /// Per-topic token buckets
    topic_buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    /// Combined client+topic buckets for fine-grained control
    combined_buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with default configuration
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            default_config: config.clone(),
            client_buckets: Arc::new(Mutex::new(HashMap::new())),
            topic_buckets: Arc::new(Mutex::new(HashMap::new())),
            combined_buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a message can be sent (consume tokens)
    /// Returns true if allowed, false if rate limited
    pub async fn check_rate_limit(&self, client_id: Option<&str>, topic: Option<&str>) -> bool {
        // Determine which config to use (per-client > per-topic > default)
        let config = if let Some(client_id) = client_id {
            if let Some(client_config) = self.default_config.per_client_limits.get(client_id) {
                Some(client_config)
            } else if let Some(topic) = topic {
                // Check for topic prefix match
                self.default_config
                    .per_topic_limits
                    .iter()
                    .find(|(prefix, _)| topic.starts_with(prefix.as_str()))
                    .map(|(_, config)| config)
            } else {
                None
            }
        } else if let Some(topic) = topic {
            self.default_config
                .per_topic_limits
                .iter()
                .find(|(prefix, _)| topic.starts_with(prefix.as_str()))
                .map(|(_, config)| config)
        } else {
            None
        };

        let config = config.unwrap_or(&self.default_config);

        // Check per-client limit if client_id provided
        if let Some(client_id) = client_id {
            let mut buckets = self.client_buckets.lock().await;
            let bucket = buckets
                .entry(client_id.to_string())
                .or_insert_with(|| TokenBucket::new(config.capacity, config.refill_rate));
            if !bucket.try_consume(1) {
                debug!("Rate limit exceeded for client: {}", client_id);
                return false;
            }
        }

        // Check per-topic limit if topic provided
        if let Some(topic) = topic {
            // Find matching topic prefix
            if let Some((prefix, topic_config)) = self
                .default_config
                .per_topic_limits
                .iter()
                .find(|(p, _)| topic.starts_with(p.as_str()))
            {
                let mut buckets = self.topic_buckets.lock().await;
                let bucket = buckets.entry(prefix.clone()).or_insert_with(|| {
                    TokenBucket::new(topic_config.capacity, topic_config.refill_rate)
                });
                if !bucket.try_consume(1) {
                    debug!("Rate limit exceeded for topic: {}", topic);
                    return false;
                }
            }
        }

        // Check combined client+topic limit if both provided
        if let (Some(client_id), Some(topic)) = (client_id, topic) {
            let key = format!("{}:{}", client_id, topic);
            let mut buckets = self.combined_buckets.lock().await;
            let bucket = buckets
                .entry(key)
                .or_insert_with(|| TokenBucket::new(config.capacity, config.refill_rate));
            if !bucket.try_consume(1) {
                debug!(
                    "Rate limit exceeded for client+topic: {}:{}",
                    client_id, topic
                );
                return false;
            }
        }

        true
    }

    /// Remove a client's rate limiter (cleanup on disconnect)
    pub async fn remove_client(&self, client_id: &str) {
        let mut buckets = self.client_buckets.lock().await;
        buckets.remove(client_id);
        // Also remove combined buckets for this client
        let mut combined = self.combined_buckets.lock().await;
        combined.retain(|key, _| !key.starts_with(&format!("{}:", client_id)));
    }

    /// Get current token count for a client (for testing/debugging)
    #[cfg(test)]
    pub async fn get_client_tokens(&self, client_id: &str) -> Option<f64> {
        let buckets = self.client_buckets.lock().await;
        buckets.get(client_id).map(|b| b.tokens())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_token_bucket_basic() {
        let mut bucket = TokenBucket::new(10, 5.0); // 10 capacity, 5 tokens/sec
        assert!(bucket.try_consume(5));
        assert_eq!(bucket.tokens(), 5.0);
        assert!(bucket.try_consume(5));
        assert_eq!(bucket.tokens(), 0.0);
        assert!(!bucket.try_consume(1)); // Should fail, no tokens
    }

    #[tokio::test]
    async fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10, 10.0); // 10 capacity, 10 tokens/sec
        assert!(bucket.try_consume(10));
        assert_eq!(bucket.tokens(), 0.0);

        // Wait 100ms, should refill ~1 token
        tokio::time::sleep(Duration::from_millis(100)).await;
        bucket.refill();
        assert!(bucket.tokens() > 0.9 && bucket.tokens() < 1.1);
    }

    #[tokio::test]
    async fn test_rate_limiter_default() {
        let config = RateLimitConfig::new(10, 10.0);
        let limiter = RateLimiter::new(config);

        // Should allow 10 messages immediately
        for i in 0..10 {
            assert!(
                limiter.check_rate_limit(None, None).await,
                "Should allow message {}",
                i
            );
        }

        // 11th should be rate limited
        assert!(!limiter.check_rate_limit(None, None).await);
    }

    #[tokio::test]
    async fn test_rate_limiter_per_client() {
        let config = RateLimitConfig::new(10, 10.0)
            .with_client_limit("client1".to_string(), RateLimitConfig::new(5, 5.0))
            .with_client_limit("client2".to_string(), RateLimitConfig::new(3, 3.0));

        let limiter = RateLimiter::new(config);

        // Client1 should allow 5 messages
        for i in 0..5 {
            assert!(
                limiter.check_rate_limit(Some("client1"), None).await,
                "Should allow message {} for client1",
                i
            );
        }
        assert!(!limiter.check_rate_limit(Some("client1"), None).await);

        // Client2 should allow 3 messages
        for i in 0..3 {
            assert!(
                limiter.check_rate_limit(Some("client2"), None).await,
                "Should allow message {} for client2",
                i
            );
        }
        assert!(!limiter.check_rate_limit(Some("client2"), None).await);
    }

    #[tokio::test]
    async fn test_rate_limiter_per_topic() {
        let config = RateLimitConfig::new(10, 10.0)
            .with_topic_limit("direct.".to_string(), RateLimitConfig::new(5, 5.0))
            .with_topic_limit("queue.".to_string(), RateLimitConfig::new(3, 3.0));

        let limiter = RateLimiter::new(config);

        // direct.* topics should allow 5 messages
        for i in 0..5 {
            assert!(
                limiter.check_rate_limit(None, Some("direct.client1")).await,
                "Should allow message {} for direct.*",
                i
            );
        }
        assert!(!limiter.check_rate_limit(None, Some("direct.client1")).await);

        // queue.* topics should allow 3 messages
        for i in 0..3 {
            assert!(
                limiter
                    .check_rate_limit(None, Some("queue.client1.topic"))
                    .await,
                "Should allow message {} for queue.*",
                i
            );
        }
        assert!(
            !limiter
                .check_rate_limit(None, Some("queue.client1.topic"))
                .await
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_refill() {
        let config = RateLimitConfig::new(10, 10.0); // 10 tokens/sec
        let limiter = RateLimiter::new(config);

        // Consume all tokens
        for _ in 0..10 {
            assert!(limiter.check_rate_limit(None, None).await);
        }
        assert!(!limiter.check_rate_limit(None, None).await);

        // Wait 100ms, should refill ~1 token
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(limiter.check_rate_limit(None, None).await);
    }

    #[tokio::test]
    async fn test_rate_limiter_remove_client() {
        let config = RateLimitConfig::new(10, 10.0);
        let limiter = RateLimiter::new(config);

        // Consume tokens for a client
        for _ in 0..5 {
            assert!(limiter.check_rate_limit(Some("client1"), None).await);
        }

        // Remove client
        limiter.remove_client("client1").await;

        // Should be able to consume again (bucket reset)
        for _ in 0..10 {
            assert!(limiter.check_rate_limit(Some("client1"), None).await);
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_sub_second_precision() {
        let config = RateLimitConfig::new(1, 10.0); // 1 capacity, 10 tokens/sec
        let limiter = RateLimiter::new(config);

        // Consume the one token
        assert!(limiter.check_rate_limit(None, None).await);
        assert!(!limiter.check_rate_limit(None, None).await);

        // Wait 50ms, should refill ~0.5 tokens (not enough for 1)
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!limiter.check_rate_limit(None, None).await);

        // Wait another 50ms, should refill ~1 token total
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(limiter.check_rate_limit(None, None).await);
    }
}
