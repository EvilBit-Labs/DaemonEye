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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// How often the background watcher samples resident set.
const WATCH_INTERVAL: Duration = Duration::from_millis(5);

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
        // SAFETY: peak RSS in bytes is far below f64's 2^53 exact-integer range.
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
///
/// Sorted once at construction: every order statistic below reads the same
/// sorted copy rather than re-sorting per call.
#[derive(Debug, Clone)]
pub struct Latency {
    /// Every sample, in run order.
    samples: Vec<Duration>,
    /// The same samples, ascending.
    sorted: Vec<Duration>,
}

impl Latency {
    /// Build from raw samples.
    #[must_use]
    pub fn new(samples: Vec<Duration>) -> Self {
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        Self { samples, sorted }
    }

    /// Every sample, in run order.
    #[must_use]
    pub fn samples(&self) -> &[Duration] {
        &self.samples
    }

    /// Number of samples.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.samples.len()
    }

    /// Smallest sample.
    #[must_use]
    pub fn min(&self) -> Duration {
        self.sorted.first().copied().unwrap_or_default()
    }

    /// Largest sample.
    #[must_use]
    pub fn max(&self) -> Duration {
        self.sorted.last().copied().unwrap_or_default()
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
        let v = &self.sorted;
        if v.is_empty() {
            return Duration::ZERO;
        }
        // SAFETY: sample counts are small (tens), so both casts are exact, and
        // the resulting rank is clamped into range on the next line.
        #[expect(
            clippy::as_conversions,
            reason = "sample counts are small; the rank is clamped into range below"
        )]
        let rank = (q * v.len() as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(v.len().saturating_sub(1));
        v.get(idx).copied().unwrap_or_default()
    }
}

/// Samples resident set on a background thread for the length of a scope.
///
/// [`RssSampler`] only records a value when something calls `sample()`, so a
/// peak that rises and falls *inside* one query — a hash join's build phase,
/// for instance — is invisible to call-boundary sampling. With only ~11 MiB of
/// headroom against the 100 MiB budget, that blind spot is large enough to hide
/// a real breach, so the measured sections run under this watcher instead.
#[derive(Debug)]
pub struct RssWatcher {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl RssWatcher {
    /// Start sampling this process every [`WATCH_INTERVAL`].
    #[must_use]
    pub fn start() -> Self {
        let peak = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (Arc::clone(&peak), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            let pid = Pid::from_u32(std::process::id());
            let mut system = System::new();
            while !s.load(Ordering::Relaxed) {
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::nothing().with_memory(),
                );
                let current = system.process(pid).map_or(0, sysinfo::Process::memory);
                p.fetch_max(current, Ordering::Relaxed);
                thread::sleep(WATCH_INTERVAL);
            }
        });
        Self {
            peak,
            stop,
            handle: Some(handle),
        }
    }

    /// Stop sampling and return the highest resident set observed.
    pub fn stop(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            drop(h.join());
        }
        self.peak.load(Ordering::Relaxed)
    }
}

/// Print the RSS and latency fields both arms report.
///
/// One copy so the two binaries cannot drift into reporting different fields
/// or different precision for the same measurement.
#[expect(
    clippy::print_stdout,
    reason = "this is the measurement record the binaries exist to emit"
)]
pub fn print_rss_and_latency(
    rss: &RssSampler,
    baseline_bytes: u64,
    after_first_bytes: u64,
    lat: &Latency,
    watched_peak_bytes: u64,
) {
    let peak = rss.peak_bytes().max(watched_peak_bytes);
    #[expect(
        clippy::as_conversions,
        reason = "RSS in bytes is far below f64's exact-integer range"
    )]
    // SAFETY: peak RSS in bytes is far below f64's 2^53 exact-integer range.
    let peak_mib = peak as f64 / (1024.0 * 1024.0);
    println!("baseline_rss_bytes={baseline_bytes}");
    println!("after_first_run_rss_bytes={after_first_bytes}");
    println!("sampled_peak_rss_bytes={}", rss.peak_bytes());
    println!("watched_peak_rss_bytes={watched_peak_bytes}");
    println!("peak_rss_bytes={peak}");
    println!("peak_rss_mib={peak_mib:.2}");
    println!("latency_samples={}", lat.count());
    println!("latency_min_ms={:.3}", lat.min().as_secs_f64() * 1000.0);
    println!("latency_p50_ms={:.3}", lat.p50().as_secs_f64() * 1000.0);
    println!("latency_p95_ms={:.3}", lat.p95().as_secs_f64() * 1000.0);
    println!("latency_max_ms={:.3}", lat.max().as_secs_f64() * 1000.0);
}

/// The platform a number was measured on (R9).
#[must_use]
pub fn platform() -> String {
    format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
}
