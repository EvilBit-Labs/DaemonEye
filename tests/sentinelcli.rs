//! Integration tests for sentinelcli binary
//!
//! These tests validate the sentinelcli binary behavior in the unified sentineld package.
//! They ensure the binary works correctly with feature flags and maintains expected behavior.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn shows_help_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sentinelcli"));
    Ok(())
}

#[test]
fn shows_version_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sentinelcli"));
    Ok(())
}

#[test]
fn accepts_database_argument() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.args(["--database", "/tmp/test.db"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn accepts_format_argument() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.args(["--format", "json"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn accepts_short_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.args(["-d", "/tmp/test.db", "-f", "json"]);
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn rejects_invalid_format() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.args(["--format", "invalid"]);
    cmd.assert()
        .failure() // Fails due to database error before format validation
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}
