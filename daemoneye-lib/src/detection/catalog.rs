//! Schema catalog, and the authentication gate that is the only way into it (R8, R10, R11, R12).
//!
//! Two things live here because they are one mechanism. [`SchemaCatalog::register`] takes a
//! [`VerifiedRegistration`], and the only way to obtain one is [`verify_spawn_token`]. A caller
//! that skipped the token check has nothing to pass, so "no descriptor enters the catalog without
//! a verified token" is a property of the types rather than of anyone remembering to call a
//! checker — the same newtype-receipt pattern that closed the binary-hashing authorization defect
//! (`docs/solutions/security-issues/binary-hashing-authorization-and-toctou-fixes.md`).
//!
//! The catalog stores the prost descriptor types from [`crate::proto`], not the serde mirror in
//! `daemoneye-eventbus`: this crate and that one are siblings with no dependency edge, and U1
//! settled that the mirror, not a crate edge, is how the two sides stay aligned. `daemoneye-agent`
//! sees both crates and converts at the registration boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use subtle::ConstantTimeEq;

use crate::detection_bounds::{
    MAX_COLUMNS_PER_TABLE, MAX_CONFORMANCE_RESULTS, MAX_IDENTIFIER_LENGTH,
    MAX_OPERATIONS_PER_COLUMN, MAX_TABLES_PER_DESCRIPTOR,
};
use crate::proto::{
    ColumnDescriptor, ConformanceResult, PredicateOp, SchemaDescriptor, TableDescriptor,
};

/// Length in ASCII characters of an issued spawn token: 32 random bytes, lowercase hex (R9).
pub const SPAWN_TOKEN_HEX_LEN: usize = 64;

/// Why a spawn token did not authenticate a registration.
///
/// Fieldless by design, exactly as `RegistrationGate` is: no variant has anywhere to hold the
/// presented token, so no rejection path can leak it into a log line or a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenRejection {
    /// The request carried no spawn token at all.
    NoTokenPresented,
    /// No spawn token was ever issued for the presented collector identity.
    UnknownCollector,
    /// A token was presented but did not match the one issued for this identity.
    TokenMismatch,
}

impl fmt::Display for TokenRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match *self {
            Self::NoTokenPresented => "no spawn token presented",
            Self::UnknownCollector => "no spawn token issued for this collector identity",
            Self::TokenMismatch => "spawn token did not match",
        };
        formatter.write_str(text)
    }
}

/// Proof that a registration presented the spawn token issued for a named collector (KTD9).
///
/// The inner field is private and this module exposes no constructor, so the only value of this
/// type that can exist anywhere in the workspace is one [`verify_spawn_token`] returned. It also
/// carries the identity it was verified *for*: a collector holding a valid token for `A` cannot
/// use it to register a descriptor claiming to be `B`, because [`SchemaCatalog::register`] checks
/// the descriptor against this field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRegistration {
    collector_id: String,
}

impl VerifiedRegistration {
    /// The collector identity this proof was issued for.
    #[must_use]
    pub fn collector_id(&self) -> &str {
        &self.collector_id
    }
}

/// Verify a presented spawn token against the one issued for `collector_id` (R8).
///
/// `issued` is the token the agent minted when it spawned this collector, or `None` when it never
/// spawned one under that identity. `presented` is the value the registration carried — the value,
/// never the path.
///
/// The comparison is constant-time via [`subtle::ConstantTimeEq`]. The length check in front of it
/// leaks nothing: [`SPAWN_TOKEN_HEX_LEN`] is a public constant, so a wrong-length token reveals
/// only what the attacker already knew.
///
/// # Errors
///
/// Returns the [`TokenRejection`] naming the specific gate that fired.
pub fn verify_spawn_token(
    collector_id: &str,
    issued: Option<&str>,
    presented: Option<&str>,
) -> Result<VerifiedRegistration, TokenRejection> {
    let Some(presented_token) = presented else {
        return Err(TokenRejection::NoTokenPresented);
    };
    let Some(issued_token) = issued else {
        return Err(TokenRejection::UnknownCollector);
    };
    if presented_token.len() != SPAWN_TOKEN_HEX_LEN || issued_token.len() != SPAWN_TOKEN_HEX_LEN {
        return Err(TokenRejection::TokenMismatch);
    }
    if bool::from(presented_token.as_bytes().ct_eq(issued_token.as_bytes())) {
        return Ok(VerifiedRegistration {
            collector_id: collector_id.to_owned(),
        });
    }
    Err(TokenRejection::TokenMismatch)
}

