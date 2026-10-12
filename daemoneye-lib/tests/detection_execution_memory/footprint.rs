//! macOS `phys_footprint`, read by shelling out to `footprint(1)`.
//!
//! Resident set size counts memory the allocator has freed but not returned: macOS `malloc`
//! marks freed pages reusable and leaves them resident, and `footprint` reports them as
//! "Reclaimable" and leaves them out of its total. A run that churns many small allocations can
//! show 200 MiB of RSS over a 20 MiB live footprint, so RSS alone overstates what a scan holds.
//! This reader exists to measure the live number beside it. On other hosts `read` returns `Err`
//! and the harness reports RSS alone.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;
use std::time::{Duration, Instant};

use crate::rss::RssReader;

/// `footprint` costs about 10 ms a call; reads closer together than this reuse the last value.
const MIN_INTERVAL: Duration = Duration::from_millis(20);

pub struct FootprintReader {
    pid: u32,
    last: Option<(Instant, u64)>,
}

impl FootprintReader {
    pub fn new() -> Self {
        Self {
            pid: std::process::id(),
            last: None,
        }
    }
}

/// Parse the `Footprint: 19 MB` figure out of `footprint -p` output, in bytes.
pub fn parse_footprint(output: &str) -> Result<u64, String> {
    let line = output
        .lines()
        .find(|l| l.contains("Footprint:"))
        .ok_or("no Footprint line")?;
    let (_, after) = line.split_once("Footprint:").ok_or("no Footprint value")?;
    let mut parts = after.split_whitespace();
    let value: u64 = parts
        .next()
        .ok_or("no Footprint number")?
        .parse()
        .map_err(|e| format!("Footprint number: {e}"))?;
    let unit: u64 = match parts.next() {
        Some("KB") => 1024,
        Some("MB") => 1024 * 1024,
        Some("GB") => 1024 * 1024 * 1024,
        Some("B") => 1,
        other => return Err(format!("unknown Footprint unit {other:?}")),
    };
    Ok(value.saturating_mul(unit))
}

impl RssReader for FootprintReader {
    fn read(&mut self) -> Result<u64, String> {
        if let Some((at, bytes)) = self.last
            && at.elapsed() < MIN_INTERVAL
        {
            return Ok(bytes);
        }
        let output = Command::new("footprint")
            .args(["-p", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("footprint did not run: {e}"))?;
        if !output.status.success() {
            return Err("footprint exited non-zero".to_owned());
        }
        let bytes = parse_footprint(&String::from_utf8_lossy(&output.stdout))?;
        self.last = Some((Instant::now(), bytes));
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_unit() {
        let mb = "x [1]: 64-bit    Footprint: 19 MB (16384 bytes per page)";
        let kb = "x [1]: 64-bit    Footprint: 2064 KB (16384 bytes per page)";
        assert_eq!(parse_footprint(mb).unwrap(), 19 * 1024 * 1024);
        assert_eq!(parse_footprint(kb).unwrap(), 2064 * 1024);
    }

    #[test]
    fn missing_or_odd_output_is_an_error_not_zero() {
        assert!(parse_footprint("nothing here").is_err());
        assert!(parse_footprint("Footprint: 5 parsecs").is_err());
    }
}
