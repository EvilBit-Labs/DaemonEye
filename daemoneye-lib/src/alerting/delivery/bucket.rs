//! Token bucket for `max_alerts_per_minute`.

use std::time::Duration;
use tokio::time::Instant;

const MINUTE: Duration = Duration::from_mins(1);

/// `capacity` tokens per minute, refilled evenly; starts full.
#[derive(Debug)]
pub(super) struct TokenBucket {
    capacity: u32,
    available: u32,
    interval_nanos: u128,
    last_refill: Instant,
}

impl TokenBucket {
    /// `None` when unlimited (unset or zero).
    pub(super) fn new(per_minute: Option<u32>) -> Option<Self> {
        let capacity = per_minute.filter(|&n| n > 0)?;
        let interval_nanos = MINUTE.as_nanos().checked_div(u128::from(capacity))?;
        Some(Self {
            capacity,
            available: capacity,
            interval_nanos: interval_nanos.max(1),
            last_refill: Instant::now(),
        })
    }

    /// Take a token now, or return how long until one exists.
    pub(super) fn try_take(&mut self) -> Result<(), Duration> {
        self.refill();
        if let Some(left) = self.available.checked_sub(1) {
            self.available = left;
            return Ok(());
        }
        let since = self.last_refill.elapsed().as_nanos();
        let wait = self.interval_nanos.saturating_sub(since);
        Err(Duration::from_nanos(
            u64::try_from(wait).unwrap_or(u64::MAX),
        ))
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_nanos();
        let earned = elapsed.checked_div(self.interval_nanos).unwrap_or(0);
        if earned == 0 {
            return;
        }
        let earned_tokens = u32::try_from(earned).unwrap_or(u32::MAX);
        self.available = self
            .available
            .saturating_add(earned_tokens)
            .min(self.capacity);
        let consumed = self.interval_nanos.saturating_mul(earned);
        let advance = Duration::from_nanos(u64::try_from(consumed).unwrap_or(u64::MAX));
        self.last_refill = if self.available == self.capacity {
            Instant::now()
        } else {
            self.last_refill
                .checked_add(advance)
                .unwrap_or_else(Instant::now)
        };
    }
}
