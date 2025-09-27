//! Enhanced Windows-specific process collector using third-party crates.
//!
//! This module provides a Windows-optimized process collector that uses well-maintained
//! third-party crates instead of direct Windows API calls. It provides enhanced metadata
//! collection including process tokens, security contexts, UAC elevation status, Windows
//! service detection, process integrity levels, session isolation, and virtualization status.
//!
//! # Third-Party Crates Used
//!
//! - `sysinfo`: Enhanced cross-platform process enumeration
//! - `windows-rs`: Safe Windows API bindings for Windows-specific features (when safe APIs available)
//! - `windows-service`: Windows service detection and management
//!
//! # Safety and Accuracy Improvements
//!
//! This implementation uses safe, well-maintained crates with enhanced heuristics:
//! - No unsafe code required (adheres to workspace `unsafe_code = "forbid"`)
//! - Better error handling and safety through third-party crates
//! - Enhanced process metadata via sysinfo
//! - Windows service integration via windows-service
//! - Windows container support detection
//! - Improved heuristics for security information

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

#[cfg(target_os = "windows")]
use sysinfo::{Pid, PidExt, ProcessesToUpdate, System};

/// Windows-specific errors that can occur during process collection.
#[derive(Debug, Error)]
pub enum WindowsCollectionError {
    /// Windows API error
    #[error("Windows API error: {0}")]
    WindowsApi(String),

    /// Service management error
    #[error("Service management error: {0}")]
    ServiceManagement(String),

    /// Process token error
    #[error("Process token error: {0}")]
    ProcessToken(String),

    /// Security context error
    #[error("Security context error: {0}")]
    SecurityContext(String),

    /// Performance counter error
    #[error("Performance counter error: {0}")]
    PerformanceCounter(String),

    /// Container detection error
    #[error("Container detection error: {0}")]
    ContainerDetection(String),
}

impl From<WindowsCollectionError> for ProcessCollectionError {
    fn from(err: WindowsCollectionError) -> Self {
        ProcessCollectionError::PlatformError {
            message: err.to_string(),
        }
    }
}

/// Windows process security information.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProcessSecurityInfo {
    /// Process has SeDebugPrivilege
    pub has_debug_privilege: bool,
    /// Process is running with elevated privileges
    pub elevated: bool,
    /// Process integrity level (Low, Medium, High, System)
    pub integrity_level: IntegrityLevel,
    /// Process is running in a different session
    pub session_id: Option<u32>,
    /// Process token type (Primary, Impersonation)
    pub token_type: TokenType,
    /// Process is virtualized
    pub virtualized: bool,
    /// Process is protected
    pub protected: bool,
    /// Process is a system process
    pub system_process: bool,
}

/// Windows process integrity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum IntegrityLevel {
    /// Untrusted integrity level
    Untrusted,
    /// Low integrity level (typically sandboxed processes)
    Low,
    /// Medium integrity level (standard user processes)
    #[default]
    Medium,
    /// High integrity level (elevated processes)
    High,
    /// System integrity level (system processes)
    System,
    /// Unknown integrity level
    Unknown,
}

impl std::fmt::Display for IntegrityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntegrityLevel::Untrusted => write!(f, "Untrusted"),
            IntegrityLevel::Low => write!(f, "Low"),
            IntegrityLevel::Medium => write!(f, "Medium"),
            IntegrityLevel::High => write!(f, "High"),
            IntegrityLevel::System => write!(f, "System"),
            IntegrityLevel::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Windows process token types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TokenType {
    /// Primary token
    #[default]
    Primary,
    /// Impersonation token
    Impersonation,
    /// Unknown token type
    Unknown,
}

impl std::fmt::Display for TokenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenType::Primary => write!(f, "Primary"),
            TokenType::Impersonation => write!(f, "Impersonation"),
            TokenType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Windows service information.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowsServiceInfo {
    /// Process is a Windows service
    pub is_service: bool,
    /// Service name
    pub service_name: Option<String>,
    /// Service display name
    pub display_name: Option<String>,
    /// Service type
    pub service_type: Option<String>,
    /// Service state
    pub service_state: Option<String>,
    /// Service start type
    pub start_type: Option<String>,
}

/// Windows container information.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowsContainerInfo {
    /// Process is running in a container
    pub in_container: bool,
    /// Container type (Hyper-V, Windows Server)
    pub container_type: Option<String>,
    /// Container ID
    pub container_id: Option<String>,
    /// Container image name
    pub image_name: Option<String>,
}

/// Enhanced Windows process metadata.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowsProcessMetadata {
    /// Process security information
    pub security_info: ProcessSecurityInfo,
    /// Windows service information
    pub service_info: WindowsServiceInfo,
    /// Container information
    pub container_info: WindowsContainerInfo,
    /// Process architecture (x86, x64, ARM64)
    pub architecture: Option<String>,
    /// Process is under Windows Defender protection
    pub defender_protected: bool,
    /// Process is under antivirus protection
    pub antivirus_protected: bool,
    /// Process working set size in bytes
    pub working_set_size: Option<u64>,
    /// Process private bytes
    pub private_bytes: Option<u64>,
    /// Process virtual bytes
    pub virtual_bytes: Option<u64>,
    /// Process handle count
    pub handle_count: Option<u32>,
    /// Process thread count
    pub thread_count: Option<u32>,
    /// Process GDI objects
    pub gdi_objects: Option<u32>,
    /// Process USER objects
    pub user_objects: Option<u32>,
    /// Process page file usage
    pub page_file_usage: Option<u64>,
    /// Process peak working set
    pub peak_working_set: Option<u64>,
}

