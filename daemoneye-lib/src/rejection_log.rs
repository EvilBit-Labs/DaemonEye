//! Agent-side hash-chained record of rule-load and registration rejections (R4, R8).
//!
//! Rejections happen in `daemoneye-agent`, and the `audit_ledger` table is procmond-write /
//! others-read. Routing agent-observed rejections through procmond would invert that boundary for
//! no forensic gain, so the agent keeps its own chain here. This store is a **bounded in-memory
//! window**, not durable retention: persistence and Merkle inclusion proofs belong to the
//! audit-ledger work and are deliberately absent.
//!
//! The chain reuses [`crate::crypto::AuditEntry::compute_entry_hash_input`] so there is exactly one
//! canonical hash-input format in the workspace for a later persistence layer to adopt.

use std::collections::VecDeque;
use std::fmt;

use crate::{
    crypto::{AuditEntry, Blake3Hasher, CryptoError},
    detection::{RegexRejection, SqlRejection},
    detection_bounds::MAX_REJECTION_RECORDS,
};

/// The actor recorded on every entry: this store only ever holds the agent's own observations.
const ACTOR: &str = "daemoneye-agent";

/// Which registration gate refused a collector.
///
/// Every variant is fieldless **by design**. A registration rejection must never carry the
/// presented spawn token, nor a prefix, length or hash of it, so the type is given no place to
/// put one: a future edit cannot smuggle credential material through it, and the derived
/// [`Debug`] cannot reproduce any runtime value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistrationGate {
    /// The request carried no spawn token at all.
    NoTokenPresented,
    /// A token was presented but did not match the one issued for this collector identity.
    TokenMismatch,
    /// No spawn token was ever issued for the presented collector identity.
    UnknownCollector,
    /// A required registration field was missing or blank.
    MalformedRequest,
    /// The collector identity is already registered.
    AlreadyRegistered,
}

impl fmt::Display for RegistrationGate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let gate = match *self {
            Self::NoTokenPresented => "no spawn token presented",
            Self::TokenMismatch => "spawn token did not match",
            Self::UnknownCollector => "no spawn token issued for this collector identity",
            Self::MalformedRequest => "registration request failed field validation",
            Self::AlreadyRegistered => "collector identity is already registered",
        };
        formatter.write_str(gate)
    }
}

/// Why a record was written.
///
/// Rule-load and registration rejections carry different identifiers and different detail, so
/// they are separate variants rather than one struct where each source leaves the other's fields
/// empty.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RejectionReason {
    /// A rule's SQL failed the load-time validation gate.
    RuleSql {
        /// The rule the operator tried to load.
        rule_id: String,
        /// The specific construct that failed, with its position where one exists.
        rejection: SqlRejection,
    },
    /// A rule's `REGEXP` pattern failed the load-time compilation gate.
    RuleRegex {
        /// The rule the operator tried to load.
        rule_id: String,
        /// The pattern and the reason it could not be compiled.
        rejection: RegexRejection,
    },
    /// A rule was refused for a reason that is neither a SQL nor a regex rejection.
    RuleOther {
        /// The rule the operator tried to load.
        rule_id: String,
        /// The error's own diagnostic, verbatim.
        message: String,
    },
    /// A collector registration was refused.
    Registration {
        /// The collector identity that was presented. Never the token it presented.
        collector_id: String,
        /// Which gate fired.
        gate: RegistrationGate,
    },
}

impl RejectionReason {
    /// Record a SQL rule-load rejection against `rule_id`.
    pub fn rule_sql(rule_id: &str, rejection: SqlRejection) -> Self {
        Self::RuleSql {
            rule_id: rule_id.to_owned(),
            rejection,
        }
    }

    /// Record a regex rule-load rejection against `rule_id`.
    pub fn rule_regex(rule_id: &str, rejection: RegexRejection) -> Self {
        Self::RuleRegex {
            rule_id: rule_id.to_owned(),
            rejection,
        }
    }

    /// Record a rule-load rejection that carries no structured rejection value.
    pub fn rule_other(rule_id: &str, message: &str) -> Self {
        Self::RuleOther {
            rule_id: rule_id.to_owned(),
            message: message.to_owned(),
        }
    }

    /// Record a registration rejection against `collector_id`.
    ///
    /// This is the API the token-verification gate calls. It takes the collector identity and the
    /// gate that fired — never the request, so the presented token is not even in scope here.
    pub fn registration(collector_id: &str, gate: RegistrationGate) -> Self {
        Self::Registration {
            collector_id: collector_id.to_owned(),
            gate,
        }
    }

    /// The chain `action` label for this reason.
    const fn action(&self) -> &'static str {
        match *self {
            Self::RuleSql { .. } => "rule-load.sql-rejected",
            Self::RuleRegex { .. } => "rule-load.regex-rejected",
            Self::RuleOther { .. } => "rule-load.rejected",
            Self::Registration { .. } => "registration.rejected",
        }
    }
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RuleSql {
                ref rule_id,
                ref rejection,
            } => write!(formatter, "rule `{rule_id}` rejected: {rejection}"),
            Self::RuleRegex {
                ref rule_id,
                ref rejection,
            } => write!(formatter, "rule `{rule_id}` rejected: {rejection}"),
            Self::RuleOther {
                ref rule_id,
                ref message,
            } => write!(formatter, "rule `{rule_id}` rejected: {message}"),
            Self::Registration {
                ref collector_id,
                gate,
            } => write!(
                formatter,
                "registration of collector `{collector_id}` rejected: {gate}"
            ),
        }
    }
}

