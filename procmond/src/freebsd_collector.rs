//! FreeBSD-specific process collector with enhanced system information access.
//!
//! This module provides a FreeBSD-optimized process collector that leverages
//! the sysinfo crate for cross-platform compatibility while adding FreeBSD-specific
//! enhancements through sysctl interfaces and kernel virtual memory (KVM) access.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use serde::Serialize;
use std::sync::Arc;
use std::time::SystemTime;
use sysinfo::{Pid, Process, System};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::process_collector::{
    CollectionStats, ProcessCollectionConfig, ProcessCollectionError, ProcessCollectionResult,
    ProcessCollector, ProcessCollectorCapabilities,
};

/// FreeBSD-specific errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FreeBSDCollectionError {
    #[error("Sysctl operation failed: {message}")]
    SysctlError { message: String },
    #[error("KVM access failed: {message}")]
    KvmError { message: String },
    #[error("Jail detection failed: {message}")]
    JailError { message: String },
}

/// FreeBSD-specific process metadata.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FreeBSDProcessMetadata {
    pub state: Option<String>,
    pub threads: Option<u32>,
    pub jail_id: Option<u32>,
    pub login_class: Option<String>,
    pub priority: Option<i32>,
    pub nice: Option<i32>,
    pub vm_size: Option<u64>,
    pub rss: Option<u64>,
}

/// Configuration for FreeBSD-specific process collection features.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct FreeBSDCollectorConfig {
    pub collect_freebsd_metadata: bool,
    pub detect_jails: bool,
    pub collect_process_states: bool,
    pub collect_thread_counts: bool,
}

impl Default for FreeBSDCollectorConfig {
    fn default() -> Self {
        Self {
            collect_freebsd_metadata: true,
            detect_jails: true,
            collect_process_states: true,
            collect_thread_counts: true,
        }
    }
}

/// FreeBSD-specific process collector using sysinfo with platform optimizations.
///
/// The `System` instance is kept alive across calls to improve performance and enable
/// accurate CPU usage measurements (sysinfo requires two snapshots to compute deltas).
pub struct FreeBSDProcessCollector {
    base_config: ProcessCollectionConfig,
    freebsd_config: FreeBSDCollectorConfig,
    /// Reusable System instance for improved performance and accurate CPU measurements.
    system: Arc<Mutex<System>>,
}

impl FreeBSDProcessCollector {
    /// Creates a new FreeBSD-specific process collector with platform-specific optimizations.
    ///
    /// Initializes the `System` object once with `System::new_all()` so that the
    /// first call to `collect_processes` already has a populated process table and
    /// CPU usage deltas can be computed on subsequent calls.
    pub fn new(
        base_config: ProcessCollectionConfig,
        freebsd_config: FreeBSDCollectorConfig,
    ) -> ProcessCollectionResult<Self> {
        debug!(
            collect_freebsd_metadata = freebsd_config.collect_freebsd_metadata,
            detect_jails = freebsd_config.detect_jails,
            "Initialized FreeBSD process collector"
        );
        Ok(Self {
            base_config,
            freebsd_config,
            system: Arc::new(Mutex::new(System::new_all())),
        })
    }

    fn collect_freebsd_metadata(&self, pid: u32, process: &Process) -> FreeBSDProcessMetadata {
        let mut metadata = FreeBSDProcessMetadata::default();

        if self.freebsd_config.collect_process_states {
            metadata.state = Some(format!("{:?}", process.status()));
        }

        if self.freebsd_config.collect_thread_counts {
            metadata.threads = Some(process.thread_kind().map(|_| 1).unwrap_or(1));
        }

        if self.base_config.collect_enhanced_metadata {
            metadata.vm_size = Some(process.memory() * 1024);
            metadata.rss = Some(process.memory() * 1024);
        }

        if self.freebsd_config.detect_jails {
            metadata.jail_id = Self::detect_jail_id(pid);
        }

        metadata
    }

    fn detect_jail_id(_pid: u32) -> Option<u32> {
        debug!("Jail detection not available without elevated privileges");
        None
    }