/// Why a descriptor was refused before it could be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogError {
    /// The descriptor claims a different collector than the verified token was issued for.
    IdentityMismatch {
        /// Identity the token authenticated.
        verified: String,
        /// Identity the descriptor claimed.
        claimed: String,
    },
    /// The descriptor names more tables than [`MAX_TABLES_PER_DESCRIPTOR`] allows.
    TooManyTables {
        /// Number of tables the descriptor carried.
        count: usize,
    },
    /// A table declares more columns than [`MAX_COLUMNS_PER_TABLE`] allows.
    TooManyColumns {
        /// The offending table.
        table: String,
        /// Number of columns it carried.
        count: usize,
    },
    /// A column declares more operations than [`MAX_OPERATIONS_PER_COLUMN`] allows.
    TooManyOperations {
        /// The offending table.
        table: String,
        /// The offending column.
        column: String,
        /// Number of operations it carried.
        count: usize,
    },
    /// An identifier exceeds [`MAX_IDENTIFIER_LENGTH`] bytes.
    IdentifierTooLong {
        /// The identifier, truncated for display.
        identifier: String,
        /// Its byte length.
        length: usize,
    },
    /// The registration carried more conformance results than [`MAX_CONFORMANCE_RESULTS`].
    TooManyConformanceResults {
        /// Number of results it carried.
        count: usize,
    },
    /// Two tables in one descriptor, or two columns in one table, share a name.
    DuplicateIdentifier {
        /// The repeated identifier.
        identifier: String,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::IdentityMismatch {
                ref verified,
                ref claimed,
            } => write!(
                formatter,
                "descriptor claims collector `{claimed}` but the token authenticated `{verified}`"
            ),
            Self::TooManyTables { count } => write!(
                formatter,
                "descriptor names {count} tables, over the limit of {MAX_TABLES_PER_DESCRIPTOR}"
            ),
            Self::TooManyColumns { ref table, count } => write!(
                formatter,
                "table `{table}` declares {count} columns, over the limit of {MAX_COLUMNS_PER_TABLE}"
            ),
            Self::TooManyOperations {
                ref table,
                ref column,
                count,
            } => write!(
                formatter,
                "column `{table}.{column}` declares {count} operations, over the limit of {MAX_OPERATIONS_PER_COLUMN}"
            ),
            Self::IdentifierTooLong {
                ref identifier,
                length,
            } => write!(
                formatter,
                "identifier `{identifier}` is {length} bytes, over the limit of {MAX_IDENTIFIER_LENGTH}"
            ),
            Self::TooManyConformanceResults { count } => write!(
                formatter,
                "registration carries {count} conformance results, over the limit of {MAX_CONFORMANCE_RESULTS}"
            ),
            Self::DuplicateIdentifier { ref identifier } => {
                write!(formatter, "identifier `{identifier}` is declared twice")
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// A table or column reference a rule made that the catalog cannot resolve (R11).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnknownReference {
    /// No registered collector serves this table.
    Table {
        /// The table the rule named.
        table: String,
    },
    /// The table exists but declares no such column.
    Column {
        /// The table the rule named.
        table: String,
        /// The column the rule named.
        column: String,
    },
}

impl fmt::Display for UnknownReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Table { ref table } => {
                write!(formatter, "no registered collector serves table `{table}`")
            }
            Self::Column {
                ref table,
                ref column,
            } => write!(formatter, "table `{table}` declares no column `{column}`"),
        }
    }
}

impl std::error::Error for UnknownReference {}

/// What a registration changed about the catalog, so the agent knows which rules to revisit (R12).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogChange {
    first_registration: bool,
    tables: BTreeSet<String>,
    removed: BTreeSet<(String, String)>,
}