/// One rejection, chained to the one before it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RejectionRecord {
    /// Zero-based position in the chain.
    pub sequence: u64,
    /// When the rejection was observed.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Why the record was written.
    pub reason: RejectionReason,
    /// BLAKE3 of the rendered reason.
    pub payload_hash: String,
    /// The previous record's `entry_hash`, or `None` for the first record.
    pub previous_hash: Option<String>,
    /// BLAKE3 over this record's canonical hash input.
    pub entry_hash: String,
}

impl RejectionRecord {
    /// Recompute this record's canonical hash input.
    fn hash_input(&self) -> String {
        AuditEntry::compute_entry_hash_input(
            self.sequence,
            &self.timestamp,
            ACTOR,
            self.reason.action(),
            &self.payload_hash,
            self.previous_hash.as_deref(),
        )
    }
}

/// Hash-chained store of the agent's own rejections, bounded to a fixed window.
///
/// In-memory only. Nothing here opens the `audit_ledger` table, for reading or for writing — the
/// durable, forensically complete record is the audit ledger, which is T8's work. This is the
/// agent's live view of its own refusals, and it is **lossy by design**: registration is reachable
/// over IPC, so an unbounded chain would let a collector retrying with a stale token exhaust the
/// agent's memory budget. At [`MAX_REJECTION_RECORDS`] the oldest record is evicted to make room
/// for the newest.
///
/// `sequence` keeps counting across evictions, so a gap between the newest record's sequence and
/// the retained count is exactly how much has been dropped.
#[derive(Debug, Default)]
pub struct RejectionLog {
    records: VecDeque<RejectionRecord>,
    /// The sequence the next record takes. Counted separately from the retained length, which
    /// stops growing at the cap and would otherwise reissue one sequence number forever.
    next_sequence: u64,
}

impl RejectionLog {
    /// Create an empty log.
    pub const fn new() -> Self {
        Self {
            records: VecDeque::new(),
            next_sequence: 0,
        }
    }

    /// Append a rejection, returning the record that was written.
    ///
    /// Evicts the oldest retained record once the log is at [`MAX_REJECTION_RECORDS`].
    pub fn record(&mut self, reason: RejectionReason) -> RejectionRecord {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let previous_hash = self.records.back().map(|record| record.entry_hash.clone());
        let timestamp = chrono::Utc::now();
        let payload_hash = Blake3Hasher::hash_string(&reason.to_string());

        // The record is built first and hashed through its own `hash_input`, so the format written
        // here and the one `verify_integrity` recomputes are the same call site rather than two
        // argument lists that can drift. `hash_input` never reads `entry_hash`, so the placeholder
        // below does not enter the digest.
        let mut record = RejectionRecord {
            sequence,
            timestamp,
            reason,
            payload_hash,
            previous_hash,
            entry_hash: String::new(),
        };
        record.entry_hash = Blake3Hasher::hash_string(&record.hash_input());
        self.records.push_back(record.clone());
        while self.records.len() > MAX_REJECTION_RECORDS {
            self.records.pop_front();
        }
        record
    }

    /// The retained window, oldest first.
    ///
    /// Not every record ever written: anything past [`MAX_REJECTION_RECORDS`] has been evicted.
    pub const fn records(&self) -> &VecDeque<RejectionRecord> {
        &self.records
    }

    /// Whether any rejection is retained.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Verify every retained record's own hash and its link to its predecessor.
    ///
    /// Eviction is not a discontinuity. The record the oldest retained entry chains to may be gone,
    /// so its `previous_hash` has nothing in the window to be compared against — it is covered by
    /// that entry's own `entry_hash`, which is checked here regardless. What stays checkable at the
    /// window's edge is the structural claim, and `sequence` is inside the digest too: only the
    /// chain head has no predecessor, and every later record must name one. Links between retained
    /// records are checked exactly as before.
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::Hash`] naming the first record whose hash does not recompute or
    /// whose `previous_hash` does not match the record before it.
    pub fn verify_integrity(&self) -> Result<(), CryptoError> {
        let mut expected_previous: Option<&str> = None;
        for (position, record) in self.records.iter().enumerate() {
            let expected = Blake3Hasher::hash_string(&record.hash_input());
            if record.entry_hash != expected {
                let sequence = record.sequence;
                return Err(CryptoError::Hash(format!(
                    "rejection record {sequence} hash mismatch"
                )));
            }
            let linked = if position == 0 {
                record.previous_hash.is_none() == (record.sequence == 0)
            } else {
                record.previous_hash.as_deref() == expected_previous
            };
            if !linked {
                let sequence = record.sequence;
                return Err(CryptoError::Hash(format!(
                    "rejection chain discontinuity at record {sequence}"
                )));
            }
            expected_previous = Some(&record.entry_hash);
        }
        Ok(())
    }
}
