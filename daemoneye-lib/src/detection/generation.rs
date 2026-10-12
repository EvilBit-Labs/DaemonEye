//! Which load of a rule a measurement belongs to (ADR-0012, KTD6).
//!
//! A rule id is stable across a reload, so a latency report naming only the id can disable the
//! fresh instance for the sins of the one it replaced, and that would defeat the operator's only
//! recovery. A [`Generation`] is the missing half of the name: the engine issues a fresh,
//! engine-unique number at every load, and a report is applied only if it carries the number the
//! engine currently holds for that id.
//!
//! What makes this a guard rather than a convention is that nothing outside this module can build
//! a [`Generation`]. The tuple field and every constructor are private to this file, so not even
//! the sibling modules of `detection` can mint one; the only source is
//! [`DetectionEngine::runnable_rules`](super::DetectionEngine::runnable_rules), which reads it
//! from the table below. A caller cannot pass a generation it was not handed.
//!
//! ```compile_fail,E0423
//! use daemoneye_lib::detection::Generation;
//! // The field is private, so a generation cannot be invented.
//! let _forged = Generation(1);
//! ```
//!
//! ```compile_fail
//! use daemoneye_lib::detection::Generation;
//! // There is no conversion from a plain integer either.
//! let _forged: Generation = 1_u64.into();
//! ```

use std::collections::HashMap;
use std::fmt;

/// The load of a rule that a plan, and so a measurement of it, belongs to.
///
/// Obtained only from [`DetectionEngine::runnable_rules`](super::DetectionEngine::runnable_rules).
/// Two generations compare equal only if one load issued them. A generation does not name its rule,
/// so it is only meaningful beside the rule id it was issued with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Generation(u64);

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The engine's record of which generation each loaded rule currently holds.
///
/// Generations are drawn from one engine-wide counter, not one counter per rule id. A per-id
/// count that restarts when a rule is removed would hand a later rule of the same id a value an
/// earlier instance already carried, and a late report for the removed instance would then match
/// the new one: the same defect ADR-0012 exists to close, reopened through `remove_rule`. With a
/// shared counter every issued generation is unique for the life of the engine, so removing an id
/// from the table is safe.
#[derive(Debug, Default)]
pub(super) struct Generations {
    last_issued: u64,
    held: HashMap<String, u64>,
}

impl Generations {
    /// Issue a fresh generation to `rule_id`, replacing any it held.
    ///
    /// Saturates rather than wrapping: a wrapped counter could revisit a retired value and let a
    /// stale report match. Reaching `u64::MAX` loads is not a reachable state, so this is a
    /// stated limit and not a handled case.
    pub(super) fn issue(&mut self, rule_id: &str) {
        self.last_issued = self.last_issued.saturating_add(1);
        let _previous = self.held.insert(rule_id.to_owned(), self.last_issued);
    }

    /// Forget a rule that has left the engine.
    pub(super) fn forget(&mut self, rule_id: &str) {
        let _removed = self.held.remove(rule_id);
    }

    /// The generation currently held for `rule_id`, if it is loaded.
    pub(super) fn current(&self, rule_id: &str) -> Option<Generation> {
        self.held.get(rule_id).copied().map(Generation)
    }
}