impl CatalogChange {
    /// Whether this collector had never registered before.
    ///
    /// First registration widens the catalog, so rules that were waiting on an empty catalog must
    /// be re-planned even though nothing was removed.
    #[must_use]
    pub const fn is_first_registration(&self) -> bool {
        self.first_registration
    }

    /// Every table this registration added, removed, or altered.
    #[must_use]
    pub const fn affected_tables(&self) -> &BTreeSet<String> {
        &self.tables
    }

    /// `(table, column)` references that existed before this registration and no longer do.
    ///
    /// A rule filtering on one of these can no longer validate, which is what makes it unhealthy.
    #[must_use]
    pub const fn removed_references(&self) -> &BTreeSet<(String, String)> {
        &self.removed
    }

    /// Whether this registration changed nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.first_registration && self.tables.is_empty()
    }
}

/// Key for the per-operation conformance-pass slot (R15).
type ConformanceKey = (String, String, String, i32);

/// The tables and columns registered collectors can currently serve.
///
/// A descriptor only enters through [`Self::register`], which demands a [`VerifiedRegistration`].
#[derive(Debug, Default)]
pub struct SchemaCatalog {
    /// Descriptor per collector identity, so a re-registration replaces exactly its own tables.
    descriptors: BTreeMap<String, SchemaDescriptor>,
    /// Table name to the collector that owns it.
    owners: BTreeMap<String, String>,
    /// Operations a conformance vector has passed for, keyed by
    /// `(collector_id, table, column, op)`. Separate from the advertised set on purpose: R15 wants
    /// "advertised" and "verified" as two facts, so a collector can advertise ahead of proof.
    /// U6 owns this slot; U10 is its real-world writer.
    conformance: BTreeSet<ConformanceKey>,
}

impl SchemaCatalog {
    /// Create an empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any collector has registered.
    ///
    /// R18 keeps rule load from running against an empty catalog; this is the check it makes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// Store `descriptor` on behalf of the collector `verified` authenticates (R10).
    ///
    /// The descriptor is bounds-checked in full before anything is stored, so an authenticated but
    /// faulty collector cannot drive planning work with a pathological schema, and a rejected
    /// descriptor leaves the previous one intact.
    ///
    /// # Errors
    ///
    /// Returns a [`CatalogError`] naming the specific bound or identity check that refused it.
    pub fn register(
        &mut self,
        verified: &VerifiedRegistration,
        descriptor: SchemaDescriptor,
    ) -> Result<CatalogChange, CatalogError> {
        let collector_id = verified.collector_id();
        if descriptor.collector_id != collector_id {
            return Err(CatalogError::IdentityMismatch {
                verified: collector_id.to_owned(),
                claimed: descriptor.collector_id,
            });
        }
        validate_descriptor(&descriptor)?;

        // Lifted off the descriptor before it is moved into the map below. R22 puts the results on
        // the same authenticated exchange as the descriptor precisely so they cannot be separated.
        let mut stored = descriptor;
        let results = std::mem::take(&mut stored.conformance_results);
        let change = self.diff(collector_id, &stored);
        // Read while the previous descriptor is still in the map. `change` only compares column
        // *names*, so a descriptor that retyped a column, flipped its nullability or altered its
        // advertised operations while keeping every name produces an empty change — and the
        // semantics a pass was produced against would have moved underneath it. The version is
        // the collector's own statement that they did; `schema.rs` documents bumping it for
        // exactly those edits.
        let version_changed = self
            .descriptors
            .get(collector_id)
            .is_none_or(|previous| previous.descriptor_version != stored.descriptor_version);

        for table in self
            .descriptors
            .get(collector_id)
            .map(|previous| &previous.tables)
            .into_iter()
            .flatten()
        {
            self.owners.remove(&table.name);
        }
        for table in &stored.tables {
            let _previous = self
                .owners
                .insert(table.name.clone(), collector_id.to_owned());
        }
        let _previous = self.descriptors.insert(collector_id.to_owned(), stored);
        // A conformance result is bound to the `descriptor_version` it was produced against, so a
        // descriptor that changed at all — by version or by shape — invalidates every result this
        // collector had. Evicting only results for tables it dropped would let an operation whose
        // *semantics* changed carry a stale pass forward, and `is_pushable` would answer for a
        // proof that no longer exists.
        if version_changed || !change.is_empty() {
            self.conformance.retain(|entry| entry.0 != collector_id);
        }
        // *After* the eviction, so a result riding on this descriptor is never wiped by the very
        // registration that stored it. Re-recording an entry the eviction left alone is a no-op:
        // the set is keyed, not counted.
        self.record_carried_results(collector_id, &results);

        Ok(change)
    }

