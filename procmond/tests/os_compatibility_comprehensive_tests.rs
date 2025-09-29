//! Comprehensive OS compatibility tests for ProcessCollector implementations.
//!
//! This module tests compatibility across different OS versions, configurations,
//! and environments to ensure robust cross-platform behavior.

use procmond::process_collector::{
    FallbackProcessCollector, ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;
use tracing_test::traced_test;

#[cfg(target_os = "linux")]
use procmond::linux_collector::{LinuxCollectorConfig, LinuxProcessCollector};

#[cfg(target_os = "macos")]
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};

#[cfg(target_os = "windows")]
use procmond::windows_collector::{WindowsCollectorConfig, WindowsProcessCollector};

/// Test timeout for OS compatibility operations.
const OS_COMPATIBILITY_TEST_TIMEOUT_SECS: u64 = 45;

/// Comprehensive OS environment information.
#[derive(Debug, Clone)]
struct OSEnvironment {
    os_name: String,
    os_version: String,
    kernel_version: String,
    architecture: String,
    family: String,
    is_virtualized: bool,
    container_runtime: Option<String>,
    security_features: Vec<String>,
}

impl OSEnvironment {
    /// Detect the current OS environment.
    fn detect() -> Self {
        let os_name = std::env::consts::OS.to_string();
        let architecture = std::env::consts::ARCH.to_string();
        let family = std::env::consts::FAMILY.to_string();

        let os_version = Self::detect_os_version();
        let kernel_version = Self::detect_kernel_version();
        let is_virtualized = Self::detect_virtualization();
        let container_runtime = Self::detect_container_runtime();
        let security_features = Self::detect_security_features();

        Self {
            os_name,
            os_version,
            kernel_version,
            architecture,
            family,
            is_virtualized,
            container_runtime,
            security_features,
        }
    }

    /// Detect OS version information.
    fn detect_os_version() -> String {
        #[cfg(target_os = "linux")]
        {
            // Try multiple sources for Linux version information
            if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
                if let Some(version) = Self::extract_field(&content, "VERSION") {
                    return version;
                }
                if let Some(pretty_name) = Self::extract_field(&content, "PRETTY_NAME") {
                    return pretty_name;
                }
            }

            if let Ok(content) = std::fs::read_to_string("/etc/lsb-release") {
                if let Some(description) = Self::extract_field(&content, "DISTRIB_DESCRIPTION") {
                    return description;
                }
            }

            // Fallback to uname
            if let Ok(output) = std::process::Command::new("uname").arg("-a").output() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
            {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Ok(build_output) = std::process::Command::new("sw_vers")
                    .arg("-buildVersion")
                    .output()
                {
                    let build = String::from_utf8_lossy(&build_output.stdout).trim();
                    return format!("{} ({})", version, build);
                }
                return version;
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Try to get Windows version information
            if let Ok(output) = std::process::Command::new("cmd")
                .args(["/C", "ver"])
                .output()
            {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }

            // Fallback to environment variables
            if let Ok(version) = std::env::var("OS") {
                return version;
            }
        }

        "unknown".to_string()
    }

