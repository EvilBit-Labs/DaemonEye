//! Fixture generator binary (U3). Not part of the size measurement.

#![expect(
    clippy::print_stdout,
    reason = "this binary reports what it generated so a run is reproducible"
)]

use datafusion_gate::{fixture, fixture_path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let spec = fixture::FixtureSpec::default();
    let stats = fixture::generate(&path, spec)?;
    println!("path={}", path.display());
    println!("rows={}", stats.rows);
    println!("buckets={}", stats.buckets);
    println!("planted={}", stats.planted);
    println!("start_ms={}", stats.start_ms);
    println!("end_ms={}", stats.end_ms);
    Ok(())
}
