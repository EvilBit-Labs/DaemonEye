//! `DataFusion` arm binary: `SessionContext` + pruning provider over the fixture.

#![expect(
    clippy::print_stdout,
    reason = "this binary's entire output is the measurement record"
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::datafusion_arm::{SessionSettings, build_context, run};
use datafusion_gate::{REPEATS, fixture, fixture_path, measure};
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path();
    if !path.exists() {
        return Err(format!(
            "fixture missing at {}; run `just spike-datafusion-fixture` first",
            path.display()
        )
        .into());
    }

    let mut rss = measure::RssSampler::new();
    let store = Arc::new(EventStore::open(&path)?);
    let spec = fixture::FixtureSpec::default();
    let start = spec.start_ms.cast_signed();
    let end = spec
        .start_ms
        .saturating_add(spec.span_hours.saturating_mul(fixture::HOUR_MS))
        .cast_signed();

    let settings = SessionSettings::default();
    let (ctx, buckets) = build_context(Arc::clone(&store), settings)?;
    rss.sample();
    let baseline_bytes = rss.peak_bytes();

    let first = run(&ctx, start, end).await?;
    rss.sample();
    let after_first_bytes = rss.peak_bytes();

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let t0 = Instant::now();
        let m = run(&ctx, start, end).await?;
        samples.push(t0.elapsed());
        rss.sample();
        if m != first {
            return Err("datafusion arm is not deterministic across runs".into());
        }
    }
    let lat = measure::Latency::new(samples);

    println!("arm=datafusion");
    println!("platform={}", measure::platform());
    println!("buckets_discovered={buckets}");
    println!("target_partitions={}", settings.target_partitions);
    println!("batch_size={}", settings.batch_size);
    println!("matches={}", first.len());
    println!("baseline_rss_bytes={baseline_bytes}");
    println!("after_first_run_rss_bytes={after_first_bytes}");
    println!("peak_rss_bytes={}", rss.peak_bytes());
    println!("peak_rss_mib={:.2}", rss.peak_mib());
    println!("latency_samples={}", lat.count());
    println!("latency_min_ms={:.3}", lat.min().as_secs_f64() * 1000.0);
    println!("latency_p50_ms={:.3}", lat.p50().as_secs_f64() * 1000.0);
    println!("latency_p95_ms={:.3}", lat.p95().as_secs_f64() * 1000.0);
    println!("latency_max_ms={:.3}", lat.max().as_secs_f64() * 1000.0);
    Ok(())
}
