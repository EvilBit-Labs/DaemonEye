use assert_cmd::prelude::*;
use insta::assert_snapshot;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.env("SENTINELAGENT_DATABASE_PATH", db_path.to_str().unwrap());
    // Enable test mode so the agent exits immediately after startup banner.
    cmd.env("SENTINELAGENT_TEST_MODE", "1");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelagent_greeting", stdout);
    Ok(())
}

#[test]
fn shows_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--help");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelagent_help", stdout);
    Ok(())
}

#[test]
fn shows_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--version");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelagent_version", stdout);
    Ok(())
}

#[test]
fn accepts_database_path() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("error");
    cmd.env("SENTINELAGENT_TEST_MODE", "1");

    // The test mode should exit before database initialization
    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelagent_database_path", stdout);
    Ok(())
}

#[test]
fn accepts_log_level() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("debug");
    cmd.env("SENTINELAGENT_TEST_MODE", "1");

    // The test mode should exit before database initialization
    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelagent_log_level", stdout);
    Ok(())
}

#[test]
fn handles_missing_database_gracefully() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("nonexistent").join("test.db");

    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("error");
    // Don't set test mode so it will try to initialize the database
    // cmd.env("SENTINELAGENT_TEST_MODE", "1");

    // Should fail due to missing directory
    let output = cmd.output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("sentinelagent_missing_database", stderr);
    Ok(())
}
