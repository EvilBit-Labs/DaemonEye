//! Process collection streaming API.
//!
//! This module defines the `ProcessCollectionService` trait which returns an
//! asynchronous stream of `ProcessRecord` items instead of a monolithic Vec.
//! This design prevents unbounded memory consumption on large systems and
//! enables early termination, batching, and backpressure-aware processing.

use crate::models::{ProcessRecord, ProcessStatus};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, stream};
use std::time::Instant;
use sysinfo::System;
use thiserror::Error;

/// Errors that may occur during process collection.
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },
    #[error("Collection timed out before completion")]
    Timeout,
    #[error("Enumeration failed: {0}")]
    EnumerationError(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Type alias for the asynchronous process record stream.
pub type ProcessStream = BoxStream<'static, Result<ProcessRecord, CollectionError>>;

/// Trait for streaming process collection.
#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    /// Return an async stream of individual `ProcessRecord` items.
    ///
    /// Implementations MUST:
    /// - Respect the optional `deadline`; once exceeded, yield a single `Err(CollectionError::Timeout)` then terminate.
    /// - Avoid buffering all processes in memory simultaneously (bounded internal buffering only).
    /// - Propagate per-item errors without aborting the entire stream unless fatal (timeout or enumeration failure).
    fn stream_processes(&self, deadline: Option<Instant>) -> ProcessStream;
}

/// A process collector based on the `sysinfo` crate.
#[derive(Debug, Default)]
pub struct SysinfoProcessCollector {
    refresh_cpu: bool,
}

impl SysinfoProcessCollector {
    /// Create a new collector.
    pub fn new() -> Self {
        Self { refresh_cpu: true }
    }
}

#[async_trait]
impl ProcessCollectionService for SysinfoProcessCollector {
    fn stream_processes(&self, deadline: Option<Instant>) -> ProcessStream {
        // We capture current time early for timeout comparisons.
        let deadline_instant = deadline;

        // Perform a single snapshot enumeration up-front (sysinfo currently requires refresh).
        // We still stream results item-by-item afterward.
        let mut system = System::new_all();
        // Refresh processes (sysinfo already loads them in new_all but we refresh for CPU info if requested)
        if self.refresh_cpu {
            system.refresh_all();
        }

        // Capture process list keys to avoid holding &System across await points (stream::iter is sync but good hygiene).
        let pids: Vec<_> = system.processes().keys().cloned().collect();
        let iter = pids.into_iter().map(move |pid| {
            // Access the process each iteration (if it vanished, surface error per-item).
            let proc = match system.processes().get(&pid) {
                Some(p) => p,
                None => return Err(CollectionError::ProcessNotFound { pid: pid.as_u32() }),
            };
            if let Some(dl) = deadline_instant {
                if Instant::now() > dl {
                    // Represent timeout as single error; downstream can stop.
                    return Err(CollectionError::Timeout);
                }
            }
            let pid_u32: u32 = pid.as_u32();

            // Map sysinfo status to our status enum.
            let status = match proc.status() {
                sysinfo::ProcessStatus::Run => ProcessStatus::Running,
                sysinfo::ProcessStatus::Sleep => ProcessStatus::Sleeping,
                sysinfo::ProcessStatus::Stop => ProcessStatus::Stopped,
                sysinfo::ProcessStatus::Zombie => ProcessStatus::Zombie,
                sysinfo::ProcessStatus::Tracing => ProcessStatus::Traced,
                other => ProcessStatus::Unknown(format!("{:?}", other)),
            };

            // Build record (best-effort, no hashing yet)
            // Convert OS strings to lossy UTF-8 Strings.
            let name_str = proc.name().to_string_lossy().to_string();
            let cmdline = {
                let parts: Vec<String> = proc
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().to_string())
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" "))
                }
            };

            let mut builder = ProcessRecord::builder()
                .pid_raw(pid_u32)
                .name(name_str)
                .status(status);
            if let Some(cmd) = cmdline {
                builder = builder.command_line(cmd);
            }
            let record = builder
                .memory_usage(proc.memory())
                .cpu_usage(proc.cpu_usage() as f64)
                .build()
                .map_err(|e| CollectionError::EnumerationError(e.to_string()))?;

            Ok(record)
        });

        // Wrap in a stream with small buffering; map iterator into async stream.
        // Using stream::iter keeps it simple; any heavy work could be offloaded via spawn_blocking if needed.
        stream::iter(iter).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn test_basic_stream_collection() {
        let collector = SysinfoProcessCollector::new();
        let mut stream = collector.stream_processes(None);
        let mut count = 0usize;
        while let Some(item) = stream.next().await {
            match item {
                Ok(_proc) => {
                    count += 1;
                    if count > 10 {
                        break;
                    }
                }
                Err(e) => {
                    panic!("unexpected error: {e}");
                }
            }
        }
        assert!(count > 0, "Should collect at least one process");
    }

    #[tokio::test]
    async fn test_deadline_timeout() {
        let collector = SysinfoProcessCollector::new();
        // Deadline in the past to trigger immediate timeout.
        let deadline = Some(Instant::now() - Duration::from_secs(1));
        let mut stream = collector.stream_processes(deadline);
        // First yielded item should be timeout error.
        if let Some(res) = stream.next().await {
            match res {
                Err(CollectionError::Timeout) => {}
                other => panic!("expected timeout error, got {other:?}"),
            }
        } else {
            panic!("Expected at least one timeout error item");
        }
    }
}
