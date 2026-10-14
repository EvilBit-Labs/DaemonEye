//! The allowlist projection of an alert for sinks that leave the host.
//!
//! [`ExternalAlert`] is built field by field, so a field added to `Alert` or to a
//! process record never reaches a network sink until it is listed here.

use crate::models::{Alert, AlertSeverity, Completeness, ProcessId};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Optional sensitive fields a sink's operator opted into.
#[derive(Debug, Clone, Copy, Default)]
pub struct IncludeFlags {
    /// Include the process command line.
    pub command_line: bool,
    /// Include the process owner (numeric user id).
    pub user: bool,
}

/// What a network sink may send for one alert.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExternalAlert {
    /// Alert id; also the idempotency key.
    pub id: String,
    /// Alert creation time, milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Severity.
    pub severity: AlertSeverity,
    /// Detection rule id.
    pub rule_id: String,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Whether the evaluation saw everything it was meant to.
    pub completeness: Completeness,
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub ppid: Option<u32>,
    /// Process name.
    pub process_name: String,
    /// Executable path.
    pub executable_path: Option<String>,
    /// Executable hash.
    pub executable_hash: Option<String>,
    /// Process start time, milliseconds since the Unix epoch.
    pub start_time_ms: Option<u64>,
    /// Host name written by enrichment.
    pub host: Option<String>,
    /// Agent version written by enrichment.
    pub agent_version: Option<String>,
    /// Ancestor chain written by enrichment.
    pub ancestry: Option<String>,
    /// Alert tags.
    pub tags: Vec<String>,
    /// Command line, only when the sink opted in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
    /// Numeric user id of the process, only when the sink opted in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    })
}

impl ExternalAlert {
    /// Project `alert`, adding the opt-in fields only when `flags` ask for them.
    #[must_use]
    pub fn project(alert: &Alert, flags: IncludeFlags) -> Self {
        let process = &alert.process_record;
        let data = |key: &str| alert.context.data.get(key).cloned();
        Self {
            id: alert.id.to_string(),
            timestamp_ms: epoch_ms(alert.timestamp),
            severity: alert.severity,
            rule_id: alert.detection_rule_id.clone(),
            title: alert.title.clone(),
            description: alert.description.clone(),
            completeness: alert.completeness.clone(),
            pid: process.pid.raw(),
            ppid: process.ppid.map(ProcessId::raw),
            process_name: process.name.clone(),
            executable_path: process
                .executable_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            executable_hash: process.executable_hash.clone(),
            start_time_ms: process.start_time.map(epoch_ms),
            host: data("host"),
            agent_version: data("agent_version"),
            ancestry: data("ancestry"),
            tags: alert.context.tags.clone(),
            command_line: flags
                .command_line
                .then(|| process.command_line.clone())
                .flatten(),
            user: flags
                .user
                .then(|| process.user_id.map(|uid| uid.to_string()))
                .flatten(),
        }
    }
}

/// Strip CR, LF, tabs and every other control character, for sinks whose wire format
/// is line- or header-oriented.
#[must_use]
pub fn sanitise_control(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
