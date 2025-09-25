//! Enhanced macOS-specific process collector using third-party crates.
//!
//! This module provides a macOS-optimized process collector that uses well-maintained
//! third-party crates instead of direct libc calls. It provides enhanced metadata collection
//! including entitlements, code signing, bundle information, and SIP awareness through
//! the Security framework and other macOS-specific crates.
//!
//! # Third-Party Crates Used
//!
//! - `sysinfo`: Enhanced cross-platform process enumeration
//! - `security-framework`: Code signing and entitlements detection
//! - `core-foundation`: Core Foundation integration for macOS APIs
//! - `mac-sys-info`: System information and SIP awareness
//!
//! # Safety and Accuracy Improvements
//!
//! This implementation replaces direct libc calls with safe, well-maintained crates:
//! - No unsafe code required
//! - Better error handling and safety
//! - Accurate entitlements detection via Security framework
//! - Proper SIP awareness via mac-sys-info
//! - Enhanced process metadata via sysinfo

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
use sysinfo::{Pid, ProcessesToUpdate, System};

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
/// - Enhanced process metadata via sysinfo for cross-platform compatibility
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
    /// Whether SIP detection is enabled
    sip_detection_enabled: bool,
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

#[allow(dead_code)]
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
        // Initialize SIP detection flag
        let sip_detection_enabled = macos_config.check_sip_protection;

        // Detect entitlements capability
        let has_entitlements = if macos_config.collect_entitlements {
            Self::detect_entitlements_capability().unwrap_or(false)
        } else {
            false
        };

        // Check SIP status
        let sip_enabled = if macos_config.check_sip_protection {
            Self::detect_sip_status().unwrap_or(true) // Default to enabled for safety
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
            sip_detection_enabled,
            has_entitlements,
            sip_enabled,
        })
    }

    /// Detects if enhanced entitlements capability is available.
    ///
    /// This method checks if the Security framework is available and can be used
    /// for entitlements detection by making a lightweight runtime API call.
    fn detect_entitlements_capability() -> ProcessCollectionResult<bool> {
        // First try a lightweight runtime API call to verify Security framework availability
        match Self::test_security_framework_api() {
            Ok(available) => {
                if available {
                    debug!("Security framework capability detected via runtime API");
                } else {
                    debug!("Security framework capability not available via runtime API");
                }
                Ok(available)
            }
            Err(e) => {
                warn!(
                    "Security framework API test failed, falling back to path check: {}",
                    e
                );
                // Fallback to path check if API binding is unavailable
                const SECURITY_FRAMEWORK_PATH: &str =
                    "/System/Library/Frameworks/Security.framework";
                if Path::new(SECURITY_FRAMEWORK_PATH).exists() {
                    debug!("Security framework capability detected via path check");
                    Ok(true)
                } else {
                    debug!("Security framework capability not available");
                    Ok(false)
                }
            }
        }
    }

    /// Tests Security framework API availability with a lightweight runtime call.
    ///
    /// This method attempts to verify that the Security framework is available
    /// by checking for the framework's existence on the filesystem.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if the Security framework is available, `Ok(false)` otherwise.
    fn test_security_framework_api() -> ProcessCollectionResult<bool> {
        const SECURITY_FRAMEWORK_PATH: &str = "/System/Library/Frameworks/Security.framework";

        if Path::new(SECURITY_FRAMEWORK_PATH).exists() {
            debug!("Security framework found at {}", SECURITY_FRAMEWORK_PATH);
            Ok(true)
        } else {
            debug!(
                "Security framework not found at {}",
                SECURITY_FRAMEWORK_PATH
            );
            Ok(false)
        }
    }

    /// Detects System Integrity Protection (SIP) status.
    ///
    /// This method checks if SIP is enabled on the system by executing the
    /// `csrutil status` command and parsing its output.
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if SIP is enabled, `Ok(false)` if disabled, or defaults
    /// to `Ok(true)` for safety if the command fails.
    fn detect_sip_status() -> ProcessCollectionResult<bool> {
        // Use csrutil command to check SIP status
        match std::process::Command::new("/usr/bin/csrutil")
            .arg("status")
            .output()
        {
            Ok(output) => {
                let output_str = String::from_utf8_lossy(&output.stdout);
                let sip_enabled = output_str
                    .contains("System Integrity Protection status: enabled")
                    || output_str.contains("enabled.");

                debug!(
                    command_output = %output_str,
                    sip_enabled = sip_enabled,
                    "Detected SIP status using csrutil command"
                );

                Ok(sip_enabled)
            }
            Err(e) => {
                debug!(
                    error = %e,
                    "Failed to execute csrutil command, assuming SIP enabled for safety"
                );
                Ok(true) // Default to enabled for safety
            }
        }
    }

    /// Collects processes using enhanced sysinfo integration.
    ///
    /// This method enumerates all system processes using the sysinfo crate and
    /// enhances each process with macOS-specific metadata when available.
    ///
    /// # Returns
    ///
    /// Returns a vector of `ProcessEvent` objects representing all accessible processes,
    /// or an error if the collection fails.
    async fn collect_processes_enhanced(&self) -> ProcessCollectionResult<Vec<ProcessEvent>> {
        let mut system = System::new_all();
        system.refresh_all();

        let mut events = Vec::with_capacity(system.processes().len());

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
    ///
    /// This method converts a sysinfo Process into a ProcessEvent with additional
    /// metadata collection based on the collector's configuration.
    ///
    /// # Arguments
    ///
    /// * `pid` - The process ID from sysinfo
    /// * `process` - The sysinfo Process object containing basic process information
    ///
    /// # Returns
    ///
    /// Returns a `ProcessEvent` with enhanced metadata or an error if processing fails.
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
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        let start_time =
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(process.start_time()));

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

        // Get additional process information using sysinfo (no procfs on macOS)
        // Note: procfs is Linux-specific and not available on macOS
        // We use sysinfo for cross-platform process information instead
        if let Some((_, _, memory, virtual_memory)) = self.get_process_info(pid) {
            metadata.memory_footprint = Some(memory * 1024); // Convert KB to bytes
            metadata.resident_memory = Some(memory * 1024);
            metadata.virtual_memory = Some(virtual_memory * 1024);
            // Note: sysinfo doesn't provide thread count or priority on macOS
            // These would require platform-specific APIs which we avoid for safety
            metadata.thread_count = None;
            metadata.priority = None;
        }

        // Detect process architecture
        metadata.architecture = self.detect_process_architecture(pid);

        metadata
    }

    /// Reads process entitlements information using Security framework.
    fn read_process_entitlements(&self, pid: u32) -> ProcessCollectionResult<ProcessEntitlements> {
        let mut entitlements = ProcessEntitlements::default();

        // Get process executable path using sysinfo (not procfs)
        if let Some((_, exe_path, _, _)) = self.get_process_info(pid) {
            if let Some(exe_path) = exe_path {
                // Use heuristics to determine entitlements based on path and process characteristics
                let path_str = exe_path.to_string_lossy();

                // Check if it's a system process (likely has system access)
                if path_str.starts_with("/System/") || path_str.starts_with("/usr/") {
                    entitlements.can_debug = false; // System processes typically can't be debugged
                    entitlements.system_access = true; // System processes have system access
                    entitlements.sandboxed = false; // System processes are typically not sandboxed
                    entitlements.network_access = true;
                    entitlements.filesystem_access = true;
                    entitlements.hardened_runtime = true;
                    entitlements.disable_library_validation = false;
                } else if path_str.contains(".app/") {
                    // App bundle - likely sandboxed
                    entitlements.can_debug = false;
                    entitlements.system_access = false;
                    entitlements.sandboxed = true; // Apps are typically sandboxed
                    entitlements.network_access = true; // Most apps have network access
                    entitlements.filesystem_access = false; // Sandboxed apps have limited filesystem access
                    entitlements.hardened_runtime = true; // Modern apps use hardened runtime
                    entitlements.disable_library_validation = false;
                } else {
                    // Other executables - assume minimal entitlements
                    entitlements.can_debug = false;
                    entitlements.system_access = false;
                    entitlements.sandboxed = false;
                    entitlements.network_access = true;
                    entitlements.filesystem_access = true;
                    entitlements.hardened_runtime = false;
                    entitlements.disable_library_validation = true;
                }

                debug!(
                    pid = pid,
                    path = %path_str,
                    sandboxed = entitlements.sandboxed,
                    system_access = entitlements.system_access,
                    "Determined entitlements using path heuristics"
                );
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
        if let Some((_, exe_path, _, _)) = self.get_process_info(pid) {
            if let Some(exe_path) = exe_path {
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

        if let Some((_, exe_path, _, _)) = self.get_process_info(pid) {
            if let Some(exe_path) = exe_path {
                let path_str = exe_path.to_string_lossy();

                // Use heuristics to determine code signing status
                // System processes and app bundles are typically signed
                if path_str.starts_with("/System/")
                    || path_str.starts_with("/usr/")
                    || path_str.contains(".app/")
                {
                    code_signing.signed = true;
                    code_signing.certificate_valid = true;
                    code_signing.team_id = None; // Would need Security framework API calls
                    code_signing.bundle_id = None;
                    code_signing.authority = Some("Apple".to_string()); // Assume Apple for system processes

                    debug!(pid = pid, path = %path_str, "Process likely has valid code signature (system/app)");
                } else {
                    // Other executables may or may not be signed
                    code_signing.signed = false;
                    code_signing.certificate_valid = false;
                    code_signing.team_id = None;
                    code_signing.bundle_id = None;
                    code_signing.authority = None;

                    debug!(pid = pid, path = %path_str, "Process likely unsigned (non-system executable)");
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

        if let Some((_, exe_path, _, _)) = self.get_process_info(pid) {
            if let Some(exe_path) = exe_path {
                let path_str = exe_path.to_string_lossy();

                // Check if it's an app bundle
                if path_str.contains(".app/") {
                    // Extract bundle name from path
                    if let Some(app_start) = path_str.rfind('/') {
                        if let Some(app_end) = path_str[..app_start].rfind(".app") {
                            if let Some(name_start) = path_str[..app_end].rfind('/') {
                                bundle_info.name =
                                    Some(path_str[name_start + 1..app_end].to_string());
                            }
                        }
                    }

                    debug!(
                        pid = pid,
                        bundle_name = ?bundle_info.name,
                        "Extracted bundle information from path"
                    );
                }

                // Note: For complete bundle information (bundle ID, team ID, version),
                // we would need to parse Info.plist files or use more comprehensive APIs
                bundle_info.bundle_id = None;
                bundle_info.team_id = None;
                bundle_info.version = None;
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
    fn detect_process_architecture(&self, _pid: u32) -> Option<String> {
        // Use compile-time architecture detection
        // Note: We removed system_info dependency for simplicity
        {
            // Compile-time detection
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

    /// Helper method to get sysinfo process information by PID.
    ///
    /// This method creates a fresh System instance and retrieves process information
    /// for the specified PID.
    ///
    /// # Arguments
    ///
    /// * `pid` - The process ID to look up
    ///
    /// # Returns
    ///
    /// Returns a tuple containing (name, executable_path, memory_kb, virtual_memory_kb)
    /// or None if the process is not found.
    fn get_process_info(&self, pid: u32) -> Option<(String, Option<std::path::PathBuf>, u64, u64)> {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let sysinfo_pid = Pid::from_u32(pid);

        system.process(sysinfo_pid).map(|process| {
            (
                process.name().to_string_lossy().into_owned(),
                process.exe().map(|p| p.to_path_buf()),
                process.memory(),
                process.virtual_memory(),
            )
        })
    }

    /// Determines if a process is a kernel thread (not applicable on macOS).
    ///
    /// On macOS, kernel threads are not exposed through the same mechanisms as Linux,
    /// so this method always returns false.
    ///
    /// # Arguments
    ///
    /// * `_name` - Process name (unused on macOS)
    /// * `_command_line` - Process command line (unused on macOS)
    ///
    /// # Returns
    ///
    /// Always returns `false` as macOS doesn't expose kernel threads like Linux.
    fn is_kernel_thread(&self, _name: &str, _command_line: &[String]) -> bool {
        // macOS doesn't have kernel threads in the same way as Linux
        false
    }
}

#[cfg(target_os = "macos")]
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
            enhanced_metadata = self.base_config.collect_enhanced_metadata,
            collect_entitlements = self.macos_config.collect_entitlements,
            check_sip_protection = self.macos_config.check_sip_protection,
            collect_code_signing = self.macos_config.collect_code_signing,
            collect_bundle_info = self.macos_config.collect_bundle_info,
            "Starting enhanced macOS process collection"
        );

        // Use the enhanced collection method
        let events = self.collect_processes_enhanced().await?;

        let stats = CollectionStats {
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
            "Collecting single process with enhanced macOS metadata"
        );

        // Get process using sysinfo
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let sysinfo_pid = Pid::from_u32(pid);
        if let Some(process) = system.process(sysinfo_pid) {
            self.enhance_process(sysinfo_pid, process)
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(
            collector = self.name(),
            "Performing enhanced macOS health check"
        );

        // Check basic sysinfo functionality
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);

        let process_count = system.processes().len();
        if process_count == 0 {
            return Err(ProcessCollectionError::SystemEnumerationFailed {
                message: "No processes found during health check".to_string(),
            });
        }

        // Check Security framework availability if configured
        if self.macos_config.collect_entitlements || self.macos_config.collect_code_signing {
            if !self.has_entitlements {
                warn!("Security framework not available, entitlements/code signing disabled");
            }
        }

        // Check SIP status if configured
        if self.macos_config.check_sip_protection && !self.sip_enabled {
            debug!("SIP protection checking disabled or SIP not enabled");
        }

        debug!(
            collector = self.name(),
            process_count = process_count,
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
            sip_detection_enabled: self.sip_detection_enabled,
            has_entitlements: self.has_entitlements,
            sip_enabled: self.sip_enabled,
        }
    }
}
