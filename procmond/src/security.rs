//! Security context and data sanitization for procmond.
//!
//! This module provides privilege detection and sensitive data sanitization
//! to ensure procmond operates with least privilege and does not leak secrets
//! in logs or published events.
//!
//! # Privilege Detection
//!
//! [`detect_privileges`] inspects the current process's capabilities on each
//! platform and returns a [`SecurityContext`] describing what procmond can access.
//! This function is infallible — on failure it returns a degraded context.
//!
//! # Data Sanitization
//!
//! [`Sanitized`] wraps a string reference for display, replacing values after
//! known sensitive flags (`--password`, `--secret`, etc.) with `[REDACTED]`.
//!
//! [`sanitize_command_line`] applies the same logic, returning an owned `String`.
//!
//! [`sanitize_env_vars`] redacts environment variable values whose keys match
//! common secret patterns (`PASSWORD`, `SECRET`, `TOKEN`, etc.).
//!
//! [`sanitize_file_path`] redacts file paths that reference well-known sensitive
//! directories (`.ssh`, `.aws`, `.gnupg`, etc.).

use std::collections::HashMap;
use std::fmt;
use tracing::{debug, info, warn};

/// Sensitive argument flags whose next value should be redacted.
const SENSITIVE_FLAGS: &[&str] = &[
    "--password",
    "--secret",
    "--token",
    "--api-key",
    "--api_key",
    "-p",
    "-s",
    "-t",
    "-k",
];

/// Environment variable key patterns (case-insensitive) that indicate secrets.
const SENSITIVE_ENV_KEYS: &[&str] = &["PASSWORD", "SECRET", "TOKEN", "KEY", "API_KEY", "AUTH"];

/// Sensitive directory segments in file paths (case-insensitive on Windows).
const SENSITIVE_PATH_SEGMENTS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube/config",
    ".docker/config",
    ".npmrc",
    ".netrc",
    ".env",
];

/// The redaction placeholder.
const REDACTED: &str = "[REDACTED]";

/// Detected platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Platform {
    /// Linux (kernel 4.x+)
    Linux,
    /// macOS (14.0+)
    MacOs,
    /// Windows (10+)
    Windows,
    /// FreeBSD (13.0+, best-effort support)
    FreeBsd,
    /// Unknown / unsupported
    Unknown,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Linux => write!(f, "linux"),
            Self::MacOs => write!(f, "macos"),
            Self::Windows => write!(f, "windows"),
            Self::FreeBsd => write!(f, "freebsd"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Security context describing the current process's capabilities.
#[derive(Debug, Clone)]
pub struct SecurityContext {
    /// Detected platform.
    pub platform: Platform,
    /// Whether the process has full access to other processes' metadata.
    pub has_full_process_access: bool,
    /// List of detected capabilities (platform-specific).
    pub capabilities: Vec<String>,
    /// Whether privilege detection itself failed (degraded mode).
    pub degraded_mode: bool,
}

impl SecurityContext {
    /// Returns a degraded context for the given platform.
    const fn degraded(platform: Platform) -> Self {
        Self {
            platform,
            has_full_process_access: false,
            capabilities: vec![],
            degraded_mode: true,
        }
    }
}

/// Detect the current process's security privileges.
///
/// This function is **infallible**: on any detection failure, it returns a
/// degraded [`SecurityContext`] with `degraded_mode = true` and logs a warning.
///
/// # Platform Behavior
///
/// - **Linux**: Parses `CapEff` from `/proc/self/status` and checks for
///   `CAP_SYS_PTRACE` (bit 19) and `CAP_DAC_READ_SEARCH` (bit 2).
/// - **macOS**: Checks `getuid() == 0` for root access.
/// - **Windows**: Checks for elevated (Administrator) privileges via `Win32_Security`.
/// - **FreeBSD**: Checks `getuid() == 0` for root access (best-effort).
pub fn detect_privileges() -> SecurityContext {
    let platform = detect_platform();

    let ctx = match platform {
        Platform::Linux => detect_linux_privileges(),
        Platform::MacOs => detect_macos_privileges(),
        Platform::Windows => detect_windows_privileges(),
        Platform::FreeBsd => detect_freebsd_privileges(),
        Platform::Unknown => SecurityContext::degraded(platform),
    };

    if ctx.degraded_mode {
        warn!(
            platform = %ctx.platform,
            "Running in degraded security mode - privilege detection failed"
        );
    } else {
        info!(
            platform = %ctx.platform,
            capabilities = ?ctx.capabilities,
            full_process_access = ctx.has_full_process_access,
            "Security context detected"
        );
    }

    ctx
}

/// Detect the current platform.
const fn detect_platform() -> Platform {
    if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "freebsd") {
        Platform::FreeBsd
    } else {
        Platform::Unknown
    }
}

