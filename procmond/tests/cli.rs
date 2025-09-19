use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn shows_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("procmond")?;
    cmd.arg("--help");
    cmd.assert().success().stdout(predicate::str::contains(
        "SentinelD Process Monitoring Daemon",
    ));
    Ok(())
}
