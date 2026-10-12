//! Durable per-sink delivery state for one alert (the `alert_deliveries` outbox row).

use super::AlertSeverity;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// What went wrong, without free text. Safe to log and serialise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeliveryErrorClass {
    Connect,
    Timeout,
    Tls,
    TlsVerification,
    HttpStatus(u16),
    SmtpStatus(u16),
    Protocol,
    Io,
    Payload,
    Recipient,
    Auth,
}

impl fmt::Display for DeliveryErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Connect => f.write_str("connect"),
            Self::Timeout => f.write_str("timeout"),
            Self::Tls => f.write_str("tls"),
            Self::TlsVerification => f.write_str("tls verification"),
            Self::HttpStatus(code) => write!(f, "http status {code}"),
            Self::SmtpStatus(code) => write!(f, "smtp status {code}"),
            Self::Protocol => f.write_str("protocol"),
            Self::Io => f.write_str("io"),
            Self::Payload => f.write_str("payload"),
            Self::Recipient => f.write_str("recipient"),
            Self::Auth => f.write_str("auth"),
        }
    }
}

/// Where a delivery row is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeliveryState {
    /// Waiting for a first or redriven attempt.
    Pending,
    /// Claimed by a worker or awaiting a retry.
    Retrying,
    /// Accepted by the sink.
    Delivered,
    /// Abandoned; see [`DeliveryRecord::reason`].
    DeadLettered,
    /// Not sent: a duplicate of the alert in [`DeliveryRecord::suppressed_by`].
    SuppressedDuplicate,
}

impl DeliveryState {
    /// Whether the row will never be attempted again without an operator redrive.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Delivered | Self::DeadLettered | Self::SuppressedDuplicate
        )
    }
}

/// Why a row was dead-lettered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeliveryReason {
    /// Retry budget spent.
    AttemptsExhausted,
    /// The sink rejected the alert in a way retrying cannot fix.
    Permanent,
    /// The sink's configuration cannot work.
    Misconfigured,
    /// Redrive budget spent.
    RedrivesExhausted,
    /// The alert row no longer exists.
    AlertMissing,
    /// The sink's open-row cap displaced or refused the row.
    QueueFull,
}

/// One `(sink, alert)` outbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecord {
    /// Lifecycle state.
    pub state: DeliveryState,
    /// Why the row is dead-lettered, when it is.
    pub reason: Option<DeliveryReason>,
    /// Severity of the alert, for eviction and displacement ordering.
    pub severity: AlertSeverity,
    /// Attempts since enqueue or the last redrive.
    pub attempts: u32,
    /// Times an operator redrove this row.
    pub redrives: u32,
    /// Class of the last failure, never free text.
    pub last_error: Option<DeliveryErrorClass>,
    /// When the row was created, in epoch milliseconds.
    pub enqueued_at_ms: u64,
    /// Earliest time of the next attempt, in epoch milliseconds.
    pub next_attempt_ms: u64,
    /// When the row went terminal, in epoch milliseconds.
    pub terminal_at_ms: Option<u64>,
    /// The surviving alert, for a suppressed duplicate.
    pub suppressed_by: Option<Uuid>,
    /// The alert's deduplication key, so the window can be re-seeded from rows alone.
    pub deduplication_key: String,
}
