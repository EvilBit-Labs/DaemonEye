//! Collector registry for tracking active collector instances.
//!
//! The registry stores metadata for registered collectors, enforces uniqueness,
//! and tracks the most recent heartbeat timestamp for liveness monitoring.

use daemoneye_eventbus::rpc::{DeregistrationRequest, RegistrationRequest, RegistrationResponse};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};
use thiserror::Error;
use tokio::sync::RwLock;

/// Default heartbeat interval applied when a collector does not request one.
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// In-memory registry for active collectors.
#[derive(Debug)]
pub struct CollectorRegistry {
    records: RwLock<HashMap<String, CollectorRecord>>,
    default_heartbeat: Duration,
}

impl CollectorRegistry {
    /// Create a new registry with the provided default heartbeat interval.
    pub fn new(default_heartbeat: Duration) -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            default_heartbeat,
        }
    }

    /// Register a collector, returning the assigned registration response.
    pub async fn register(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, RegistryError> {
        validate_registration(&request)?;

        let mut records = self.records.write().await;
        let collector_id = request.collector_id.clone();
        if records.contains_key(&collector_id) {
            return Err(RegistryError::AlreadyRegistered(collector_id));
        }

        let now = SystemTime::now();
        let heartbeat_interval = request
            .heartbeat_interval_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_heartbeat);

        let record = CollectorRecord {
            registration: request,
            registered_at: now,
            last_heartbeat: now,
            heartbeat_interval,
        };
        records.insert(collector_id.clone(), record);

        let assigned_topics = vec![
            format!("control.collector.{}", collector_id),
            format!("events.collector.{}", collector_id),
        ];

        Ok(RegistrationResponse {
            collector_id,
            accepted: true,
            heartbeat_interval_ms: heartbeat_interval
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            assigned_topics,
            message: None,
        })
    }

    /// Deregister a collector, removing it from the registry.
    pub async fn deregister(&self, request: DeregistrationRequest) -> Result<(), RegistryError> {
        let mut records = self.records.write().await;
        match records.remove(&request.collector_id) {
            Some(_) => Ok(()),
            None => Err(RegistryError::NotFound(request.collector_id)),
        }
    }

    /// Update the heartbeat timestamp for a collector.
    pub async fn update_heartbeat(&self, collector_id: &str) -> Result<(), RegistryError> {
        let mut records = self.records.write().await;
        match records.get_mut(collector_id) {
            Some(record) => {
                record.last_heartbeat = SystemTime::now();
                Ok(())
            }
            None => Err(RegistryError::NotFound(collector_id.to_string())),
        }
    }

    /// Retrieve a snapshot of registered collectors.
    #[allow(dead_code)]
    pub async fn list(&self) -> Vec<CollectorRecord> {
        let records = self.records.read().await;
        records.values().cloned().collect()
    }

    /// Fetch a collector record if it exists.
    #[allow(dead_code)]
    pub async fn get(&self, collector_id: &str) -> Option<CollectorRecord> {
        let records = self.records.read().await;
        records.get(collector_id).cloned()
    }
}

impl Default for CollectorRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_INTERVAL)
    }
}

/// Registry entry for a registered collector.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CollectorRecord {
    /// Original registration request data.
    pub registration: RegistrationRequest,
    /// Timestamp when the collector was registered.
    pub registered_at: SystemTime,
    /// Timestamp of the last heartbeat received.
    pub last_heartbeat: SystemTime,
    /// Heartbeat interval assigned to the collector.
    pub heartbeat_interval: Duration,
}

/// Errors that can occur when interacting with the collector registry.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Collector is already registered.
    #[error("collector `{0}` is already registered")]
    AlreadyRegistered(String),
    /// Collector could not be found for the requested operation.
    #[error("collector `{0}` is not registered")]
    NotFound(String),
    /// Registration request failed validation.
    #[error("registration validation failed: {0}")]
    Validation(String),
}

fn validate_registration(request: &RegistrationRequest) -> Result<(), RegistryError> {
    if request.collector_id.trim().is_empty() {
        return Err(RegistryError::Validation(
            "collector_id cannot be empty".to_string(),
        ));
    }

    if request.collector_type.trim().is_empty() {
        return Err(RegistryError::Validation(
            "collector_type cannot be empty".to_string(),
        ));
    }

    if request.hostname.trim().is_empty() {
        return Err(RegistryError::Validation(
            "hostname cannot be empty".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_request() -> RegistrationRequest {
        RegistrationRequest {
            collector_id: "procmond".to_string(),
            collector_type: "procmond".to_string(),
            hostname: "localhost".to_string(),
            version: Some("1.2.3".to_string()),
            pid: Some(1001),
            capabilities: vec!["process".to_string()],
            attributes: HashMap::new(),
            heartbeat_interval_ms: Some(10_000),
        }
    }

    #[tokio::test]
    async fn register_and_list() {
        let registry = CollectorRegistry::default();
        let response = registry
            .register(sample_request())
            .await
            .expect("registration succeeds");
        assert!(response.accepted);
        assert_eq!(response.collector_id, "procmond");

        let records = registry.list().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].registration.collector_id, "procmond");
    }

    #[tokio::test]
    async fn prevent_duplicate_registration() {
        let registry = CollectorRegistry::default();
        registry
            .register(sample_request())
            .await
            .expect("first registration succeeds");

        let error = registry
            .register(sample_request())
            .await
            .expect_err("duplicate registration rejected");
        assert!(matches!(error, RegistryError::AlreadyRegistered(id) if id == "procmond"));
    }

    #[tokio::test]
    async fn deregister_removes_entry() {
        let registry = CollectorRegistry::default();
        registry
            .register(sample_request())
            .await
            .expect("registration succeeds");
        registry
            .deregister(DeregistrationRequest {
                collector_id: "procmond".to_string(),
                reason: None,
                force: false,
            })
            .await
            .expect("deregistration succeeds");
        assert!(registry.list().await.is_empty());
    }
}
