//! Execution of detection rules over the event store with `DataFusion` (ADR-0006).
//!
//! This module root currently carries the locked-down session ([`session`]) and the allowlisted
//! SQL functions ([`functions`], [`regexp`]). The executor itself arrives in later units.

pub mod functions;
pub mod regexp;
pub mod session;