/// Linux-specific privilege detection via `/proc/self/status`.
#[cfg(target_os = "linux")]
fn detect_linux_privileges() -> SecurityContext {
    use std::fs;

    let platform = Platform::Linux;

    let status = match fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Failed to read /proc/self/status");
            return SecurityContext::degraded(platform);
        }
    };

    let cap_eff = status
        .lines()
        .find(|line| line.starts_with("CapEff:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok());

    let Some(bitmask) = cap_eff else {
        warn!("Failed to parse CapEff from /proc/self/status");
        return SecurityContext::degraded(platform);
    };

    let mut capabilities = vec![];
    let mut has_full_process_access = false;

    // CAP_DAC_READ_SEARCH = bit 2
    if bitmask & (1 << 2) != 0 {
        capabilities.push("CAP_DAC_READ_SEARCH".to_owned());
    }

    // CAP_SYS_PTRACE = bit 19
    if bitmask & (1 << 19) != 0 {
        capabilities.push("CAP_SYS_PTRACE".to_owned());
        has_full_process_access = true;
    }

    SecurityContext {
        platform,
        has_full_process_access,
        capabilities,
        degraded_mode: false,
    }
}

/// macOS-specific privilege detection.
///
/// Checks for root access via `getuid() == 0`. On macOS, root access implies
/// full process inspection capabilities including `task_for_pid()`.
///
/// Note: Detecting `task_for_pid()` entitlements directly requires the
/// `SecTaskCopyValueForEntitlement` API which involves unsafe FFI. Since the
/// workspace forbids `unsafe_code`, we check for root access as a proxy —
/// root always has `task_for_pid()` capability.
#[cfg(target_os = "macos")]
fn detect_macos_privileges() -> SecurityContext {
    let platform = Platform::MacOs;
    let mut capabilities = vec![];

    let is_root = uzers::get_current_uid() == 0;

    if is_root {
        capabilities.push("root".to_owned());
        capabilities.push("task_for_pid".to_owned());
    }

    SecurityContext {
        platform,
        has_full_process_access: is_root,
        capabilities,
        degraded_mode: false,
    }
}

/// Windows-specific privilege detection via `Win32_Security`.
///
/// Checks whether the current process token has the `SeDebugPrivilege`
/// enabled, which grants full access to other processes.
#[cfg(target_os = "windows")]
fn detect_windows_privileges() -> SecurityContext {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Security::{
        GetTokenInformation, LookupPrivilegeValueW, OpenProcessToken, PRIVILEGE_SET,
        PrivilegeCheck, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let platform = Platform::Windows;
    let mut capabilities = vec![];
    let mut has_full_process_access = false;

    // Open the current process token
    let mut token_handle = HANDLE::default();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) };

    if opened.is_err() {
        warn!("Failed to open process token for privilege detection");
        return SecurityContext::degraded(platform);
    }

    // Look up SeDebugPrivilege LUID
    let privilege_name = windows::core::w!("SeDebugPrivilege");
    let mut luid = LUID::default();
    let lookup = unsafe { LookupPrivilegeValueW(None, privilege_name, &mut luid) };

    if lookup.is_err() {
        warn!("Failed to look up SeDebugPrivilege LUID");
        let _ = unsafe { CloseHandle(token_handle) };
        return SecurityContext::degraded(platform);
    }

    // Check if the privilege is enabled
    let mut privilege_set = PRIVILEGE_SET {
        PrivilegeCount: 1,
        Control: 1, // PRIVILEGE_SET_ALL_NECESSARY
        Privilege: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: windows::Win32::Security::SE_PRIVILEGE_ENABLED,
        }],
    };

    let mut result = windows::Win32::Foundation::BOOL(0);
    let check = unsafe { PrivilegeCheck(token_handle, &mut privilege_set, &mut result) };

    let _ = unsafe { CloseHandle(token_handle) };

    if check.is_err() {
        warn!("Failed to check SeDebugPrivilege");
        return SecurityContext::degraded(platform);
    }

    if result.as_bool() {
        capabilities.push("SeDebugPrivilege".to_owned());
        has_full_process_access = true;
    }

    // Also check if running as Administrator
    let is_admin = is_windows_elevated();
    if is_admin {
        capabilities.push("Administrator".to_owned());
    }

    SecurityContext {
        platform,
        has_full_process_access,
        capabilities,
        degraded_mode: false,
    }
}

