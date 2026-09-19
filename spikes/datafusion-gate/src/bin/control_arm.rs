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
use datafusion_gate::{REPEATS, control, fixture, fixture_path, measure, require_fixture};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path();
    require_fixture(&path)?;

    let mut rss = measure::RssSampler::new();

    // Baseline is the store open and nothing else, matched to the DataFusion
    // arm's baseline so the marginal comparison is between like and like.
    let store = EventStore::open(&path)?;
    rss.sample();
    let baseline_bytes = rss.peak_bytes();

    let spec = fixture::FixtureSpec::default();
    let (start, end) = (spec.start_ms, spec.end_ms());

    // Watch resident set continuously across the measured section: a peak
    // that rises and falls inside one query is invisible to call-boundary
    // sampling alone.
    let watcher = measure::RssWatcher::start();

    // One warm run establishes the answer; the timed loop measures.
    let first = control::run(&store, start, end)?;
    rss.sample();
    let after_first_bytes = rss.peak_bytes();

    let mut samples = Vec::with_capacity(REPEATS);
    for i in 0..REPEATS {
        let t0 = Instant::now();
        let r = control::run(&store, start, end)?;
        samples.push(t0.elapsed());
        rss.sample();
        if r.matches != first.matches {
            return Err(format!(
                "control arm run {i} diverged from the warm run: {} pid(s) differ",
                first.matches.symmetric_difference(&r.matches).count()
            )
            .into());
        }
    }
    let lat = measure::Latency::new(samples);
    let watched_peak = watcher.stop();

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
    measure::print_rss_and_latency(&rss, baseline_bytes, after_first_bytes, &lat, watched_peak);
    println!("decode_only_ms={:.3}", decode_only.as_secs_f64() * 1000.0);
    Ok(())
}
