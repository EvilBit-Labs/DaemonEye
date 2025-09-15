//! Multi-channel alert delivery system.
//!
//! This module provides a flexible alert delivery system with support for multiple
//! output channels including stdout, files, webhooks, syslog, and email.
//!
//! Features:
//! - Factory pattern for creating alert sinks
//! - Comprehensive error handling with specific error types
//! - Concurrent alert delivery with proper error isolation
//! - Rate limiting and deduplication
//! - Health monitoring for all sinks

use crate::models::Alert;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use thiserror::Error;

/// Alert delivery errors with specific error types for better error handling.
#[derive(Debug, Error)]
pub enum AlertingError {
    #[error("Alert sink error: {0}")]
    SinkError(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Delivery timeout: {0}")]
    Timeout(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("YAML serialization error: {0}")]
    YamlSerializationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("File sink error: {path} - {error}")]
    FileSinkError { path: String, error: String },

    #[error("Stdout sink error: {0}")]
    StdoutSinkError(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Deduplication error: {0}")]
    DeduplicationError(String),

    #[error("Health check failed for sink {sink}: {error}")]
    HealthCheckFailed { sink: String, error: String },

    #[error("Unknown sink type: {sink_type}")]
    UnknownSinkType { sink_type: String },
}

/// Alert delivery result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliveryResult {
    /// Sink name
    pub sink_name: String,
    /// Delivery success status
    pub success: bool,
    /// Delivery timestamp
    pub delivered_at: chrono::DateTime<chrono::Utc>,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// Delivery duration in milliseconds
    pub duration_ms: u64,
}

/// Alert sink trait for different delivery mechanisms.
#[async_trait]
pub trait AlertSink: Send + Sync {
    /// Send an alert through this sink.
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError>;

    /// Get the sink name.
    fn name(&self) -> &str;

    /// Check if the sink is healthy.
    async fn health_check(&self) -> Result<(), AlertingError>;
}

/// Standard output alert sink.
pub struct StdoutSink {
    name: String,
    format: OutputFormat,
}

impl StdoutSink {
    /// Create a new stdout sink.
    pub fn new(name: String, format: OutputFormat) -> Self {
        Self { name, format }
    }
}

#[async_trait]
impl AlertSink for StdoutSink {
    /// Sends the alert to stdout using the sink's configured output format and returns a DeliveryResult.
    ///
    /// The alert is formatted according to `self.format`:
    /// - `Json`: pretty-prints the alert as JSON (may produce `AlertingError::SerializationError`).
    /// - `Yaml`: serializes to YAML (maps YAML errors to `AlertingError::YamlSerializationError`).
    /// - `Human`: a concise human-readable single-line string using `severity`, `detection_rule_id`, `title`, and `description`.
    /// - `Csv`: a single-line CSV with `id,severity,detection_rule_id,title,description`.
    ///
    /// Returns a `DeliveryResult` containing the sink name, success=true, the delivery timestamp, and the elapsed duration in milliseconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::Utc;
    /// use tokio::runtime::Runtime;
    ///
    /// // Construct a minimal Alert and a StdoutSink (types from the crate)
    /// let alert = crate::models::Alert {
    ///     id: "a1".into(),
    ///     title: "Test".into(),
    ///     severity: crate::models::AlertSeverity::Info,
    ///     description: "Example".into(),
    ///     detection_rule_id: "rule-1".into(),
    ///     process: crate::models::ProcessRecord::default(),
    ///     deduplication_key: "key".into(),
    ///     created_at: Utc::now(),
    /// };
    /// let sink = crate::sinks::StdoutSink::new("stdout-test".into(), crate::sinks::OutputFormat::Human);
    ///
    /// // Run the async send
    /// let rt = Runtime::new().unwrap();
    /// let res = rt.block_on(async { sink.send(&alert).await.unwrap() });
    /// assert_eq!(res.sink_name, "stdout-test");
    /// assert!(res.success);
    /// ```
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = match self.format {
            OutputFormat::Json => serde_json::to_string_pretty(alert)?,
            OutputFormat::Human => format!(
                "[{}] {} - {}: {}",
                alert.severity, alert.detection_rule_id, alert.title, alert.description
            ),
            OutputFormat::Yaml => serde_yaml::to_string(alert)
                .map_err(|e| AlertingError::YamlSerializationError(e.to_string()))?,
            OutputFormat::Csv => format!(
                "{},{},{},{},{}",
                alert.id, alert.severity, alert.detection_rule_id, alert.title, alert.description
            ),
        };