    /// Record the passes a registration carried, ignoring anything its descriptor does not
    /// advertise (R15, R22).
    ///
    /// A `passed = false` result records nothing: R15 treats an operation with no passing result
    /// exactly as it treats an unadvertised one. A result naming a `table.column.op` outside the
    /// descriptor just stored is dropped rather than refused — it grants nothing either way,
    /// because [`Self::is_pushable`] also demands the advertisement.
    fn record_carried_results(&mut self, collector_id: &str, results: &[ConformanceResult]) {
        for result in results {
            if !result.passed {
                continue;
            }
            let op = result.op();
            if !self.is_advertised(&result.table, &result.column, op) {
                continue;
            }
            if self.owners.get(&result.table).map(String::as_str) != Some(collector_id) {
                continue;
            }
            self.record_conformance_pass(collector_id, &result.table, &result.column, op);
        }
    }

    /// Look up a table by the name a rule addresses it with.
    #[must_use]
    pub fn table(&self, table: &str) -> Option<&TableDescriptor> {
        let owner = self.owners.get(table)?;
        self.descriptors
            .get(owner)?
            .tables
            .iter()
            .find(|candidate| candidate.name == table)
    }

    /// Look up a column within a table.
    #[must_use]
    pub fn column(&self, table: &str, column: &str) -> Option<&ColumnDescriptor> {
        self.table(table)?
            .columns
            .iter()
            .find(|candidate| candidate.name == column)
    }

    /// The collector that serves `table`, if any.
    #[must_use]
    pub fn owner_of(&self, table: &str) -> Option<&str> {
        self.owners.get(table).map(String::as_str)
    }

    /// Resolve a `table.column` reference, naming the specific unknown part when it fails (R11).
    ///
    /// # Errors
    ///
    /// Returns [`UnknownReference`] identifying the table or the column that is not in the catalog.
    pub fn resolve_reference(
        &self,
        table: &str,
        column: &str,
    ) -> Result<&ColumnDescriptor, UnknownReference> {
        let Some(descriptor) = self.table(table) else {
            return Err(UnknownReference::Table {
                table: table.to_owned(),
            });
        };
        descriptor
            .columns
            .iter()
            .find(|candidate| candidate.name == column)
            .ok_or_else(|| UnknownReference::Column {
                table: table.to_owned(),
                column: column.to_owned(),
            })
    }

    /// Whether the owning collector *claims* it can evaluate `op` on this column.
    #[must_use]
    pub fn is_advertised(&self, table: &str, column: &str, op: PredicateOp) -> bool {
        let op_value = i32::from(op);
        self.column(table, column)
            .is_some_and(|descriptor| descriptor.supported_ops.contains(&op_value))
    }

    /// Whether a conformance vector has passed for `op` on this column.
    ///
    /// The slot U10 writes. Read separately from [`Self::is_advertised`] so the two facts R15
    /// distinguishes never collapse into one.
    #[must_use]
    pub fn is_conformance_passed(&self, table: &str, column: &str, op: PredicateOp) -> bool {
        let Some(owner) = self.owners.get(table) else {
            return false;
        };
        self.conformance.contains(&(
            owner.clone(),
            table.to_owned(),
            column.to_owned(),
            i32::from(op),
        ))
    }

    /// Whether a predicate over `op` on this column may be pushed (R15).
    ///
    /// Both facts must hold: advertised *and* conformance-passed. Anything else stays in the
    /// residual.
    #[must_use]
    pub fn is_pushable(&self, table: &str, column: &str, op: PredicateOp) -> bool {
        self.is_advertised(table, column, op) && self.is_conformance_passed(table, column, op)
    }

