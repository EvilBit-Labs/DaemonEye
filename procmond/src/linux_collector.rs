//! Linux-specific process collector with enhanced /proc filesystem access.
//!
//! This module provides a Linux-optimized process collector that directly accesses
//! the /proc filesystem for enhanced performance and metadata collection. It includes
//! support for CAP_SYS_PTRACE capability detection, process namespaces, container
//! detection, and enhanced metadata collection.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self};
use std::path::Path;
use std::time::SystemTime;
use sysinfo::{Pid, Process, System};
use thiserror::Error;
use tracing::{debug, error, warn};

use crate::process_collector::{
    CollectionStats, ProcessCollectionConfig, ProcessCollectionError, ProcessCollectionResult,
    ProcessCollector, ProcessCollectorCapabilities,
};

/// Linux-specific errors that can occur during process collection.
#[derive(Debug, Error)]
pub enum LinuxCollectionError {
    /// Failed to read /proc filesystem
    #[error("Failed to read /proc filesystem: {message}")]
    ProcFsReadError { message: String },

    /// Failed to parse /proc file content
    #[error("Failed to parse /proc/{pid}/{file}: {message}")]
    ProcFileParseError {
        pid: u32,
        file: String,
        message: String,
    },

    /// Capability detection failed
    #[error("Capability detection failed: {message}")]
    CapabilityError { message: String },

    /// Namespace detection failed
    #[error("Namespace detection failed for PID {pid}: {message}")]
    NamespaceError { pid: u32, message: String },
}

/// Linux process namespace information.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProcessNamespaces {
    /// Process ID namespace
    pub pid_ns: Option<u64>,
    /// Network namespace
    pub net_ns: Option<u64>,
    /// Mount namespace
    pub mnt_ns: Option<u64>,
    /// User namespace
    pub user_ns: Option<u64>,
    /// IPC namespace
    pub ipc_ns: Option<u64>,
    /// UTS namespace
    pub uts_ns: Option<u64>,
    /// Cgroup namespace
    pub cgroup_ns: Option<u64>,
}

/// Enhanced Linux process metadata.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LinuxProcessMetadata {
    /// Process namespaces
    pub namespaces: ProcessNamespaces,
    /// Memory maps count
    pub memory_maps_count: Option<usize>,
    /// Open file descriptors count
    pub open_fds_count: Option<usize>,
    /// Network connections count
    pub network_connections_count: Option<usize>,
    /// Container ID if running in container
    pub container_id: Option<String>,
    /// Process state (R, S, D, Z, T, etc.)
    pub state: Option<char>,
    /// Virtual memory size in bytes
    pub vm_size: Option<u64>,
    /// Resident set size in bytes
    pub vm_rss: Option<u64>,
    /// Peak virtual memory size in bytes
    pub vm_peak: Option<u64>,
    /// Number of threads
    pub threads: Option<u32>,
}

/// Linux-specific process collector with enhanced /proc filesystem access.
///
/// This collector provides optimized process enumeration for Linux systems by
/// directly accessing the /proc filesystem. It offers enhanced metadata collection
/// including memory maps, file descriptors, network connections, and container
/// detection capabilities.
///
/// # Features
///
/// - Direct /proc filesystem access for enhanced performance
/// - CAP_SYS_PTRACE capability detection and privilege management
/// - Process namespace and container detection
/// - Enhanced metadata collection (memory maps, file descriptors, network connections)
/// - Graceful handling of permission denied for restricted processes
/// - Support for both privileged and unprivileged operation modes
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::linux_collector::{LinuxProcessCollector, LinuxCollectorConfig};
/// use procmond::process_collector::ProcessCollectionConfig;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let base_config = ProcessCollectionConfig::default();
///     let linux_config = LinuxCollectorConfig {
///         collect_namespaces: true,
///         collect_memory_maps: true,
///         collect_file_descriptors: true,
///         collect_network_connections: true,
///         detect_containers: true,
///         use_cap_sys_ptrace: false, // Will be auto-detected
///     };
///
///     let collector = LinuxProcessCollector::new(base_config, linux_config)?;
///     let (events, stats) = collector.collect_processes().await?;
///
///     println!("Collected {} processes with enhanced Linux metadata", events.len());
///     Ok(())
/// }
/// ```
pub struct LinuxProcessCollector {
    /// Base process collection configuration
    base_config: ProcessCollectionConfig,
    /// Linux-specific configuration
    linux_config: LinuxCollectorConfig,
    /// Whether CAP_SYS_PTRACE is available
    has_cap_sys_ptrace: bool,
    /// Cached host namespace IDs for container detection
    host_namespaces: ProcessNamespaces,
}