    /// Detect kernel version information.
    fn detect_kernel_version() -> String {
        #[cfg(unix)]
        {
            if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Windows kernel version is typically part of the OS version
            if let Ok(output) = std::process::Command::new("cmd")
                .args(["/C", "ver"])
                .output()
            {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }

        "unknown".to_string()
    }

    /// Detect if running in a virtualized environment.
    fn detect_virtualization() -> bool {
        #[cfg(target_os = "linux")]
        {
            // Check for common virtualization indicators
            if std::fs::read_to_string("/proc/cpuinfo")
                .map(|content| content.contains("hypervisor"))
                .unwrap_or(false)
            {
                return true;
            }

            if std::fs::read_to_string("/sys/class/dmi/id/product_name")
                .map(|content| {
                    let content_lower = content.to_lowercase();
                    content_lower.contains("vmware")
                        || content_lower.contains("virtualbox")
                        || content_lower.contains("kvm")
                        || content_lower.contains("qemu")
                })
                .unwrap_or(false)
            {
                return true;
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Check for macOS virtualization indicators
            if let Ok(output) = std::process::Command::new("system_profiler")
                .arg("SPHardwareDataType")
                .output()
            {
                let content = String::from_utf8_lossy(&output.stdout).to_lowercase();
                if content.contains("vmware")
                    || content.contains("parallels")
                    || content.contains("virtualbox")
                {
                    return true;
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Check Windows virtualization indicators
            if let Ok(output) = std::process::Command::new("wmic")
                .args(["computersystem", "get", "model"])
                .output()
            {
                let content = String::from_utf8_lossy(&output.stdout).to_lowercase();
                if content.contains("vmware")
                    || content.contains("virtualbox")
                    || content.contains("hyper-v")
                {
                    return true;
                }
            }
        }

        false
    }

    /// Detect container runtime.
    fn detect_container_runtime() -> Option<String> {
        // Check for Docker
        if std::fs::read_to_string("/proc/1/cgroup")
            .map(|content| content.contains("docker"))
            .unwrap_or(false)
        {
            return Some("docker".to_string());
        }

        // Check for Kubernetes/containerd
        if std::fs::read_to_string("/proc/1/cgroup")
            .map(|content| content.contains("kubepods"))
            .unwrap_or(false)
        {
            return Some("kubernetes".to_string());
        }

        // Check for Podman
        if std::env::var("container").as_deref() == Ok("podman") {
            return Some("podman".to_string());
        }

        // Check for LXC
        if std::fs::read_to_string("/proc/1/environ")
            .map(|content| content.contains("container=lxc"))
            .unwrap_or(false)
        {
            return Some("lxc".to_string());
        }

        None
    }

    /// Detect security features.
    fn detect_security_features() -> Vec<String> {
        let mut features = Vec::new();

        #[cfg(target_os = "linux")]
        {
            // Check for SELinux
            if std::fs::read_to_string("/sys/fs/selinux/enforce").is_ok() {
                features.push("selinux".to_string());
            }

            // Check for AppArmor
            if std::fs::read_to_string("/sys/kernel/security/apparmor/profiles").is_ok() {
                features.push("apparmor".to_string());
            }

            // Check for seccomp
            if std::fs::read_to_string("/proc/sys/kernel/seccomp/actions_avail").is_ok() {
                features.push("seccomp".to_string());
            }

            // Check for capabilities
            if std::fs::read_to_string("/proc/sys/kernel/cap_last_cap").is_ok() {
                features.push("capabilities".to_string());
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Check for System Integrity Protection (SIP)
            if let Ok(output) = std::process::Command::new("csrutil").arg("status").output() {
                let output_str = String::from_utf8_lossy(&output.stdout);
                if output_str.contains("enabled") {
                    features.push("sip".to_string());
                }
            }

            // Check for Gatekeeper
            if let Ok(output) = std::process::Command::new("spctl").arg("--status").output() {
                let output_str = String::from_utf8_lossy(&output.stdout);
                if output_str.contains("enabled") {
                    features.push("gatekeeper".to_string());
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Check for Windows Defender
            if std::process::Command::new("powershell")
                .args(["-Command", "Get-MpComputerStatus"])
                .output()
                .is_ok()
            {
                features.push("windows_defender".to_string());
            }

            // Check for UAC
            features.push("uac".to_string()); // UAC is always present on modern Windows
        }

        features
    }

    /// Extract field value from key=value format.
    #[allow(dead_code)]
    fn extract_field(content: &str, field: &str) -> Option<String> {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix(&format!("{}=", field)) {
                return Some(value.trim_matches('"').to_string());
            }
        }
        None
    }
}

/// Test comprehensive OS environment detection and compatibility.
#[tokio::test]
#[traced_test]
async fn test_os_environment_detection() {
    let env = OSEnvironment::detect();

    println!("Detected OS Environment:");
    println!("  OS: {} {}", env.os_name, env.os_version);
    println!("  Kernel: {}", env.kernel_version);
    println!("  Architecture: {} ({})", env.architecture, env.family);
    println!("  Virtualized: {}", env.is_virtualized);
    println!("  Container: {:?}", env.container_runtime);
    println!("  Security Features: {:?}", env.security_features);

    // Basic environment validation
    assert!(!env.os_name.is_empty(), "OS name should be detected");
    assert!(
        !env.architecture.is_empty(),
        "Architecture should be detected"
    );
    assert!(!env.family.is_empty(), "OS family should be detected");

    // Test that collectors work in this environment
    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 100,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing collector {} in detected environment", name);

        let result = timeout(
            Duration::from_secs(OS_COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            result.is_ok(),
            "Collector {} should work in environment: {:?}",
            name,
            env
        );

        let collection = result.unwrap();
        assert!(
            collection.is_ok(),
            "Collector {} should succeed in environment: {:?}: {:?}",
            name,
            env,
            collection.err()
        );

        let (events, _stats) = collection.unwrap();
        assert!(
            !events.is_empty(),
            "Collector {} should find processes in environment: {:?}",
            name,
            env
        );

        println!(
            "✓ Collector {} works in environment: {} processes collected",
            name,
            events.len()
        );
    }
}

/// Test OS-specific feature compatibility.
#[tokio::test]
#[traced_test]
async fn test_os_specific_feature_compatibility() {
    let env = OSEnvironment::detect();

    println!("Testing OS-specific features for: {}", env.os_name);

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 200,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!("Testing OS-specific features for collector: {}", name);

        let _capabilities = collector.capabilities();

        // Test capabilities match OS expectations
        match env.os_name.as_str() {
            "linux" => {
                #[cfg(target_os = "linux")]
                test_linux_specific_features(&*collector, &env).await;
                #[cfg(not(target_os = "linux"))]
                test_generic_os_features(&*collector, &env).await;
            }
            "macos" => {
                #[cfg(target_os = "macos")]
                test_macos_specific_features(&*collector, &env).await;
                #[cfg(not(target_os = "macos"))]
                test_generic_os_features(&*collector, &env).await;
            }
            "windows" => {
                #[cfg(target_os = "windows")]
                test_windows_specific_features(&*collector, &env).await;
                #[cfg(not(target_os = "windows"))]
                test_generic_os_features(&*collector, &env).await;
            }
            _ => {
                test_generic_os_features(&*collector, &env).await;
            }
        }

        // Test that collector respects security features
        test_security_feature_compatibility(&*collector, &env).await;

        println!("✓ OS-specific feature test passed for {}", name);
    }
}

/// Test virtualization and container compatibility.
#[tokio::test]
#[traced_test]
async fn test_virtualization_container_compatibility() {
    let env = OSEnvironment::detect();

    println!(
        "Testing virtualization/container compatibility: virtualized={}, container={:?}",
        env.is_virtualized, env.container_runtime
    );

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 150,
    };

    let collectors = create_all_available_collectors(config);

    for (name, collector) in collectors {
        println!(
            "Testing virtualization compatibility for collector: {}",
            name
        );

        let result = timeout(
            Duration::from_secs(OS_COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;

        assert!(
            result.is_ok(),
            "Collector {} should work in virtualized environment",
            name
        );

        let collection = result.unwrap();
        assert!(
            collection.is_ok(),
            "Collector {} should succeed in virtualized environment: {:?}",
            name,
            collection.err()
        );

        let (events, stats) = collection.unwrap();

        // Analyze virtualization-specific behavior
        if env.is_virtualized {
            println!(
                "Virtualized environment results for {}: {} processes, {} successful",
                name,
                events.len(),
                stats.successful_collections
            );

            // In virtualized environments, we might see different process patterns
            let hypervisor_processes = events
                .iter()
                .filter(|e| {
                    let name_lower = e.name.to_lowercase();
                    name_lower.contains("vmware")
                        || name_lower.contains("vbox")
                        || name_lower.contains("qemu")
                        || name_lower.contains("kvm")
                })
                .count();

            if hypervisor_processes > 0 {
                println!("✓ Detected hypervisor processes: {}", hypervisor_processes);
            }
        }

        // Test container-specific behavior
        if let Some(ref container_runtime) = env.container_runtime {
            println!(
                "Container environment ({}) results for {}: {} processes",
                container_runtime,
                name,
                events.len()
            );

            // In containers, we typically see fewer processes
            if events.len() < 50 {
                println!("✓ Container environment shows expected reduced process count");
            }

            // Look for container-specific processes
            let container_processes = events
                .iter()
                .filter(|e| {
                    let name_lower = e.name.to_lowercase();
                    name_lower.contains("containerd")
                        || name_lower.contains("dockerd")
                        || name_lower.contains("podman")
                        || name_lower.contains("runc")
                })
                .count();

            if container_processes > 0 {
                println!(
                    "✓ Detected container runtime processes: {}",
                    container_processes
                );
            }
        }

        println!(
            "✓ Virtualization/container compatibility test passed for {}",
            name
        );
    }
}

/// Test different OS configuration scenarios.
#[tokio::test]
#[traced_test]
async fn test_os_configuration_scenarios() {
    let env = OSEnvironment::detect();

    println!("Testing OS configuration scenarios for: {}", env.os_name);

    // Test different configuration scenarios
    let scenarios = vec![
        (
            "minimal_security",
            ProcessCollectionConfig {
                collect_enhanced_metadata: false,
                compute_executable_hashes: false,
                skip_system_processes: true,
                skip_kernel_threads: true,
                max_processes: 20,
            },
        ),
        (
            "balanced_security",
            ProcessCollectionConfig {
                collect_enhanced_metadata: true,
                compute_executable_hashes: false,
                skip_system_processes: false,
                skip_kernel_threads: true,
                max_processes: 100,
            },
        ),
        (
            "comprehensive_security",
            ProcessCollectionConfig {
                collect_enhanced_metadata: true,
                compute_executable_hashes: true,
                skip_system_processes: false,
                skip_kernel_threads: false,
                max_processes: 500,
            },
        ),
    ];

    for (scenario_name, config) in scenarios {
        println!("Testing configuration scenario: {}", scenario_name);

        let collectors = create_all_available_collectors(config.clone());

        for (name, collector) in collectors {
            let result = timeout(
                Duration::from_secs(OS_COMPATIBILITY_TEST_TIMEOUT_SECS),
                collector.collect_processes(),
            )
            .await;

            assert!(
                result.is_ok(),
                "Collector {} should work with {} configuration",
                name,
                scenario_name
            );

            let collection = result.unwrap();
            assert!(
                collection.is_ok(),
                "Collector {} should succeed with {} configuration: {:?}",
                name,
                scenario_name,
                collection.err()
            );

            let (events, stats) = collection.unwrap();

            println!(
                "Configuration scenario {} results for {}: {} processes, {} successful",
                scenario_name,
                name,
                events.len(),
                stats.successful_collections
            );

            // Verify configuration is respected
            assert!(
                events.len() <= config.max_processes,
                "Should respect max_processes for {} with {}",
                name,
                scenario_name
            );

            println!(
                "✓ Configuration scenario {} passed for {}",
                scenario_name, name
            );
        }
    }
}

/// Test performance characteristics across different OS environments.
#[tokio::test]
#[traced_test]
async fn test_os_performance_characteristics() {
    let env = OSEnvironment::detect();

    println!("Testing performance characteristics for: {}", env.os_name);

    let config = ProcessCollectionConfig {
        collect_enhanced_metadata: true,
        compute_executable_hashes: false,
        skip_system_processes: false,
        skip_kernel_threads: false,
        max_processes: 1000,
    };

    let collectors = create_all_available_collectors(config);

    let mut performance_results = HashMap::new();

    for (name, collector) in collectors {
        println!("Testing performance for collector: {}", name);

        let start_time = std::time::Instant::now();
        let result = timeout(
            Duration::from_secs(OS_COMPATIBILITY_TEST_TIMEOUT_SECS),
            collector.collect_processes(),
        )
        .await;
        let total_duration = start_time.elapsed();

        if let Ok(Ok((events, _stats))) = result {
            let rate = events.len() as f64 / total_duration.as_secs_f64();

            performance_results.insert(name.to_string(), (events.len(), rate, total_duration));

            println!(
                "Performance for {} on {}: {} processes in {:.2}ms, rate: {:.1} proc/sec",
                name,
                env.os_name,
                events.len(),
                total_duration.as_millis(),
                rate
            );

            // Basic performance sanity checks
            assert!(
                total_duration.as_secs() < OS_COMPATIBILITY_TEST_TIMEOUT_SECS,
                "Collection should complete within reasonable time for {}",
                name
            );

            assert!(
                rate > 0.0,
                "Should have positive collection rate for {}",
                name
            );
        }
    }

    // Compare performance across collectors
    if performance_results.len() > 1 {
        println!("Performance comparison on {}:", env.os_name);
        for (name, (count, rate, duration)) in &performance_results {
            println!(
                "  {}: {} processes, {:.1} proc/sec, {:.2}ms",
                name,
                count,
                rate,
                duration.as_millis()
            );
        }
    }
}

/// Helper function to test Linux-specific features.
#[cfg(target_os = "linux")]
async fn test_linux_specific_features(collector: &dyn ProcessCollector, env: &OSEnvironment) {
    println!("Testing Linux-specific features");

    // Test /proc filesystem access
    let proc_accessible = std::fs::read_dir("/proc").is_ok();
    println!("✓ /proc filesystem accessible: {}", proc_accessible);

    // Test for security features
    for feature in &env.security_features {
        match feature.as_str() {
            "selinux" => {
                println!("✓ SELinux detected - testing process collection under SELinux");
                // SELinux might restrict some process access
            }
            "apparmor" => {
                println!("✓ AppArmor detected - testing process collection under AppArmor");
                // AppArmor might restrict some process access
            }
            "seccomp" => {
                println!("✓ seccomp detected - testing process collection with seccomp");
            }
            "capabilities" => {
                println!("✓ Linux capabilities detected");
            }
            _ => {}
        }
    }

    // Test process collection works
    let result = collector.collect_processes().await;
    assert!(result.is_ok(), "Linux process collection should work");

    if let Ok((events, _)) = result {
        // Look for Linux-specific processes
        let kernel_threads = events
            .iter()
            .filter(|e| e.name.starts_with('[') && e.name.ends_with(']'))
            .count();

        println!("✓ Linux kernel threads detected: {}", kernel_threads);
    }
}

/// Helper function to test macOS-specific features.
#[cfg(target_os = "macos")]
async fn test_macos_specific_features(collector: &dyn ProcessCollector, env: &OSEnvironment) {
    println!("Testing macOS-specific features");

    // Test for security features
    for feature in &env.security_features {
        match feature.as_str() {
            "sip" => {
                println!("✓ System Integrity Protection (SIP) detected");
                // SIP restricts access to system processes
            }
            "gatekeeper" => {
                println!("✓ Gatekeeper detected");
            }
            _ => {}
        }
    }

    // Test process collection works under macOS restrictions
    let result = collector.collect_processes().await;
    assert!(result.is_ok(), "macOS process collection should work");

    if let Ok((events, stats)) = result {
        println!(
            "✓ macOS process collection: {} processes, {} inaccessible",
            events.len(),
            stats.inaccessible_processes
        );

        // macOS typically has more inaccessible processes due to security restrictions
        if stats.inaccessible_processes > 0 {
            println!("✓ macOS security restrictions properly handled");
        }
    }
}

/// Helper function to test Windows-specific features.
#[cfg(target_os = "windows")]
async fn test_windows_specific_features(collector: &dyn ProcessCollector, env: &OSEnvironment) {
    println!("Testing Windows-specific features");

    // Test for security features
    for feature in &env.security_features {
        match feature.as_str() {
            "windows_defender" => {
                println!("✓ Windows Defender detected");
            }
            "uac" => {
                println!("✓ User Account Control (UAC) detected");
            }
            _ => {}
        }
    }

    // Test process collection works
    let result = collector.collect_processes().await;
    assert!(result.is_ok(), "Windows process collection should work");

    if let Ok((events, _)) = result {
        // Look for Windows-specific processes
        let system_processes = events
            .iter()
            .filter(|e| {
                let name_lower = e.name.to_lowercase();
                name_lower.contains("system")
                    || name_lower.contains("service")
                    || name_lower.contains("svchost")
            })
            .count();

        println!("✓ Windows system processes detected: {}", system_processes);
    }
}

/// Helper function to test generic OS features.
async fn test_generic_os_features(collector: &dyn ProcessCollector, env: &OSEnvironment) {
    println!("Testing generic OS features for: {}", env.os_name);

    // Test basic process collection works
    let result = collector.collect_processes().await;
    assert!(result.is_ok(), "Generic process collection should work");

    if let Ok((events, stats)) = result {
        println!(
            "✓ Generic process collection: {} processes, {} successful",
            events.len(),
            stats.successful_collections
        );

        // Basic validation
        assert!(!events.is_empty(), "Should find some processes");
        assert!(
            stats.successful_collections > 0,
            "Should have some successful collections"
        );
    }
}

/// Helper function to test security feature compatibility.
async fn test_security_feature_compatibility(
    collector: &dyn ProcessCollector,
    env: &OSEnvironment,
) {
    if env.security_features.is_empty() {
        println!("No security features detected - skipping security compatibility test");
        return;
    }

    println!(
        "Testing security feature compatibility: {:?}",
        env.security_features
    );

    // Test that collector works despite security restrictions
    let result = collector.collect_processes().await;
    assert!(
        result.is_ok(),
        "Process collection should work with security features enabled"
    );

    if let Ok((events, stats)) = result {
        println!(
            "Security compatibility results: {} processes, {} successful, {} inaccessible",
            events.len(),
            stats.successful_collections,
            stats.inaccessible_processes
        );

        // With security features enabled, some processes might be inaccessible
        // This is expected and acceptable
        if stats.inaccessible_processes > 0 {
            println!(
                "✓ Security restrictions properly handled: {} inaccessible processes",
                stats.inaccessible_processes
            );
        }

        // Should still collect some processes
        assert!(
            stats.successful_collections > 0,
            "Should collect some processes despite security restrictions"
        );
    }
}

/// Helper function to create all available collectors for OS compatibility testing.
fn create_all_available_collectors(
    config: ProcessCollectionConfig,
) -> Vec<(&'static str, Box<dyn ProcessCollector>)> {
    let mut collectors: Vec<(&'static str, Box<dyn ProcessCollector>)> = Vec::new();

    // Always available collectors
    collectors.push((
        "sysinfo-collector",
        Box::new(SysinfoProcessCollector::new(config.clone())),
    ));

    collectors.push((
        "fallback-collector",
        Box::new(FallbackProcessCollector::new(config.clone())),
    ));

    // Platform-specific collectors
    #[cfg(target_os = "linux")]
    {
        let linux_config = LinuxCollectorConfig::default();
        if let Ok(linux_collector) = LinuxProcessCollector::new(config.clone(), linux_config) {
            collectors.push(("linux-proc-collector", Box::new(linux_collector)));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let macos_config = MacOSCollectorConfig::default();
        if let Ok(macos_collector) = EnhancedMacOSCollector::new(config.clone(), macos_config) {
            collectors.push(("enhanced-macos-collector", Box::new(macos_collector)));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let windows_config = WindowsCollectorConfig::default();
        if let Ok(windows_collector) = WindowsProcessCollector::new(config.clone(), windows_config)
        {
            collectors.push(("windows-collector", Box::new(windows_collector)));
        }
    }

    collectors
}
