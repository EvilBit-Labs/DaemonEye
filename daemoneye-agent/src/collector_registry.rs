//! Collector registry for tracking active collector instances.
//!
//! The registry stores metadata for registered collectors, enforces uniqueness,
//! and tracks the most recent heartbeat timestamp for liveness monitoring.

use crate::collector_admission::{AdmissionError, CollectorAdmission, gate_for_admission};
use daemoneye_eventbus::rpc::{DeregistrationRequest, RegistrationRequest, RegistrationResponse};
use daemoneye_lib::detection_bounds::MAX_IDENTIFIER_LENGTH;
use daemoneye_lib::rejection_log::{
    RegistrationGate, RejectionLog, RejectionReason, RejectionRecord,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
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
    /// Agent-side chain of refused registrations (R8). Kept behind a synchronous lock that is
    /// never held across an `.await`; the `audit_ledger` table is procmond's to write.
    rejections: Mutex<RejectionLog>,
    /// Whether, and how, a registration is authenticated (R8, R10).
    admission: AdmissionPolicy,
}

/// How a registry treats an arriving registration.
///
/// An enum rather than an `Option<Arc<CollectorAdmission>>` because the absent case is not one
/// state but two with opposite meanings, and the old shape silently defaulted to the permissive
/// one: when the spawn-token store could not be opened, every collector registered unauthenticated.
/// Naming the three states makes that downgrade impossible to reach without writing its name.
#[derive(Debug)]
enum AdmissionPolicy {
    /// Every registration must present the spawn token issued for its identity.
    Gated(Arc<CollectorAdmission>),
    /// No gate could be opened, so nothing may register. Fail-closed: a monitoring daemon with no
    /// collectors is degraded and observable; one that admits unauthenticated collectors is not.
    Closed,
    /// No gate at all, and every registration admitted.
    ///
    /// **Test-only.** Nothing in the agent constructs this; it exists so tests that exercise
    /// heartbeats and the state machine need not stand up a token store. It is `pub`-reachable
    /// only through [`CollectorRegistry::unauthenticated`], whose name is the warning.
    // Dead in the binary on purpose: that the agent never constructs this is the property, and
    // the compiler saying so is the proof. The library's tests and `tests/` do construct it.
    #[allow(dead_code)]
    Unauthenticated,
}