/// Check if the current Windows process is running elevated (as Administrator).
#[cfg(target_os = "windows")]
fn is_windows_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, OpenProcessToken, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut token_handle = HANDLE::default();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) };

    if opened.is_err() {
        return false;
    }

    let mut elevation = TOKEN_ELEVATION::default();
    let mut return_length = 0_u32;
    let info = unsafe {
        GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(std::ptr::addr_of_mut!(elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        )
    };

    let _ = unsafe { windows::Win32::Foundation::CloseHandle(token_handle) };

    info.is_ok() && elevation.TokenIsElevated != 0
}

/// FreeBSD-specific privilege detection.
///
/// FreeBSD is a "best-effort" platform. We check for root access, which
/// implies full process inspection via `sysctl kern.proc`.
#[cfg(target_os = "freebsd")]
fn detect_freebsd_privileges() -> SecurityContext {
    let platform = Platform::FreeBsd;
    let mut capabilities = vec![];

    let is_root = uzers::get_current_uid() == 0;
    if is_root {
        capabilities.push("root".to_owned());
    }

    info!(
        platform = %platform,
        "FreeBSD detected - using FallbackProcessCollector (basic metadata only)"
    );

    SecurityContext {
        platform,
        has_full_process_access: is_root,
        capabilities,
        degraded_mode: false,
    }
}

/// Fallback stubs for platforms where the native implementation is unavailable.
#[cfg(not(target_os = "linux"))]
const fn detect_linux_privileges() -> SecurityContext {
    SecurityContext::degraded(Platform::Linux)
}

#[cfg(not(target_os = "macos"))]
const fn detect_macos_privileges() -> SecurityContext {
    SecurityContext::degraded(Platform::MacOs)
}

#[cfg(not(target_os = "windows"))]
const fn detect_windows_privileges() -> SecurityContext {
    SecurityContext::degraded(Platform::Windows)
}

#[cfg(not(target_os = "freebsd"))]
const fn detect_freebsd_privileges() -> SecurityContext {
    SecurityContext::degraded(Platform::FreeBsd)
}

/// Display wrapper that redacts sensitive argument values.
///
/// When formatted, scans the wrapped string for known sensitive flags
/// (`--password`, `--secret`, `--token`, `--api-key`, `-p`, `-s`, `-t`, `-k`)
/// and replaces the value immediately following each flag with `[REDACTED]`.
///
/// This wrapper is intended for use at `tracing` macro call sites only —
/// it is never stored or serialized.
///
/// # Example
///
/// ```
/// use procmond::security::Sanitized;
///
/// let cmd = "/usr/bin/app --password secret123 --verbose";
/// let display = format!("{}", Sanitized(cmd));
/// assert!(display.contains("[REDACTED]"));
/// assert!(!display.contains("secret123"));
/// ```
pub struct Sanitized<'a>(pub &'a str);

impl fmt::Display for Sanitized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sanitized = sanitize_command_line(self.0);
        f.write_str(&sanitized)
    }
}

/// Sanitize a command-line string by redacting values after sensitive flags.
///
/// Scans for known sensitive flags and replaces the token immediately
/// following each flag with `[REDACTED]`.
///
/// # Example
///
/// ```
/// use procmond::security::sanitize_command_line;
///
/// let result = sanitize_command_line("app --password secret123 --verbose");
/// assert_eq!(result, "app --password [REDACTED] --verbose");
/// ```
pub fn sanitize_command_line(cmd: &str) -> String {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut result = Vec::with_capacity(tokens.len());
    let mut redact_next = false;

    for token in &tokens {
        if redact_next {
            result.push(REDACTED);
            redact_next = false;
        } else if SENSITIVE_FLAGS
            .iter()
            .any(|flag| token.eq_ignore_ascii_case(flag))
        {
            result.push(token);
            redact_next = true;
        } else {
            result.push(token);
        }
    }

    result.join(" ")
}