        println!("{}", output);

        let duration = start_time.elapsed();
        Ok(DeliveryResult {
            sink_name: self.name.clone(),
            success: true,
            delivered_at: chrono::Utc::now(),
            error_message: None,
            duration_ms: duration.as_millis() as u64,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        // stdout is always available
        Ok(())
    }
}

/// File output alert sink.
pub struct FileSink {
    name: String,
    file_path: std::path::PathBuf,
    format: OutputFormat,
}

impl FileSink {
    /// Create a new file sink.
    pub fn new(name: String, file_path: std::path::PathBuf, format: OutputFormat) -> Self {
        Self {
            name,
            file_path,
            format,
        }
    }
}

#[async_trait]
impl AlertSink for FileSink {
    /// Appends the given alert to the configured file using the sink's OutputFormat and returns a DeliveryResult with timing and delivery metadata.
    ///
    /// The alert is serialized according to `self.format`:
    /// - `Json`: pretty-printed JSON
    /// - `Human`: single-line human readable string including severity, detection rule ID, title, and description
    /// - `Yaml`: YAML (errors are mapped to `AlertingError::YamlSerializationError`)
    /// - `Csv`: comma-separated values (id,severity,detection_rule_id,title,description)
    ///
    /// Returns an error if serialization fails or the file cannot be opened/written.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    /// use std::path::PathBuf;
    ///
    /// // create a runtime for the async example
    /// let rt = Runtime::new().unwrap();
    /// rt.block_on(async {
    ///     // construct a file sink (FileSink::new(name, path, format))
    ///     let sink = crate::sinks::FileSink::new("test-sink".into(), PathBuf::from("/tmp/alerts.log"), crate::OutputFormat::Human);
    ///
    ///     // construct an Alert (use the crate's Alert constructor)
    ///     let alert = crate::models::Alert::new(
    ///         "Example alert",
    ///         crate::models::AlertSeverity::Medium,
    ///         "An example description",
    ///         "rule-123",
    ///         crate::models::ProcessRecord::default(),
    ///     );
    ///
    ///     let result = sink.send(&alert).await.unwrap();
    ///     assert!(result.success);
    /// });
    /// ```
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = match self.format {
            OutputFormat::Json => serde_json::to_string_pretty(alert)?,
            OutputFormat::Human => format!(
                "[{}] {} - {}: {}",
                alert.severity, alert.detection_rule_id, alert.title, alert.description
            ),
            OutputFormat::Yaml => serde_yaml::to_string(alert)
                .map_err(|e| AlertingError::YamlSerializationError(e.to_string()))?,
            OutputFormat::Csv => format!(
                "{},{},{},{},{}",
                alert.id, alert.severity, alert.detection_rule_id, alert.title, alert.description
            ),
        };

        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?
            .write_all(format!("{}\n", output).as_bytes())?;

        let duration = start_time.elapsed();
        Ok(DeliveryResult {
            sink_name: self.name.clone(),
            success: true,
            delivered_at: chrono::Utc::now(),
            error_message: None,
            duration_ms: duration.as_millis() as u64,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        // Check if we can write to the file
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?;
        Ok(())
    }
}

/// Output format enumeration with string parsing support.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OutputFormat {
    Json,
    Human,
    Yaml,
    Csv,
}

impl std::str::FromStr for OutputFormat {
    type Err = AlertingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "human" => Ok(OutputFormat::Human),
            "yaml" => Ok(OutputFormat::Yaml),
            "csv" => Ok(OutputFormat::Csv),
            _ => Err(AlertingError::ConfigurationError(format!(
                "Unknown output format: {}",
                s
            ))),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Human => write!(f, "human"),
            OutputFormat::Yaml => write!(f, "yaml"),
            OutputFormat::Csv => write!(f, "csv"),
        }
    }
}

/// Configuration for creating alert sinks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SinkConfig {
    /// Sink name
    pub name: String,
    /// Sink type
    pub sink_type: String,
    /// Output format
    pub format: OutputFormat,
    /// Additional configuration
    pub config: HashMap<String, String>,
}

