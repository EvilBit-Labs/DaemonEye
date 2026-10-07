//! Execution of detection rules over the event store with `DataFusion` (ADR-0006).
//!
//! This module root currently carries the locked-down session ([`session`]) and the allowlisted
//! SQL functions ([`functions`], [`regexp`]), and plan derivation ([`mod@derive`]). The executor
//! itself arrives in a later unit.

pub mod derive;
pub mod functions;
pub mod regexp;
pub mod session;
