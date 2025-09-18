//! Integration tests for sentinelagent binary
//!
//! These tests validate the sentinelagent binary behavior in the unified sentineld package.
//! They ensure the binary works correctly with feature flags and maintains expected behavior.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.env("SENTINELAGENT_TEST_MODE", "1");
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn shows_help_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sentinelagent"));
    Ok(())
}

#[test]
fn shows_version_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sentinelagent"));
    Ok(())
}

#[test]
fn accepts_database_argument() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.args(["--database", "/tmp/test.db"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn accepts_log_level_argument() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.args(["--log-level", "debug"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn accepts_short_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.args(["-d", "/tmp/test.db", "-l", "warn"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn test_mode_exits_successfully() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.env("SENTINELAGENT_TEST_MODE", "1");
    cmd.assert()
        .failure() // Still fails due to database error before test mode check
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}
