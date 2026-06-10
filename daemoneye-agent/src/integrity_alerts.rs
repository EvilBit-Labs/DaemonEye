//! Agent-side integrity-signal alert bridge.
//!
//! procmond cannot emit alerts (it has no `AlertManager` and no network by
//! design), so the per-process integrity flags it sets on the wire
//! (`ssdeep_degraded`, `on_disk_mismatch`) are turned into alerts here in the
//! orchestrator, independently of the SQL rule engine. The flags live on the
//! protobuf `ProcessRecord`; the native model does not carry them, so this
//! bridge reads the proto records before they are converted to the native model.

use daemoneye_lib::integrity::fuzzy::{self, DEFAULT_SSDEEP_SIMILARITY_THRESHOLD, FuzzyConfig};
use daemoneye_lib::models::process::ProcessRecord as NativeProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity};
use daemoneye_lib::proto::ProcessRecord as ProtoProcessRecord;
use std::collections::HashMap;

/// Synthetic detection-rule id for the degraded-integrity-coverage alert
/// (ssdeep failed while the SHA-256 identity hash succeeded).
pub const RULE_DEGRADED_COVERAGE: &str = "integrity.coverage.degraded";

/// Synthetic detection-rule id for the on-disk-vs-running mismatch alert.
pub const RULE_DISK_MISMATCH: &str = "integrity.disk_mismatch";

/// Synthetic detection-rule id for the ssdeep binary-change observation.
pub const RULE_BINARY_CHANGE: &str = "integrity.binary_change";

/// Session-scoped tracker for ssdeep binary-change observations (R2 AC7).
///
/// Holds the most recent ssdeep digest per executable path seen this session.
/// When a process's current ssdeep similarity to its previously recorded value
/// falls *below* the configured threshold, a binary-change observation is
/// emitted. The first observation for a path only seeds the baseline (no alert).
///
/// "Previously recorded value" is interpreted as the agent's last in-memory value
/// for the path: this reads no storage and adds no storage schema, staying within
/// the ticket's "no new storage logic" boundary. Baselines reset on agent restart.
pub struct BinaryChangeTracker {
    threshold: u8,
    last_ssdeep: HashMap<String, String>,
}

impl Default for BinaryChangeTracker {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_SSDEEP_SIMILARITY_THRESHOLD,
            last_ssdeep: HashMap::new(),
        }
    }
}

impl BinaryChangeTracker {
    /// Create a tracker from a fuzzy config.
    ///
    /// # Errors
    ///
    /// Returns [`fuzzy::FuzzyHashError::InvalidConfig`] when the config's
    /// `similarity_threshold` is outside the accepted range.
    pub fn new(config: FuzzyConfig) -> Result<Self, fuzzy::FuzzyHashError> {
        config.validate()?;
        Ok(Self {
            threshold: config.similarity_threshold,
            last_ssdeep: HashMap::new(),
        })
    }

    /// Observe a scan's records, returning binary-change alerts and updating the
    /// per-path baselines. A comparison failure is skipped silently (no false
    /// alert); a similarity at or above the threshold does not fire.
    #[must_use]
    pub fn observe(&mut self, records: &[ProtoProcessRecord]) -> Vec<Alert> {
        let mut alerts = Vec::new();
        for record in records {
            let (Some(path), Some(current)) = (
                record.executable_path.as_deref(),
                record.ssdeep_hash.as_deref(),
            ) else {
                continue;
            };
            if let Some(previous) = self.last_ssdeep.get(path)
                && let Ok(score) = fuzzy::similarity(previous, current)
                && score < self.threshold
            {
                let description = format!(
                    "executable of process {} (pid {}) changed: ssdeep similarity {} fell below \
                     threshold {} versus the previously recorded value",
                    record.name, record.pid, score, self.threshold
                );
                alerts.push(build_alert(
                    record,
                    AlertSeverity::Medium,
                    RULE_BINARY_CHANGE,
                    "Executable binary change",
                    description,
                ));
            }
            // Seed/update the baseline to the current value.
            self.last_ssdeep.insert(path.to_owned(), current.to_owned());
        }
        alerts
    }
}

/// Build integrity alerts from the per-process flags on proto process records.
///
/// A single record may produce both alerts (they carry distinct deduplication
/// keys); clean records produce none. The `AlertManager` deduplicates repeated
/// signals within its window, so re-observing the same condition across scans
/// does not flood downstream sinks.
#[must_use]
pub fn detect_integrity_alerts(records: &[ProtoProcessRecord]) -> Vec<Alert> {
    let mut alerts = Vec::new();
    for record in records {
        if record.ssdeep_degraded {
            let description = format!(
                "ssdeep fuzzy hash failed while the SHA-256 identity hash succeeded for \
                 process {} (pid {}); binary-change detection is degraded for this process",
                record.name, record.pid
            );
            alerts.push(build_alert(
                record,
                AlertSeverity::Medium,
                RULE_DEGRADED_COVERAGE,
                "Degraded integrity coverage",
                description,
            ));
        }
        if record.on_disk_mismatch {
            let description = format!(
                "running image of process {} (pid {}) differs from its on-disk executable \
                 (the backing file was deleted or replaced while the process runs)",
                record.name, record.pid
            );
            alerts.push(build_alert(
                record,
                AlertSeverity::High,
                RULE_DISK_MISMATCH,
                "On-disk executable mismatch",
                description,
            ));
        }
    }
    alerts
}

