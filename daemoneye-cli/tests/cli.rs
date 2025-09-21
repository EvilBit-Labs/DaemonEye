use assert_cmd::prelude::*;
use insta::assert_snapshot;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::cargo_bin("sentinelcli")?;
    if let Some(path_str) = db_path.to_str() {
        cmd.env("SENTINELCLI_DATABASE_PATH", path_str);
    } else {
        return Err("Invalid database path".into());
    }

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("sentinelcli_greeting", stdout);
    Ok(())
}
