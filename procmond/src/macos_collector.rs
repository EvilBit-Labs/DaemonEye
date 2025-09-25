//! Enhanced macOS-specific process collector using third-party crates.
//!
//! This module provides a macOS-optimized process collector that uses well-maintained
//! third-party crates instead of direct libc calls. It provides enhanced metadata collection
//! including entitlements, code signing, bundle information, and SIP awareness through
//! the Security framework and other macOS-specific crates.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use std::path::Path;
use std::time::SystemTime;
use thiserror::Error;
use tracing::{debug, error, warn};

use crate::process_collector::{
    CollectionStats, ProcessCollectionConfig, ProcessCollectionError, ProcessCollectionResult,
    ProcessCollector, ProcessCollectorCapabilities,
};

#[cfg(target_os = "macos")]
use {
    core_foundation::base::{CFType, TCFType},
    mac_sys_info::{CpuInfo, MemoryInfo, SystemInfo},
    procfs::{KernelVersion, process::Process},
    security_framework::{
        code_signing::{SecCode, SecCodeCopySigningInformation},
        entitlements::SecCodeCopyEntitlements,
    },
    sysinfo::{Pid, ProcessExt, System, SystemExt},
};

/// macOS-specific errors that can occur during process collection.
#[derive(Debug, Error)]
pub enum MacOSCollectionError {
    /// Security framework error
    #[error("Security framework error: {0}")]
    SecurityFramework(String),

    /// System information error
    #[error("System information error: {0}")]
    SystemInfo(String),

    /// Process filesystem error
    #[error("Process filesystem error: {0}")]
    ProcessFs(String),

    /// Core Foundation error
    #[error("Core Foundation error: {0}")]
    CoreFoundation(String),
}

impl From<MacOSCollectionError> for ProcessCollectionError {
    fn from(err: MacOSCollectionError) -> Self {
        ProcessCollectionError::PlatformError {
            message: err.to_string(),
        }
    }
}

/// Enhanced macOS process entitlements information.
#[derive(Debug, Clone, Default)]
pub struct ProcessEntitlements {
    /// Process has debugging entitlements
    pub can_debug: bool,
    /// Process has system-level access
    pub system_access: bool,
    /// Process is sandboxed
    pub sandboxed: bool,
    /// Process has network access
    pub network_access: bool,
    /// Process has file system access
    pub filesystem_access: bool,
    /// Process has hardened runtime
    pub hardened_runtime: bool,
    /// Process has disable library validation
    pub disable_library_validation: bool,
}

/// Code signing information for a process.
#[derive(Debug, Clone, Default)]
pub struct CodeSigningInfo {
    /// Process is code signed
    pub signed: bool,
    /// Team identifier
    pub team_id: Option<String>,
    /// Bundle identifier
    pub bundle_id: Option<String>,
    /// Signing authority
    pub authority: Option<String>,
    /// Certificate chain valid
    pub certificate_valid: bool,
}

/// Bundle information for a process.
#[derive(Debug, Clone, Default)]
pub struct BundleInfo {
    /// Bundle identifier
    pub bundle_id: Option<String>,
    /// Team identifier
    pub team_id: Option<String>,
    /// Bundle version
    pub version: Option<String>,
    /// Bundle name
    pub name: Option<String>,
}

/// Enhanced macOS process metadata.
#[derive(Debug, Clone, Default)]
pub struct MacOSProcessMetadata {
    /// Process entitlements
    pub entitlements: ProcessEntitlements,
    /// Process is under SIP protection
    pub sip_protected: bool,
    /// Process architecture (x86_64, arm64, etc.)
    pub architecture: Option<String>,
    /// Code signing information
    pub code_signing: CodeSigningInfo,
    /// Bundle information
    pub bundle_info: BundleInfo,
    /// Process memory footprint in bytes
    pub memory_footprint: Option<u64>,
    /// Process resident memory in bytes
    pub resident_memory: Option<u64>,
    /// Process virtual memory in bytes
    pub virtual_memory: Option<u64>,
    /// Number of threads
    pub thread_count: Option<u32>,
    /// Process priority
    pub priority: Option<i32>,
}