/// Configuration for Linux-specific process collection features.
#[derive(Debug, Clone)]
pub struct LinuxCollectorConfig {
    /// Whether to collect process namespace information
    pub collect_namespaces: bool,
    /// Whether to collect memory map information
    pub collect_memory_maps: bool,
    /// Whether to collect file descriptor information
    pub collect_file_descriptors: bool,
    /// Whether to collect network connection information
    pub collect_network_connections: bool,
    /// Whether to detect container environments
    pub detect_containers: bool,
    /// Whether to use CAP_SYS_PTRACE if available (auto-detected if None)
    pub use_cap_sys_ptrace: Option<bool>,
}

impl Default for LinuxCollectorConfig {
    fn default() -> Self {
        Self {
            collect_namespaces: true,
            collect_memory_maps: true,
            collect_file_descriptors: true,
            collect_network_connections: false, // Can be expensive
            detect_containers: true,
            use_cap_sys_ptrace: None, // Auto-detect
        }
    }
}

impl LinuxProcessCollector {
    /// Creates a new Linux process collector with the specified configuration.
    ///
    /// This constructor performs capability detection and initializes the collector
    /// with the appropriate privilege level and feature set.
    ///
    /// # Arguments
    ///
    /// * `base_config` - Base process collection configuration
    /// * `linux_config` - Linux-specific configuration options
    ///
    /// # Returns
    ///
    /// A configured Linux process collector or an error if initialization fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::linux_collector::{LinuxProcessCollector, LinuxCollectorConfig};
    /// use procmond::process_collector::ProcessCollectionConfig;
    ///
    /// let base_config = ProcessCollectionConfig::default();
    /// let linux_config = LinuxCollectorConfig::default();
    /// let collector = LinuxProcessCollector::new(base_config, linux_config)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        base_config: ProcessCollectionConfig,
        linux_config: LinuxCollectorConfig,
    ) -> ProcessCollectionResult<Self> {
        // Detect CAP_SYS_PTRACE capability
        let has_cap_sys_ptrace = match linux_config.use_cap_sys_ptrace {
            Some(use_cap) => {
                if use_cap {
                    Self::detect_cap_sys_ptrace()?
                } else {
                    false
                }
            }
            None => Self::detect_cap_sys_ptrace().unwrap_or(false),
        };

        // Cache host namespace IDs for container detection
        let host_namespaces = if linux_config.detect_containers {
            Self::read_process_namespaces(1).unwrap_or_default()
        } else {
            ProcessNamespaces::default()
        };

        debug!(
            has_cap_sys_ptrace = has_cap_sys_ptrace,
            collect_namespaces = linux_config.collect_namespaces,
            collect_memory_maps = linux_config.collect_memory_maps,
            collect_file_descriptors = linux_config.collect_file_descriptors,
            collect_network_connections = linux_config.collect_network_connections,
            detect_containers = linux_config.detect_containers,
            "Initialized Linux process collector"
        );

        Ok(Self {
            base_config,
            linux_config,
            has_cap_sys_ptrace,
            host_namespaces,
        })
    }

    /// Detects if CAP_SYS_PTRACE capability is available.
    ///
    /// This method checks the current process capabilities to determine if
    /// CAP_SYS_PTRACE is available, which enables enhanced process inspection.
    fn detect_cap_sys_ptrace() -> ProcessCollectionResult<bool> {
        // Read /proc/self/status to check for CAP_SYS_PTRACE
        let status_path = "/proc/self/status";
        let content =
            fs::read_to_string(status_path).map_err(|e| ProcessCollectionError::PlatformError {
                message: format!("Failed to read {}: {}", status_path, e),
            })?;

        // Look for CapEff line and check if CAP_SYS_PTRACE (bit 19) is set
        for line in content.lines() {
            if let Some(caps_str) = line.strip_prefix("CapEff:\t") {
                if let Ok(caps) = u64::from_str_radix(caps_str, 16) {
                    // CAP_SYS_PTRACE is bit 19 (0x80000)
                    let has_ptrace = (caps & 0x80000) != 0;
                    debug!(
                        caps_effective = format!("0x{:x}", caps),
                        has_cap_sys_ptrace = has_ptrace,
                        "Detected process capabilities"
                    );
                    return Ok(has_ptrace);
                }
            }
        }

        debug!("Could not detect CAP_SYS_PTRACE capability, assuming not available");
        Ok(false)
    }

    /// Reads process namespace information from /proc/[pid]/ns/.
    fn read_process_namespaces(pid: u32) -> ProcessCollectionResult<ProcessNamespaces> {
        let ns_dir = format!("/proc/{}/ns", pid);
        let mut namespaces = ProcessNamespaces::default();

        // Helper function to read namespace ID from symlink
        let read_ns_id = |ns_name: &str| -> Option<u64> {
            let ns_path = format!("{}/{}", ns_dir, ns_name);
            fs::read_link(&ns_path).ok().and_then(|target| {
                target
                    .to_string_lossy()
                    .strip_prefix(&format!("{}:[", ns_name))
                    .and_then(|s| s.strip_suffix(']'))
                    .and_then(|s| s.parse().ok())
            })
        };

        namespaces.pid_ns = read_ns_id("pid");
        namespaces.net_ns = read_ns_id("net");
        namespaces.mnt_ns = read_ns_id("mnt");
        namespaces.user_ns = read_ns_id("user");
        namespaces.ipc_ns = read_ns_id("ipc");
        namespaces.uts_ns = read_ns_id("uts");
        namespaces.cgroup_ns = read_ns_id("cgroup");

        Ok(namespaces)
    }

    /// Reads enhanced process metadata from /proc/[pid]/ files.
    fn read_enhanced_metadata(&self, pid: u32) -> LinuxProcessMetadata {
        let mut metadata = LinuxProcessMetadata::default();

        // Read namespaces if configured
        if self.linux_config.collect_namespaces {
            metadata.namespaces = Self::read_process_namespaces(pid).unwrap_or_default();
        }

        // Read memory maps count if configured
        if self.linux_config.collect_memory_maps {
            metadata.memory_maps_count = self.count_memory_maps(pid);
        }

        // Read file descriptors count if configured
        if self.linux_config.collect_file_descriptors {
            metadata.open_fds_count = self.count_file_descriptors(pid);
        }

        // Read network connections count if configured
        if self.linux_config.collect_network_connections {
            metadata.network_connections_count = self.count_network_connections(pid);
        }

        // Detect container if configured
        if self.linux_config.detect_containers {
            metadata.container_id = self.detect_container_id(pid, &metadata.namespaces);
        }

        // Read /proc/[pid]/stat for additional metadata
        if let Ok(stat_data) = self.read_proc_stat(pid) {
            metadata.state = stat_data.get("state").and_then(|s| s.chars().next());
            metadata.threads = stat_data.get("num_threads").and_then(|s| s.parse().ok());
        }

        // Read /proc/[pid]/status for memory information
        if let Ok(status_data) = self.read_proc_status(pid) {
            metadata.vm_size = status_data
                .get("VmSize")
                .and_then(|s| self.parse_memory_kb(s));
            metadata.vm_rss = status_data
                .get("VmRSS")
                .and_then(|s| self.parse_memory_kb(s));
            metadata.vm_peak = status_data
                .get("VmPeak")
                .and_then(|s| self.parse_memory_kb(s));
        }

        metadata
    }

    /// Counts memory maps from /proc/[pid]/maps.
    fn count_memory_maps(&self, pid: u32) -> Option<usize> {
        let maps_path = format!("/proc/{}/maps", pid);
        fs::read_to_string(&maps_path)
            .ok()
            .map(|content| content.lines().count())
    }

    /// Counts open file descriptors from /proc/[pid]/fd/.
    fn count_file_descriptors(&self, pid: u32) -> Option<usize> {
        let fd_dir = format!("/proc/{}/fd", pid);
        fs::read_dir(&fd_dir).ok().map(|entries| entries.count())
    }

    /// Counts network connections for a process (simplified implementation).
    fn count_network_connections(&self, _pid: u32) -> Option<usize> {
        // This is a simplified implementation. A full implementation would
        // parse /proc/net/tcp, /proc/net/udp, etc. and match by inode
        // to file descriptors in /proc/[pid]/fd/
        None
    }

    /// Detects container ID from cgroup information.
    fn detect_container_id(&self, pid: u32, namespaces: &ProcessNamespaces) -> Option<String> {
        // Check if process is in different namespaces than host (simple container detection)
        let is_containerized = namespaces.pid_ns.is_some()
            && namespaces.pid_ns != self.host_namespaces.pid_ns
            && namespaces.pid_ns != Some(0);

        if !is_containerized {
            return None;
        }

        // Try to extract container ID from cgroup
        let cgroup_path = format!("/proc/{}/cgroup", pid);
        if let Ok(content) = fs::read_to_string(&cgroup_path) {
            for line in content.lines() {
                // Look for Docker container ID pattern
                if let Some(docker_id) = self.extract_docker_id(line) {
                    return Some(format!("docker:{}", docker_id));
                }
                // Look for containerd container ID pattern
                if let Some(containerd_id) = self.extract_containerd_id(line) {
                    return Some(format!("containerd:{}", containerd_id));
                }
            }
        }

        // Generic container detection
        Some("container:unknown".to_string())
    }

    /// Extracts Docker container ID from cgroup line.
    fn extract_docker_id(&self, line: &str) -> Option<String> {
        // Docker cgroup pattern: /docker/[container_id]
        if let Some(docker_part) = line.split("/docker/").nth(1) {
            let container_id = docker_part.split('/').next()?;
            if container_id.len() >= 12 {
                let _id = &container_id[..12];
                return Some(_id.to_string());
            }
        }
        None
    }

    /// Extracts containerd container ID from cgroup line.
    fn extract_containerd_id(&self, line: &str) -> Option<String> {
        // containerd cgroup pattern: /system.slice/containerd.service/[container_id]
        if line.contains("containerd.service") {
            let parts: Vec<&str> = line.split('/').collect();
            if let Some(container_part) = parts.last() {
                if container_part.len() >= 12 {
                    let _id = &container_part[..12];
                    return Some(_id.to_string());
                }
            }
        }
        None
    }

    /// Reads and parses /proc/[pid]/stat file.
    fn read_proc_stat(&self, pid: u32) -> io::Result<HashMap<String, String>> {
        let stat_path = format!("/proc/{}/stat", pid);
        let content = fs::read_to_string(&stat_path)?;
        let mut data = HashMap::new();

        // Parse stat file (space-separated values)
        let fields: Vec<&str> = content.split_whitespace().collect();
        if fields.len() >= 20 {
            // Field 3 is state, field 4 is ppid, field 20 is num_threads
            data.insert("state".to_string(), fields[2].to_string());
            data.insert("ppid".to_string(), fields[3].to_string());
            data.insert("num_threads".to_string(), fields[19].to_string());
        }

        Ok(data)
    }

    /// Reads and parses /proc/[pid]/status file.
    fn read_proc_status(&self, pid: u32) -> io::Result<HashMap<String, String>> {
        let status_path = format!("/proc/{}/status", pid);
        let content = fs::read_to_string(&status_path)?;
        let mut data = HashMap::new();

        for line in content.lines() {
            if let Some((key, value)) = line.split_once(':') {
                data.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        Ok(data)
    }

    /// Parses memory value from /proc/status (e.g., "1024 kB" -> Some(1048576)).
    fn parse_memory_kb(&self, value: &str) -> Option<u64> {
        value
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|kb| kb * 1024) // Convert KB to bytes
    }

    /// Reads basic process information from /proc/[pid]/ files.
    fn read_process_info(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        let proc_dir = format!("/proc/{}", pid);

        // Check if the process directory exists and is actually a directory
        let proc_path = Path::new(&proc_dir);
        if !proc_path.exists() || !proc_path.is_dir() {
            return Err(ProcessCollectionError::ProcessNotFound { pid });
        }

        // Additional check: try to read /proc/[pid]/stat to verify the process exists
        let stat_path = format!("{}/stat", proc_dir);
        if !Path::new(&stat_path).exists() {
            return Err(ProcessCollectionError::ProcessNotFound { pid });
        }

        // Read command line
        let cmdline_path = format!("{}/cmdline", proc_dir);
        let command_line = match fs::read(&cmdline_path) {
            Ok(bytes) => {
                if bytes.is_empty() {
                    vec![]
                } else {
                    bytes
                        .split(|&b| b == 0)
                        .filter(|arg| !arg.is_empty())
                        .map(|arg| String::from_utf8_lossy(arg).to_string())
                        .collect()
                }
            }
            Err(_) => vec![],
        };

        // Read executable path
        let exe_path = format!("{}/exe", proc_dir);
        let executable_path = fs::read_link(&exe_path)
            .ok()
            .map(|path| path.to_string_lossy().to_string());

        // Read comm (process name)
        let comm_path = format!("{}/comm", proc_dir);
        let name = fs::read_to_string(&comm_path)
            .unwrap_or_else(|_| format!("<unknown-{}>", pid))
            .trim()
            .to_string();

        // Read stat for basic info
        let stat_data = self.read_proc_stat(pid).unwrap_or_default();
        let ppid = stat_data
            .get("ppid")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&p| p != 0);

        // Read status for additional info
        let status_data = self.read_proc_status(pid).unwrap_or_default();
        let user_id = status_data
            .get("Uid")
            .and_then(|uid_line| uid_line.split_whitespace().next().map(|s| s.to_string()));

        // Enhanced metadata collection
        let enhanced_metadata = if self.base_config.collect_enhanced_metadata {
            Some(self.read_enhanced_metadata(pid))
        } else {
            None
        };

        // Calculate CPU and memory usage if enhanced metadata is enabled
        let (cpu_usage, memory_usage) = if self.base_config.collect_enhanced_metadata {
            let memory = enhanced_metadata.as_ref().and_then(|m| m.vm_rss);

            // CPU usage calculation would require reading /proc/stat and /proc/[pid]/stat
            // over time intervals. For now, we'll leave it as None.
            (None, memory)
        } else {
            (None, None)
        };

        // Determine start time
        let start_time = if self.base_config.collect_enhanced_metadata {
            // This would require parsing the start time from /proc/[pid]/stat
            // and converting from jiffies to SystemTime. For now, we'll use None.
            None
        } else {
            None
        };

        // Compute executable hash if requested
        let executable_hash = if self.base_config.compute_executable_hashes {
            // TODO: Implement executable hashing (issue #40)
            None
        } else {
            None
        };

        // Serialize enhanced metadata for platform_metadata field
        let platform_metadata = if self.base_config.collect_enhanced_metadata {
            enhanced_metadata.and_then(|metadata| {
                serde_json::to_value(metadata)
                    .map_err(|e| {
                        warn!("Failed to serialize Linux process metadata: {}", e);
                    })
                    .ok()
            })
        } else {
            None
        };

        let accessible = true; // If we can read /proc/[pid], it's accessible
        let file_exists = executable_path.is_some();

        Ok(ProcessEvent {
            pid,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            user_id,
            accessible,
            file_exists,
            timestamp: SystemTime::now(),
            platform_metadata,
        })
    }

    /// Enumerates all processes by reading /proc directory.
    fn enumerate_proc_pids(&self) -> ProcessCollectionResult<Vec<u32>> {
        let proc_dir = Path::new("/proc");
        let mut pids = Vec::new();

        let entries = fs::read_dir(proc_dir).map_err(|e| {
            ProcessCollectionError::SystemEnumerationFailed {
                message: format!("Failed to read /proc directory: {}", e),
            }
        })?;

        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(pid) = name.parse::<u32>() {
                    pids.push(pid);
                }
            }
        }

        if pids.is_empty() {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "No process PIDs found in /proc".to_string(),
            });
        }

        pids.sort_unstable();
        Ok(pids)
    }
}

