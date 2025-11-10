use insta::assert_snapshot;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("daemoneye-cli"));
    cmd.arg("--database").arg(&db_path);

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Command failed with stderr: {}", stderr);
    }
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("daemoneye-cli_greeting", stdout);
    Ok(())
}
