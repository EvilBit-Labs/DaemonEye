//! Multi-channel alert delivery system.
//!
//! This module provides a flexible alert delivery system with support for multiple
//! output channels including stdout, files, webhooks, syslog, and email.

use crate::models::Alert;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use thiserror::Error;

/// Alert delivery errors.
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

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
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
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = match self.format {
            OutputFormat::Json => serde_json::to_string_pretty(alert)?,
            OutputFormat::Human => format!(
                "[{}] {} - {}: {}",
                alert.severity, alert.category, alert.title, alert.description
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
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let start_time = std::time::Instant::now();

        let output = match self.format {
            OutputFormat::Json => serde_json::to_string_pretty(alert)?,
            OutputFormat::Human => format!(
                "[{}] {} - {}: {}",
                alert.severity, alert.category, alert.title, alert.description
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

/// Output format enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OutputFormat {
    Json,
    Human,
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

    /// Add an alert sink.
    pub fn add_sink(&mut self, sink: Box<dyn AlertSink>) {
        self.sinks.push(sink);
    }

    /// Set the deduplication window.
    pub fn set_dedup_window(&mut self, seconds: u64) {
        self.dedup_window_seconds = seconds;
    }

    /// Set the maximum alerts per minute rate limit.
    pub fn set_rate_limit(&mut self, max_per_minute: Option<u32>) {
        self.max_alerts_per_minute = max_per_minute;
    }

    /// Send an alert to all configured sinks.
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
            return Err(AlertingError::SinkError("Rate limit exceeded".to_string()));
        }

        // Record the alert for deduplication
        self.record_alert(alert);

        // Send to all sinks in parallel
        let mut tasks = Vec::new();
        for sink in &self.sinks {
            let sink_ref = sink.as_ref();
            let alert_clone = alert.clone();
            tasks.push(async move { sink_ref.send(&alert_clone).await });
        }

        let results = futures::future::join_all(tasks).await;
        let mut delivery_results = Vec::new();

        for result in results {
            match result {
                Ok(delivery_result) => delivery_results.push(delivery_result),
                Err(e) => {
                    // Log error but continue with other sinks
                    eprintln!("Alert delivery failed: {}", e);
                }
            }
        }

        Ok(delivery_results)
    }

    /// Check if an alert is a duplicate.
    fn is_duplicate(&self, alert: &Alert) -> bool {
        if let Some(last_seen) = self.recent_alerts.get(&alert.dedup_key) {
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

    /// Record an alert for deduplication and rate limiting.
    fn record_alert(&mut self, alert: &Alert) {
        self.recent_alerts
            .insert(alert.dedup_key.clone(), chrono::Utc::now());

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

    /// Perform health checks on all sinks.
    pub async fn health_check(&self) -> HashMap<String, Result<(), AlertingError>> {
        let mut results = HashMap::new();

        for sink in &self.sinks {
            let sink_name = sink.name().to_string();
            let health_result = sink.health_check().await;
            results.insert(sink_name, health_result);
        }

        results
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AlertSeverity;

    #[tokio::test]
    async fn test_stdout_sink() {
        let sink = StdoutSink::new("test".to_string(), OutputFormat::Json);
        let alert = Alert::new(
            "test-alert".to_string(),
            AlertSeverity::High,
            "Test Alert".to_string(),
            "This is a test alert".to_string(),
            "test".to_string(),
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

        let alert = Alert::new(
            "test-alert".to_string(),
            AlertSeverity::High,
            "Test Alert".to_string(),
            "This is a test alert".to_string(),
            "test".to_string(),
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

        let alert = Alert::new(
            "test-alert".to_string(),
            AlertSeverity::High,
            "Test Alert".to_string(),
            "This is a test alert".to_string(),
            "test".to_string(),
        );

        // Send the same alert twice
        let results1 = manager.send_alert(&alert).await.unwrap();
        let results2 = manager.send_alert(&alert).await.unwrap();

        assert_eq!(results1.len(), 1);
        assert_eq!(results2.len(), 0); // Should be deduplicated
    }
}
