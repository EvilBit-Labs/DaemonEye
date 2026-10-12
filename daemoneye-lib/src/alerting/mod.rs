//! Multi-channel alert delivery system.
//!
//! This module provides a flexible alert delivery system with support for multiple
//! output channels including stdout, files, webhooks, syslog, and email.
//!
//! Features:
//! - Factory pattern for creating alert sinks
//! - Comprehensive error handling with specific error types
//! - Concurrent alert delivery with proper error isolation
//! - Durable outbox delivery with retry, circuit breaking and dead-lettering
//! - Deduplication window and paced delivery
//! - Health monitoring for all sinks

pub mod config;
mod delivery;
pub mod enrich;
mod factory;
mod health;
pub mod reliability;
mod render;

pub use config::{CredentialResolver, ResolvedSink, SinkSpec};
pub use delivery::{
    AlertManager, AlertPublisher, EnqueueOutcome, NoopPublisher, PublishError, REDRIVE_BATCH,
    SHUTDOWN_DRAIN,
};
pub use enrich::{EnrichContext, enrich};
pub use factory::{AlertSinkFactory, SinkBuilder};
pub use health::{DeliveryAggregate, DeliveryHealth, SinkHealthSnapshot};
pub use reliability::{
    BreakerState, CircuitBreaker, DeliveryError, DeliveryErrorClass, Permit, RetryPolicy,
};
pub use render::{ExternalAlert, IncludeFlags, sanitise_control};

use crate::models::{Alert, Completeness};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use thiserror::Error;