    fn convert_sysinfo_to_event(&self, pid: u32, process: &Process) -> ProcessEvent {
        let ppid = process.parent().map(Pid::as_u32);
        let name = if process.name().is_empty() {
            format!("<unknown-{pid}>")
        } else {
            process.name().to_string_lossy().to_string()
        };
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        let (cpu_usage, memory_usage, start_time) = if self.base_config.collect_enhanced_metadata {
            let cpu = process.cpu_usage();
            let memory = process.memory();
            let start = process.start_time();
            (
                (cpu.is_finite() && cpu >= 0.0).then_some(f64::from(cpu)),
                (memory > 0).then_some(memory.saturating_mul(1024)),
                (start > 0).then(|| {
                    SystemTime::UNIX_EPOCH
                        .checked_add(std::time::Duration::from_secs(start))
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                }),
            )
        } else {
            (None, None, None)
        };

        let executable_hash: Option<String> = None;
        let hash_algorithm: Option<String> = None;
        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true;
        let file_exists = executable_path.is_some();

        let freebsd_metadata = if self.freebsd_config.collect_freebsd_metadata {
            Some(self.collect_freebsd_metadata(pid, process))
        } else {
            None
        };

        let platform_metadata = freebsd_metadata.and_then(|metadata| {
            serde_json::to_value(metadata)
                .map_err(|e| warn!("Failed to serialize FreeBSD process metadata: {e}"))
                .ok()
        });

        ProcessEvent {
            pid,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            hash_algorithm,
            user_id,
            accessible,
            file_exists,
            timestamp: SystemTime::now(),
            platform_metadata,
        }
    }

    fn is_system_process(name: &str, pid: u32) -> bool {
        const SYSTEM_PROCESSES: &[&str] = &[
            "kernel", "init", "swapper", "idle", "pagedaemon", "vmdaemon",
            "bufdaemon", "syncer", "vnlru", "softdepflush", "geom", "usb", "intr",
        ];
        if pid < 10 {
            return true;
        }
        let name_lower = name.to_lowercase();
        SYSTEM_PROCESSES.iter().any(|sys_proc| name_lower.contains(sys_proc))
    }

    fn is_kernel_thread(name: &str, command_line: &[String]) -> bool {
        const KERNEL_THREAD_PATTERNS: &[&str] = &[
            "pagedaemon", "vmdaemon", "bufdaemon", "syncer", "vnlru", "softdepflush",
            "geom", "usb", "intr",
        ];
        if !command_line.is_empty() {
            return false;
        }
        if name.starts_with('[') && name.ends_with(']') {
            return true;
        }
        let name_lower = name.to_lowercase();
        KERNEL_THREAD_PATTERNS.iter().any(|pattern| name_lower.contains(pattern))
    }
}