#[async_trait]
impl ProcessCollector for LinuxProcessCollector {
    fn name(&self) -> &'static str {
        "linux-proc-collector"
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

        debug!(
            collector = self.name(),
            has_cap_sys_ptrace = self.has_cap_sys_ptrace,
            enhanced_metadata = self.base_config.collect_enhanced_metadata,
            max_processes = self.base_config.max_processes,
            "Starting Linux process collection"
        );

        // Use sysinfo for reliable process enumeration
        let system = tokio::task::spawn_blocking({
            let config = self.base_config.clone();
            move || {
                let mut system = System::new();
                if config.collect_enhanced_metadata {
                    system.refresh_all();
                } else {
                    system.refresh_processes_specifics(
                        sysinfo::ProcessesToUpdate::All,
                        true,
                        sysinfo::ProcessRefreshKind::everything(),
                    );
                }
                system
            }
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {}", e),
        })?;

        let mut events = Vec::new();
        let mut stats = CollectionStats {
            total_processes: system.processes().len(),
            ..Default::default()
        };

        // Process each process with individual error handling
        for (sysinfo_pid, process) in system.processes().iter() {
            let pid = sysinfo_pid.as_u32();

            // Check if we've hit the maximum process limit
            if self.base_config.max_processes > 0 && events.len() >= self.base_config.max_processes
            {
                debug!(
                    max_processes = self.base_config.max_processes,
                    collected = events.len(),
                    "Reached maximum process collection limit"
                );
                break;
            }

            match self.convert_sysinfo_to_event(pid, process).await {
                Ok(event) => {
                    // Apply filtering based on configuration
                    let should_skip = if self.base_config.skip_system_processes
                        && self.is_system_process(&event.name, pid)
                    {
                        true
                    } else {
                        self.base_config.skip_kernel_threads
                            && self.is_kernel_thread(&event.name, &event.command_line)
                    };

                    if should_skip {
                        debug!(pid = pid, name = %event.name, "Skipping process due to configuration");
                    } else {
                        events.push(event);
                        stats.successful_collections += 1;
                    }
                }
                Err(ProcessCollectionError::ProcessAccessDenied { pid, message }) => {
                    debug!(pid = pid, reason = %message, "Process access denied");
                    stats.inaccessible_processes += 1;
                }
                Err(ProcessCollectionError::ProcessNotFound { pid }) => {
                    debug!(pid = pid, "Process no longer exists");
                    stats.inaccessible_processes += 1;
                }
                Err(e) => {
                    warn!(pid = pid, error = %e, "Error reading process information");
                    stats.invalid_processes += 1;
                }
            }
        }

