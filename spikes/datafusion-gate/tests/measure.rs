//! U7 verification — the measurement harness reports what it claims to.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use datafusion_gate::measure::{Latency, RssSampler, platform};
use std::time::Duration;

#[test]
fn the_sampler_observes_a_large_allocation_in_resident_set() {
    // Confirms the sampler reads resident set rather than reporting a constant.
    let mut rss = RssSampler::new();
    let before = rss.peak_bytes();

    let mut ballast: Vec<u8> = vec![0; 64 * 1024 * 1024];
    // Touch every page so the allocation is actually resident.
    for i in (0..ballast.len()).step_by(4096) {
        ballast[i] = 1;
    }
    rss.sample();
    let after = rss.peak_bytes();
    drop(ballast);

    assert!(
        after > before,
        "peak RSS must rise after a 64 MiB resident allocation: {before} -> {after}"
    );
    assert!(
        after - before > 32 * 1024 * 1024,
        "the rise must be on the order of the allocation, saw {} bytes",
        after - before
    );
}

#[test]
fn the_latency_report_carries_a_distribution_not_a_mean() {
    let lat = Latency::new(vec![
        Duration::from_millis(5),
        Duration::from_millis(9),
        Duration::from_millis(1),
        Duration::from_millis(7),
    ]);
    assert_eq!(lat.count(), 4, "every sample must be retained");
    assert_eq!(lat.min(), Duration::from_millis(1));
    assert_eq!(lat.max(), Duration::from_millis(9));
    assert_eq!(lat.p50(), Duration::from_millis(5));
    assert_eq!(lat.p95(), Duration::from_millis(9));
}

#[test]
fn an_empty_latency_report_does_not_panic() {
    let lat = Latency::new(vec![]);
    assert_eq!(lat.count(), 0);
    assert_eq!(lat.p50(), Duration::ZERO);
    assert_eq!(lat.max(), Duration::ZERO);
}

#[test]
fn every_record_names_a_platform_and_architecture() {
    let p = platform();
    assert!(p.contains('/'), "platform must be os/arch, got {p}");
    let (os, arch) = p.split_once('/').unwrap();
    assert!(!os.is_empty(), "os must be named");
    assert!(!arch.is_empty(), "arch must be named");
}