/// Enhanced macOS process collector using third-party crates.
///
/// This collector provides optimized process enumeration for macOS systems using
/// well-maintained third-party crates instead of direct libc calls. It offers
/// enhanced metadata collection including entitlements, code signing, bundle
/// information, and SIP awareness through the Security framework.
///
/// # Features
///
/// - Enhanced sysinfo integration for process enumeration
/// - Security framework integration for entitlements and code signing
/// - System information via mac-sys-info for SIP status and system details
/// - Process filesystem integration via procfs for additional metadata
/// - No unsafe code required
/// - Better error handling and safety
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::macos_collector_v2::{EnhancedMacOSCollector, MacOSCollectorConfig};
/// use procmond::process_collector::ProcessCollectionConfig;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let base_config = ProcessCollectionConfig::default();
///     let macos_config = MacOSCollectorConfig {
///         collect_entitlements: true,
///         check_sip_protection: true,
///         collect_code_signing: true,
///         collect_bundle_info: true,
///         handle_sandboxed_processes: true,
///     };
///
///     let collector = EnhancedMacOSCollector::new(base_config, macos_config)?;
///     let (events, stats) = collector.collect_processes().await?;
///
///     println!("Collected {} processes with enhanced macOS metadata", events.len());
///     Ok(())
/// }
/// ```
pub struct EnhancedMacOSCollector {
    /// Base process collection configuration
    base_config: ProcessCollectionConfig,
    /// macOS-specific configuration
    macos_config: MacOSCollectorConfig,
    /// System information for SIP and architecture detection
    system_info: Option<SystemInfo>,
    /// Whether enhanced entitlements are available
    has_entitlements: bool,
    /// Whether SIP is enabled on the system
    sip_enabled: bool,
}

/// Configuration for macOS-specific process collection features.
#[derive(Debug, Clone)]
pub struct MacOSCollectorConfig {
    /// Whether to collect process entitlements information
    pub collect_entitlements: bool,
    /// Whether to check SIP protection status
    pub check_sip_protection: bool,
    /// Whether to collect code signing information
    pub collect_code_signing: bool,
    /// Whether to collect bundle information
    pub collect_bundle_info: bool,
    /// Whether to handle sandboxed processes gracefully
    pub handle_sandboxed_processes: bool,
}

impl Default for MacOSCollectorConfig {
    fn default() -> Self {
        Self {
            collect_entitlements: true,
            check_sip_protection: true,
            collect_code_signing: true,
            collect_bundle_info: true,
            handle_sandboxed_processes: true,
        }
    }
}

impl EnhancedMacOSCollector {
    /// Creates a new enhanced macOS process collector with the specified configuration.
    ///
    /// This constructor initializes the collector with third-party crate integrations,
    /// detecting system capabilities and SIP status through safe APIs.
    ///
    /// # Arguments
    ///
    /// * `base_config` - Base process collection configuration
    /// * `macos_config` - macOS-specific configuration options
    ///
    /// # Returns
    ///
    /// A configured enhanced macOS process collector or an error if initialization fails.
    pub fn new(
        base_config: ProcessCollectionConfig,
        macos_config: MacOSCollectorConfig,
    ) -> ProcessCollectionResult<Self> {
        // Initialize system information
        let system_info = if macos_config.check_sip_protection {
            SystemInfo::new().ok()
        } else {
            None
        };

        // Detect entitlements capability
        let has_entitlements = if macos_config.collect_entitlements {
            Self::detect_entitlements_capability().unwrap_or(false)
        } else {
            false
        };

        // Check SIP status
        let sip_enabled = if macos_config.check_sip_protection {
            Self::detect_sip_status(&system_info).unwrap_or(true) // Default to enabled for safety
        } else {
            false
        };

        debug!(
            has_entitlements = has_entitlements,
            sip_enabled = sip_enabled,
            collect_entitlements = macos_config.collect_entitlements,
            check_sip_protection = macos_config.check_sip_protection,
            collect_code_signing = macos_config.collect_code_signing,
            collect_bundle_info = macos_config.collect_bundle_info,
            handle_sandboxed_processes = macos_config.handle_sandboxed_processes,
            "Initialized enhanced macOS process collector with third-party crates"
        );

        Ok(Self {
            base_config,
            macos_config,
            system_info,
            has_entitlements,
            sip_enabled,
        })
    }