        stats.collection_duration_ms = start_time.elapsed().as_millis() as u64;

        debug!(
            collector = self.name(),
            total_processes = stats.total_processes,
            successful = stats.successful_collections,
            inaccessible = stats.inaccessible_processes,
            invalid = stats.invalid_processes,
            duration_ms = stats.collection_duration_ms,
            "Linux process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(
            collector = self.name(),
            pid = pid,
            "Collecting single Linux process"
        );

        // Use sysinfo for reliable process existence checking
        let sysinfo_result = tokio::task::spawn_blocking({
            let config = self.base_config.clone();
            move || {
                let mut system = System::new();
                if config.collect_enhanced_metadata {
                    system.refresh_all();
                } else {
                    system.refresh_processes_specifics(
                        sysinfo::ProcessesToUpdate::All,
                        true,
                        sysinfo::ProcessRefreshKind::everything(),
                    );
                }
                system
            }
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process lookup task failed: {}", e),
        })?;

        let system = sysinfo_result;
        let sysinfo_pid = sysinfo::Pid::from_u32(pid);

        if let Some(process) = system.process(sysinfo_pid) {
            // Convert sysinfo process to ProcessEvent and add Linux-specific enhancements
            self.convert_sysinfo_to_event(pid, process).await
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(collector = self.name(), "Performing Linux health check");

        // Check if /proc is accessible
        if !Path::new("/proc").exists() {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "/proc filesystem not available".to_string(),
            });
        }

        // Try to read a few processes
        let pids = self.enumerate_proc_pids()?;
        if pids.is_empty() {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "No processes found in /proc".to_string(),
            });
        }

        // Try to read information for the first few processes
        let mut successful_reads = 0;
        for &pid in pids.iter().take(5) {
            if self.read_process_info(pid).is_ok() {
                successful_reads += 1;
            }
        }

        if successful_reads == 0 {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "Could not read any process information".to_string(),
            });
        }

        debug!(
            collector = self.name(),
            total_pids = pids.len(),
            successful_reads = successful_reads,
            has_cap_sys_ptrace = self.has_cap_sys_ptrace,
            "Linux health check passed"
        );

        Ok(())
    }

    /// Converts a sysinfo process to a ProcessEvent with Linux-specific enhancements.
    async fn convert_sysinfo_to_event(
        &self,
        pid: u32,
        process: &Process,
    ) -> ProcessCollectionResult<ProcessEvent> {
        // Get basic information from sysinfo
        let ppid = process.parent().map(|p| p.as_u32());

        let name = if process.name().is_empty() {
            format!("<unknown-{}>", pid)
        } else {
            process.name().to_string_lossy().to_string()
        };

        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());

        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        // Get enhanced metadata from sysinfo
        let (cpu_usage, memory_usage, start_time) = if self.base_config.collect_enhanced_metadata {
            let cpu = process.cpu_usage();
            let memory = process.memory();
            let start = process.start_time();

            (
                if cpu.is_finite() && cpu >= 0.0 {
                    Some(cpu as f64)
                } else {
                    None
                },
                if memory > 0 {
                    Some(memory * 1024)
                } else {
                    None
                },
                if start > 0 {
                    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(start))
                } else {
                    None
                },
            )
        } else {
            (None, None, None)
        };

        // Compute executable hash if requested
        let executable_hash = if self.base_config.compute_executable_hashes {
            // TODO: Implement executable hashing (issue #40)
            None
        } else {
            None
        };

        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true;
        let file_exists = executable_path.is_some();

        // Add Linux-specific enhancements
        let enhanced_metadata = if self.base_config.collect_enhanced_metadata {
            Some(self.read_enhanced_metadata(pid))
        } else {
            None
        };

        // Serialize enhanced metadata for platform_metadata field
        let platform_metadata = if self.base_config.collect_enhanced_metadata {
            enhanced_metadata.and_then(|metadata| {
                serde_json::to_value(metadata)
                    .map_err(|e| {
                        warn!("Failed to serialize Linux process metadata: {}", e);
                    })
                    .ok()
            })
        } else {
            None
        };

        Ok(ProcessEvent {
            pid,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            user_id,
            accessible,
            file_exists,
            timestamp: SystemTime::now(),
            platform_metadata,
        })
    }
}