impl SinkConfig {
    /// Create a new sink configuration.
    pub fn new(
        name: impl Into<String>,
        sink_type: impl Into<String>,
        format: OutputFormat,
    ) -> Self {
        Self {
            name: name.into(),
            sink_type: sink_type.into(),
            format,
            config: HashMap::new(),
        }
    }

    /// Add a configuration parameter.
    pub fn with_config(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.insert(key.into(), value.into());
        self
    }
}

/// Factory for creating alert sinks based on configuration.
pub struct AlertSinkFactory;

impl AlertSinkFactory {
    /// Create an alert sink from configuration.
    pub fn create_sink(config: SinkConfig) -> Result<Box<dyn AlertSink>, AlertingError> {
        match config.sink_type.as_str() {
            "stdout" => {
                let sink = StdoutSink::new(config.name, config.format);
                Ok(Box::new(sink))
            }
            "file" => {
                let file_path = config
                    .config
                    .get("path")
                    .ok_or_else(|| {
                        AlertingError::ConfigurationError(
                            "File path is required for file sink".to_string(),
                        )
                    })?
                    .clone();

                let sink = FileSink::new(config.name, file_path.into(), config.format);
                Ok(Box::new(sink))
            }
            _ => Err(AlertingError::UnknownSinkType {
                sink_type: config.sink_type,
            }),
        }
    }

    /// Create multiple sinks from a list of configurations.
    pub fn create_sinks(
        configs: Vec<SinkConfig>,
    ) -> Result<Vec<Box<dyn AlertSink>>, AlertingError> {
        let mut sinks = Vec::new();
        for config in configs {
            sinks.push(Self::create_sink(config)?);
        }
        Ok(sinks)
    }
}

/// Alert manager for coordinating delivery to multiple sinks.
pub struct AlertManager {
    sinks: Vec<Box<dyn AlertSink>>,
    dedup_window_seconds: u64,
    max_alerts_per_minute: Option<u32>,
    recent_alerts: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
}

impl AlertManager {
    /// Create a new alert manager.
    pub fn new() -> Self {
        Self {
            sinks: Vec::new(),
            dedup_window_seconds: 300, // 5 minutes
            max_alerts_per_minute: None,
            recent_alerts: std::collections::HashMap::new(),
        }
    }

    /// Create an alert manager with sinks from configurations.
    pub fn from_configs(configs: Vec<SinkConfig>) -> Result<Self, AlertingError> {
        let sinks = AlertSinkFactory::create_sinks(configs)?;
        Ok(Self {
            sinks,
            dedup_window_seconds: 300,
            max_alerts_per_minute: None,
            recent_alerts: std::collections::HashMap::new(),
        })
    }

    /// Add an alert sink.
    pub fn add_sink(&mut self, sink: Box<dyn AlertSink>) {
        self.sinks.push(sink);
    }

    /// Add multiple sinks from configurations.
    pub fn add_sinks_from_configs(
        &mut self,
        configs: Vec<SinkConfig>,
    ) -> Result<(), AlertingError> {
        let sinks = AlertSinkFactory::create_sinks(configs)?;
        self.sinks.extend(sinks);
        Ok(())
    }

    /// Set the deduplication window.
    pub fn set_dedup_window(&mut self, seconds: u64) {
        self.dedup_window_seconds = seconds;
    }

    /// Set the maximum alerts per minute rate limit.
    pub fn set_rate_limit(&mut self, max_per_minute: Option<u32>) {
        self.max_alerts_per_minute = max_per_minute;
    }