    /// Record that a conformance vector passed for one advertised operation.
    ///
    /// U10 calls this with the results a collector carried on its authenticated registration.
    pub fn record_conformance_pass(
        &mut self,
        collector_id: &str,
        table: &str,
        column: &str,
        op: PredicateOp,
    ) {
        let _inserted = self.conformance.insert((
            collector_id.to_owned(),
            table.to_owned(),
            column.to_owned(),
            i32::from(op),
        ));
    }

    /// What `descriptor` would change about this collector's existing entry.
    fn diff(&self, collector_id: &str, descriptor: &SchemaDescriptor) -> CatalogChange {
        let Some(previous) = self.descriptors.get(collector_id) else {
            return CatalogChange {
                first_registration: true,
                tables: descriptor
                    .tables
                    .iter()
                    .map(|table| table.name.clone())
                    .collect(),
                removed: BTreeSet::new(),
            };
        };

        let before = reference_set(previous);
        let after = reference_set(descriptor);
        let removed: BTreeSet<(String, String)> = before.difference(&after).cloned().collect();
        let tables: BTreeSet<String> = before
            .symmetric_difference(&after)
            .map(|entry| entry.0.clone())
            .collect();

        CatalogChange {
            first_registration: false,
            tables,
            removed,
        }
    }
}

/// Every `(table, column)` pair a descriptor declares.
fn reference_set(descriptor: &SchemaDescriptor) -> BTreeSet<(String, String)> {
    descriptor
        .tables
        .iter()
        .flat_map(|table| {
            table
                .columns
                .iter()
                .map(move |column| (table.name.clone(), column.name.clone()))
        })
        .collect()
}

/// Reject a descriptor exceeding any configured bound, before a single entry is stored.
///
/// # Errors
///
/// Returns the [`CatalogError`] naming the first bound that was exceeded.
fn validate_descriptor(descriptor: &SchemaDescriptor) -> Result<(), CatalogError> {
    check_identifier(&descriptor.collector_id)?;
    check_identifier(&descriptor.descriptor_version)?;

    if descriptor.conformance_results.len() > MAX_CONFORMANCE_RESULTS {
        return Err(CatalogError::TooManyConformanceResults {
            count: descriptor.conformance_results.len(),
        });
    }

    if descriptor.tables.len() > MAX_TABLES_PER_DESCRIPTOR {
        return Err(CatalogError::TooManyTables {
            count: descriptor.tables.len(),
        });
    }

    let mut seen_tables = BTreeSet::new();
    for table in &descriptor.tables {
        check_identifier(&table.name)?;
        if !seen_tables.insert(table.name.as_str()) {
            return Err(CatalogError::DuplicateIdentifier {
                identifier: table.name.clone(),
            });
        }
        check_table(table)?;
    }
    Ok(())
}

/// Bounds-check one table's columns and their advertised operations.
fn check_table(table: &TableDescriptor) -> Result<(), CatalogError> {
    if table.columns.len() > MAX_COLUMNS_PER_TABLE {
        return Err(CatalogError::TooManyColumns {
            table: table.name.clone(),
            count: table.columns.len(),
        });
    }

    let mut seen_columns = BTreeSet::new();
    for column in &table.columns {
        check_identifier(&column.name)?;
        if !seen_columns.insert(column.name.as_str()) {
            return Err(CatalogError::DuplicateIdentifier {
                identifier: column.name.clone(),
            });
        }
        if column.supported_ops.len() > MAX_OPERATIONS_PER_COLUMN {
            return Err(CatalogError::TooManyOperations {
                table: table.name.clone(),
                column: column.name.clone(),
                count: column.supported_ops.len(),
            });
        }
    }
    Ok(())
}

/// Reject an identifier longer than [`MAX_IDENTIFIER_LENGTH`] bytes.
///
/// The length compared is the byte length, and the error's copy of the identifier is truncated on
/// a character boundary rather than sliced, per the CWE-135 lesson in
/// `docs/solutions/security-issues/binary-hashing-authorization-and-toctou-fixes.md`.
fn check_identifier(identifier: &str) -> Result<(), CatalogError> {
    let length = identifier.len();
    if length <= MAX_IDENTIFIER_LENGTH {
        return Ok(());
    }
    Err(CatalogError::IdentifierTooLong {
        identifier: identifier.chars().take(32).collect(),
        length,
    })
}
