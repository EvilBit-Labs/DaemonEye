use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("sentinelagent")?;
    cmd.env("SENTINELAGENT_DATABASE_PATH", db_path.to_str().unwrap());
    // Enable test mode so the agent exits immediately after startup banner.
    cmd.env("SENTINELAGENT_TEST_MODE", "1");
    cmd.assert().success().stdout(predicate::str::contains(
        "sentinelagent started successfully",
    ));
    Ok(())
}