/// Alert delivery errors with specific error types for better error handling.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AlertingError {
    #[error("Alert sink error: {0}")]
    SinkError(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Delivery timeout: {0}")]
    Timeout(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

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

    /// A classified delivery failure from a sink.
    #[error("Delivery failed: {0}")]
    Delivery(#[from] DeliveryError),
}

/// Alert delivery result.
///
/// This struct is returned inside `Ok(...)` from [`AlertSink::send`], meaning
/// delivery succeeded. Error cases are represented by `Err(AlertingError)` and
/// never reach this type. There is therefore no `success` flag or
/// `error_message` field — those semantics belong to the `Result` wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryResult {
    /// Sink name
    pub sink_name: String,
    /// Delivery timestamp
    pub delivered_at: chrono::DateTime<chrono::Utc>,
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

    /// Whether the sink stays on the host. Local sinks never trip a circuit breaker and receive
    /// alerts the store could not persist.
    fn is_local(&self) -> bool {
        false
    }

    /// Whether the sink cannot confirm receipt, so a delivery is only a send.
    fn is_fire_and_forget(&self) -> bool {
        false
    }
}

/// Standard output alert sink.
pub struct StdoutSink {
    name: String,
    format: OutputFormat,
}

impl StdoutSink {
    /// Create a new stdout sink.
    pub const fn new(name: String, format: OutputFormat) -> Self {
        Self { name, format }
    }
}

#[async_trait]
impl AlertSink for StdoutSink {
    /// Sends the alert to stdout using the sink's configured output format and returns a `DeliveryResult`.
    ///
    /// The alert is formatted according to `self.format`:
    /// - `Json`: pretty-prints the alert as JSON (may produce `AlertingError::SerializationError`).
    /// - `Human`: a concise human-readable single-line string using `severity`, `detection_rule_id`, `title`, and `description`.
    /// - `Csv`: a single-line CSV with `id,severity,detection_rule_id,title,description`.
    ///
    /// Returns a `DeliveryResult` containing the sink name, the delivery timestamp, and the elapsed duration in milliseconds.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use daemoneye_lib::alerting::{AlertSink, StdoutSink, OutputFormat};
    /// use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord};
    ///
    /// let sink = StdoutSink::new("stdout-test".to_owned(), OutputFormat::Human);
    /// let alert = Alert::new(
    ///     AlertSeverity::Low,
    ///     "Test",
    ///     "Example",
    ///     "rule-1",
    ///     ProcessRecord::new(1, "proc".to_owned()),
    ///     Completeness::complete(),
    /// );
    /// let rt = tokio::runtime::Runtime::new().unwrap();
    /// let res = rt.block_on(async { sink.send(&alert).await }).unwrap();
    /// assert_eq!(res.sink_name, "stdout-test");
    /// ```
    #[allow(clippy::print_stdout)] // Intentional stdout output for StdoutSink
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = render_alert(&self.format, alert)?;

        println!("{output}");

        let duration = start_time.elapsed();
        Ok(DeliveryResult {
            sink_name: self.name.clone(),
            delivered_at: chrono::Utc::now(),
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(0),
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        // stdout is always available
        Ok(())
    }

    fn is_local(&self) -> bool {
        true
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
    pub const fn new(name: String, file_path: std::path::PathBuf, format: OutputFormat) -> Self {
        Self {
            name,
            file_path,
            format,
        }
    }
}

#[async_trait]
impl AlertSink for FileSink {
    /// Appends the given alert to the configured file using the sink's `OutputFormat` and returns a `DeliveryResult` with timing and delivery metadata.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use daemoneye_lib::alerting::{AlertSink, FileSink, OutputFormat};
    /// use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord};
    /// use std::path::PathBuf;
    ///
    /// let path = PathBuf::from("/tmp/daemoneye-alerts.log");
    /// let sink = FileSink::new("file".to_owned(), path.clone(), OutputFormat::Json);
    /// let alert = Alert::new(
    ///     AlertSeverity::High,
    ///     "Title",
    ///     "Desc",
    ///     "rule-1",
    ///     ProcessRecord::new(1, "proc".to_owned()),
    ///     Completeness::complete(),
    /// );
    /// let rt = tokio::runtime::Runtime::new().unwrap();
    /// let _ = rt.block_on(async { sink.send(&alert).await }).unwrap();
    /// assert!(path.exists());
    /// ```
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = render_alert(&self.format, alert)?;

        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?
            .write_all(format!("{output}\n").as_bytes())?;

        let duration = start_time.elapsed();
        Ok(DeliveryResult {
            sink_name: self.name.clone(),
            delivered_at: chrono::Utc::now(),
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(0),
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

    fn is_local(&self) -> bool {
        true
    }
}

/// Output format enumeration with string parsing support.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum OutputFormat {
    Json,
    Human,
    Csv,
}

impl std::str::FromStr for OutputFormat {
    type Err = AlertingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "human" => Ok(Self::Human),
            "csv" => Ok(Self::Csv),
            _ => Err(AlertingError::ConfigurationError(format!(
                "Unknown output format: {s}"
            ))),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Json => write!(f, "json"),
            Self::Human => write!(f, "human"),
            Self::Csv => write!(f, "csv"),
        }
    }
}

/// Health summary for all alert sinks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub const fn is_all_healthy(&self) -> bool {
        self.unhealthy_sinks == 0
    }

    /// Get the health percentage.
    #[allow(clippy::arithmetic_side_effects)]
    pub fn health_percentage(&self) -> f64 {
        if self.total_sinks == 0 {
            100.0
        } else {
            let healthy = f64::from(u32::try_from(self.healthy_sinks).unwrap_or(0));
            let total = f64::from(u32::try_from(self.total_sinks).unwrap_or(1).max(1));
            (healthy / total) * 100.0
        }
    }
}

/// `field` quoted for a CSV cell: wrapped in double quotes with any inner quote doubled, so a
/// comma or quote inside it cannot shift the columns.
/// One alert in `format`.
///
/// Every format carries the completeness marker: an operator reading Human or CSV output must be
/// able to tell a complete detection from a degraded one, as the JSON reader can.
fn render_alert(format: &OutputFormat, alert: &Alert) -> Result<String, AlertingError> {
    Ok(match *format {
        OutputFormat::Json => serde_json::to_string_pretty(alert)?,
        OutputFormat::Human => format!(
            "[{}] {} - {}: {} [{}]",
            alert.severity,
            alert.detection_rule_id,
            alert.title,
            alert.description,
            completeness_text(&alert.completeness)
        ),
        OutputFormat::Csv => format!(
            "{},{},{},{},{},{}",
            alert.id,
            alert.severity,
            csv_field(&alert.detection_rule_id),
            csv_field(&alert.title),
            csv_field(&alert.description),
            csv_field(&completeness_text(&alert.completeness))
        ),
    })
}

