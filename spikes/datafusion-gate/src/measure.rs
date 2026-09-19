//! U7 — measurement harness.
//!
//! Peak RSS, a latency distribution over repeated runs, and release binary
//! size, all tagged with the platform they were measured on (R6, R7, R8, R9).
//!
//! RSS is sampled from the OS at process level rather than by instrumenting
//! allocations (KTD9): the budget in question is resident set, not heap. The
//! sampler uses `sysinfo`, already a workspace dependency, because the direct
//! OS APIs all require unsafe FFI and this crate mirrors the workspace's
//! `unsafe_code = "forbid"`.

use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Tracks the high-water mark of this process's resident set.
#[derive(Debug)]
pub struct RssSampler {
    system: System,
    pid: Pid,
    peak_bytes: u64,
}

impl RssSampler {
    /// Start sampling this process.
    #[must_use]
    pub fn new() -> Self {
        let mut s = Self {
            system: System::new(),
            pid: Pid::from_u32(std::process::id()),
            peak_bytes: 0,
        };
        s.sample();
        s
    }

    /// Take a sample, updating the high-water mark.
    pub fn sample(&mut self) -> u64 {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let current = self
            .system
            .process(self.pid)
            .map_or(0, sysinfo::Process::memory);
        self.peak_bytes = self.peak_bytes.max(current);
        current
    }

    /// The highest resident set observed so far, in bytes.
    #[must_use]
    pub const fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }

    /// The highest resident set observed so far, in mebibytes.
    #[must_use]
    pub fn peak_mib(&self) -> f64 {
        #[expect(
            clippy::as_conversions,
            reason = "RSS in bytes is far below f64's exact-integer range"
        )]
        let bytes = self.peak_bytes as f64;
        bytes / (1024.0 * 1024.0)
    }
}

impl Default for RssSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// A latency distribution over repeated runs.
#[derive(Debug, Clone)]
pub struct Latency {
    /// Every sample, in run order.
    pub samples: Vec<Duration>,
}

impl Latency {
    /// Build from raw samples.
    #[must_use]
    pub const fn new(samples: Vec<Duration>) -> Self {
        Self { samples }
    }

    /// Number of samples.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.samples.len()
    }

    /// Sorted copy, used for the order statistics below.
    fn sorted(&self) -> Vec<Duration> {
        let mut v = self.samples.clone();
        v.sort_unstable();
        v
    }

    /// Smallest sample.
    #[must_use]
    pub fn min(&self) -> Duration {
        self.sorted().first().copied().unwrap_or_default()
    }

    /// Largest sample.
    #[must_use]
    pub fn max(&self) -> Duration {
        self.sorted().last().copied().unwrap_or_default()
    }

    /// Median sample.
    #[must_use]
    pub fn p50(&self) -> Duration {
        self.quantile(0.50)
    }

    /// 95th-percentile sample (nearest-rank).
    #[must_use]
    pub fn p95(&self) -> Duration {
        self.quantile(0.95)
    }

    /// Nearest-rank quantile.
    fn quantile(&self, q: f64) -> Duration {
        let v = self.sorted();
        if v.is_empty() {
            return Duration::ZERO;
        }
        #[expect(
            clippy::as_conversions,
            reason = "sample counts are small; the rank is clamped into range below"
        )]
        let rank = (q * v.len() as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(v.len().saturating_sub(1));
        v.get(idx).copied().unwrap_or_default()
    }
}

/// The platform a number was measured on (R9).
#[must_use]
pub fn platform() -> String {
    format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
}