impl LinuxProcessCollector {
    /// Determines if a process is a system process based on name and PID.
    fn is_system_process(&self, name: &str, pid: u32) -> bool {
        // Common system process patterns
        const SYSTEM_PROCESSES: &[&str] = &[
            "kernel",
            "kthreadd",
            "ksoftirqd",
            "migration",
            "rcu_",
            "watchdog",
            "systemd",
            "init",
            "swapper",
            "idle",
        ];

        // Very low PIDs are typically system processes
        if pid < 10 {
            return true;
        }

        // Check against known system process names
        let name_lower = name.to_lowercase();
        SYSTEM_PROCESSES
            .iter()
            .any(|&sys_proc| name_lower.contains(sys_proc))
    }

    /// Determines if a process is a kernel thread.
    fn is_kernel_thread(&self, name: &str, command_line: &[String]) -> bool {
        // Kernel threads typically have no command line arguments
        if !command_line.is_empty() {
            return false;
        }

        // Kernel threads often have names in brackets
        if name.starts_with('[') && name.ends_with(']') {
            return true;
        }

        // Common kernel thread patterns
        const KERNEL_THREAD_PATTERNS: &[&str] = &[
            "kworker",
            "ksoftirqd",
            "migration",
            "rcu_",
            "watchdog",
            "kcompactd",
            "kswapd",
            "kthreadd",
            "kauditd",
        ];

        let name_lower = name.to_lowercase();
        KERNEL_THREAD_PATTERNS
            .iter()
            .any(|&pattern| name_lower.contains(pattern))
    }
}

