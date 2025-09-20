//! Alert data structures and types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use thiserror::Error;
use uuid::Uuid;

use crate::models::process::ProcessRecord;

/// Strongly-typed alert identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertId(u64);

impl AlertId {
    /// Create a new `AlertId` from a raw numeric identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertId;
    /// let aid = AlertId::new(42);
    /// assert_eq!(aid.raw(), 42);
    /// ```
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the underlying numeric value of the `AlertId`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertId;
    /// let id = AlertId::new(42);
    /// assert_eq!(id.raw(), 42);
    /// ```
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AlertId {
    /// Formats the `AlertId` as its underlying numeric ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertId;
    /// let id = AlertId::new(42);
    /// assert_eq!(id.to_string(), "42");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Alert severity levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    /// Low severity alert
    Low,
    /// Medium severity alert
    Medium,
    /// High severity alert
    High,
    /// Critical severity alert
    Critical,
}

impl fmt::Display for AlertSeverity {
    /// Formats the `AlertSeverity` as a lowercase string.
    ///
    /// This is the `Display` implementation used when converting a severity to text
    /// (e.g., via `to_string()` or formatting macros).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertSeverity;
    /// assert_eq!(AlertSeverity::Low.to_string(), "low");
    /// assert_eq!(AlertSeverity::Critical.to_string(), "critical");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for AlertSeverity {
    type Err = String;

    /// Parses a case-insensitive string into an `AlertSeverity`.
    ///
    /// Accepts `"low"`, `"medium"`, `"high"`, and `"critical"` (any letter case) and returns the
    /// corresponding `AlertSeverity` variant. Returns an `Err(String)` describing the invalid input
    /// for any other value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::str::FromStr;
    /// use sentinel_lib::models::alert::AlertSeverity;
    /// let s = "High";
    /// let sev = AlertSeverity::from_str(s).unwrap();
    /// assert_eq!(sev, AlertSeverity::High);
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err(format!("Invalid alert severity: {s}")),
        }
    }
}

/// Alert context information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AlertContext {
    /// Additional context data
    pub data: HashMap<String, String>,
    /// Alert tags for filtering
    pub tags: Vec<String>,
    /// Source system or component
    pub source: Option<String>,
    /// Confidence level (0.0 to 1.0)
    pub confidence: Option<f64>,
}

impl AlertContext {
    /// Creates a new, empty `AlertContext`.
    ///
    /// The returned context has an empty `data` map, no `tags`, and `source` and `confidence` set to `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// let ctx = sentinel_lib::models::alert::AlertContext::new();
    /// assert!(ctx.data.is_empty());
    /// assert!(ctx.tags.is_empty());
    /// assert!(ctx.source.is_none());
    /// assert!(ctx.confidence.is_none());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a key/value pair to the context's data map and returns the modified context for chaining.
    ///
    /// The provided `key` and `value` are converted into `String` and inserted into `self.data`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertContext;
    /// let ctx = AlertContext::new().with_data("user", "alice");
    /// assert_eq!(ctx.data.get("user").map(|s| s.as_str()), Some("alice"));
    /// ```
    #[must_use]
    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    /// Appends a tag to the context's tags vector and returns the modified context.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertContext;
    /// let ctx = AlertContext::new().with_tag("suspicious");
    /// assert_eq!(ctx.tags, vec!["suspicious".to_owned()]);
    /// ```
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Sets the context source and returns the modified context.
    ///
    /// This is a builder-style method that assigns the provided `source` string to the
    /// context's `source` field and returns `self` for chaining.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertContext;
    /// let ctx = AlertContext::new().with_source("kernel");
    /// assert_eq!(ctx.source.as_deref(), Some("kernel"));
    /// ```
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Sets the context confidence level after validating it is finite and within [0.0, 1.0].
    ///
    /// Returns an error if `confidence` is NaN, infinite, or outside the inclusive range 0.0..=1.0.
    ///
    /// # Errors
    ///
    /// Returns `AlertError::InvalidConfidence(confidence)` when the provided value is not finite
    /// or not within the valid range [0.0, 1.0].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::{AlertContext, AlertError};
    /// let ctx = AlertContext::new();
    /// let ctx = ctx.with_confidence(0.85).expect("valid confidence");
    /// assert_eq!(ctx.confidence, Some(0.85));
    ///
    /// let err = AlertContext::new().with_confidence(f64::NAN).unwrap_err();
    /// matches!(err, AlertError::InvalidConfidence(_));
    /// ```
    pub fn with_confidence(mut self, confidence: f64) -> Result<Self, AlertError> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(AlertError::InvalidConfidence(confidence));
        }
        self.confidence = Some(confidence);
        Ok(self)
    }
}