    /// Send an alert to all configured sinks with improved error handling.
    pub async fn send_alert(
        &mut self,
        alert: &Alert,
    ) -> Result<Vec<DeliveryResult>, AlertingError> {
        // Check deduplication
        if self.is_duplicate(alert) {
            return Ok(vec![]);
        }

        // Check rate limiting
        if self.is_rate_limited() {
            return Err(AlertingError::RateLimitExceeded(
                "Alert rate limit exceeded".to_string(),
            ));
        }

        // Record the alert for deduplication
        self.record_alert(alert);

        // Send to all sinks in parallel with proper error isolation
        let mut tasks = Vec::new();
        for sink in &self.sinks {
            let sink_ref = sink.as_ref();
            let alert_clone = alert.clone();
            let sink_name = sink.name().to_string();
            tasks.push(async move {
                match sink_ref.send(&alert_clone).await {
                    Ok(result) => Ok((sink_name, result)),
                    Err(e) => Err((sink_name, e)),
                }
            });
        }

        let results = futures::future::join_all(tasks).await;
        let mut delivery_results = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok((_sink_name, delivery_result)) => {
                    delivery_results.push(delivery_result);
                }
                Err((sink_name, error)) => {
                    errors.push((sink_name, error));
                }
            }
        }

        // Log errors but don't fail the entire operation
        if !errors.is_empty() {
            for (sink_name, error) in errors {
                eprintln!("Alert delivery failed for sink {}: {}", sink_name, error);
            }
        }

        Ok(delivery_results)
    }

    /// Returns true if the given alert's deduplication key was seen within the manager's deduplication window.
    ///
    /// This checks `recent_alerts` for `alert.deduplication_key` and compares the stored timestamp to
    /// the current time using `dedup_window_seconds`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut mgr = AlertManager::new();
    /// let alert = Alert::new("Title", AlertSeverity::Info, "Desc", "rule-1", ProcessRecord::default());
    /// // Record the alert now; it should be considered a duplicate immediately afterwards.
    /// mgr.record_alert(&alert);
    /// assert!(mgr.is_duplicate(&alert));
    /// ```
    fn is_duplicate(&self, alert: &Alert) -> bool {
        if let Some(last_seen) = self.recent_alerts.get(&alert.deduplication_key) {
            let now = chrono::Utc::now();
            let time_diff = now.signed_duration_since(*last_seen);
            time_diff.num_seconds() < self.dedup_window_seconds as i64
        } else {
            false
        }
    }

    /// Check if we're rate limited.
    fn is_rate_limited(&self) -> bool {
        if let Some(max_per_minute) = self.max_alerts_per_minute {
            let now = chrono::Utc::now();
            let one_minute_ago = now - chrono::Duration::minutes(1);

            let recent_count = self
                .recent_alerts
                .values()
                .filter(|&&timestamp| timestamp > one_minute_ago)
                .count();

            recent_count >= max_per_minute as usize
        } else {
            false
        }
    }

    /// Record the given alert for deduplication and rate-limiting.
    ///
    /// Inserts or updates the alert's `deduplication_key` in the manager's
    /// `recent_alerts` map with the current UTC timestamp. When the map grows
    /// beyond 1000 entries this function prunes entries older than one hour to
    /// bound memory usage.
    ///
    /// This is used by the sending pipeline to (1) detect duplicates within the
    /// configured deduplication window and (2) help enforce rate limits.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use sentinel_lib::{AlertManager, Alert, ProcessRecord, AlertSeverity};
    /// let mut mgr = AlertManager::new();
    /// let process = ProcessRecord::default();
    /// let alert = Alert::new("Title", AlertSeverity::Info, "Desc", "rule-1", process);
    /// mgr.record_alert(&alert);
    /// ```
    fn record_alert(&mut self, alert: &Alert) {
        self.recent_alerts
            .insert(alert.deduplication_key.clone(), chrono::Utc::now());

        // Clean up old entries periodically
        if self.recent_alerts.len() > 1000 {
            let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
            self.recent_alerts
                .retain(|_, &mut timestamp| timestamp > cutoff);
        }
    }

    /// Get all configured sinks.
    pub fn get_sinks(&self) -> &[Box<dyn AlertSink>] {
        &self.sinks
    }

    /// Perform health checks on all sinks with detailed error reporting.
    pub async fn health_check(&self) -> HashMap<String, Result<(), AlertingError>> {
        let mut results = HashMap::new();

        for sink in &self.sinks {
            let sink_name = sink.name().to_string();
            let health_result =
                sink.health_check()
                    .await
                    .map_err(|e| AlertingError::HealthCheckFailed {
                        sink: sink_name.clone(),
                        error: e.to_string(),
                    });
            results.insert(sink_name, health_result);
        }

        results
    }

    /// Get health status summary for all sinks.
    pub async fn health_summary(&self) -> HealthSummary {
        let health_results = self.health_check().await;
        let total_sinks = health_results.len();
        let healthy_sinks = health_results
            .values()
            .filter(|result| result.is_ok())
            .count();
        let unhealthy_sinks = total_sinks - healthy_sinks;

        let details = health_results
            .into_iter()
            .map(|(sink_name, result)| {
                let status = match result {
                    Ok(()) => "healthy".to_string(),
                    Err(e) => format!("unhealthy: {}", e),
                };
                (sink_name, status)
            })
            .collect();

        HealthSummary {
            total_sinks,
            healthy_sinks,
            unhealthy_sinks,
            details,
        }
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Health summary for all alert sinks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthSummary {
    /// Total number of sinks
    pub total_sinks: usize,
    /// Number of healthy sinks
    pub healthy_sinks: usize,
    /// Number of unhealthy sinks
    pub unhealthy_sinks: usize,
    /// Detailed health results for each sink (errors as strings for serialization)
    pub details: HashMap<String, String>,
}

impl HealthSummary {
    /// Check if all sinks are healthy.
    pub fn is_all_healthy(&self) -> bool {
        self.unhealthy_sinks == 0
    }

    /// Get the health percentage.
    pub fn health_percentage(&self) -> f64 {
        if self.total_sinks == 0 {
            100.0
        } else {
            (self.healthy_sinks as f64 / self.total_sinks as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AlertSeverity, ProcessRecord};

    #[tokio::test]
    async fn test_stdout_sink() {
        let sink = StdoutSink::new("test".to_string(), OutputFormat::Json);
        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        let result = sink.send(&alert).await.unwrap();
        assert!(result.success);
        assert_eq!(result.sink_name, "test");
    }

    #[tokio::test]
    async fn test_alert_manager() {
        let mut manager = AlertManager::new();
        let stdout_sink = Box::new(StdoutSink::new("stdout".to_string(), OutputFormat::Json));
        manager.add_sink(stdout_sink);

        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        let results = manager.send_alert(&alert).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let mut manager = AlertManager::new();
        let stdout_sink = Box::new(StdoutSink::new("stdout".to_string(), OutputFormat::Json));
        manager.add_sink(stdout_sink);

        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        // Send the same alert twice
        let results1 = manager.send_alert(&alert).await.unwrap();
        let results2 = manager.send_alert(&alert).await.unwrap();

        assert_eq!(results1.len(), 1);
        assert_eq!(results2.len(), 0); // Should be deduplicated
    }

    #[test]
    fn test_output_format_parsing() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!(
            "human".parse::<OutputFormat>().unwrap(),
            OutputFormat::Human
        );
        assert_eq!("yaml".parse::<OutputFormat>().unwrap(), OutputFormat::Yaml);
        assert_eq!("csv".parse::<OutputFormat>().unwrap(), OutputFormat::Csv);
        assert!("invalid".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_sink_config_creation() {
        let config =
            SinkConfig::new("test-sink", "stdout", OutputFormat::Json).with_config("level", "info");

        assert_eq!(config.name, "test-sink");
        assert_eq!(config.sink_type, "stdout");
        assert_eq!(config.format, OutputFormat::Json);
        assert_eq!(config.config.get("level"), Some(&"info".to_string()));
    }

    #[test]
    fn test_alert_sink_factory() {
        let config = SinkConfig::new("test-sink", "stdout", OutputFormat::Json);
        let sink = AlertSinkFactory::create_sink(config).unwrap();
        assert_eq!(sink.name(), "test-sink");
    }

    #[test]
    fn test_alert_sink_factory_unknown_type() {
        let config = SinkConfig::new("test-sink", "unknown", OutputFormat::Json);
        let result = AlertSinkFactory::create_sink(config);
        assert!(result.is_err());
        if let Err(AlertingError::UnknownSinkType { sink_type }) = result {
            assert_eq!(sink_type, "unknown");
        } else {
            panic!("Expected UnknownSinkType error");
        }
    }

    #[tokio::test]
    async fn test_alert_manager_from_configs() {
        let configs = vec![SinkConfig::new("stdout", "stdout", OutputFormat::Json)];
        let manager = AlertManager::from_configs(configs).unwrap();
        assert_eq!(manager.get_sinks().len(), 1);
    }

    #[tokio::test]
    async fn test_health_summary() {
        let mut manager = AlertManager::new();
        let stdout_sink = Box::new(StdoutSink::new("stdout".to_string(), OutputFormat::Json));
        manager.add_sink(stdout_sink);

        let summary = manager.health_summary().await;
        assert_eq!(summary.total_sinks, 1);
        assert_eq!(summary.healthy_sinks, 1);
        assert_eq!(summary.unhealthy_sinks, 0);
        assert!(summary.is_all_healthy());
        assert_eq!(summary.health_percentage(), 100.0);
    }

    #[test]
    fn test_health_summary_creation() {
        let summary = HealthSummary {
            total_sinks: 5,
            healthy_sinks: 4,
            unhealthy_sinks: 1,
            details: std::collections::HashMap::new(),
        };

        assert_eq!(summary.total_sinks, 5);
        assert_eq!(summary.healthy_sinks, 4);
        assert_eq!(summary.unhealthy_sinks, 1);
        assert!(!summary.is_all_healthy());
        assert_eq!(summary.health_percentage(), 80.0);
    }

    #[test]
    fn test_health_summary_all_healthy() {
        let summary = HealthSummary {
            total_sinks: 3,
            healthy_sinks: 3,
            unhealthy_sinks: 0,
            details: std::collections::HashMap::new(),
        };

        assert!(summary.is_all_healthy());
        assert_eq!(summary.health_percentage(), 100.0);
    }

    #[test]
    fn test_health_summary_no_sinks() {
        let summary = HealthSummary {
            total_sinks: 0,
            healthy_sinks: 0,
            unhealthy_sinks: 0,
            details: std::collections::HashMap::new(),
        };

        assert!(summary.is_all_healthy());
        assert_eq!(summary.health_percentage(), 100.0);
    }

    #[test]
    fn test_delivery_result_creation() {
        let result = DeliveryResult {
            success: true,
            sink_name: "test-sink".to_string(),
            delivered_at: chrono::Utc::now(),
            error_message: None,
            duration_ms: 100,
        };

        assert!(result.success);
        assert_eq!(result.sink_name, "test-sink");
        assert!(result.error_message.is_none());
    }

    #[test]
    fn test_delivery_result_serialization() {
        let result = DeliveryResult {
            success: true,
            sink_name: "test-sink".to_string(),
            delivered_at: chrono::Utc::now(),
            error_message: None,
            duration_ms: 100,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: DeliveryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result.success, deserialized.success);
        assert_eq!(result.sink_name, deserialized.sink_name);
        assert_eq!(result.duration_ms, deserialized.duration_ms);
    }

    #[test]
    fn test_alerting_error_display() {
        let errors = vec![
            AlertingError::UnknownSinkType {
                sink_type: "test".to_string(),
            },
            AlertingError::ConfigurationError("test error".to_string()),
            AlertingError::ConfigurationError("test error".to_string()),
        ];

        for error in errors {
            let error_string = format!("{}", error);
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_sink_config_with_config() {
        let mut config = SinkConfig::new("test", "stdout", OutputFormat::Json);
        config = config.with_config("key1", "value1");
        config = config.with_config("key2", "value2");

        assert_eq!(config.config.get("key1"), Some(&"value1".to_string()));
        assert_eq!(config.config.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_sink_config_serialization() {
        let config =
            SinkConfig::new("test", "stdout", OutputFormat::Json).with_config("level", "info");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SinkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.name, deserialized.name);
        assert_eq!(config.sink_type, deserialized.sink_type);
        assert_eq!(config.format, deserialized.format);
    }

    #[tokio::test]
    async fn test_alert_manager_empty() {
        let manager = AlertManager::new();
        assert_eq!(manager.get_sinks().len(), 0);
    }

    #[tokio::test]
    async fn test_alert_manager_multiple_sinks() {
        let mut manager = AlertManager::new();
        let sink1 = Box::new(StdoutSink::new("sink1".to_string(), OutputFormat::Json));
        let sink2 = Box::new(StdoutSink::new("sink2".to_string(), OutputFormat::Human));
        manager.add_sink(sink1);
        manager.add_sink(sink2);

        assert_eq!(manager.get_sinks().len(), 2);
    }

    #[tokio::test]
    async fn test_alert_manager_health_summary_multiple_sinks() {
        let mut manager = AlertManager::new();
        let sink1 = Box::new(StdoutSink::new("sink1".to_string(), OutputFormat::Json));
        let sink2 = Box::new(StdoutSink::new("sink2".to_string(), OutputFormat::Human));
        manager.add_sink(sink1);
        manager.add_sink(sink2);

        let summary = manager.health_summary().await;
        assert_eq!(summary.total_sinks, 2);
        assert_eq!(summary.healthy_sinks, 2);
        assert_eq!(summary.unhealthy_sinks, 0);
    }
}
