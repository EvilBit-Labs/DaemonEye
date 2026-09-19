//! `DataFusion` arm binary: `SessionContext` + pruning provider over the fixture.

#![expect(
    clippy::print_stdout,
    reason = "this binary's entire output is the measurement record"
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::datafusion_arm::{SessionSettings, build_context, run};
use datafusion_gate::{REPEATS, fixture, fixture_path, measure, require_fixture};
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path();
    require_fixture(&path)?;

    let mut rss = measure::RssSampler::new();

    // Baseline is the store open and nothing else — the same point the control
    // arm samples at. Building the SessionContext runs a granularity check that
    // decodes a bucket, so folding it into the baseline would charge DataFusion
    // for a validation helper rather than for itself. That cost is reported
    // separately as `after_context_rss_bytes`.
    let store = Arc::new(EventStore::open(&path)?);
    rss.sample();
    let baseline_bytes = rss.peak_bytes();

    let spec = fixture::FixtureSpec::default();
    let (start, end) = (spec.start_ms.cast_signed(), spec.end_ms().cast_signed());

    let settings = SessionSettings::default();
    let (ctx, buckets) = build_context(Arc::clone(&store), settings)?;
    rss.sample();
    let after_context_bytes = rss.peak_bytes();

    let watcher = measure::RssWatcher::start();

    let first = run(&ctx, start, end).await?;
    rss.sample();
    let after_first_bytes = rss.peak_bytes();

    let mut samples = Vec::with_capacity(REPEATS);
    for i in 0..REPEATS {
        let t0 = Instant::now();
        let m = run(&ctx, start, end).await?;
        samples.push(t0.elapsed());
        rss.sample();
        if m != first {
            return Err(format!(
                "datafusion arm run {i} diverged from the warm run: {} pid(s) differ",
                first.symmetric_difference(&m).count()
            )
            .into());
        }
    }
    let lat = measure::Latency::new(samples);
    let watched_peak = watcher.stop();

    println!("arm=datafusion");
    println!("platform={}", measure::platform());
    println!("buckets_discovered={buckets}");
    println!("target_partitions={}", settings.target_partitions);
    println!("batch_size={}", settings.batch_size);
    println!("matches={}", first.len());
    println!("after_context_rss_bytes={after_context_bytes}");
    measure::print_rss_and_latency(&rss, baseline_bytes, after_first_bytes, &lat, watched_peak);
    Ok(())
}