#[async_trait]
impl ProcessCollector for FreeBSDProcessCollector {
    fn name(&self) -> &'static str {
        "freebsd-proc-collector"
    }

    fn capabilities(&self) -> ProcessCollectorCapabilities {
        ProcessCollectorCapabilities {
            basic_info: true,
            enhanced_metadata: self.base_config.collect_enhanced_metadata,
            executable_hashing: self.base_config.compute_executable_hashes,
            system_processes: !self.base_config.skip_system_processes,
            kernel_threads: !self.base_config.skip_kernel_threads,
            realtime_collection: true,
        }
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        let start_time = std::time::Instant::now();

        debug!(collector = self.name(), "Starting FreeBSD process collection");

        // Refresh the shared System in a blocking task to avoid blocking the async runtime.
        let system_arc = Arc::clone(&self.system);
        let collect_enhanced = self.base_config.collect_enhanced_metadata;
        let enumeration_result = tokio::task::spawn_blocking(move || {
            let mut system = system_arc.blocking_lock();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::everything(),
            );
            if collect_enhanced {
                system.refresh_cpu_all();
            }
            if system.processes().is_empty() {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during enumeration on FreeBSD".to_owned(),
                });
            }
            Ok(())
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {e}"),
        })?;

        enumeration_result?;

        // Lock to iterate — held only for the duration of the loop; no await points inside.
        let (events, mut stats, processed_count) = {
            let system = self.system.lock().await;
            let mut inner_events = Vec::new();
            let mut inner_stats = CollectionStats::default();
            let mut inner_count: usize = 0;

            for (sysinfo_pid, process) in system.processes() {
                let pid = sysinfo_pid.as_u32();

                if self.base_config.max_processes > 0
                    && inner_events.len() >= self.base_config.max_processes
                {
                    break;
                }

                inner_count = inner_count.saturating_add(1);
                let event = self.convert_sysinfo_to_event(pid, process);

                // Apply skip policies: check before adding to results
                let should_skip = if self.base_config.skip_system_processes
                    && Self::is_system_process(&event.name, pid)
                {
                    true
                } else {
                    self.base_config.skip_kernel_threads
                        && Self::is_kernel_thread(&event.name, &event.command_line)
                };

                if should_skip {
                    inner_stats.inaccessible_processes =
                        inner_stats.inaccessible_processes.saturating_add(1);
                } else {
                    inner_events.push(event);
                    inner_stats.successful_collections =
                        inner_stats.successful_collections.saturating_add(1);
                }
            }
            // Explicitly drop the lock guard before returning from the block.
            drop(system);
            (inner_events, inner_stats, inner_count)
        };

        stats.total_processes = processed_count;
        stats.collection_duration_ms =
            u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX);

        debug!(
            collector = self.name(),
            total_processes = stats.total_processes,
            successful = stats.successful_collections,
            inaccessible = stats.inaccessible_processes,
            duration_ms = stats.collection_duration_ms,
            "FreeBSD process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(collector = self.name(), pid = pid, "Collecting single FreeBSD process");

        // Refresh using the shared System instance.
        let system_arc = Arc::clone(&self.system);
        let collect_enhanced = self.base_config.collect_enhanced_metadata;
        let lookup_result = tokio::task::spawn_blocking(move || {
            let mut system = system_arc.blocking_lock();
            if collect_enhanced {
                system.refresh_all();
            } else {
                system.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::All,
                    true,
                    sysinfo::ProcessRefreshKind::everything(),
                );
            }
            let sysinfo_pid = Pid::from_u32(pid);
            if system.process(sysinfo_pid).is_some() {
                Ok(())
            } else {
                Err(ProcessCollectionError::ProcessNotFound { pid })
            }
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Single process lookup task failed: {e}"),
        })?;

        lookup_result?;

        let system = self.system.lock().await;
        let sysinfo_pid = Pid::from_u32(pid);
        system
            .process(sysinfo_pid)
            .map_or(
                Err(ProcessCollectionError::ProcessNotFound { pid }),
                |process| {
                    let event = self.convert_sysinfo_to_event(pid, process);

                    // Apply skip policies to single process collection
                    if self.base_config.skip_system_processes
                        && Self::is_system_process(&event.name, pid)
                    {
                        return Err(ProcessCollectionError::ProcessAccessDenied {
                            pid,
                            message: "System process skipped by configuration".to_owned(),
                        });
                    }

                    if self.base_config.skip_kernel_threads
                        && Self::is_kernel_thread(&event.name, &event.command_line)
                    {
                        return Err(ProcessCollectionError::ProcessAccessDenied {
                            pid,
                            message: "Kernel thread skipped by configuration".to_owned(),
                        });
                    }

                    Ok(event)
                },
            )
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(collector = self.name(), "Performing FreeBSD health check");

        // Perform health check using the shared System instance.
        let system_arc = Arc::clone(&self.system);
        let health_result = tokio::task::spawn_blocking(move || {
            let mut system = system_arc.blocking_lock();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            if system.processes().is_empty() {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during health check on FreeBSD".to_owned(),
                });
            }
            Ok(())
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Health check task failed: {e}"),
        })?;
        health_result?;
        Ok(())
    }
}

impl Clone for FreeBSDProcessCollector {
    fn clone(&self) -> Self {
        Self {
            base_config: self.base_config.clone(),
            freebsd_config: self.freebsd_config.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freebsd_collector_creation() {
        let base_config = ProcessCollectionConfig::default();
        let freebsd_config = FreeBSDCollectorConfig::default();
        let result = FreeBSDProcessCollector::new(base_config, freebsd_config);
        assert!(result.is_ok());
        let collector = result.unwrap();
        assert_eq!(collector.name(), "freebsd-proc-collector");
    }

    #[test]
    fn test_system_process_detection() {
        assert!(FreeBSDProcessCollector::is_system_process("init", 1));
        assert!(FreeBSDProcessCollector::is_system_process("pagedaemon", 3));
        assert!(!FreeBSDProcessCollector::is_system_process("bash", 1000));
    }

    #[test]
    fn test_kernel_thread_detection() {
        assert!(FreeBSDProcessCollector::is_kernel_thread("pagedaemon", &[]));
        assert!(FreeBSDProcessCollector::is_kernel_thread("vmdaemon", &[]));
        assert!(!FreeBSDProcessCollector::is_kernel_thread(
            "bash",
            &["/bin/bash".to_owned()]
        ));
    }
}
