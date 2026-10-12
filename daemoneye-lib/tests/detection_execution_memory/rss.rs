//! Resident-set sampling for the memory measurement (KTD15).
//!
//! A sampler that fails must say so. The T4 gate's first harness printed `peak_rss_mib=0.00` after
//! a total `sysinfo` failure, the most flattering number possible, so here a failed read is
//! counted, never folded in as zero, and [`Sampler::finish`] returns `Err` for any failure or for
//! an empty sample set.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// The background sampling interval the T4 gate used.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(5);

/// Reads this process's resident set, in bytes.
pub trait RssReader: Send + 'static {
    fn read(&mut self) -> Result<u64, String>;
}

/// The real reader: `sysinfo`'s view of this process.
pub struct SysinfoReader {
    system: System,
    pid: Pid,
}

impl SysinfoReader {
    pub fn new() -> Result<Self, String> {
        let pid = sysinfo::get_current_pid().map_err(str::to_owned)?;
        Ok(Self {
            system: System::new(),
            pid,
        })
    }
}

impl RssReader for SysinfoReader {
    fn read(&mut self) -> Result<u64, String> {
        let updated = self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        if updated == 0 {
            return Err("sysinfo refreshed no process".to_owned());
        }
        self.system
            .process(self.pid)
            .map(sysinfo::Process::memory)
            .ok_or_else(|| "sysinfo has no entry for this process".to_owned())
    }
}

/// Shared tally of every read, from the thread and from call boundaries alike.
#[derive(Default)]
struct Tally {
    peak: AtomicU64,
    reads: AtomicU64,
    failures: AtomicU64,
}

impl Tally {
    fn record(&self, read: &Result<u64, String>) -> Option<u64> {
        let Ok(&bytes) = read.as_ref() else {
            self.failures.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        self.peak.fetch_max(bytes, Ordering::Relaxed);
        self.reads.fetch_add(1, Ordering::Relaxed);
        Some(bytes)
    }
}

impl Tally {
    fn verdict(&self) -> Result<u64, String> {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures > 0 {
            return Err(format!("{failures} RSS reads failed"));
        }
        if self.reads.load(Ordering::Relaxed) == 0 {
            return Err("no RSS sample was taken".to_owned());
        }
        Ok(self.peak.load(Ordering::Relaxed))
    }
}

/// A 5 ms background sampler plus on-demand boundary samples, reporting the larger peak.
pub struct Sampler {
    tally: Arc<Tally>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    boundary: Box<dyn RssReader>,
}

impl Sampler {
    /// Start sampling. `background` runs on the thread, `boundary` serves [`Sampler::sample`].
    pub fn start(mut background: impl RssReader, boundary: impl RssReader) -> Self {
        let tally = Arc::new(Tally::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_tally, thread_stop) = (Arc::clone(&tally), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                thread_tally.record(&background.read());
                std::thread::sleep(SAMPLE_INTERVAL);
            }
        });
        Self {
            tally,
            stop,
            thread,
            boundary: Box::new(boundary),
        }
    }

    /// A call-boundary sample. Returns the reading, or `None` if it failed (counted).
    pub fn sample(&mut self) -> Option<u64> {
        self.tally.record(&self.boundary.read())
    }

    /// Stop and return the peak in bytes, or why the trace cannot be trusted.
    pub fn finish(self) -> Result<u64, String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .join()
            .map_err(|_panic| "the RSS sampler thread panicked".to_owned())?;
        self.tally.verdict()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing;
    impl RssReader for Failing {
        fn read(&mut self) -> Result<u64, String> {
            Err("read failed".to_owned())
        }
    }

    struct Fixed(u64);
    impl RssReader for Fixed {
        fn read(&mut self) -> Result<u64, String> {
            Ok(self.0)
        }
    }

    #[test]
    fn a_failing_reader_is_an_error_never_a_zero_peak() {
        let mut sampler = Sampler::start(Failing, Failing);
        sampler.sample();
        std::thread::sleep(Duration::from_millis(30));

        let result = sampler.finish();

        assert!(result.is_err(), "failed reads must not become a peak");
    }

    #[test]
    fn one_failed_boundary_read_poisons_an_otherwise_healthy_trace() {
        let mut sampler = Sampler::start(Fixed(7), Failing);
        std::thread::sleep(Duration::from_millis(30));
        sampler.sample();

        assert!(sampler.finish().is_err(), "a single failure is reported");
    }

    #[test]
    fn a_trace_with_no_samples_is_an_error() {
        assert!(Tally::default().verdict().is_err(), "empty is not zero");
    }

    #[test]
    fn the_peak_is_the_larger_of_thread_and_boundary_reads() {
        let mut sampler = Sampler::start(Fixed(100), Fixed(900));
        sampler.sample();
        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(sampler.finish().unwrap(), 900, "boundary peak wins");
    }

    #[test]
    fn the_real_reader_reports_a_nonzero_resident_set() {
        let mut reader = SysinfoReader::new().unwrap();
        assert!(
            reader.read().unwrap() > 0,
            "this process has resident pages"
        );
    }
}
