//! The network stack reaches exactly one crate.
//!
//! `daemoneye-sinks` is the only workspace member allowed to link TLS, HTTP and SMTP
//! (R24), and the provider must be `ring`: aws-lc-rs needs NASM on Windows and
//! OpenSSL is banned by cargo-deny. A feature flag could not keep the stack out of
//! procmond because Cargo unifies features across a workspace build, so the gate
//! walks the resolved workspace graph instead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;

const FORBIDDEN_ANYWHERE: &[&str] = &["aws-lc-rs", "aws-lc-sys", "openssl-sys", "native-tls"];
const NETWORK_STACK: &[&str] = &[
    "daemoneye-sinks",
    "reqwest",
    "lettre",
    "rustls",
    "tokio-rustls",
];
const MUST_NOT_REACH_NETWORK: &[&str] = &[
    "procmond",
    "collector-core",
    "daemoneye-cli",
    "daemoneye-lib",
];

fn metadata() -> serde_json::Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../Cargo.toml");
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
            "--manifest-path",
            manifest,
        ])
        .output()
        .expect("cargo metadata runs");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("metadata is json")
}

/// `id -> name` for every package, and `id -> dependency ids` from the resolve graph.
fn graph(meta: &serde_json::Value) -> (HashMap<String, String>, HashMap<String, Vec<String>>) {
    let packages = meta
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .expect("packages");
    let names = packages
        .iter()
        .map(|p| (field(p, "id").to_owned(), field(p, "name").to_owned()))
        .collect();
    let nodes = meta
        .get("resolve")
        .and_then(|r| r.get("nodes"))
        .and_then(serde_json::Value::as_array)
        .expect("resolve nodes");
    let edges = nodes
        .iter()
        .map(|n| {
            let deps = n
                .get("dependencies")
                .and_then(serde_json::Value::as_array)
                .expect("dependencies")
                .iter()
                .map(|d| d.as_str().expect("dependency id").to_owned())
                .collect();
            (field(n, "id").to_owned(), deps)
        })
        .collect();
    (names, edges)
}

fn field<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .expect("string field")
}

fn reachable_names(
    root: &str,
    names: &HashMap<String, String>,
    edges: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let start = names
        .iter()
        .find(|entry| entry.1.as_str() == root)
        .map(|entry| entry.0.clone())
        .expect("root in graph");
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        for dep in edges.get(&id).into_iter().flatten() {
            queue.push_back(dep.clone());
        }
    }
    seen.into_iter()
        .filter_map(|id| names.get(&id).cloned())
        .collect()
}

/// No forbidden TLS backend anywhere, and the network stack is reachable from the agent only.
#[test]
fn network_stack_is_reachable_only_from_the_agent() {
    let meta = metadata();
    let (names, edges) = graph(&meta);
    let all: HashSet<&str> = names.values().map(String::as_str).collect();
    for crate_name in FORBIDDEN_ANYWHERE {
        assert!(
            !all.contains(crate_name),
            "{crate_name} is in the workspace resolve"
        );
    }
    let agent = reachable_names("daemoneye-agent", &names, &edges);
    for crate_name in NETWORK_STACK {
        assert!(
            agent.contains(*crate_name),
            "the agent must reach {crate_name}"
        );
    }
    for root in MUST_NOT_REACH_NETWORK {
        let reach = reachable_names(root, &names, &edges);
        for crate_name in NETWORK_STACK {
            assert!(!reach.contains(*crate_name), "{root} reaches {crate_name}");
        }
    }
}
