//! Delivery error classification, retry backoff and a per-sink circuit breaker.
//!
//! Shared by every sink and the delivery worker. Nothing here locks; the
//! caller owns synchronisation. Time comes from `tokio::time` so tests run
//! under paused time.

pub use crate::models::DeliveryErrorClass;
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::time::Duration;
use tokio::time::Instant;

const MAX_BACKOFF_SHIFT: u32 = 31;
const SMTP_AUTH_FAILED: u16 = 535;
const HTTP_REQUEST_TIMEOUT: u16 = 408;
const HTTP_TOO_MANY_REQUESTS: u16 = 429;

/// Outcome of a failed delivery attempt.
///
/// `Transient` is retried and counts toward the breaker threshold,
/// `Permanent` is neither, `Misconfigured` opens the breaker at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeliveryError {
    /// Retrying may succeed.
    Transient {
        /// What failed.
        class: DeliveryErrorClass,
        /// The peer's own wait hint, when it gave one.
        retry_after: Option<Duration>,
    },
    /// The peer answered and refused this alert; retrying cannot help.
    Permanent {
        /// What failed.
        class: DeliveryErrorClass,
    },
    /// The sink's configuration cannot work (credential, certificate, protocol).
    Misconfigured {
        /// What failed.
        class: DeliveryErrorClass,
    },
}

impl DeliveryError {
    /// The error class, whatever the variant.
    pub const fn class(&self) -> &DeliveryErrorClass {
        match *self {
            Self::Transient { ref class, .. }
            | Self::Permanent { ref class }
            | Self::Misconfigured { ref class } => class,
        }
    }

    /// Classify an HTTP status; `retry_after` is only kept for transient statuses.
    pub const fn from_http_status(status: u16, retry_after: Option<Duration>) -> Self {
        let class = DeliveryErrorClass::HttpStatus(status);
        match status {
            500..=599 | HTTP_REQUEST_TIMEOUT | HTTP_TOO_MANY_REQUESTS => {
                Self::Transient { class, retry_after }
            }
            401 | 403 => Self::Misconfigured {
                class: DeliveryErrorClass::Auth,
            },
            _ => Self::Permanent { class },
        }
    }

    /// Classify an SMTP reply code.
    pub const fn from_smtp_status(code: u16) -> Self {
        let class = DeliveryErrorClass::SmtpStatus(code);
        match code {
            400..=499 => Self::Transient {
                class,
                retry_after: None,
            },
            SMTP_AUTH_FAILED => Self::Misconfigured {
                class: DeliveryErrorClass::Auth,
            },
            _ => Self::Permanent { class },
        }
    }

    /// A retryable failure with no retry hint.
    pub const fn transient(class: DeliveryErrorClass) -> Self {
        Self::Transient {
            class,
            retry_after: None,
        }
    }

    /// Certificate verification failed; retrying cannot help.
    pub const fn tls_verification() -> Self {
        Self::Misconfigured {
            class: DeliveryErrorClass::TlsVerification,
        }
    }
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match *self {
            Self::Transient { .. } => "transient",
            Self::Permanent { .. } => "permanent",
            Self::Misconfigured { .. } => "misconfigured",
        };
        write!(f, "{kind}: {}", self.class())
    }
}

impl std::error::Error for DeliveryError {}

impl From<io::Error> for DeliveryError {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "io::ErrorKind is non_exhaustive; unlisted kinds are generic Io"
    )]
    fn from(error: io::Error) -> Self {
        let class = match error.kind() {
            io::ErrorKind::TimedOut => DeliveryErrorClass::Timeout,
            io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected => DeliveryErrorClass::Connect,
            _ => DeliveryErrorClass::Io,
        };
        Self::Transient {
            class,
            retry_after: None,
        }
    }
}

/// Exponential backoff with full jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_attempts: u32,
    base: Duration,
    cap: Duration,
}

impl RetryPolicy {
    /// At most `max_attempts` sends (never fewer than one), with delays drawn from
    /// `[0, min(cap, base * 2^attempt)]`. A `base` above `cap` is capped; the config loader
    /// refuses that pairing before it gets here.
    pub const fn new(max_attempts: u32, base: Duration, cap: Duration) -> Self {
        Self {
            max_attempts: if max_attempts == 0 { 1 } else { max_attempts },
            base: if base.as_nanos() > cap.as_nanos() {
                cap
            } else {
                base
            },
            cap,
        }
    }

    /// Sends allowed per row, including the first.
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Longest delay between two attempts.
    #[must_use]
    pub const fn cap(&self) -> Duration {
        self.cap
    }

    /// Uniform draw in `[0, min(cap, base * 2^attempt)]`. The draw happens
    /// before the caller sleeps, so no RNG is held across an await.
    pub fn delay_for(&self, attempt: u32, rng: &mut impl Rng) -> Duration {
        let multiplier = 1_u32 << attempt.min(MAX_BACKOFF_SHIFT);
        let ceiling = self.base.saturating_mul(multiplier).min(self.cap);
        let ceiling_nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
        Duration::from_nanos(rng.random_range(0..=ceiling_nanos))
    }
}

