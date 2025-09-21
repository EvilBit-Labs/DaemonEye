use assert_cmd::prelude::*;
use insta::assert_snapshot;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.env("DAEMONEYE_AGENT_DATABASE_PATH", db_path.to_str().unwrap());
    // Enable test mode so the agent exits immediately after startup banner.
    cmd.env("DAEMONEYE_AGENT_TEST_MODE", "1");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("daemoneye-agent_greeting", stdout);
    Ok(())
}

#[test]
fn shows_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.arg("--help");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Demonstrate idiomatic with_settings! macro approach
    insta::with_settings!({
        filters => vec![
            // Normalize executable name: remove .exe extension on Windows
            (r"\bdaemoneye-agent\.exe\b", "daemoneye-agent"),
        ]
    }, {
        assert_snapshot!("daemoneye-agent_help", stdout.as_ref());
    });

    Ok(())
}

#[test]
fn shows_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.arg("--version");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("daemoneye-agent_version", stdout);
    Ok(())
}

#[test]
fn accepts_database_path() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("error");
    cmd.env("DAEMONEYE_AGENT_TEST_MODE", "1");

    // The test mode should exit before database initialization
    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("daemoneye-agent_database_path", stdout);
    Ok(())
}

#[test]
fn accepts_log_level() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("debug");
    cmd.env("DAEMONEYE_AGENT_TEST_MODE", "1");

    // The test mode should exit before database initialization
    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("daemoneye-agent_log_level", stdout);
    Ok(())
}

#[test]
fn handles_missing_database_gracefully() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("nonexistent").join("test.db");

    let mut cmd = Command::cargo_bin("daemoneye-agent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("error");
    // Don't set test mode so it will try to initialize the database
    // cmd.env("DAEMONEYE_AGENT_TEST_MODE", "1");

    // Should fail due to missing directory
    let output = cmd.output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Use insta settings to normalize the temporary directory path
    insta::with_settings!({
        filters => vec![
            // Normalize macOS temporary directory paths
            (r"/var/folders/[^/]+/[^/]+/[^/]+/[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Normalize Unix/Linux temporary directory paths
            (r"/tmp/\.[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Normalize Windows temporary directory paths (forward slashes) - AppData/Local/Temp
            (r"C:/Users/[^/]+/AppData/Local/Temp/[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Normalize Windows temporary directory paths (backslashes) - AppData/Local/Temp
            (r"C:\\Users\\[^\\]+\\AppData\\Local\\Temp\\[^\\]+\\nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Windows system TEMP directory patterns
            (r"C:/Windows/Temp/[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            (r"C:\\Windows\\Temp\\[^\\]+\\nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Generic Windows temp patterns with different drive letters
            (r"[A-Z]:/temp/[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            (r"[A-Z]:\\temp\\[^\\]+\\nonexistent", "/tmp/<TEMP>/nonexistent"),
            // Generic Windows Users temp patterns
            (r"[A-Z]:/Users/[^/]+/AppData/Local/Temp/[^/]+/nonexistent", "/tmp/<TEMP>/nonexistent"),
            (r"[A-Z]:\\Users\\[^\\]+\\AppData\\Local\\Temp\\[^\\]+\\nonexistent", "/tmp/<TEMP>/nonexistent"),
        ]
    }, {
        assert_snapshot!("daemoneye-agent_missing_database", stderr.as_ref());
    });
    Ok(())
}
