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
    /// Create a new alert ID.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw alert ID value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for AlertId {
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AlertSeverity::Low => write!(f, "low"),
            AlertSeverity::Medium => write!(f, "medium"),
            AlertSeverity::High => write!(f, "high"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for AlertSeverity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(AlertSeverity::Low),
            "medium" => Ok(AlertSeverity::Medium),
            "high" => Ok(AlertSeverity::High),
            "critical" => Ok(AlertSeverity::Critical),
            _ => Err(format!("Invalid alert severity: {}", s)),
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
    /// Create a new alert context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a context data entry.
    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set the source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the confidence level.
    ///
    /// # Errors
    ///
    /// Returns `AlertError::InvalidConfidence` if the confidence value is not finite
    /// or not within the valid range [0.0, 1.0].
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
    /// Create a new alert with required fields.
    pub fn new(
        severity: AlertSeverity,
        title: impl Into<String>,
        description: impl Into<String>,
        detection_rule_id: impl Into<String>,
        process_record: ProcessRecord,
    ) -> Self {
        let title = title.into();
        let description = description.into();
        let detection_rule_id = detection_rule_id.into();
        let deduplication_key = format!("{}:{}:{}", severity, detection_rule_id, title);
        Self {
            id: Uuid::new_v4(),
            severity,
            title,
            description,
            detection_rule_id,
            process_record,
            timestamp: SystemTime::now(),
            deduplication_key,
            context: AlertContext::new(),
        }
    }

    /// Add context data to the alert.
    pub fn with_context_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.data.insert(key.into(), value.into());
        self
    }

    /// Add a tag to the alert.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.context.tags.push(tag.into());
        self
    }

    /// Set the alert source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.context.source = Some(source.into());
        self
    }

    /// Set the confidence level.
    ///
    /// # Errors
    ///
    /// Returns `AlertError::InvalidConfidence` if the confidence value is not finite
    /// or not within the valid range [0.0, 1.0].
    pub fn with_confidence(mut self, confidence: f64) -> Result<Self, AlertError> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(AlertError::InvalidConfidence(confidence));
        }
        self.context.confidence = Some(confidence);
        Ok(self)
    }

    /// Get the alert age in seconds.
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
    ///     "Test Alert".to_string(),
    ///     "Test Description".to_string(),
    ///     "rule-001".to_string(),
    ///     ProcessRecord::new(1234, "test-process".to_string()),
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

    /// Check if the alert is recent using the configured threshold from AlertingConfig.
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
    ///     "Test Alert".to_string(),
    ///     "Test Description".to_string(),
    ///     "rule-001".to_string(),
    ///     ProcessRecord::new(1234, "test-process".to_string()),
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
mod tests {
    use super::*;
    use crate::models::process::ProcessRecord;

    #[test]
    fn test_alert_creation() {
        let process = ProcessRecord::new(1234, "test-process".to_string());
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
        let process = ProcessRecord::new(1234, "test-process".to_string());
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
        .unwrap();

        // Test JSON serialization
        let json = serde_json::to_string(&alert).unwrap();
        let deserialized: Alert = serde_json::from_str(&json).unwrap();
        assert_eq!(alert, deserialized);
    }

    #[test]
    fn test_alert_severity_parsing() {
        assert_eq!(
            "high".parse::<AlertSeverity>().unwrap(),
            AlertSeverity::High
        );
        assert_eq!(
            "MEDIUM".parse::<AlertSeverity>().unwrap(),
            AlertSeverity::Medium
        );
        assert_eq!(
            "Critical".parse::<AlertSeverity>().unwrap(),
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
            .unwrap();

        assert_eq!(context.data.get("key"), Some(&"value".to_string()));
        assert!(context.tags.contains(&"test".to_string()));
        assert_eq!(context.source, Some("test-system".to_string()));
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
        let process = ProcessRecord::new(1234, "test-process".to_string());
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
        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::Medium,
            "Test Alert".to_string(),
            "Test description".to_string(),
            "rule-001".to_string(),
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

        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::Medium,
            "Test Alert".to_string(),
            "Test description".to_string(),
            "rule-001".to_string(),
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
