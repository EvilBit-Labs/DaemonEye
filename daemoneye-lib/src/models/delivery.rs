//! Durable per-sink delivery state for one alert (the `alert_deliveries` outbox row).

use super::AlertSeverity;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// What went wrong, without free text. Safe to log and serialise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeliveryErrorClass {
    /// The peer could not be reached or dropped the connection.
    Connect,
    /// The attempt ran past its deadline.
    Timeout,
    /// The TLS exchange failed for a reason other than verification.
    Tls,
    /// The peer's certificate was not accepted.
    TlsVerification,
    /// A non-success HTTP status.
    HttpStatus(u16),
    /// A non-success SMTP reply code.
    SmtpStatus(u16),
    /// The peer and the sink disagree on the protocol (no STARTTLS, a malformed reply).
    Protocol,
    /// A read or write failed.
    Io,
    /// The alert could not be rendered or the peer refused its content.
    Payload,
    /// The peer refused a recipient.
    Recipient,
    /// The credential was refused.
    Auth,
    /// The sink reported that its own configuration cannot work.
    Config,
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
            Self::Config => f.write_str("config"),
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
    /// Whether the row is finished: it is attempted again only if a redrive moves it back to
    /// `Pending`, which only a dead letter can be.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Delivered | Self::DeadLettered | Self::SuppressedDuplicate
        )
    }
}

/// Why a row was dead-lettered; set only while the state is `DeadLettered`.
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
    /// Times the row was redriven, by an operator or automatically.
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

/// Every state change goes through one of these, so the fields that belong to a state are set
/// together: a terminal state always has `terminal_at_ms`, only a dead letter has `reason`, only
/// a suppressed duplicate has `suppressed_by`.
impl DeliveryRecord {
    /// Abandon the row for `reason` at `now_ms`.
    pub const fn dead_letter(&mut self, reason: DeliveryReason, now_ms: u64) {
        self.state = DeliveryState::DeadLettered;
        self.reason = Some(reason);
        self.terminal_at_ms = Some(now_ms);
    }

    /// The sink accepted the alert at `now_ms`.
    pub const fn deliver(&mut self, now_ms: u64) {
        self.state = DeliveryState::Delivered;
        self.reason = None;
        self.terminal_at_ms = Some(now_ms);
    }

    /// Mark the row a duplicate of `survivor`, never to be sent.
    pub const fn suppress(&mut self, survivor: Uuid, now_ms: u64) {
        self.state = DeliveryState::SuppressedDuplicate;
        self.suppressed_by = Some(survivor);
        self.terminal_at_ms = Some(now_ms);
    }

    /// Take the row for an attempt starting at `now_ms`.
    pub const fn claim(&mut self, now_ms: u64) {
        self.state = DeliveryState::Retrying;
        self.attempts = self.attempts.saturating_add(1);
        self.next_attempt_ms = now_ms;
    }

    /// Keep the row open for another attempt no earlier than `at_ms`.
    pub const fn retry_at(&mut self, at_ms: u64) {
        self.state = DeliveryState::Retrying;
        self.next_attempt_ms = at_ms;
    }

    /// Move a dead letter back to `Pending` at `now_ms`, with a fresh attempt budget.
    pub const fn redrive(&mut self, now_ms: u64) {
        self.state = DeliveryState::Pending;
        self.reason = None;
        self.attempts = 0;
        self.redrives = self.redrives.saturating_add(1);
        self.next_attempt_ms = now_ms;
        self.terminal_at_ms = None;
    }
}
