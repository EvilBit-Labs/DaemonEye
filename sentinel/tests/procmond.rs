// =============================================================================
// BUNDLED PROCMOND TESTS
// =============================================================================
// These tests validate the bundled procmond binary in the sentinel package.
// They ensure the distribution package works correctly and maintains the same
// behavior as the individual procmond package.
//
// TEST STRATEGY:
// - Test the bundled binary behavior matches the individual package
// - Validate error handling and expected failure modes
// - Ensure proper database error handling when database is missing
// - Verify CLI argument parsing and help output
//
// NOTE: These tests are for the distribution package, not the individual
// procmond package. The individual package has its own comprehensive tests.
// =============================================================================

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("procmond")?;
    cmd.assert()
        .failure() // Expected to fail due to missing database
        .stderr(predicate::str::contains("DatabaseError"));
    Ok(())
}

#[test]
fn shows_help_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("procmond")?;
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("procmond"));
    Ok(())
}

#[test]
fn shows_version_when_requested() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("procmond")?;
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("procmond"));
    Ok(())
}