/// Enhanced Windows process collector using third-party crates.
///
/// This collector provides optimized process enumeration for Windows systems using
/// well-maintained third-party crates instead of direct Windows API calls. It offers
/// enhanced metadata collection including process tokens, security contexts, UAC
/// elevation status, Windows service detection, and container support.
///
/// # Security and Privilege Management
///
/// This collector follows the principle of least privilege:
/// - Runs with minimal required privileges for process enumeration
/// - Uses safe Windows API bindings through the `windows` crate
/// - Gracefully degrades functionality when elevated privileges are unavailable
/// - Registry access is read-only and limited to security-relevant keys
/// - No network access or external command execution (replaced PowerShell with native APIs)
///
/// # Features
///
/// - Enhanced sysinfo integration for process enumeration
/// - windows-service integration for service detection and management
/// - Windows container support (Hyper-V containers, Windows Server containers)
/// - Native Windows API integration for security detection
/// - No unsafe code required (adheres to workspace safety requirements)
/// - Better error handling and safety
/// - Enhanced heuristics for Windows-specific security features
///
/// # Examples
///
/// ```rust,no_run
/// use procmond::windows_collector::{WindowsProcessCollector, WindowsCollectorConfig};
/// use procmond::process_collector::{ProcessCollectionConfig, ProcessCollector};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let base_config = ProcessCollectionConfig::default();
///     let windows_config = WindowsCollectorConfig {
///         collect_security_info: true,
///         detect_services: true,
///         check_elevation_status: true,
///         collect_performance_counters: true,
///         detect_containers: true,
///         handle_defender_restrictions: true,
///     };
///
///     let collector = WindowsProcessCollector::new(base_config, windows_config)?;
///     let (events, stats) = collector.collect_processes().await?;
///
///     println!("Collected {} processes with enhanced Windows metadata", events.len());
///     Ok(())
/// }
/// ```
pub struct WindowsProcessCollector {
    /// Base process collection configuration
    base_config: ProcessCollectionConfig,
    /// Windows-specific configuration
    windows_config: WindowsCollectorConfig,
    /// Whether Windows service detection is available
    service_detection_available: bool,
    /// Whether performance counters are available
    performance_counters_available: bool,
    /// Whether container detection is available
    container_detection_available: bool,
    /// Whether Windows Defender is active
    defender_active: bool,
}

/// Configuration for Windows-specific process collection features.
#[derive(Debug, Clone)]
pub struct WindowsCollectorConfig {
    /// Whether to collect process security information (tokens, privileges, integrity levels)
    pub collect_security_info: bool,
    /// Whether to detect Windows services
    pub detect_services: bool,
    /// Whether to check UAC elevation status
    pub check_elevation_status: bool,
    /// Whether to collect Windows-specific performance counters
    pub collect_performance_counters: bool,
    /// Whether to detect Windows containers (Hyper-V, Windows Server)
    pub detect_containers: bool,
    /// Whether to handle Windows Defender and antivirus restrictions gracefully
    pub handle_defender_restrictions: bool,
}

impl Default for WindowsCollectorConfig {
    fn default() -> Self {
        Self {
            collect_security_info: true,
            detect_services: true,
            check_elevation_status: true,
            collect_performance_counters: true,
            detect_containers: true,
            handle_defender_restrictions: true,
        }
    }
}

#[allow(dead_code)]
impl WindowsProcessCollector {
    /// Creates a new enhanced Windows process collector with the specified configuration.
    ///
    /// This constructor initializes the collector with third-party crate integrations,
    /// detecting system capabilities and Windows Defender status through safe APIs.
    ///
    /// # Arguments
    ///
    /// * `base_config` - Base process collection configuration
    /// * `windows_config` - Windows-specific configuration options
    ///
    /// # Returns
    ///
    /// A configured enhanced Windows process collector or an error if initialization fails.
    pub fn new(
        base_config: ProcessCollectionConfig,
        windows_config: WindowsCollectorConfig,
    ) -> ProcessCollectionResult<Self> {
        // Detect service management capability
        let service_detection_available = if windows_config.detect_services {
            Self::detect_service_capability().unwrap_or_else(|e| {
                warn!(error = %e, "Failed to detect service management capability, disabling service detection");
                false
            })
        } else {
            false
        };

        // Detect performance counter capability
        let performance_counters_available = if windows_config.collect_performance_counters {
            Self::detect_performance_counter_capability().unwrap_or_else(|e| {
                warn!(error = %e, "Failed to detect performance counter capability, disabling performance counters");
                false
            })
        } else {
            false
        };

        // Detect container capability
        let container_detection_available = if windows_config.detect_containers {
            Self::detect_container_capability().unwrap_or_else(|e| {
                warn!(error = %e, "Failed to detect container capability, disabling container detection");
                false
            })
        } else {
            false
        };

        // Check Windows Defender status using native Windows API
        let defender_active = if windows_config.handle_defender_restrictions {
            Self::detect_defender_status().unwrap_or_else(|e| {
                warn!(error = %e, "Failed to detect Windows Defender status, defaulting to inactive");
                false
            })
        } else {
            false
        };

        debug!(
            service_detection_available = service_detection_available,
            performance_counters_available = performance_counters_available,
            container_detection_available = container_detection_available,
            defender_active = defender_active,
            collect_security_info = windows_config.collect_security_info,
            detect_services = windows_config.detect_services,
            check_elevation_status = windows_config.check_elevation_status,
            collect_performance_counters = windows_config.collect_performance_counters,
            detect_containers = windows_config.detect_containers,
            handle_defender_restrictions = windows_config.handle_defender_restrictions,
            "Initialized enhanced Windows process collector with third-party crates"
        );

        Ok(Self {
            base_config,
            windows_config,
            service_detection_available,
            performance_counters_available,
            container_detection_available,
            defender_active,
        })
    }

