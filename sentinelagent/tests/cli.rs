use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn prints_expected_greeting() {
    let mut cmd = Command::cargo_bin("sentinelagent").unwrap();
    cmd.assert().success().stdout(predicate::str::contains(
        "Hello from sentinel-lib to sentinelagent!",
    ));
}