/// Sanitize environment variables by redacting values with sensitive key names.
///
/// Keys matching (case-insensitive) `PASSWORD`, `SECRET`, `TOKEN`, `KEY`,
/// `API_KEY`, or `AUTH` will have their values replaced with `[REDACTED]`.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use procmond::security::sanitize_env_vars;
///
/// let mut env = HashMap::new();
/// env.insert("DB_PASSWORD".to_owned(), "supersecret".to_owned());
/// env.insert("HOME".to_owned(), "/home/user".to_owned());
///
/// let sanitized = sanitize_env_vars(&env);
/// assert_eq!(sanitized.get("DB_PASSWORD").unwrap(), "[REDACTED]");
/// assert_eq!(sanitized.get("HOME").unwrap(), "/home/user");
/// ```
pub fn sanitize_env_vars<S: std::hash::BuildHasher>(
    env: &HashMap<String, String, S>,
) -> HashMap<String, String> {
    env.iter()
        .map(|(key, value)| {
            let upper_key = key.to_uppercase();
            let is_sensitive = SENSITIVE_ENV_KEYS
                .iter()
                .any(|pattern| upper_key.contains(pattern));

            if is_sensitive {
                debug!(key = %key, "Redacting sensitive environment variable");
                (key.clone(), REDACTED.to_owned())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

/// Sanitize a file path by redacting paths to well-known sensitive directories.
///
/// Paths containing segments like `.ssh`, `.aws`, `.gnupg`, `.kube/config`,
/// `.docker/config`, `.npmrc`, `.netrc`, or `.env` are replaced with
/// `[REDACTED]`.
///
/// On Windows, backslash separators are also handled.
///
/// # Example
///
/// ```
/// use procmond::security::sanitize_file_path;
///
/// let result = sanitize_file_path("/home/user/.ssh/id_rsa");
/// assert_eq!(result, "[REDACTED]");
///
/// let safe = sanitize_file_path("/usr/bin/bash");
/// assert_eq!(safe, "/usr/bin/bash");
/// ```
pub fn sanitize_file_path(path: &str) -> String {
    // Normalize Windows backslashes for matching
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_lowercase();

    let is_sensitive = SENSITIVE_PATH_SEGMENTS
        .iter()
        .any(|segment| lower.contains(&format!("/{segment}")));

    if is_sensitive {
        debug!(path_prefix = %path.chars().take(20).collect::<String>(), "Redacting sensitive file path");
        REDACTED.to_owned()
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::as_conversions
)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_command_line_password() {
        let cmd = "app --password secret123 --verbose";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app --password [REDACTED] --verbose");
    }

    #[test]
    fn test_sanitize_command_line_multiple_flags() {
        let cmd = "app --password pass1 --token tok1 --debug";
        let result = sanitize_command_line(cmd);
        assert_eq!(
            result,
            "app --password [REDACTED] --token [REDACTED] --debug"
        );
    }

    #[test]
    fn test_sanitize_command_line_short_flags() {
        let cmd = "app -p secret -t token123 -v";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app -p [REDACTED] -t [REDACTED] -v");
    }

    #[test]
    fn test_sanitize_command_line_no_sensitive() {
        let cmd = "app --verbose --debug";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app --verbose --debug");
    }

    #[test]
    fn test_sanitize_command_line_flag_at_end() {
        let cmd = "app --password";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app --password");
    }

    #[test]
    fn test_sanitize_command_line_case_insensitive() {
        let cmd = "app --PASSWORD secret";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app --PASSWORD [REDACTED]");
    }

    #[test]
    fn test_sanitize_command_line_api_key() {
        let cmd = "app --api-key abc123 --output file.txt";
        let result = sanitize_command_line(cmd);
        assert_eq!(result, "app --api-key [REDACTED] --output file.txt");
    }

    #[test]
    fn test_sanitize_env_vars_sensitive() {
        let mut env = HashMap::new();
        env.insert("DB_PASSWORD".to_owned(), "supersecret".to_owned());
        env.insert("API_KEY".to_owned(), "key123".to_owned());
        env.insert("AUTH_TOKEN".to_owned(), "tok".to_owned());
        env.insert("HOME".to_owned(), "/home/user".to_owned());
        env.insert("PATH".to_owned(), "/usr/bin".to_owned());

        let sanitized = sanitize_env_vars(&env);

        assert_eq!(sanitized.get("DB_PASSWORD").unwrap(), REDACTED);
        assert_eq!(sanitized.get("API_KEY").unwrap(), REDACTED);
        assert_eq!(sanitized.get("AUTH_TOKEN").unwrap(), REDACTED);
        assert_eq!(sanitized.get("HOME").unwrap(), "/home/user");
        assert_eq!(sanitized.get("PATH").unwrap(), "/usr/bin");
    }

    #[test]
    fn test_sanitize_env_vars_case_insensitive() {
        let mut env = HashMap::new();
        env.insert("my_secret_value".to_owned(), "hidden".to_owned());

        let sanitized = sanitize_env_vars(&env);
        assert_eq!(sanitized.get("my_secret_value").unwrap(), REDACTED);
    }

    #[test]
    fn test_sanitize_env_vars_empty() {
        let env = HashMap::new();
        let sanitized = sanitize_env_vars(&env);
        assert!(sanitized.is_empty());
    }

    #[test]
    fn test_sanitized_display() {
        let cmd = "app --password secret --verbose";
        let display = format!("{}", Sanitized(cmd));
        assert!(!display.contains("secret"));
        assert!(display.contains("[REDACTED]"));
        assert!(display.contains("--verbose"));
    }

    #[test]
    fn test_detect_privileges_is_infallible() {
        // Should never panic regardless of platform
        let ctx = detect_privileges();
        assert!(!format!("{}", ctx.platform).is_empty());
    }

    #[test]
    fn test_security_context_degraded() {
        let ctx = SecurityContext::degraded(Platform::Linux);
        assert!(ctx.degraded_mode);
        assert!(!ctx.has_full_process_access);
        assert!(ctx.capabilities.is_empty());
    }

    #[test]
    fn test_platform_display() {
        assert_eq!(format!("{}", Platform::Linux), "linux");
        assert_eq!(format!("{}", Platform::MacOs), "macos");
        assert_eq!(format!("{}", Platform::Windows), "windows");
        assert_eq!(format!("{}", Platform::FreeBsd), "freebsd");
        assert_eq!(format!("{}", Platform::Unknown), "unknown");
    }

    #[test]
    fn test_sanitize_file_path_ssh() {
        assert_eq!(sanitize_file_path("/home/user/.ssh/id_rsa"), REDACTED);
        assert_eq!(sanitize_file_path("/home/user/.ssh/known_hosts"), REDACTED);
    }

    #[test]
    fn test_sanitize_file_path_aws() {
        assert_eq!(sanitize_file_path("/home/user/.aws/credentials"), REDACTED);
    }

    #[test]
    fn test_sanitize_file_path_gnupg() {
        assert_eq!(
            sanitize_file_path("/home/user/.gnupg/private-keys"),
            REDACTED
        );
    }

    #[test]
    fn test_sanitize_file_path_windows_backslash() {
        assert_eq!(
            sanitize_file_path("C:\\Users\\admin\\.ssh\\id_rsa"),
            REDACTED
        );
        assert_eq!(
            sanitize_file_path("C:\\Users\\admin\\.aws\\credentials"),
            REDACTED
        );
    }

    #[test]
    fn test_sanitize_file_path_safe() {
        assert_eq!(sanitize_file_path("/usr/bin/bash"), "/usr/bin/bash");
        assert_eq!(
            sanitize_file_path("/home/user/Documents/report.pdf"),
            "/home/user/Documents/report.pdf"
        );
    }

    #[test]
    fn test_sanitize_file_path_env_and_netrc() {
        assert_eq!(sanitize_file_path("/app/.env"), REDACTED);
        assert_eq!(sanitize_file_path("/home/user/.netrc"), REDACTED);
        assert_eq!(sanitize_file_path("/home/user/.npmrc"), REDACTED);
    }

    #[test]
    fn test_sanitize_file_path_kube_docker() {
        assert_eq!(sanitize_file_path("/home/user/.kube/config/ctx"), REDACTED);
        assert_eq!(
            sanitize_file_path("/home/user/.docker/config.json"),
            REDACTED
        );
    }

    #[test]
    fn test_freebsd_platform_degraded() {
        let ctx = SecurityContext::degraded(Platform::FreeBsd);
        assert!(ctx.degraded_mode);
        assert_eq!(ctx.platform, Platform::FreeBsd);
    }
}
