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
    /// Unknown / unsupported
    Unknown,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Linux => write!(f, "linux"),
            Self::MacOs => write!(f, "macos"),
            Self::Windows => write!(f, "windows"),
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
/// - **Windows**: Returns degraded context (full implementation requires Win32 API).
pub fn detect_privileges() -> SecurityContext {
    let platform = detect_platform();

    let ctx = match platform {
        Platform::Linux => detect_linux_privileges(),
        Platform::MacOs => detect_macos_privileges(),
        Platform::Windows => detect_windows_privileges(),
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
#[cfg(target_os = "macos")]
fn detect_macos_privileges() -> SecurityContext {
    let platform = Platform::MacOs;
    let mut capabilities = vec![];

    // Check if running as root using uzers crate (safe wrapper around getuid)
    let is_root = uzers::get_current_uid() == 0;

    if is_root {
        capabilities.push("root".to_owned());
    }

    SecurityContext {
        platform,
        has_full_process_access: is_root,
        capabilities,
        degraded_mode: false,
    }
}

/// Windows-specific privilege detection (stub).
#[cfg(target_os = "windows")]
fn detect_windows_privileges() -> SecurityContext {
    // Full implementation would use OpenProcessToken + LookupPrivilegeValue
    warn!("Windows privilege detection not fully implemented");
    SecurityContext::degraded(Platform::Windows)
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
        assert_eq!(format!("{}", Platform::Unknown), "unknown");
    }
}