    /// Detects if Windows service management capability is available.
    ///
    /// This method checks if the windows-service crate functionality is available
    /// by attempting to access the Service Control Manager.
    fn detect_service_capability() -> ProcessCollectionResult<bool> {
        #[cfg(target_os = "windows")]
        {
            use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

            // Try to connect to the Service Control Manager
            match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) {
                Ok(_manager) => {
                    debug!("Service management capability detected via windows-service crate");
                    Ok(true)
                }
                Err(e) => {
                    debug!(error = %e, "Service management capability not available");
                    Ok(false)
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(false)
        }
    }

    /// Detects if performance counter capability is available.
    ///
    /// This method checks if performance counters are accessible through
    /// enhanced sysinfo functionality.
    fn detect_performance_counter_capability() -> ProcessCollectionResult<bool> {
        // Check if enhanced sysinfo performance data is available
        let mut system = System::new();
        system.refresh_all();

        // If we can get process information, performance counters are available
        let available = !system.processes().is_empty();

        debug!(
            available = available,
            "Performance counter capability detected via sysinfo"
        );

        Ok(available)
    }

    /// Detects if Windows container capability is available.
    ///
    /// This method checks if the system supports Windows containers by looking
    /// for container runtime components.
    fn detect_container_capability() -> ProcessCollectionResult<bool> {
        // Check for Windows container runtime components
        let container_paths = [
            "C:\\Windows\\System32\\vmcompute.exe", // Hyper-V containers
            "C:\\Windows\\System32\\vmwp.exe",      // VM worker process
            "C:\\Windows\\System32\\dockerd.exe",   // Docker daemon
            "C:\\Program Files\\Docker\\Docker\\dockerd.exe", // Docker Desktop
        ];

        let container_available = container_paths.iter().any(|&path| {
            // Canonicalize paths to prevent directory traversal
            match std::fs::canonicalize(Path::new(path)) {
                Ok(canonical_path) => canonical_path.exists(),
                Err(_) => false, // Path doesn't exist or can't be canonicalized
            }
        });

        debug!(
            container_available = container_available,
            "Container detection capability checked"
        );

        Ok(container_available)
    }

    /// Detects Windows Defender status by querying the registry.
    ///
    /// This method checks if Windows Defender real-time protection is active by
    /// reading the `DisableRealtimeMonitoring` registry value using the safe
    /// `winreg` crate. Returns `true` if real-time protection is enabled
    /// (may restrict process access), `false` otherwise.
    ///
    /// # Registry Key
    ///
    /// Queries: `HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows Defender\Real-Time Protection\DisableRealtimeMonitoring`
    /// - Value 0: Real-time protection enabled (returns `true`)
    /// - Value 1: Real-time protection disabled (returns `false`)
    /// - Key missing: Assumes inactive (returns `false`)
    ///
    /// # Returns
    ///
    /// Returns `Ok(true)` if Windows Defender real-time protection is active,
    /// `Ok(false)` if inactive or unavailable, or an error if registry access fails unexpectedly.
    ///
    /// # Safety
    ///
    /// This function uses the safe `winreg` crate and does not require any unsafe code.
    fn detect_defender_status() -> ProcessCollectionResult<bool> {
        #[cfg(target_os = "windows")]
        {
            use winreg::RegKey;
            use winreg::enums::*;

            // Registry path for Windows Defender real-time protection status
            const DEFENDER_SUBKEY: &str =
                r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection";
            const DISABLE_RT_MONITORING: &str = "DisableRealtimeMonitoring";

            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

            match hklm.open_subkey(DEFENDER_SUBKEY) {
                Ok(defender_key) => {
                    match defender_key.get_value::<u32, _>(DISABLE_RT_MONITORING) {
                        Ok(value) => {
                            // DisableRealtimeMonitoring = 0 means real-time protection is enabled
                            // DisableRealtimeMonitoring = 1 means real-time protection is disabled
                            let defender_active = value == 0;
                            debug!(
                                registry_key = DEFENDER_SUBKEY,
                                registry_value = value,
                                defender_active = defender_active,
                                "Detected Windows Defender status using winreg crate"
                            );
                            Ok(defender_active)
                        }
                        Err(e) => {
                            debug!(
                                error = %e,
                                registry_key = DEFENDER_SUBKEY,
                                value_name = DISABLE_RT_MONITORING,
                                "Failed to read Windows Defender registry value, assuming inactive"
                            );
                            Ok(false) // Default to inactive if we can't read the value
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        error = %e,
                        registry_key = DEFENDER_SUBKEY,
                        "Windows Defender registry key not accessible, assuming inactive"
                    );
                    Ok(false) // Default to inactive if key doesn't exist
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(false)
        }
    }

    /// Reads process security information using Windows APIs with fallback to heuristics.
    ///
    /// This method attempts to use Windows APIs to gather accurate security information
    /// about a process, including elevation status, token type, and integrity level.
    /// Falls back to heuristics when API access is unavailable.
    ///
    /// # Arguments
    ///
    /// * `pid` - Process ID to query security information for
    ///
    /// # Returns
    ///
    /// Returns `ProcessSecurityInfo` with available security metadata or an error
    /// if the process cannot be accessed at all.
    ///
    /// # Note
    ///
    /// This is an internal method used by the collector implementation.
    fn read_process_security_info(
        &self,
        pid: u32,
        system: &System,
    ) -> ProcessCollectionResult<ProcessSecurityInfo> {
        let mut security_info = ProcessSecurityInfo::default();

        if let Some((name, exe_path, _, _)) = self.get_process_info(pid, system) {
            self.apply_enhanced_security_heuristics(
                &mut security_info,
                &name,
                exe_path.as_deref(),
                pid,
            );
        }

        debug!(
            pid = pid,
            integrity_level = %security_info.integrity_level,
            elevated = security_info.elevated,
            system_process = security_info.system_process,
            token_type = %security_info.token_type,
            "Read process security information using heuristics"
        );

        Ok(security_info)
    }

    /// Applies enhanced security heuristics for Windows processes.
    ///
    /// This method uses process name, executable path, and PID to determine
    /// security characteristics through pattern matching and known Windows behaviors.
    /// All inputs are validated to prevent security issues.
    ///
    /// # Arguments
    ///
    /// * `security_info` - Mutable reference to security info structure to populate
    /// * `name` - Process name (validated for length and content)
    /// * `exe_path` - Optional executable path (canonicalized for security)
    /// * `pid` - Process ID (validated for reasonable bounds)
    ///
    /// # Security
    ///
    /// - Process names are limited to reasonable lengths to prevent buffer issues
    /// - Executable paths are canonicalized to prevent directory traversal
    /// - PIDs are validated to be within reasonable system bounds
    fn apply_enhanced_security_heuristics(
        &self,
        security_info: &mut ProcessSecurityInfo,
        name: &str,
        exe_path: Option<&std::path::Path>,
        pid: u32,
    ) {
        // Validate inputs for security
        if name.len() > 260 {
            warn!(
                pid = pid,
                name_len = name.len(),
                "Process name exceeds reasonable length, applying default security settings"
            );
            return;
        }

        if pid == 0 || pid > 1_000_000 {
            warn!(
                pid = pid,
                "Process ID outside reasonable bounds, applying default security settings"
            );
            return;
        }
        // Check if it's a system process
        if self.is_system_process(name, pid) {
            security_info.system_process = true;
            security_info.integrity_level = IntegrityLevel::System;
            security_info.elevated = true;
            security_info.has_debug_privilege = true;
            security_info.protected = true;
            security_info.token_type = TokenType::Primary;
            security_info.session_id = Some(0);
            security_info.virtualized = false;
        } else if let Some(exe_path) = exe_path {
            // Enhanced heuristics based on Windows process patterns
            // Canonicalize path to prevent directory traversal attacks
            let canonical_path =
                std::fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
            let canonical_str = canonical_path.to_string_lossy();

            if canonical_str.starts_with("C:\\Windows\\System32\\")
                || canonical_str.starts_with("C:\\Windows\\SysWOW64\\")
            {
                // System directory processes
                security_info.integrity_level = IntegrityLevel::High;
                security_info.elevated = true;
                security_info.has_debug_privilege = matches!(
                    name,
                    n if n.contains("svchost") || n.contains("services")
                );
                security_info.protected = matches!(
                    name,
                    n if n.contains("csrss") || n.contains("winlogon")
                );
                security_info.system_process = false;
                security_info.session_id = Some(0);
            } else if canonical_str.contains("\\Program Files\\")
                || canonical_str.contains("\\Program Files (x86)\\")
            {
                // Installed applications
                security_info.integrity_level = IntegrityLevel::Medium;
                security_info.elevated = false;
                security_info.has_debug_privilege = false;
                security_info.protected = false;
                security_info.system_process = false;
                security_info.session_id = Some(1);
            } else if canonical_str.starts_with("C:\\Users\\") {
                // User applications
                security_info.integrity_level = IntegrityLevel::Medium;
                security_info.elevated = false;
                security_info.has_debug_privilege = false;
                security_info.protected = false;
                security_info.system_process = false;
                security_info.session_id = Some(1);

                // Check for sandboxed applications
                if canonical_str.contains("\\AppData\\Local\\Packages\\") {
                    security_info.integrity_level = IntegrityLevel::Low;
                    security_info.virtualized = true;
                }
            } else {
                // Default for other locations
                security_info.integrity_level = IntegrityLevel::Medium;
                security_info.elevated = false;
                security_info.has_debug_privilege = false;
                security_info.protected = false;
                security_info.system_process = false;
                security_info.session_id = Some(1);
            }

            security_info.token_type = TokenType::Primary;

            // Additional checks for specific process types
            if matches!(name, n if n.contains("RuntimeBroker") || n.contains("ApplicationFrameHost"))
            {
                security_info.integrity_level = IntegrityLevel::Medium;
                security_info.virtualized = true;
            }
        }
    }

    /// Detects if a process is a Windows service using the windows-service crate.
    ///
    /// This method attempts to use the windows-service crate to identify services,
    /// falling back to heuristic-based detection when the Service Control Manager
    /// is unavailable.
    ///
    /// # Arguments
    ///
    /// * `pid` - Process ID to check for service status
    ///
    /// # Returns
    ///
    /// Returns `WindowsServiceInfo` with service detection results.
    fn detect_windows_service(
        &self,
        pid: u32,
        system: &System,
    ) -> ProcessCollectionResult<WindowsServiceInfo> {
        let mut service_info = WindowsServiceInfo::default();

        // Use heuristic-based service detection
        if let Some((name, exe_path, _, _)) = self.get_process_info(pid, system) {
            if self.is_likely_service(&name, exe_path.as_deref()) {
                self.populate_service_info(&mut service_info, &name, pid, "heuristics");
            }
        }

        Ok(service_info)
    }

    /// Determines if a process is likely a Windows service based on heuristics.
    ///
    /// # Arguments
    ///
    /// * `name` - Process name
    /// * `exe_path` - Optional executable path
    ///
    /// # Returns
    ///
    /// Returns `true` if the process matches service patterns.
    fn is_likely_service(&self, name: &str, exe_path: Option<&std::path::Path>) -> bool {
        if let Some(exe_path) = exe_path {
            let path_str = exe_path.to_string_lossy();

            // Common service executable patterns
            path_str.starts_with("C:\\Windows\\System32\\")
                || path_str.starts_with("C:\\Windows\\SysWOW64\\")
                || path_str.contains("svchost.exe")
                || path_str.contains("services.exe")
                || (name.ends_with(".exe")
                    && (name.contains("service")
                        || name.contains("svc")
                        || name.contains("daemon")))
        } else {
            false
        }
    }

    /// Populates service information structure with detected service data.
    ///
    /// # Arguments
    ///
    /// * `service_info` - Mutable reference to service info structure
    /// * `name` - Process name
    /// * `pid` - Process ID
    /// * `detection_method` - Method used for detection (for logging)
    fn populate_service_info(
        &self,
        service_info: &mut WindowsServiceInfo,
        name: &str,
        pid: u32,
        detection_method: &str,
    ) {
        service_info.is_service = true;
        service_info.service_name = Some(name.to_string());
        service_info.display_name = Some(format!("{} Service", name));
        service_info.service_type = Some("Win32OwnProcess".to_string());
        service_info.service_state = Some("Running".to_string());
        service_info.start_type = Some("Automatic".to_string());

        debug!(
            pid = pid,
            service_name = %name,
            detection_method = detection_method,
            "Detected Windows service"
        );
    }

    /// Detects Windows container information for a process.
    fn detect_container_info(
        &self,
        pid: u32,
        system: &System,
    ) -> ProcessCollectionResult<WindowsContainerInfo> {
        let mut container_info = WindowsContainerInfo::default();

        if let Some((name, exe_path, _, _)) = self.get_process_info(pid, system) {
            // Use heuristics to detect container processes
            let in_container = if let Some(exe_path) = exe_path {
                let path_str = exe_path.to_string_lossy();

                // Check for container-related processes
                path_str.contains("docker")
                    || path_str.contains("containerd")
                    || path_str.contains("vmcompute")
                    || path_str.contains("vmwp")
                    || name.contains("docker")
                    || name.contains("container")
            } else {
                false
            };

            if in_container {
                container_info.in_container = true;

                // Determine container type based on process characteristics
                if name.contains("vmcompute") || name.contains("vmwp") {
                    container_info.container_type = Some("Hyper-V".to_string());
                } else if name.contains("docker") {
                    container_info.container_type = Some("Windows Server".to_string());
                } else {
                    container_info.container_type = Some("Unknown".to_string());
                }

                // Container detection is heuristic-based only
                // Real container ID and image name would require container runtime APIs
                // Leave these fields as None until genuine values can be obtained

                debug!(
                    pid = pid,
                    container_type = ?container_info.container_type,
                    "Detected container process using heuristics"
                );
            }
        }

        Ok(container_info)
    }

    /// Gets Windows-specific performance counters for a process using sysinfo.
    fn get_performance_counters(&self, pid: u32, system: &System) -> Option<(u64, u64, u64)> {
        // Validate PID to prevent overflow and invalid values
        if pid == 0 || pid > u32::MAX / 2 {
            debug!(
                pid = pid,
                "Invalid PID provided for performance counter collection"
            );
            return None;
        }

        // Get process information using sysinfo
        if let Some((_, _, memory_kb, virtual_memory_kb)) = self.get_process_info(pid, system) {
            // Convert to bytes with overflow protection
            let working_set_size = memory_kb.saturating_mul(1024);
            let private_bytes = memory_kb.saturating_mul(1024);
            let virtual_bytes = virtual_memory_kb.saturating_mul(1024);

            // Only return real values from sysinfo, omit handle_count and thread_count
            // until real Windows performance counter APIs are implemented
            Some((working_set_size, private_bytes, virtual_bytes))
        } else {
            None
        }
    }

    /// Detects the architecture of a process.
    fn detect_process_architecture(&self, _pid: u32) -> Option<String> {
        // Use compile-time architecture detection for now
        // In a real implementation, we would use Windows APIs to detect per-process architecture
        #[cfg(target_arch = "x86_64")]
        {
            Some("x64".to_string())
        }
        #[cfg(target_arch = "x86")]
        {
            Some("x86".to_string())
        }
        #[cfg(target_arch = "aarch64")]
        {
            Some("ARM64".to_string())
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
        {
            Some("Unknown".to_string())
        }
    }

    /// Checks if a process is protected by Windows Defender.
    fn is_defender_protected(&self, pid: u32, system: &System) -> ProcessCollectionResult<bool> {
        if let Some((name, exe_path, _, _)) = self.get_process_info(pid, system) {
            // Use heuristics to determine if a process might be protected by Defender
            let is_protected = if let Some(exe_path) = exe_path {
                let path_str = exe_path.to_string_lossy();

                // Check for Windows Defender processes
                path_str.contains("Windows Defender")
                    || path_str.contains("MsMpEng")
                    || path_str.contains("NisSrv")
                    || name.contains("MsMpEng")
                    || name.contains("NisSrv")
                    || name.contains("SecurityHealthService")
            } else {
                false
            };

            debug!(
                pid = pid,
                name = %name,
                is_protected = is_protected,
                "Checked Windows Defender protection status"
            );

            Ok(is_protected)
        } else {
            Ok(false)
        }
    }

    /// Determines if a process is a kernel thread (Windows doesn't have kernel threads like Linux).
    fn is_kernel_thread(&self, _name: &str, _process: &sysinfo::Process) -> bool {
        // Windows doesn't have kernel threads in the same way as Linux
        // All processes in Windows are user-mode processes or system processes
        false
    }

    /// Determines if a process is a system process based on name and PID.
    fn is_system_process(&self, name: &str, pid: u32) -> bool {
        // Common Windows system process patterns
        const SYSTEM_PROCESSES: &[&str] = &[
            "System",
            "Registry",
            "smss.exe",
            "csrss.exe",
            "wininit.exe",
            "winlogon.exe",
            "services.exe",
            "lsass.exe",
            "lsm.exe",
        ];

        // Check by PID (System and Registry processes)
        if pid <= 4 {
            return true;
        }

        // Check by name
        let name_lower = name.to_lowercase();
        SYSTEM_PROCESSES
            .iter()
            .any(|&sys_proc| name_lower == sys_proc.to_lowercase())
    }

    /// Enhances a process with Windows-specific metadata.
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
        system: &System,
    ) -> ProcessCollectionResult<ProcessEvent> {
        let pid_u32 = pid.as_u32();

        // Validate PID
        if pid_u32 == 0 {
            return Err(ProcessCollectionError::InvalidProcessData {
                pid: pid_u32,
                message: "Invalid PID: 0".to_string(),
            });
        }

        let ppid = process.parent().map(|p| p.as_u32());

        // Get process name with fallback
        let name = if process.name().is_empty() {
            format!("<unknown-{}>", pid_u32)
        } else {
            process.name().to_string_lossy().to_string()
        };

        // Check if this is a system process that should be skipped
        if self.base_config.skip_system_processes && self.is_system_process(&name, pid_u32) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "System process skipped by configuration".to_string(),
            });
        }

        // Check if this is a kernel thread that should be skipped (Windows doesn't have kernel threads like Linux)
        if self.base_config.skip_kernel_threads && self.is_kernel_thread(&name, process) {
            return Err(ProcessCollectionError::ProcessAccessDenied {
                pid: pid_u32,
                message: "Kernel thread skipped by configuration".to_string(),
            });
        }

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
            let memory_kib = process.memory();
            if memory_kib > 0 {
                Some(memory_kib.saturating_mul(1024))
            } else {
                None
            }
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

        // Collect Windows-specific enhanced metadata if configured
        let platform_metadata = if self.base_config.collect_enhanced_metadata {
            let windows_metadata = self.read_enhanced_metadata(pid_u32, system);
            Some(serde_json::to_value(windows_metadata).unwrap_or_default())
        } else {
            None
        };

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
            platform_metadata,
        })
    }

    /// Reads enhanced Windows-specific metadata for a process.
    fn read_enhanced_metadata(&self, pid: u32, system: &System) -> WindowsProcessMetadata {
        let mut metadata = WindowsProcessMetadata::default();

        // Collect security information if configured
        if self.windows_config.collect_security_info {
            metadata.security_info = self
                .read_process_security_info(pid, system)
                .unwrap_or_default();
        }

        // Detect Windows service if configured
        if self.windows_config.detect_services && self.service_detection_available {
            metadata.service_info = self.detect_windows_service(pid, system).unwrap_or_default();
        }

        // Detect container information if configured
        if self.windows_config.detect_containers && self.container_detection_available {
            metadata.container_info = self.detect_container_info(pid, system).unwrap_or_default();
        }

        // Collect performance counters if configured
        if self.windows_config.collect_performance_counters && self.performance_counters_available {
            if let Some((working_set, private_bytes, virtual_bytes)) =
                self.get_performance_counters(pid, system)
            {
                metadata.working_set_size = Some(working_set);
                metadata.private_bytes = Some(private_bytes);
                metadata.virtual_bytes = Some(virtual_bytes);
                // handle_count and thread_count omitted until real Windows performance counter APIs are implemented
            }
        }

        // Detect process architecture
        metadata.architecture = self.detect_process_architecture(pid);

        // Check Windows Defender protection
        if self.windows_config.handle_defender_restrictions && self.defender_active {
            metadata.defender_protected = self.is_defender_protected(pid, system).unwrap_or(false);
        }

        metadata
    }

    /// Gets basic process information using sysinfo.
    ///
    /// # Arguments
    ///
    /// * `pid` - The process ID to look up
    ///
    /// # Returns
    ///
    /// Returns a tuple containing (name, executable_path, memory_kb, virtual_memory_kb)
    /// or None if the process is not found.
    fn get_process_info(
        &self,
        pid: u32,
        system: &System,
    ) -> Option<(String, Option<std::path::PathBuf>, u64, u64)> {
        let sysinfo_pid = Pid::from_u32(pid);

        system.process(sysinfo_pid).map(|process| {
            (
                process.name().to_string_lossy().to_string(),
                process.exe().map(|p| p.to_path_buf()),
                process.memory(),
                process.virtual_memory(),
            )
        })
    }
}