/// Structured alert information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    /// Unique alert identifier
    pub id: Uuid,
    /// Alert severity level
    pub severity: AlertSeverity,
    /// Alert title/summary
    pub title: String,
    /// Detailed alert description
    pub description: String,
    /// Source detection rule ID
    pub detection_rule_id: String,
    /// Affected process information
    pub process_record: ProcessRecord,
    /// Alert creation timestamp
    pub timestamp: SystemTime,
    /// Alert deduplication key
    pub deduplication_key: String,
    /// Alert context information
    pub context: AlertContext,
}

impl Alert {
    /// Create a new alert with the required fields.
    ///
    /// The returned Alert will have:
    /// - a freshly generated UUID (`id`),
    /// - `timestamp` set to the current system time,
    /// - `deduplication_key` built as `<severity>:<detection_rule_id>:<title>`,
    /// - an empty `AlertContext`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::{Alert, AlertSeverity};
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let proc = ProcessRecord::new(1, "proc".to_owned());
    /// let alert = Alert::new(AlertSeverity::High, "CPU spike", "High CPU usage observed", "rule-123", proc);
    /// assert_eq!(alert.severity, AlertSeverity::High);
    /// assert!(alert.deduplication_key.contains("CPU spike"));
    /// ```
    pub fn new(
        severity: AlertSeverity,
        title: impl Into<String>,
        description: impl Into<String>,
        detection_rule_id: impl Into<String>,
        process_record: ProcessRecord,
    ) -> Self {
        let alert_title = title.into();
        let alert_description = description.into();
        let alert_detection_rule_id = detection_rule_id.into();
        let deduplication_key = format!("{severity}:{alert_detection_rule_id}:{alert_title}");
        Self {
            id: Uuid::new_v4(),
            severity,
            title: alert_title,
            description: alert_description,
            detection_rule_id: alert_detection_rule_id,
            process_record,
            timestamp: SystemTime::now(),
            deduplication_key,
            context: AlertContext::new(),
        }
    }