fn build_alert(
    record: &ProtoProcessRecord,
    severity: AlertSeverity,
    rule_id: &str,
    title: &str,
    description: String,
) -> Alert {
    let native: NativeProcessRecord = record.clone().into();
    Alert::new(severity, title, description, rule_id, native)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;

    fn record(name: &str, pid: u32, degraded: bool, mismatch: bool) -> ProtoProcessRecord {
        ProtoProcessRecord {
            pid,
            name: name.to_owned(),
            ssdeep_degraded: degraded,
            on_disk_mismatch: mismatch,
            ..Default::default()
        }
    }

    #[test]
    fn degraded_record_yields_one_medium_alert() {
        let alerts = detect_integrity_alerts(&[record("p", 7, true, false)]);
        assert_eq!(alerts.len(), 1);
        let alert = alerts.first().expect("one alert");
        assert_eq!(alert.severity, AlertSeverity::Medium);
        assert_eq!(alert.detection_rule_id, RULE_DEGRADED_COVERAGE);
    }

    #[test]
    fn mismatch_record_yields_one_high_alert() {
        let alerts = detect_integrity_alerts(&[record("p", 7, false, true)]);
        assert_eq!(alerts.len(), 1);
        let alert = alerts.first().expect("one alert");
        assert_eq!(alert.severity, AlertSeverity::High);
        assert_eq!(alert.detection_rule_id, RULE_DISK_MISMATCH);
    }

    #[test]
    fn clean_record_yields_no_alerts() {
        let alerts = detect_integrity_alerts(&[record("p", 7, false, false)]);
        assert!(alerts.is_empty());
    }

    #[test]
    fn degraded_and_mismatch_yields_two_distinct_alerts() {
        let alerts = detect_integrity_alerts(&[record("p", 7, true, true)]);
        assert_eq!(alerts.len(), 2);
        // Distinct deduplication keys so both reach the sinks.
        let keys: std::collections::HashSet<&str> = alerts
            .iter()
            .map(|alert| alert.deduplication_key.as_str())
            .collect();
        assert_eq!(keys.len(), 2);
    }

    fn record_ssdeep(path: &str, ssdeep: &str) -> ProtoProcessRecord {
        ProtoProcessRecord {
            pid: 1,
            name: "p".to_owned(),
            executable_path: Some(path.to_owned()),
            ssdeep_hash: Some(ssdeep.to_owned()),
            ..Default::default()
        }
    }

    fn ssdeep_of(seed: u8) -> String {
        let buffer: Vec<u8> = (0_usize..16_384)
            .map(|i| seed.wrapping_add(u8::try_from(i % 251).unwrap_or(0)))
            .collect();
        fuzzy::compute_ssdeep_from_bytes(&buffer).expect("ssdeep compute")
    }

    #[test]
    fn new_rejects_invalid_threshold() {
        assert!(
            BinaryChangeTracker::new(FuzzyConfig {
                similarity_threshold: 0
            })
            .is_err()
        );
        assert!(
            BinaryChangeTracker::new(FuzzyConfig {
                similarity_threshold: 100
            })
            .is_err()
        );
        assert!(
            BinaryChangeTracker::new(FuzzyConfig {
                similarity_threshold: 80
            })
            .is_ok()
        );
    }

    #[test]
    fn first_observation_seeds_without_alert() {
        let mut tracker = BinaryChangeTracker::default();
        assert!(
            tracker
                .observe(&[record_ssdeep("/bin/x", &ssdeep_of(1))])
                .is_empty()
        );
    }

    #[test]
    fn identical_executable_does_not_fire() {
        let digest = ssdeep_of(1);
        let mut tracker = BinaryChangeTracker::default();
        assert!(
            tracker
                .observe(&[record_ssdeep("/bin/x", &digest)])
                .is_empty()
        );
        // Same digest -> similarity 100 -> no binary-change observation.
        assert!(
            tracker
                .observe(&[record_ssdeep("/bin/x", &digest)])
                .is_empty()
        );
    }

    #[test]
    fn changed_executable_below_threshold_fires() {
        let mut tracker = BinaryChangeTracker::new(FuzzyConfig {
            similarity_threshold: 80,
        })
        .expect("valid");
        // Seed with one binary, then observe an unrelated binary on the same path.
        assert!(
            tracker
                .observe(&[record_ssdeep("/bin/x", &ssdeep_of(1))])
                .is_empty()
        );
        let alerts = tracker.observe(&[record_ssdeep("/bin/x", &ssdeep_of(200))]);
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts.first().expect("alert").detection_rule_id,
            RULE_BINARY_CHANGE
        );
    }

    #[test]
    fn record_without_ssdeep_is_ignored() {
        let mut tracker = BinaryChangeTracker::default();
        // No ssdeep_hash -> nothing to compare, no panic, no alert.
        assert!(tracker.observe(&[record("p", 7, false, false)]).is_empty());
    }
}