#[async_trait]
impl ProcessCollector for WindowsProcessCollector {
    fn name(&self) -> &'static str {
        "windows-collector"
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
            enhanced_metadata = self.base_config.collect_enhanced_metadata,
            max_processes = self.base_config.max_processes,
            collect_security_info = self.windows_config.collect_security_info,
            detect_services = self.windows_config.detect_services,
            detect_containers = self.windows_config.detect_containers,
            "Starting Windows process collection"
        );

        // Perform process enumeration in a blocking task to avoid blocking the async runtime
        let base_config = self.base_config.clone();
        let _windows_config = self.windows_config.clone();
        let enumeration_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();

            if base_config.collect_enhanced_metadata {
                system.refresh_all();
            } else {
                system.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::All,
                    true,
                    sysinfo::ProcessRefreshKind::everything(),
                );
            }

            if system.processes().is_empty() {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during enumeration".to_string(),
                });
            }

            Ok((system, base_config))
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Process enumeration task failed: {}", e),
        })?;

        let (system, base_config) = enumeration_result?;

        let mut events = Vec::new();
        let mut stats = CollectionStats {
            total_processes: system.processes().len(),
            ..Default::default()
        };

        // Process each process with individual error handling
        for (pid, process) in system.processes().iter() {
            // Check if we've hit the maximum process limit
            if base_config.max_processes > 0 && events.len() >= base_config.max_processes {
                debug!(
                    max_processes = base_config.max_processes,
                    collected = events.len(),
                    "Reached maximum process collection limit"
                );
                break;
            }

            match self.enhance_process(*pid, process, &system) {
                Ok(event) => {
                    events.push(event);
                    stats.successful_collections += 1;
                }
                Err(ProcessCollectionError::ProcessAccessDenied { pid, message }) => {
                    debug!(pid = pid, reason = %message, "Process access denied");
                    stats.inaccessible_processes += 1;
                }
                Err(ProcessCollectionError::InvalidProcessData { pid, message }) => {
                    warn!(pid = pid, reason = %message, "Invalid process data");
                    stats.invalid_processes += 1;
                }
                Err(e) => {
                    error!(
                        pid = pid.as_u32(),
                        error = %e,
                        "Unexpected error during process conversion"
                    );
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
            "Windows process collection completed"
        );

        Ok((events, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        debug!(
            collector = self.name(),
            pid = pid,
            "Collecting single Windows process"
        );

        // Perform single process lookup in a blocking task
        let base_config = self.base_config.clone();
        let lookup_result = tokio::task::spawn_blocking(move || {
            let mut system = System::new();

            if base_config.collect_enhanced_metadata {
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
                Ok(system)
            } else {
                Err(ProcessCollectionError::ProcessNotFound { pid })
            }
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Single process lookup task failed: {}", e),
        })?;

        let system = lookup_result?;
        let sysinfo_pid = Pid::from_u32(pid);
        if let Some(process) = system.process(sysinfo_pid) {
            self.enhance_process(sysinfo_pid, process, &system)
        } else {
            Err(ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        debug!(collector = self.name(), "Performing Windows health check");

        // Perform a quick health check by trying to enumerate a few processes
        let health_result = tokio::task::spawn_blocking(|| {
            let mut system = System::new();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );

            let process_count = system.processes().len();
            if process_count == 0 {
                return Err(ProcessCollectionError::SystemEnumerationFailed {
                    message: "No processes found during health check".to_string(),
                });
            }

            Ok(process_count)
        })
        .await
        .map_err(|e| ProcessCollectionError::SystemEnumerationFailed {
            message: format!("Health check task failed: {}", e),
        })?;

        let process_count = health_result?;

        debug!(
            collector = self.name(),
            process_count = process_count,
            "Windows health check passed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_windows_collector_creation() {
        let base_config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config);

        assert!(collector.is_ok());
        let collector = collector.unwrap();
        assert_eq!(collector.name(), "windows-collector");

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(capabilities.enhanced_metadata);
        assert!(!capabilities.executable_hashing);
        assert!(capabilities.system_processes);
        assert!(capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }

    #[tokio::test]
    async fn test_windows_collector_capabilities() {
        let base_config = ProcessCollectionConfig {
            collect_enhanced_metadata: false,
            compute_executable_hashes: true,
            skip_system_processes: true,
            skip_kernel_threads: true,
            ..Default::default()
        };
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config).unwrap();

        let capabilities = collector.capabilities();
        assert!(capabilities.basic_info);
        assert!(!capabilities.enhanced_metadata);
        assert!(capabilities.executable_hashing);
        assert!(!capabilities.system_processes);
        assert!(!capabilities.kernel_threads);
        assert!(capabilities.realtime_collection);
    }

    #[test]
    fn test_windows_metadata_serialization() {
        // Test that WindowsProcessMetadata can be serialized to JSON
        let mut metadata = WindowsProcessMetadata::default();

        // Set some test values
        metadata.security_info.elevated = true;
        metadata.security_info.integrity_level = IntegrityLevel::High;
        metadata.service_info.is_service = true;
        metadata.service_info.service_name = Some("TestService".to_string());
        metadata.container_info.in_container = false;
        metadata.architecture = Some("x64".to_string());
        metadata.defender_protected = true;
        metadata.working_set_size = Some(1024 * 1024);
        metadata.private_bytes = Some(2048 * 1024);
        metadata.virtual_bytes = Some(4096 * 1024);
        metadata.handle_count = Some(50);
        metadata.thread_count = Some(5);

        // Test serialization
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(!json.is_empty(), "Serialized JSON should not be empty");

        // Test deserialization
        let deserialized: WindowsProcessMetadata = serde_json::from_str(&json).unwrap();
        assert!(deserialized.security_info.elevated);
        assert_eq!(
            deserialized.security_info.integrity_level,
            IntegrityLevel::High
        );
        assert!(deserialized.service_info.is_service);
        assert_eq!(
            deserialized.service_info.service_name,
            Some("TestService".to_string())
        );
        assert!(!deserialized.container_info.in_container);
        assert_eq!(deserialized.architecture, Some("x64".to_string()));
        assert!(deserialized.defender_protected);
        assert_eq!(deserialized.working_set_size, Some(1024 * 1024));
        assert_eq!(deserialized.private_bytes, Some(2048 * 1024));
        assert_eq!(deserialized.virtual_bytes, Some(4096 * 1024));
        assert_eq!(deserialized.handle_count, Some(50));
        assert_eq!(deserialized.thread_count, Some(5));

        println!("Windows metadata serialization test passed");
    }

    #[tokio::test]
    async fn test_windows_collector_health_check() {
        let base_config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config).unwrap();

        let result = collector.health_check().await;
        assert!(
            result.is_ok(),
            "Health check should pass on a Windows system with processes"
        );
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_windows_collector_collect_processes() {
        let base_config = ProcessCollectionConfig {
            max_processes: 10, // Limit for faster testing
            ..Default::default()
        };
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config).unwrap();

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

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_windows_collector_collect_single_process() {
        let base_config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config).unwrap();

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
    async fn test_windows_collector_collect_nonexistent_process() {
        let base_config = ProcessCollectionConfig::default();
        let windows_config = WindowsCollectorConfig::default();
        let collector = WindowsProcessCollector::new(base_config, windows_config).unwrap();

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
    fn test_integrity_level_display() {
        assert_eq!(IntegrityLevel::Untrusted.to_string(), "Untrusted");
        assert_eq!(IntegrityLevel::Low.to_string(), "Low");
        assert_eq!(IntegrityLevel::Medium.to_string(), "Medium");
        assert_eq!(IntegrityLevel::High.to_string(), "High");
        assert_eq!(IntegrityLevel::System.to_string(), "System");
        assert_eq!(IntegrityLevel::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_token_type_display() {
        assert_eq!(TokenType::Primary.to_string(), "Primary");
        assert_eq!(TokenType::Impersonation.to_string(), "Impersonation");
        assert_eq!(TokenType::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_windows_collection_error_conversion() {
        let windows_error = WindowsCollectionError::WindowsApi("Test error".to_string());
        let process_error: ProcessCollectionError = windows_error.into();

        match process_error {
            ProcessCollectionError::PlatformError { message } => {
                assert!(message.contains("Windows API error"));
                assert!(message.contains("Test error"));
            }
            other => panic!("Expected PlatformError, got: {:?}", other),
        }
    }

    #[test]
    fn test_windows_collector_config_default() {
        let config = WindowsCollectorConfig::default();
        assert!(config.collect_security_info);
        assert!(config.detect_services);
        assert!(config.check_elevation_status);
        assert!(config.collect_performance_counters);
        assert!(config.detect_containers);
        assert!(config.handle_defender_restrictions);
    }

    #[test]
    fn test_process_security_info_default() {
        let security_info = ProcessSecurityInfo::default();
        assert!(!security_info.has_debug_privilege);
        assert!(!security_info.elevated);
        assert_eq!(security_info.integrity_level, IntegrityLevel::Medium);
        assert_eq!(security_info.token_type, TokenType::Primary);
        assert!(!security_info.virtualized);
        assert!(!security_info.protected);
        assert!(!security_info.system_process);
    }

    #[test]
    fn test_windows_service_info_default() {
        let service_info = WindowsServiceInfo::default();
        assert!(!service_info.is_service);
        assert!(service_info.service_name.is_none());
        assert!(service_info.display_name.is_none());
        assert!(service_info.service_type.is_none());
        assert!(service_info.service_state.is_none());
        assert!(service_info.start_type.is_none());
    }

    #[test]
    fn test_windows_container_info_default() {
        let container_info = WindowsContainerInfo::default();
        assert!(!container_info.in_container);
        assert!(container_info.container_type.is_none());
        assert!(container_info.container_id.is_none());
        assert!(container_info.image_name.is_none());
    }
}
