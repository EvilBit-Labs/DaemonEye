use insta::assert_snapshot;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn shows_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("procmond"));
    cmd.arg("--help");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Demonstrate idiomatic with_settings! macro approach
    insta::with_settings!({
        filters => vec![
            // Normalize executable name: remove .exe extension on Windows
            (r"\bprocmond\.exe\b", "procmond"),
        ]
    }, {
        assert_snapshot!("procmond_help", stdout.as_ref());
    });

    Ok(())
}

#[test]
fn shows_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("procmond"));
    cmd.arg("--version");

    let output = cmd.output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("procmond_version", stdout);
    Ok(())
}

#[test]
fn accepts_database_path() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("procmond"));
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("error");

    // The command should start but we'll kill it quickly
    let mut child = cmd.spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(100));
    child.kill()?;

    Ok(())
}

#[test]
fn accepts_log_level() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("procmond"));
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--log-level").arg("debug");

    // The command should start but we'll kill it quickly
    let mut child = cmd.spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(100));
    child.kill()?;

    Ok(())
}

#[test]
fn accepts_collection_interval() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("procmond"));
    cmd.arg("--database").arg(&db_path);
    cmd.arg("--interval").arg("60");
    cmd.arg("--log-level").arg("error");

    // The command should start but we'll kill it quickly
    let mut child = cmd.spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(100));
    child.kill()?;

    Ok(())
}