    /// Adds a key/value pair to the alert's context data and returns the modified alert.
    ///
    /// The provided `key` and `value` are converted to `String` and inserted into `self.context.data`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Given an existing `Alert` named `alert`:
    /// let alert = alert.with_context_data("user", "alice");
    /// // now alert.context.data.get("user") == Some(&"alice".to_owned())
    /// ```
    #[must_use]
    pub fn with_context_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.data.insert(key.into(), value.into());
        self
    }

    /// Adds a tag to the alert's context and returns the modified Alert.
    ///
    /// The tag is appended to `self.context.tags`. This method consumes the alert
    /// and returns an updated instance so it can be used in builder-style chains.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use sentinel_lib::models::alert::{Alert, AlertSeverity};
    /// use sentinel_lib::models::process::ProcessRecord;
    /// let process_record = ProcessRecord::default();
    /// let alert = Alert::new(AlertSeverity::Low, "title", "desc", "rule-1", process_record)
    ///     .with_tag("network");
    /// assert!(alert.context.tags.contains(&"network".to_owned()));
    /// ```
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.context.tags.push(tag.into());
        self
    }

    /// Set the alert source.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.context.source = Some(source.into());
        self
    }

    /// Set the confidence level on this context (or alert) if the value is valid.
    ///
    /// The confidence must be a finite floating-point number inside the inclusive range
    /// [0.0, 1.0]. On success this returns `Ok(self)` with `self.context.confidence` set
    /// to `Some(confidence)`.
    ///
    /// # Errors
    ///
    /// Returns `AlertError::InvalidConfidence(confidence)` when `confidence` is non-finite
    /// (NaN or infinite) or outside the [0.0, 1.0] range.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::AlertContext;
    /// let ctx = AlertContext::new().with_confidence(0.85).unwrap();
    /// assert_eq!(ctx.confidence, Some(0.85));
    /// ```
    pub fn with_confidence(mut self, confidence: f64) -> Result<Self, AlertError> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(AlertError::InvalidConfidence(confidence));
        }
        self.context.confidence = Some(confidence);
        Ok(self)
    }

    /// Returns the alert's age in whole seconds.
    ///
    /// The value is computed as the duration elapsed since the alert's stored `timestamp`.
    /// If the system clock reports an error (e.g., the timestamp is in the future or
    /// elapsed cannot be computed), this returns `0`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // given an Alert instance `alert`:
    /// let age = alert.age_seconds();
    /// println!("alert age: {}s", age);
    /// ```
    pub fn age_seconds(&self) -> u64 {
        self.timestamp.elapsed().map(|d| d.as_secs()).unwrap_or(0)
    }

    /// Check if the alert is recent (within the specified threshold).
    ///
    /// # Arguments
    ///
    /// * `threshold_seconds` - Optional threshold in seconds. If None, uses the default of 3600 seconds (1 hour).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::{Alert, AlertSeverity};
    /// use sentinel_lib::models::process::ProcessRecord;
    ///
    /// let alert = Alert::new(
    ///     AlertSeverity::Medium,
    ///     "Test Alert".to_owned(),
    ///     "Test Description".to_owned(),
    ///     "rule-001".to_owned(),
    ///     ProcessRecord::new(1234, "test-process".to_owned()),
    /// );
    ///
    /// // Use default threshold (3600 seconds)
    /// assert!(alert.is_recent(None));
    ///
    /// // Use custom threshold (1800 seconds = 30 minutes)
    /// assert!(alert.is_recent(Some(1800)));
    /// ```
    pub fn is_recent(&self, threshold_seconds: Option<u64>) -> bool {
        let threshold = threshold_seconds.unwrap_or(3600);
        self.age_seconds() < threshold
    }

    /// Check if the alert is recent using the configured threshold from `AlertingConfig`.
    ///
    /// This is a convenience method that uses the `recent_threshold_seconds` value
    /// from the provided configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The alerting configuration containing the threshold value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sentinel_lib::models::alert::{Alert, AlertSeverity};
    /// use sentinel_lib::models::process::ProcessRecord;
    /// use sentinel_lib::config::AlertingConfig;
    ///
    /// let alert = Alert::new(
    ///     AlertSeverity::Medium,
    ///     "Test Alert".to_owned(),
    ///     "Test Description".to_owned(),
    ///     "rule-001".to_owned(),
    ///     ProcessRecord::new(1234, "test-process".to_owned()),
    /// );
    ///
    /// let config = AlertingConfig::default();
    /// assert!(alert.is_recent_with_config(&config));
    /// ```
    pub fn is_recent_with_config(&self, config: &crate::config::AlertingConfig) -> bool {
        self.is_recent(Some(config.recent_threshold_seconds))
    }
}

/// Alert-related errors.
#[derive(Debug, Error)]
pub enum AlertError {
    #[error("Invalid alert severity: {0}")]
    InvalidSeverity(String),
    #[error("Missing required field: {0}")]
    MissingField(&'static str),
    #[error("Invalid confidence level: {0}")]
    InvalidConfidence(f64),
    #[error("Alert creation failed: {0}")]
    CreationFailed(String),
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::process::ProcessRecord;

    #[test]
    fn test_alert_creation() {
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Suspicious Process Detected",
            "A potentially malicious process was detected",
            "rule-001",
            process.clone(),
        );

        assert_eq!(alert.severity, AlertSeverity::High);
        assert_eq!(alert.title, "Suspicious Process Detected");
        assert_eq!(
            alert.description,
            "A potentially malicious process was detected"
        );
        assert_eq!(alert.detection_rule_id, "rule-001");
        assert_eq!(alert.process_record, process);
        assert!(alert.is_recent(None));
    }

    #[test]
    fn test_alert_serialization() {
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "Test description",
            "rule-001",
            process,
        )
        .with_tag("test")
        .with_source("test-system")
        .with_confidence(0.95)
        .expect("Failed to create alert in test_alert_serialization");