/// `complete`, or `degraded: <reason>; <reason>`.
fn completeness_text(completeness: &Completeness) -> String {
    if completeness.reasons().is_empty() {
        return "complete".to_owned();
    }
    let reasons: Vec<String> = completeness
        .reasons()
        .iter()
        .map(|reason| format!("{reason:?}"))
        .collect();
    format!("degraded: {}", reasons.join("; "))
}

fn csv_field(field: &str) -> String {
    format!("\"{}\"", field.replace('"', "\"\""))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::assertions_on_constants,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::models::{AlertSeverity, Completeness, ProcessRecord};

    /// Human and CSV output carry the completeness marker, as JSON does through the field.
    #[test]
    fn human_and_csv_output_carry_the_completeness_marker() {
        use crate::models::{AlertSeverity, CompletenessReason, ProcessRecord};
        let degraded = Completeness::degraded(vec![CompletenessReason::ResultCapped { cap: 3 }])
            .expect("one reason");
        let alert = Alert::new(
            AlertSeverity::High,
            "Capped",
            "more matches than the cap",
            "rule-1",
            ProcessRecord::new(1, "proc".to_owned()),
            degraded,
        );
        let complete = Alert::new(
            AlertSeverity::High,
            "Plain",
            "a match",
            "rule-1",
            ProcessRecord::new(1, "proc".to_owned()),
            Completeness::complete(),
        );

        let human = render_alert(&OutputFormat::Human, &alert).expect("renders");
        assert!(
            human.ends_with("[degraded: ResultCapped { cap: 3 }]"),
            "{human}"
        );
        assert!(
            render_alert(&OutputFormat::Human, &complete)
                .expect("renders")
                .ends_with("[complete]")
        );

        let csv = render_alert(&OutputFormat::Csv, &alert).expect("renders");
        assert_eq!(csv.matches(',').count(), 5, "six columns");
        assert!(
            csv.ends_with(",\"degraded: ResultCapped { cap: 3 }\""),
            "{csv}"
        );
        assert!(
            render_alert(&OutputFormat::Csv, &complete)
                .expect("renders")
                .ends_with(",\"complete\"")
        );
    }

    /// A title carrying a comma or a quote stays one CSV cell.
    #[test]
    fn csv_fields_are_quoted_and_inner_quotes_doubled() {
        assert_eq!(csv_field("plain"), "\"plain\"");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[tokio::test]
    async fn test_stdout_sink() {
        let sink = StdoutSink::new("test".to_owned(), OutputFormat::Json);
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
            Completeness::complete(),
        );

        let result = sink.send(&alert).await.expect("Failed to send alert");
        assert_eq!(result.sink_name, "test");
    }

    #[test]
    fn test_output_format_parsing() {
        assert_eq!(
            "json"
                .parse::<OutputFormat>()
                .expect("Failed to parse json"),
            OutputFormat::Json
        );
        assert_eq!(
            "human"
                .parse::<OutputFormat>()
                .expect("Failed to parse human"),
            OutputFormat::Human
        );

        assert_eq!(
            "csv".parse::<OutputFormat>().expect("Failed to parse csv"),
            OutputFormat::Csv
        );
        assert!("invalid".parse::<OutputFormat>().is_err());
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
            sink_name: "test-sink".to_owned(),
            delivered_at: chrono::Utc::now(),
            duration_ms: 100,
        };

        assert_eq!(result.sink_name, "test-sink");
        assert_eq!(result.duration_ms, 100);
    }

    #[test]
    fn test_delivery_result_serialization() {
        let result = DeliveryResult {
            sink_name: "test-sink".to_owned(),
            delivered_at: chrono::Utc::now(),
            duration_ms: 100,
        };

        let json = serde_json::to_string(&result).expect("Failed to serialize delivery result");
        let deserialized: DeliveryResult =
            serde_json::from_str(&json).expect("Failed to deserialize delivery result");
        assert_eq!(result.sink_name, deserialized.sink_name);
        assert_eq!(result.duration_ms, deserialized.duration_ms);
    }

    #[test]
    fn test_alerting_error_display() {
        let errors = vec![
            AlertingError::UnknownSinkType {
                sink_type: "test".to_owned(),
            },
            AlertingError::ConfigurationError("test error".to_owned()),
            AlertingError::ConfigurationError("test error".to_owned()),
        ];

        for error in errors {
            let error_string = format!("{}", error);
            assert!(!error_string.is_empty());
        }
    }
}