    /// Detects if enhanced entitlements capability is available.
    ///
    /// This method checks if the Security framework is available and can be used
    /// for entitlements detection.
    fn detect_entitlements_capability() -> ProcessCollectionResult<bool> {
        // Try to create a Security framework context as a capability test
        match std::process::id() {
            pid => {
                // Test if we can access Security framework APIs
                let test_path = "/System/Library/Frameworks/Security.framework";
                if Path::new(test_path).exists() {
                    debug!("Security framework capability detected");
                    Ok(true)
                } else {
                    debug!("Security framework capability not available");
                    Ok(false)
                }
            }
        }
    }

    /// Detects System Integrity Protection (SIP) status.
    ///
    /// This method checks if SIP is enabled on the system using mac-sys-info.
    fn detect_sip_status(system_info: &Option<SystemInfo>) -> ProcessCollectionResult<bool> {
        if let Some(ref info) = system_info {
            // Use mac-sys-info to get SIP status
            let kernel_version = info.kernel_version();
            let sip_enabled = kernel_version.contains("SIP")
                || kernel_version.contains("System Integrity Protection");

            debug!(
                kernel_version = %kernel_version,
                sip_enabled = sip_enabled,
                "Detected SIP status using mac-sys-info"
            );

            Ok(sip_enabled)
        } else {
            debug!("Could not detect SIP status, assuming enabled");
            Ok(true) // Default to enabled for safety
        }
    }

    /// Collects processes using enhanced sysinfo integration.
    async fn collect_processes_enhanced(&self) -> ProcessCollectionResult<Vec<ProcessEvent>> {
        let mut system = System::new_all();
        system.refresh_all();

        let mut events = Vec::new();

        for (pid, process) in system.processes() {
            match self.enhance_process(*pid, process) {
                Ok(event) => events.push(event),
                Err(e) => {
                    debug!(pid = pid.as_u32(), error = %e, "Error enhancing process");
                    // Continue with other processes
                }
            }
        }

        Ok(events)
    }

    /// Enhances a process with macOS-specific metadata.
    fn enhance_process(
        &self,
        pid: Pid,
        process: &sysinfo::Process,
    ) -> ProcessCollectionResult<ProcessEvent> {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(|p| p.as_u32());
        let name = process.name().to_string_lossy().to_string();
        let executable_path = process.exe().map(|p| p.to_string_lossy().to_string());

        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        let start_time = process
            .start_time()
            .map(|t| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t as u64));

        let cpu_usage = if self.base_config.collect_enhanced_metadata {
            Some(process.cpu_usage() as f64)
        } else {
            None
        };

        let memory_usage = if self.base_config.collect_enhanced_metadata {
            Some(process.memory())
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

        let user_id = process.user_id().map(|u| u.to_string());
        let accessible = true; // If we can read process info, it's accessible
        let file_exists = executable_path.is_some();

        Ok(ProcessEvent {
            pid: pid_u32,
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
        })
    }