        // Test JSON serialization
        let json =
            serde_json::to_string(&alert).expect("Failed to serialize alert to JSON in test");
        let deserialized: Alert =
            serde_json::from_str(&json).expect("Failed to deserialize alert from JSON in test");
        assert_eq!(alert, deserialized);
    }

    #[test]
    fn test_alert_severity_parsing() {
        assert_eq!(
            "high"
                .parse::<AlertSeverity>()
                .expect("Failed to parse 'high' as AlertSeverity in test"),
            AlertSeverity::High
        );
        assert_eq!(
            "MEDIUM"
                .parse::<AlertSeverity>()
                .expect("Failed to parse 'MEDIUM' as AlertSeverity in test"),
            AlertSeverity::Medium
        );
        assert_eq!(
            "Critical"
                .parse::<AlertSeverity>()
                .expect("Failed to parse 'Critical' as AlertSeverity in test"),
            AlertSeverity::Critical
        );
        assert!("invalid".parse::<AlertSeverity>().is_err());
    }

    #[test]
    fn test_alert_severity_display() {
        assert_eq!(AlertSeverity::Low.to_string(), "low");
        assert_eq!(AlertSeverity::Medium.to_string(), "medium");
        assert_eq!(AlertSeverity::High.to_string(), "high");
        assert_eq!(AlertSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn test_alert_context() {
        let context = AlertContext::new()
            .with_data("key", "value")
            .with_tag("test")
            .with_source("test-system")
            .with_confidence(0.8)
            .expect("Failed to create alert context in test");

        assert_eq!(context.data.get("key"), Some(&"value".to_owned()));
        assert!(context.tags.contains(&"test".to_owned()));
        assert_eq!(context.source, Some("test-system".to_owned()));
        assert_eq!(context.confidence, Some(0.8));
    }

    #[test]
    fn test_confidence_validation() {
        let context = AlertContext::new();

        // Valid confidence values should work
        assert!(context.clone().with_confidence(0.0).is_ok());
        assert!(context.clone().with_confidence(0.5).is_ok());
        assert!(context.clone().with_confidence(1.0).is_ok());

        // Invalid confidence values should fail
        assert!(context.clone().with_confidence(-0.1).is_err());
        assert!(context.clone().with_confidence(1.1).is_err());
        assert!(context.clone().with_confidence(f64::NAN).is_err());
        assert!(context.clone().with_confidence(f64::INFINITY).is_err());
        assert!(context.clone().with_confidence(f64::NEG_INFINITY).is_err());

        // Check error type
        let result = context.with_confidence(2.0);
        assert!(matches!(result, Err(AlertError::InvalidConfidence(2.0))));
    }

    #[test]
    fn test_alert_id_operations() {
        let id = AlertId::new(12345);
        assert_eq!(id.raw(), 12345);
        assert_eq!(id.to_string(), "12345");
    }

    #[test]
    fn test_alert_age() {
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "Test description",
            "rule-001",
            process,
        );

        // Alert should be recent (just created)
        assert!(alert.is_recent(None));
        assert_eq!(alert.age_seconds(), 0);
    }

    #[test]
    fn test_is_recent_with_custom_threshold() {
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::Medium,
            "Test Alert".to_owned(),
            "Test description".to_owned(),
            "rule-001".to_owned(),
            process,
        );

        // Should be recent with default threshold (3600 seconds)
        assert!(alert.is_recent(None));
        assert!(alert.is_recent(Some(3600)));

        // Should be recent with custom threshold (7200 seconds = 2 hours)
        assert!(alert.is_recent(Some(7200)));

        // Should not be recent with very small threshold (0 seconds)
        assert!(!alert.is_recent(Some(0)));
    }

    #[test]
    fn test_is_recent_with_config() {
        use crate::config::AlertingConfig;

        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::Medium,
            "Test Alert".to_owned(),
            "Test description".to_owned(),
            "rule-001".to_owned(),
            process,
        );

        // Test with default config (3600 seconds)
        let config = AlertingConfig::default();
        assert!(alert.is_recent_with_config(&config));
        assert_eq!(config.recent_threshold_seconds, 3600);

        // Test with custom config
        let custom_config = AlertingConfig {
            recent_threshold_seconds: 1800, // 30 minutes
            ..Default::default()
        };
        assert!(alert.is_recent_with_config(&custom_config));

        // Test with very small threshold
        let zero_config = AlertingConfig {
            recent_threshold_seconds: 0,
            ..Default::default()
        };
        assert!(!alert.is_recent_with_config(&zero_config));
    }
}