impl CollectorRegistry {
    /// Create a registry with the provided default heartbeat interval and admission policy.
    fn new(default_heartbeat: Duration, admission: AdmissionPolicy) -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            default_heartbeat,
            rejections: Mutex::new(RejectionLog::new()),
            admission,
        }
    }

    /// Create a registry that admits only collectors presenting their issued spawn token (R8).
    pub fn with_admission(admission: Arc<CollectorAdmission>) -> Self {
        Self::new(
            DEFAULT_HEARTBEAT_INTERVAL,
            AdmissionPolicy::Gated(admission),
        )
    }

    /// Create a registry that refuses every registration, for use when no gate could be opened.
    ///
    /// The spawn-token store is what authenticates a collector, so a store that cannot be opened
    /// leaves nothing to authenticate against. Every registration is refused as
    /// [`RegistrationGate::NoTokenPresented`] and recorded, so the condition is visible in the
    /// rejection chain rather than inferred from collectors that silently succeed.
    pub fn closed() -> Self {
        Self::new(DEFAULT_HEARTBEAT_INTERVAL, AdmissionPolicy::Closed)
    }

    /// Create a registry that admits every registration without authenticating it.
    ///
    /// **Test-only**, and deliberately not reachable by accident: there is no `Default` impl, so
    /// every construction names which of the three policies it meant. Nothing in the agent calls
    /// this.
    // Dead in the binary for the same reason the variant is; see its note.
    #[allow(dead_code)]
    pub fn unauthenticated() -> Self {
        Self::new(DEFAULT_HEARTBEAT_INTERVAL, AdmissionPolicy::Unauthenticated)
    }

    /// Register a collector, returning the assigned registration response.
    ///
    /// Every refusal is recorded in the registry's rejection chain before it is returned.
    pub async fn register(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, RegistryError> {
        let collector_id = request.collector_id.clone();
        let result = self.register_inner(request).await;
        if let Err(ref error) = result {
            self.record_registration_rejection(&collector_id, gate_for(error));
        }
        result
    }

    /// Record a refused registration against `collector_id`.
    ///
    /// The gate is the whole reason: no spawn token, nor any prefix, length or digest of one,
    /// crosses this boundary, because [`RegistrationGate`] has nowhere to hold it.
    ///
    /// The identity is truncated to [`MAX_IDENTIFIER_LENGTH`] characters here as well as refused
    /// by `validate_registration`. Refusing bounds what may register; truncating bounds what is
    /// *retained*, and the retention path is reached before authentication, so the bound has to
    /// sit on the recording side too or the chain still holds up to a transport frame per record.
    pub fn record_registration_rejection(&self, collector_id: &str, gate: RegistrationGate) {
        // Bounded in *bytes*, matching how `validate_registration` reads the same constant:
        // `chars().take(..)` would bound characters and admit four times the bytes for multi-byte
        // UTF-8. `char_indices` keeps the split on a character boundary without slicing, which
        // `clippy::string_slice` forbids on untrusted input.
        let bounded: String = collector_id
            .char_indices()
            .take_while(|&(offset, character)| {
                offset.saturating_add(character.len_utf8()) <= MAX_IDENTIFIER_LENGTH
            })
            .map(|(_offset, character)| character)
            .collect();
        let mut log = self
            .rejections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _record = log.record(RejectionReason::registration(&bounded, gate));
    }

    /// Every registration rejection recorded so far, oldest first.
    // Read path for the rejection chain; unused by the binary until the CLI surfaces it.
    #[allow(dead_code)]
    pub fn rejection_records(&self) -> Vec<RejectionRecord> {
        let log = self
            .rejections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        log.records().iter().cloned().collect()
    }

    async fn register_inner(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, RegistryError> {
        validate_registration(&request)?;

        // The registry lock is held across authentication on purpose. Admission mutates the schema
        // catalog and re-plans rules, and a registration this function goes on to refuse must not
        // leave either of them changed — so every reason to refuse is evaluated first, and no
        // concurrent registration can slip between the duplicate check and the admission.
        #[allow(clippy::significant_drop_tightening)]
        let mut records = self.records.write().await;
        let collector_id = request.collector_id.clone();
        if records.contains_key(&collector_id) {
            return Err(RegistryError::AlreadyRegistered(collector_id));
        }

        // Exhaustive on purpose: a fourth policy must break this match rather than fall into a
        // wildcard that would decide, by default, not to authenticate.
        match self.admission {
            AdmissionPolicy::Gated(ref admission) => admission.admit(&request).await?,
            AdmissionPolicy::Closed => {
                return Err(RegistryError::NotAdmitted {
                    message: "collector registration cannot be authenticated: no spawn-token store"
                        .to_owned(),
                    gate: RegistrationGate::NoTokenPresented,
                });
            }
            AdmissionPolicy::Unauthenticated => {}
        }

        let now = SystemTime::now();
        let heartbeat_interval = request
            .heartbeat_interval_ms
            .map_or(self.default_heartbeat, Duration::from_millis);

        let record = CollectorRecord {
            registration: request,
            registered_at: now,
            last_heartbeat: now,
            heartbeat_interval,
            missed_heartbeats: 0,
        };
        records.insert(collector_id.clone(), record);
        drop(records);

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

    /// Update the heartbeat timestamp for a collector and reset missed count.
    pub async fn update_heartbeat(&self, collector_id: &str) -> Result<(), RegistryError> {
        let mut records = self.records.write().await;
        match records.get_mut(collector_id) {
            Some(record) => {
                record.last_heartbeat = SystemTime::now();
                record.missed_heartbeats = 0;
                Ok(())
            }
            None => Err(RegistryError::NotFound(collector_id.to_owned())),
        }
    }

    /// Check heartbeat status for all collectors and increment missed counts.
    ///
    /// This should be called periodically (e.g., every heartbeat interval) to
    /// detect collectors that have stopped sending heartbeats.
    ///
    /// Returns a list of (`collector_id`, `HeartbeatStatus`) for collectors that
    /// have missed at least one heartbeat.
    #[allow(dead_code)]
    #[allow(clippy::significant_drop_tightening)] // Lock must be held while iterating and mutating
    pub async fn check_heartbeats(&self) -> Vec<(String, HeartbeatStatus)> {
        let now = SystemTime::now();
        let mut records = self.records.write().await;
        let mut results = Vec::new();

        for (collector_id, record) in records.iter_mut() {
            let elapsed = now
                .duration_since(record.last_heartbeat)
                .unwrap_or(Duration::ZERO);

            // Check if heartbeat is overdue (allow 10% grace period)
            let expected_interval = record.heartbeat_interval;
            // Use saturating_add to avoid overflow; division by 10 is always safe
            #[allow(clippy::arithmetic_side_effects)]
            let grace_period = expected_interval.saturating_add(expected_interval / 10);

            if elapsed > grace_period {
                record.missed_heartbeats = record.missed_heartbeats.saturating_add(1);

                let status = if record.missed_heartbeats >= MAX_MISSED_HEARTBEATS {
                    HeartbeatStatus::Failed {
                        missed_count: record.missed_heartbeats,
                        time_since_last: elapsed,
                    }
                } else {
                    HeartbeatStatus::Degraded {
                        missed_count: record.missed_heartbeats,
                    }
                };

                results.push((collector_id.clone(), status));
            }
        }

        results
    }

    /// Get collectors that need recovery action (missed >= `MAX_MISSED_HEARTBEATS`).
    #[allow(dead_code)]
    #[allow(clippy::pattern_type_mismatch)] // Conflicting lint with needless_borrowed_reference
    pub async fn collectors_needing_recovery(&self) -> Vec<(String, HeartbeatStatus)> {
        self.check_heartbeats()
            .await
            .into_iter()
            .filter(|(_, status)| status.needs_recovery())
            .collect()
    }

    /// Get the heartbeat status for a specific collector.
    #[allow(dead_code)]
    pub async fn heartbeat_status(&self, collector_id: &str) -> Option<HeartbeatStatus> {
        let now = SystemTime::now();

        // Clone record to release lock early
        let record = {
            let records = self.records.read().await;
            records.get(collector_id).cloned()?
        };

        let elapsed = now
            .duration_since(record.last_heartbeat)
            .unwrap_or(Duration::ZERO);

        let expected_interval = record.heartbeat_interval;
        // Division by 10 is always safe
        #[allow(clippy::arithmetic_side_effects)]
        let grace_period = expected_interval.saturating_add(expected_interval / 10);

        if elapsed <= grace_period && record.missed_heartbeats == 0 {
            Some(HeartbeatStatus::Healthy)
        } else if record.missed_heartbeats >= MAX_MISSED_HEARTBEATS {
            Some(HeartbeatStatus::Failed {
                missed_count: record.missed_heartbeats,
                time_since_last: elapsed,
            })
        } else if record.missed_heartbeats > 0 {
            Some(HeartbeatStatus::Degraded {
                missed_count: record.missed_heartbeats,
            })
        } else {
            Some(HeartbeatStatus::Healthy)
        }
    }

    /// Reset missed heartbeat count for a collector (e.g., after successful recovery).
    #[allow(dead_code)]
    pub async fn reset_missed_heartbeats(&self, collector_id: &str) -> Result<(), RegistryError> {
        let mut records = self.records.write().await;
        match records.get_mut(collector_id) {
            Some(record) => {
                record.missed_heartbeats = 0;
                Ok(())
            }
            None => Err(RegistryError::NotFound(collector_id.to_owned())),
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

    /// Get list of registered collector IDs.
    #[allow(dead_code)]
    pub async fn list_collector_ids(&self) -> Vec<String> {
        let records = self.records.read().await;
        records.keys().cloned().collect()
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
    /// Number of consecutive missed heartbeats.
    pub missed_heartbeats: u32,
}

/// Maximum consecutive missed heartbeats before triggering recovery actions.
#[allow(dead_code)]
pub const MAX_MISSED_HEARTBEATS: u32 = 3;

/// Status of a collector's heartbeat health.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
#[non_exhaustive]
pub enum HeartbeatStatus {
    /// Collector is healthy - heartbeat received within expected interval.
    Healthy,
    /// Collector missed one or more heartbeats but below threshold.
    Degraded {
        /// Number of consecutive missed heartbeats.
        missed_count: u32,
    },
    /// Collector has missed too many heartbeats - recovery action needed.
    Failed {
        /// Number of consecutive missed heartbeats.
        missed_count: u32,
        /// Duration since last heartbeat.
        time_since_last: Duration,
    },
}

#[allow(dead_code)]
impl HeartbeatStatus {
    /// Returns true if the collector needs recovery action.
    #[must_use]
    pub const fn needs_recovery(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Returns true if the collector is healthy.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// Errors that can occur when interacting with the collector registry.
#[derive(Debug, Error)]
#[non_exhaustive]
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
    /// The registration did not authenticate, or its descriptor was refused.
    #[error("{message}")]
    NotAdmitted {
        /// The admission error's own diagnostic, verbatim. Never the presented token.
        message: String,
        /// Which gate refused it, carried so the rejection record names the specific gate instead
        /// of collapsing every admission failure into a generic validation failure.
        gate: RegistrationGate,
    },
}

impl From<AdmissionError> for RegistryError {
    fn from(error: AdmissionError) -> Self {
        Self::NotAdmitted {
            message: error.to_string(),
            gate: gate_for_admission(&error),
        }
    }
}

/// Map a registration failure onto the gate that refused it.
const fn gate_for(error: &RegistryError) -> RegistrationGate {
    match *error {
        RegistryError::AlreadyRegistered(_) => RegistrationGate::AlreadyRegistered,
        // `NotFound` means "absent from the registry map", not U6's "no token was issued", so
        // it deliberately does not claim `UnknownCollector`.
        RegistryError::NotAdmitted { gate, .. } => gate,
        RegistryError::Validation(_) | RegistryError::NotFound(_) => {
            RegistrationGate::MalformedRequest
        }
    }
}

fn validate_registration(request: &RegistrationRequest) -> Result<(), RegistryError> {
    check_identity_field("collector_id", &request.collector_id)?;
    check_identity_field("collector_type", &request.collector_type)?;
    check_identity_field("hostname", &request.hostname)?;
    Ok(())
}

/// Reject a blank or over-long registration identity field.
///
/// The length bound is what keeps a pre-authentication path from retaining unbounded bytes: this
/// runs before `admit`, and every refusal here writes a rejection record. `SpawnTokenStore::issue`
/// already caps a collector id at 64 characters, so an identity longer than
/// [`MAX_IDENTIFIER_LENGTH`] could never have authenticated — it only ever reached the recorder.
fn check_identity_field(field: &str, value: &str) -> Result<(), RegistryError> {
    if value.trim().is_empty() {
        return Err(RegistryError::Validation(format!(
            "{field} cannot be empty"
        )));
    }
    if value.len() > MAX_IDENTIFIER_LENGTH {
        return Err(RegistryError::Validation(format!(
            "{field} exceeds the {MAX_IDENTIFIER_LENGTH}-byte bound"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::str_to_string,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]
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
            descriptor: None,
            spawn_token: None,
        }
    }

    #[tokio::test]
    async fn register_and_list() {
        let registry = CollectorRegistry::unauthenticated();
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
        let registry = CollectorRegistry::unauthenticated();
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
    async fn rejected_registration_is_recorded_without_the_presented_token() {
        // Arrange
        const TOKEN: &str = "ZZsupersecretspawntokenZZ";
        let registry = CollectorRegistry::unauthenticated();
        let mut request = sample_request();
        request.hostname = String::new();
        request.spawn_token = Some(TOKEN.to_string());

        // Act
        let error = registry
            .register(request)
            .await
            .expect_err("blank hostname is refused");

        // Assert
        assert!(matches!(error, RegistryError::Validation(_)));
        let records = registry.rejection_records();
        assert_eq!(records.len(), 1);
        let surfaces = [records[0].reason.to_string(), format!("{:?}", records[0])];
        for surface in &surfaces {
            // Message carries no secret material: CodeQL rust/cleartext-logging.
            assert!(
                !surface.contains(TOKEN),
                "a record surface reproduced the presented token"
            );
        }
        assert!(records[0].reason.to_string().contains("procmond"));
    }

    #[tokio::test]
    async fn accepted_registration_records_nothing() {
        let registry = CollectorRegistry::unauthenticated();
        registry
            .register(sample_request())
            .await
            .expect("registration succeeds");
        assert!(registry.rejection_records().is_empty());
    }

    #[tokio::test]
    async fn a_closed_registry_refuses_and_records_every_registration() {
        // Arrange: the registry the agent builds when no spawn-token store could be opened.
        let registry = CollectorRegistry::closed();

        // Act
        let error = registry
            .register(sample_request())
            .await
            .expect_err("a closed registry authenticates nothing, so it admits nothing");

        // Assert
        assert!(matches!(
            error,
            RegistryError::NotAdmitted {
                gate: RegistrationGate::NoTokenPresented,
                ..
            }
        ));
        assert!(registry.list().await.is_empty());
        let records = registry.rejection_records();
        assert_eq!(records.len(), 1);
        assert!(
            records[0]
                .reason
                .to_string()
                .contains("no spawn token presented")
        );
    }

    #[tokio::test]
    async fn an_over_long_identity_is_refused_and_its_record_is_bounded() {
        // Arrange
        let registry = CollectorRegistry::unauthenticated();
        let mut request = sample_request();
        request.collector_id = "a".repeat(MAX_IDENTIFIER_LENGTH * 16);
        let presented_length = request.collector_id.len();

        // Act
        let error = registry
            .register(request)
            .await
            .expect_err("an identity past the bound is refused");

        // Assert
        assert!(matches!(error, RegistryError::Validation(_)));
        let records = registry.rejection_records();
        assert_eq!(records.len(), 1);
        // The whole rendered reason, not just the identity: an untruncated record would carry the
        // full presented id, and the bound has to hold for what is actually retained.
        let rendered_length = records[0].reason.to_string().len();
        assert!(
            rendered_length <= MAX_IDENTIFIER_LENGTH * 2,
            "the recorded reason must stay bounded by the identifier bound, got {rendered_length}"
        );
        // Removing the truncation leaves the whole presented identity in the record, so the
        // rendered reason would be longer than what was presented rather than a fraction of it.
        assert!(
            rendered_length < presented_length,
            "the recorded reason must be far shorter than the {presented_length}-byte identity \
             presented, got {rendered_length}"
        );
    }

    #[tokio::test]
    async fn deregister_removes_entry() {
        let registry = CollectorRegistry::unauthenticated();
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
