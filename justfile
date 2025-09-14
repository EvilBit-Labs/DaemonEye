set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just help

help:
    @just --list

fmt:
    @cargo fmt --all

fmt-check:
    @cargo fmt --all --check

lint-rust:
    @just fmt-check
    @cargo clippy --workspace --all-targets --all-features -- -D warnings

lint-just:
    @just --fmt --check --unstable

lint:
    @just lint-rust
    @just lint-just

build:
    @cargo build --workspace

check:
    @cargo check --workspace

test:
    @cargo test --workspace

run-procmond args='':
    @cargo run -p procmond -- {{ args }}

run-sentinelcli args='':
    @cargo run -p sentinelcli -- {{ args }}

run-sentinelagent args='':
    @cargo run -p sentinelagent -- {{ args }}
