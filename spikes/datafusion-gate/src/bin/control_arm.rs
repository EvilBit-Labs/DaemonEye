//! Control arm binary: the materialization ceiling and the reference answer.
//!
//! Also the size baseline for R8 — it links `daemoneye-lib` and the fixture
//! code but not `DataFusion`, so the delta against `datafusion-arm` is exactly
//! what `DataFusion` costs (KTD7).

// The measurement result is the product of this binary; printing it is the point.
#![expect(
    clippy::print_stdout,
    reason = "this binary's entire output is the measurement record"
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::{REPEATS, control, fixture, fixture_path, measure};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path();
    if !path.exists() {
        return Err(format!(
            "fixture missing at {}; run `just spike-datafusion-fixture` first",
            path.display()
        )
        .into());
    }

    let mut rss = measure::RssSampler::new();
    let store = EventStore::open(&path)?;
    rss.sample();
    let baseline_bytes = rss.peak_bytes();
    let spec = fixture::FixtureSpec::default();
    let start = spec.start_ms;
    let end = spec
        .start_ms
        .saturating_add(spec.span_hours.saturating_mul(fixture::HOUR_MS));

    // One warm run establishes the answer; the timed loop measures.
    let first = control::run(&store, start, end)?;
    rss.sample();
    let after_first_bytes = rss.peak_bytes();

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let t0 = Instant::now();
        let r = control::run(&store, start, end)?;
        samples.push(t0.elapsed());
        rss.sample();
        if r.matches != first.matches {
            return Err("control arm is not deterministic across runs".into());
        }
    }
    let lat = measure::Latency::new(samples);

    // A decode-only pass separates codec cost from the match computation.
    let t0 = Instant::now();
    let decoded = control::decode_only(&store, start, end)?;
    let decode_only = t0.elapsed();
    rss.sample();

    println!("arm=control");
    println!("platform={}", measure::platform());
    println!("rows_decoded={}", first.rows_decoded);
    println!("decode_only_rows={decoded}");
    println!("matches={}", first.matches.len());
    println!("baseline_rss_bytes={baseline_bytes}");
    println!("after_first_run_rss_bytes={after_first_bytes}");
    println!("peak_rss_bytes={}", rss.peak_bytes());
    println!("peak_rss_mib={:.2}", rss.peak_mib());
    println!("latency_samples={}", lat.count());
    println!("latency_min_ms={:.3}", lat.min().as_secs_f64() * 1000.0);
    println!("latency_p50_ms={:.3}", lat.p50().as_secs_f64() * 1000.0);
    println!("latency_p95_ms={:.3}", lat.p95().as_secs_f64() * 1000.0);
    println!("latency_max_ms={:.3}", lat.max().as_secs_f64() * 1000.0);
    println!("decode_only_ms={:.3}", decode_only.as_secs_f64() * 1000.0);
    Ok(())
}