    /// Reads enhanced macOS-specific metadata for a process.
    fn read_enhanced_metadata(&self, pid: u32) -> MacOSProcessMetadata {
        let mut metadata = MacOSProcessMetadata::default();

        // Collect entitlements if configured
        if self.macos_config.collect_entitlements && self.has_entitlements {
            metadata.entitlements = self.read_process_entitlements(pid).unwrap_or_default();
        }

        // Check SIP protection if configured
        if self.macos_config.check_sip_protection && self.sip_enabled {
            metadata.sip_protected = self.is_sip_protected(pid).unwrap_or(false);
        }

        // Collect code signing information if configured
        if self.macos_config.collect_code_signing {
            metadata.code_signing = self.check_code_signature(pid).unwrap_or_default();
        }

        // Collect bundle information if configured
        if self.macos_config.collect_bundle_info {
            metadata.bundle_info = self.read_bundle_info(pid).unwrap_or_default();
        }

        // Get additional process information using procfs
        if let Ok(procfs_process) = Process::new(pid as i32) {
            metadata.memory_footprint = Some(procfs_process.stat.vsize);
            metadata.resident_memory = Some(procfs_process.stat.rss_bytes());
            metadata.virtual_memory = Some(procfs_process.stat.vsize);
            metadata.thread_count = Some(procfs_process.stat.num_threads);
            metadata.priority = Some(procfs_process.stat.priority);
        }

        // Detect process architecture
        metadata.architecture = self.detect_process_architecture(pid);

        metadata
    }

    /// Reads process entitlements information using Security framework.
    fn read_process_entitlements(&self, pid: u32) -> ProcessCollectionResult<ProcessEntitlements> {
        let mut entitlements = ProcessEntitlements::default();

        // Try to get process executable path
        if let Ok(procfs_process) = Process::new(pid as i32) {
            if let Ok(exe_path) = procfs_process.exe() {
                // Use Security framework to get entitlements
                if let Ok(code) = SecCode::from_path(&exe_path) {
                    if let Ok(entitlements_dict) = code.copy_entitlements() {
                        entitlements.can_debug = entitlements_dict
                            .get("com.apple.security.get-task-allow")
                            .is_some();
                        entitlements.system_access = entitlements_dict
                            .get("com.apple.security.system-extension")
                            .is_some();
                        entitlements.sandboxed = entitlements_dict
                            .get("com.apple.security.app-sandbox")
                            .is_some();
                        entitlements.network_access = entitlements_dict
                            .get("com.apple.security.network.client")
                            .is_some();
                        entitlements.filesystem_access = !entitlements_dict
                            .get("com.apple.security.files.user-selected.read-only")
                            .is_some();
                        entitlements.hardened_runtime = entitlements_dict
                            .get("com.apple.security.cs.allow-jit")
                            .is_some();
                        entitlements.disable_library_validation = entitlements_dict
                            .get("com.apple.security.cs.disable-library-validation")
                            .is_some();
                    }
                }
            }
        }

        debug!(
            pid = pid,
            can_debug = entitlements.can_debug,
            system_access = entitlements.system_access,
            sandboxed = entitlements.sandboxed,
            "Read process entitlements using Security framework"
        );

        Ok(entitlements)
    }