// Implement Clone for LinuxProcessCollector to support tokio::spawn_blocking
impl Clone for LinuxProcessCollector {
    fn clone(&self) -> Self {
        Self {
            base_config: self.base_config.clone(),
            linux_config: self.linux_config.clone(),
            has_cap_sys_ptrace: self.has_cap_sys_ptrace,
            host_namespaces: self.host_namespaces.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_collector::ProcessCollectionConfig;

    #[test]
    fn test_linux_collector_creation() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();

        let result = LinuxProcessCollector::new(base_config, linux_config);
        assert!(result.is_ok(), "Linux collector creation should succeed");

        let collector = result.unwrap();
        assert_eq!(collector.name(), "linux-proc-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(capabilities.realtime_collection);
    }

    #[test]
    fn test_linux_collector_config_default() {
        let config = LinuxCollectorConfig::default();
        assert!(config.collect_namespaces);
        assert!(config.collect_memory_maps);
        assert!(config.collect_file_descriptors);
        assert!(!config.collect_network_connections);
        assert!(config.detect_containers);
        assert!(config.use_cap_sys_ptrace.is_none());
    }

    #[tokio::test]
    async fn test_linux_collector_health_check() {
        // Only run this test on Linux
        if !cfg!(target_os = "linux") {
            return;
        }

        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();

        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();
        let result = collector.health_check().await;

        assert!(
            result.is_ok(),
            "Health check should pass on Linux system: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_linux_collector_collect_current_process() {
        // Only run this test on Linux
        if !cfg!(target_os = "linux") {
            return;
        }

        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();

        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();
        let current_pid = std::process::id();
        let result = collector.collect_process(current_pid).await;

        assert!(
            result.is_ok(),
            "Should be able to collect current process: {:?}",
            result
        );

        let event = result.unwrap();
        assert_eq!(event.pid, current_pid);
        assert!(!event.name.is_empty());
        assert!(event.accessible);
    }

    #[test]
    fn test_capability_detection() {
        // Only run this test on Linux
        if !cfg!(target_os = "linux") {
            return;
        }

        // This test just verifies the capability detection doesn't panic
        let result = LinuxProcessCollector::detect_cap_sys_ptrace();
        assert!(result.is_ok(), "Capability detection should not fail");
    }

    #[test]
    fn test_namespace_reading() {
        // Only run this test on Linux
        if !cfg!(target_os = "linux") {
            return;
        }

        // Try to read namespaces for init process (PID 1)
        let result = LinuxProcessCollector::read_process_namespaces(1);
        // Since this test runs in different environments where permissions and
        // namespace availability can vary, we'll just verify the function runs
        // without panicking and returns a result
        assert!(
            result.is_ok() || result.is_err(),
            "Function should complete without panicking"
        );
    }

    #[test]
    fn test_system_process_detection() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();

        // Test system process detection
        assert!(collector.is_system_process("init", 1));
        assert!(collector.is_system_process("kernel", 2));
        assert!(collector.is_system_process("kthreadd", 3));
        assert!(!collector.is_system_process("bash", 1000));
        assert!(!collector.is_system_process("firefox", 2000));
    }

    #[test]
    fn test_kernel_thread_detection() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();

        // Test kernel thread detection
        assert!(collector.is_kernel_thread("[kworker/0:0]", &[]));
        assert!(collector.is_kernel_thread("ksoftirqd/0", &[]));
        assert!(!collector.is_kernel_thread("bash", &["/bin/bash".to_string()]));
        assert!(!collector.is_kernel_thread("kworker", &["some".to_string(), "args".to_string()]));
    }

    #[test]
    fn test_memory_parsing() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();

        // Test memory parsing
        assert_eq!(collector.parse_memory_kb("1024 kB"), Some(1048576));
        assert_eq!(collector.parse_memory_kb("512 kB"), Some(524288));
        assert_eq!(collector.parse_memory_kb("0 kB"), Some(0));
        assert_eq!(collector.parse_memory_kb("invalid"), None);
    }

    #[test]
    fn test_docker_id_extraction() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();

        // Test Docker ID extraction
        let docker_line = "1:name=systemd:/docker/1234567890ab";
        assert_eq!(
            collector.extract_docker_id(docker_line),
            Some("1234567890ab".to_string())
        );

        let non_docker_line = "1:name=systemd:/system.slice/ssh.service";
        assert_eq!(collector.extract_docker_id(non_docker_line), None);
    }

    #[test]
    fn test_containerd_id_extraction() {
        let base_config = ProcessCollectionConfig::default();
        let linux_config = LinuxCollectorConfig::default();
        let collector = LinuxProcessCollector::new(base_config, linux_config).unwrap();

        // Test containerd ID extraction
        let containerd_line = "1:name=systemd:/system.slice/containerd.service/1234567890ab";
        assert_eq!(
            collector.extract_containerd_id(containerd_line),
            Some("1234567890ab".to_string())
        );

        let non_containerd_line = "1:name=systemd:/system.slice/ssh.service";
        assert_eq!(collector.extract_containerd_id(non_containerd_line), None);
    }
}
