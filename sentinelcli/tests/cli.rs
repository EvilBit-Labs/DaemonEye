use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("sentinelcli")?;
    cmd.assert().success().stdout(predicate::str::contains(
        "Hello from sentinel-lib to sentinelcli!",
    ));
    Ok(())
}