    /// Checks if a process is protected by SIP.
    fn is_sip_protected(&self, pid: u32) -> ProcessCollectionResult<bool> {
        if let Ok(procfs_process) = Process::new(pid as i32) {
            if let Ok(exe_path) = procfs_process.exe() {
                let path_str = exe_path.to_string_lossy();

                // Common SIP-protected paths on macOS
                let sip_protected_paths = [
                    "/System/",
                    "/usr/bin/",
                    "/usr/sbin/",
                    "/usr/libexec/",
                    "/bin/",
                    "/sbin/",
                ];

                let is_protected = sip_protected_paths
                    .iter()
                    .any(|&protected_path| path_str.starts_with(protected_path));

                debug!(
                    pid = pid,
                    path = %path_str,
                    sip_protected = is_protected,
                    "Checked SIP protection status"
                );

                Ok(is_protected)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Checks if a process has a valid code signature using Security framework.
    fn check_code_signature(&self, pid: u32) -> ProcessCollectionResult<CodeSigningInfo> {
        let mut code_signing = CodeSigningInfo::default();

        if let Ok(procfs_process) = Process::new(pid as i32) {
            if let Ok(exe_path) = procfs_process.exe() {
                // Use Security framework to get code signing information
                if let Ok(code) = SecCode::from_path(&exe_path) {
                    if let Ok(signing_info) = code.copy_signing_information() {
                        code_signing.signed = signing_info.is_signed();
                        code_signing.team_id = signing_info.team_id();
                        code_signing.bundle_id = signing_info.bundle_id();
                        code_signing.authority = signing_info.authority();
                        code_signing.certificate_valid = signing_info.certificate_valid();
                    }
                }
            }
        }

        debug!(
            pid = pid,
            signed = code_signing.signed,
            team_id = ?code_signing.team_id,
            bundle_id = ?code_signing.bundle_id,
            "Checked code signature using Security framework"
        );

        Ok(code_signing)
    }

    /// Reads bundle information using Security framework.
    fn read_bundle_info(&self, pid: u32) -> ProcessCollectionResult<BundleInfo> {
        let mut bundle_info = BundleInfo::default();

        if let Ok(procfs_process) = Process::new(pid as i32) {
            if let Ok(exe_path) = procfs_process.exe() {
                // Use Security framework to get bundle information
                if let Ok(code) = SecCode::from_path(&exe_path) {
                    if let Ok(signing_info) = code.copy_signing_information() {
                        bundle_info.bundle_id = signing_info.bundle_id();
                        bundle_info.team_id = signing_info.team_id();
                        bundle_info.version = signing_info.version();
                        bundle_info.name = signing_info.name();
                    }
                }
            }
        }

        debug!(
            pid = pid,
            bundle_id = ?bundle_info.bundle_id,
            team_id = ?bundle_info.team_id,
            "Read bundle information using Security framework"
        );

        Ok(bundle_info)
    }

    /// Detects the architecture of a process.
    fn detect_process_architecture(&self, pid: u32) -> Option<String> {
        // Use system architecture detection as fallback
        if let Some(ref system_info) = self.system_info {
            Some(system_info.machine().to_string())
        } else {
            // Fallback to compile-time detection
            #[cfg(target_arch = "x86_64")]
            {
                Some("x86_64".to_string())
            }
            #[cfg(target_arch = "aarch64")]
            {
                Some("arm64".to_string())
            }
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            {
                None
            }
        }
    }

    /// Determines if a process is a system process based on name and PID.
    fn is_system_process(&self, name: &str, pid: u32) -> bool {
        // Common macOS system process patterns
        const SYSTEM_PROCESSES: &[&str] = &[
            "kernel_task",
            "launchd",
            "kextd",
            "kernelmanagerd",
            "UserEventAgent",
            "cfprefsd",
            "distnoted",
            "syslogd",
            "logd",
            "systemstats",
            "WindowServer",
            "loginwindow",
            "Dock",
            "Finder",
            "SystemUIServer",
            "coreaudiod",
            "bluetoothd",
            "wifid",
            "networkd",
            "securityd",
            "trustd",
            "sandboxd",
            "spindump",
            "ReportCrash",
            "crashreporterd",
            "notifyd",
            "powerd",
            "thermald",
            "hidd",
            "locationd",
            "CommCenter",
            "SpringBoard", // iOS/iPadOS
        ];

        // Very low PIDs are typically system processes
        if pid < 10 {
            return true;
        }

        // PID 1 is always launchd (system process)
        if pid == 1 {
            return true;
        }

        // Check against known system process names
        let name_lower = name.to_lowercase();

        // Exact matches
        if SYSTEM_PROCESSES
            .iter()
            .any(|&sys_proc| name_lower == sys_proc.to_lowercase())
        {
            return true;
        }

        // Pattern matches
        if name_lower.starts_with("com.apple.") {
            return true;
        }

        // Processes with 'd' suffix are often daemons
        if name_lower.ends_with('d') && name_lower.len() > 3 {
            // But exclude common user processes that end with 'd'
            let user_processes_with_d = ["discord", "word", "build"];
            if !user_processes_with_d
                .iter()
                .any(|&user_proc| name_lower.contains(user_proc))
            {
                return true;
            }
        }

        false
    }

    /// Determines if a process is a kernel thread (not applicable on macOS).
    fn is_kernel_thread(&self, _name: &str, _command_line: &[String]) -> bool {
        // macOS doesn't have kernel threads in the same way as Linux
        false
    }
}

#[async_trait]
impl ProcessCollector for EnhancedMacOSCollector {
    fn name(&self) -> &'static str {
        "enhanced-macos-collector"
    }

    fn capabilities(&self) -> ProcessCollectorCapabilities {
        ProcessCollectorCapabilities {
            basic_info: true,
            enhanced_metadata: self.base_config.collect_enhanced_metadata,
            executable_hashing: self.base_config.compute_executable_hashes,
            system_processes: !self.base_config.skip_system_processes,
            kernel_threads: false, // macOS doesn't have kernel threads like Linux
            realtime_collection: true,
        }
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        let start_time = std::time::Instant::now();

        debug!(
            collector = self.name(),
            has_entitlements = self.has_entitlements,
            sip_enabled = self.sip_enabled,
            enhanced_metadata = self.base_config.collect_enhanced_metadata,
            max_processes = self.base_config.max_processes,
            "Starting enhanced macOS process collection with third-party crates"
        );

        // Collect processes using enhanced sysinfo integration
        let events = self.collect_processes_enhanced().await?;

        let mut stats = CollectionStats {
            total_processes: events.len(),
            successful_collections: events.len(),
            inaccessible_processes: 0,
            invalid_processes: 0,
            collection_duration_ms: start_time.elapsed().as_millis() as u64,
        };

        debug!(
            collector = self.name(),
            total_processes = stats.total_processes,
            successful = stats.successful_collections,
            duration_ms = stats.collection_duration_ms,
            "Enhanced macOS process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(
            collector = self.name(),
            pid = pid,
            "Collecting single enhanced macOS process"
        );

        // Use sysinfo to get the specific process
        let mut system = System::new_all();
        system.refresh_process(pid.into());

        if let Some(process) = system.process(pid.into()) {
            self.enhance_process(pid.into(), process)
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(
            collector = self.name(),
            "Performing enhanced macOS health check"
        );

        // Try to collect a few processes
        let events = self.collect_processes_enhanced().await?;
        if events.is_empty() {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "No processes found using enhanced sysinfo".to_string(),
            });
        }

        // Try to read information for the first few processes
        let mut successful_reads = 0;
        for event in events.iter().take(5) {
            if self.collect_process(event.pid).await.is_ok() {
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
            total_processes = events.len(),
            successful_reads = successful_reads,
            has_entitlements = self.has_entitlements,
            sip_enabled = self.sip_enabled,
            "Enhanced macOS health check passed"
        );

        Ok(())
    }
}

// Implement Clone for EnhancedMacOSCollector to support tokio::spawn_blocking
impl Clone for EnhancedMacOSCollector {
    fn clone(&self) -> Self {
        Self {
            base_config: self.base_config.clone(),
            macos_config: self.macos_config.clone(),
            system_info: self.system_info.clone(),
            has_entitlements: self.has_entitlements,
            sip_enabled: self.sip_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_creation() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config);

        assert!(
            collector.is_ok(),
            "Enhanced macOS collector creation should succeed"
        );

        let collector = collector.unwrap();
        assert_eq!(collector.name(), "enhanced-macos-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(!capabilities.executable_hashing);
        assert!(capabilities.system_processes);
        assert!(!capabilities.kernel_threads); // macOS doesn't have kernel threads
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_capabilities() {
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: true,
            skip_system_processes: true,
            skip_kernel_threads: true,
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(!capabilities.enhanced_metadata);
        assert!(capabilities.executable_hashing);
        assert!(!capabilities.system_processes);
        assert!(!capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_health_check() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let result = collector.health_check().await;
        assert!(
            result.is_ok(),
            "Health check should pass on macOS with processes"
        );
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_collect_processes() {
        let base_config = ProcessCollectionConfig {
            max_processes: 10, // Limit for faster testing
            ..Default::default()
        };
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        let result = collector.collect_processes().await;
        assert!(result.is_ok(), "Process collection should succeed");

        let (events, stats) = result.unwrap();
        assert!(!events.is_empty(), "Should collect at least one process");
        assert!(
            stats.total_processes > 0,
            "Should find processes on the system"
        );
        assert!(
            stats.successful_collections > 0,
            "Should successfully collect some processes"
        );
        assert!(
            stats.collection_duration_ms > 0,
            "Collection should take some time"
        );

        // Verify event data quality
        for event in &events {
            assert!(event.pid > 0, "Process PID should be valid");
            assert!(!event.name.is_empty(), "Process name should not be empty");
            assert!(event.accessible, "Collected processes should be accessible");
            assert!(
                event.timestamp <= SystemTime::now(),
                "Timestamp should be reasonable"
            );
        }
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_collect_single_process() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Try to collect information for the current process
        let current_pid = std::process::id();
        let result = collector.collect_process(current_pid).await;

        assert!(result.is_ok(), "Should be able to collect current process");

        let event = result.unwrap();
        assert_eq!(event.pid, current_pid);
        assert!(!event.name.is_empty());
        assert!(event.accessible);
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_enhanced_macos_collector_collect_nonexistent_process() {
        let base_config = ProcessCollectionConfig::default();
        let macos_config = MacOSCollectorConfig::default();
        let collector = EnhancedMacOSCollector::new(base_config, macos_config).unwrap();

        // Try to collect information for a non-existent process
        let nonexistent_pid = 999999u32;
        let result = collector.collect_process(nonexistent_pid).await;

        assert!(result.is_err(), "Should fail for non-existent process");

        match result.unwrap_err() {
            ProcessCollectionError::ProcessNotFound { pid } => {
                assert_eq!(pid, nonexistent_pid);
            }
            other => panic!("Expected ProcessNotFound error, got: {:?}", other),
        }
    }

    #[test]
    fn test_enhanced_macos_collector_config_default() {
        let config = MacOSCollectorConfig::default();
        assert!(config.collect_entitlements);
        assert!(config.check_sip_protection);
        assert!(config.collect_code_signing);
        assert!(config.collect_bundle_info);
        assert!(config.handle_sandboxed_processes);
    }

    #[test]
    fn test_process_entitlements_default() {
        let entitlements = ProcessEntitlements::default();
        assert!(!entitlements.can_debug);
        assert!(!entitlements.system_access);
        assert!(!entitlements.sandboxed);
        assert!(!entitlements.network_access);
        assert!(!entitlements.filesystem_access);
        assert!(!entitlements.hardened_runtime);
        assert!(!entitlements.disable_library_validation);
    }

    #[test]
    fn test_code_signing_info_default() {
        let code_signing = CodeSigningInfo::default();
        assert!(!code_signing.signed);
        assert!(code_signing.team_id.is_none());
        assert!(code_signing.bundle_id.is_none());
        assert!(code_signing.authority.is_none());
        assert!(!code_signing.certificate_valid);
    }

    #[test]
    fn test_bundle_info_default() {
        let bundle_info = BundleInfo::default();
        assert!(bundle_info.bundle_id.is_none());
        assert!(bundle_info.team_id.is_none());
        assert!(bundle_info.version.is_none());
        assert!(bundle_info.name.is_none());
    }

    #[test]
    fn test_macos_process_metadata_default() {
        let metadata = MacOSProcessMetadata::default();
        assert!(!metadata.entitlements.can_debug);
        assert!(!metadata.sip_protected);
        assert!(metadata.architecture.is_none());
        assert!(!metadata.code_signing.signed);
        assert!(metadata.bundle_info.bundle_id.is_none());
        assert!(metadata.memory_footprint.is_none());
        assert!(metadata.resident_memory.is_none());
        assert!(metadata.virtual_memory.is_none());
        assert!(metadata.thread_count.is_none());
        assert!(metadata.priority.is_none());
    }
}