/// Parse a `Retry-After` header. Only ASCII delta-seconds are honoured (clamped to `cap`);
/// HTTP-dates and overflowing values yield `None`.
#[must_use]
pub fn parse_retry_after(header: &str, cap: Duration) -> Option<Duration> {
    let value = header.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = value.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds).min(cap))
}

/// Where a [`CircuitBreaker`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum BreakerState {
    /// Calls go through.
    Closed,
    /// No calls until the cool-down ends.
    Open,
    /// One probe is out; its answer closes or reopens the breaker.
    HalfOpen,
}

/// Answer to "may I call the sink now?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[must_use = "a granted probe must be reported back, or the breaker stays half-open"]
pub enum Permit {
    /// The breaker is closed.
    Allow,
    /// The breaker was open and its cool-down is over: make exactly one call and report the
    /// answer with [`CircuitBreaker::record_success`], [`CircuitBreaker::record_failure`] or
    /// [`CircuitBreaker::abandon_probe`].
    AllowProbe,
    /// Make no call.
    Deny,
}

#[derive(Debug, Clone, Copy)]
enum Inner {
    Closed { failures: u32, since: Instant },
    Open { until: Instant },
    HalfOpen,
}

/// Closed / open / half-open breaker. An open breaker makes no calls.
///
/// Consecutive transient failures open it at the threshold and a misconfigured outcome opens it
/// at once. After the cool-down [`permit`](Self::permit) grants one probe; the probe's answer
/// closes the breaker (a success, or a permanent rejection, which proves the sink is up) or
/// reopens it for a full cool-down.
#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    inner: Inner,
}

impl CircuitBreaker {
    /// Opens after `failure_threshold` consecutive transient failures (never fewer than one)
    /// and probes again `cooldown` later.
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold: failure_threshold.max(1),
            cooldown,
            inner: Inner::Closed {
                failures: 0,
                since: Instant::now(),
            },
        }
    }

    /// The sink answered well: close, and forget the failure count.
    pub fn record_success(&mut self) {
        let since = match self.inner {
            Inner::Closed { since, .. } => since,
            Inner::Open { .. } | Inner::HalfOpen => Instant::now(),
        };
        self.inner = Inner::Closed { failures: 0, since };
    }

    /// The sink failed. A transient failure counts toward the threshold and reopens a half-open
    /// breaker; a misconfigured one opens it at once; a permanent one is an answer, so it closes
    /// a half-open breaker and is otherwise ignored.
    pub fn record_failure(&mut self, error: &DeliveryError) {
        match *error {
            DeliveryError::Permanent { .. } => {
                if matches!(self.inner, Inner::HalfOpen) {
                    self.record_success();
                }
            }
            DeliveryError::Misconfigured { .. } => self.open(),
            DeliveryError::Transient { .. } => match self.inner {
                Inner::Closed { failures, since } => {
                    let count = failures.saturating_add(1);
                    if count >= self.failure_threshold {
                        self.open();
                    } else {
                        self.inner = Inner::Closed {
                            failures: count,
                            since,
                        };
                    }
                }
                Inner::HalfOpen => self.open(),
                Inner::Open { .. } => {}
            },
        }
    }

    /// A granted probe produced no answer (the call never went out): back to open for a full
    /// cool-down, so the breaker cannot stay half-open with nothing in flight.
    pub fn abandon_probe(&mut self) {
        if matches!(self.inner, Inner::HalfOpen) {
            self.open();
        }
    }

    /// Whether a call may go out now. Moves an open breaker whose cool-down has passed to
    /// half-open, granting the one probe; the caller must report that probe's answer back.
    pub fn permit(&mut self) -> Permit {
        match self.inner {
            Inner::Closed { .. } => Permit::Allow,
            Inner::Open { until } if Instant::now() >= until => {
                self.inner = Inner::HalfOpen;
                Permit::AllowProbe
            }
            Inner::Open { .. } | Inner::HalfOpen => Permit::Deny,
        }
    }

    /// The breaker's state as a plain value.
    pub const fn state(&self) -> BreakerState {
        match self.inner {
            Inner::Closed { .. } => BreakerState::Closed,
            Inner::Open { .. } => BreakerState::Open,
            Inner::HalfOpen => BreakerState::HalfOpen,
        }
    }

    /// When the breaker last became closed; `None` while it is not closed.
    pub const fn closed_since(&self) -> Option<Instant> {
        match self.inner {
            Inner::Closed { since, .. } => Some(since),
            Inner::Open { .. } | Inner::HalfOpen => None,
        }
    }

    fn open(&mut self) {
        let now = Instant::now();
        // An unrepresentable deadline (cooldown of centuries) probes at once
        // rather than panicking.
        let until = now.checked_add(self.cooldown).unwrap_or(now);
        self.inner = Inner::Open { until };
    }
}
