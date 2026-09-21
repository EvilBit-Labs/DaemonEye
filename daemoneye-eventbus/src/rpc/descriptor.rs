//! Collector schema descriptors carried on the registration exchange.
//!
//! These types mirror the protobuf messages of the same names in
//! `daemoneye-lib/proto/common.proto` (`SchemaDescriptor`, `TableDescriptor`,
//! `ColumnDescriptor`, `PredicateOp`, `ColumnType`). Field names and semantics
//! are identical on both sides; the mirror exists because the eventbus RPC
//! surface is plain serde JSON and this crate does not depend on `prost`.
//! Change one side and change the other.

use serde::{Deserialize, Serialize};

/// Comparison operation a collector can claim support for on a column.
///
/// Mirrors `PredicateOp` in `common.proto`, with one deliberate difference in
/// unknown-value handling: prost keeps an unrecognized proto enum as a raw
/// `i32`, while this type folds an unrecognized JSON string into
/// [`PredicateOp::Unspecified`]. The name is lost, the effect is the same — an
/// operation the agent cannot name is not pushable, so predicates over it stay
/// in the residual. Folding rather than erroring matters: a newer collector
/// advertising an op this build predates must still register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum PredicateOp {
    /// Unset or unrecognized. Never treat as a comparison.
    #[default]
    Unspecified,
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `IN (...)`
    In,
    /// SQL `LIKE` pattern match.
    Like,
    /// Regular-expression match.
    Regexp,
}

/// Declared type of a catalog column.
///
/// Mirrors `ColumnType` in `common.proto`. An unrecognized JSON string folds
/// into [`ColumnType::Unspecified`] rather than failing the whole
/// registration, for the same reason as [`PredicateOp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum ColumnType {
    /// Unset or unrecognized.
    #[default]
    Unspecified,
    /// UTF-8 text.
    String,
    /// Signed 64-bit integer.
    Int,
    /// Unsigned 64-bit integer.
    Uint,
    /// 64-bit float.
    Float,
    /// Boolean.
    Bool,
}

/// One column a collector declares it can serve.
///
/// Mirrors `ColumnDescriptor` in `common.proto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDescriptor {
    /// Column name, unique within its table.
    pub name: String,
    /// Declared type of the column's values.
    pub column_type: ColumnType,
    /// Whether the column can carry SQL NULL.
    pub nullable: bool,
    /// Operations the collector claims it can evaluate for this column. A
    /// predicate over an operation not listed here stays in the residual.
    pub supported_ops: Vec<PredicateOp>,
}

/// One table a collector declares.
///
/// Mirrors `TableDescriptor` in `common.proto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDescriptor {
    /// Table name as rules address it.
    pub name: String,
    /// Columns the table exposes.
    pub columns: Vec<ColumnDescriptor>,
}

/// Everything a collector advertises about the data it can serve.
///
/// Mirrors `SchemaDescriptor` in `common.proto`. Conformance-vector results
/// attach here in a later unit so a result is always bound to the collector and
/// the `descriptor_version` it was produced against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDescriptor {
    /// Collector that owns these tables.
    pub collector_id: String,
    /// Opaque version of this descriptor; compiled rules and conformance
    /// results are bound to it.
    pub descriptor_version: String,
    /// Tables the collector can serve.
    pub tables: Vec<TableDescriptor>,
}

impl PredicateOp {
    /// Wire name of this operation, matching the `common.proto` enum value.
    #[must_use]
    pub const fn as_wire_name(self) -> &'static str {
        match self {
            Self::Unspecified => "PREDICATE_OP_UNSPECIFIED",
            Self::Eq => "PREDICATE_OP_EQ",
            Self::Ne => "PREDICATE_OP_NE",
            Self::Lt => "PREDICATE_OP_LT",
            Self::Le => "PREDICATE_OP_LE",
            Self::Gt => "PREDICATE_OP_GT",
            Self::Ge => "PREDICATE_OP_GE",
            Self::In => "PREDICATE_OP_IN",
            Self::Like => "PREDICATE_OP_LIKE",
            Self::Regexp => "PREDICATE_OP_REGEXP",
        }
    }
}

impl From<String> for PredicateOp {
    /// Unrecognized names fold to [`PredicateOp::Unspecified`]; see the type docs.
    fn from(value: String) -> Self {
        match value.as_str() {
            "PREDICATE_OP_EQ" => Self::Eq,
            "PREDICATE_OP_NE" => Self::Ne,
            "PREDICATE_OP_LT" => Self::Lt,
            "PREDICATE_OP_LE" => Self::Le,
            "PREDICATE_OP_GT" => Self::Gt,
            "PREDICATE_OP_GE" => Self::Ge,
            "PREDICATE_OP_IN" => Self::In,
            "PREDICATE_OP_LIKE" => Self::Like,
            "PREDICATE_OP_REGEXP" => Self::Regexp,
            _unrecognized => Self::Unspecified,
        }
    }
}

impl From<PredicateOp> for String {
    fn from(value: PredicateOp) -> Self {
        value.as_wire_name().to_owned()
    }
}

impl ColumnType {
    /// Wire name of this column type, matching the `common.proto` enum value.
    #[must_use]
    pub const fn as_wire_name(self) -> &'static str {
        match self {
            Self::Unspecified => "COLUMN_TYPE_UNSPECIFIED",
            Self::String => "COLUMN_TYPE_STRING",
            Self::Int => "COLUMN_TYPE_INT",
            Self::Uint => "COLUMN_TYPE_UINT",
            Self::Float => "COLUMN_TYPE_FLOAT",
            Self::Bool => "COLUMN_TYPE_BOOL",
        }
    }
}

impl From<String> for ColumnType {
    /// Unrecognized names fold to [`ColumnType::Unspecified`]; see the type docs.
    fn from(value: String) -> Self {
        match value.as_str() {
            "COLUMN_TYPE_STRING" => Self::String,
            "COLUMN_TYPE_INT" => Self::Int,
            "COLUMN_TYPE_UINT" => Self::Uint,
            "COLUMN_TYPE_FLOAT" => Self::Float,
            "COLUMN_TYPE_BOOL" => Self::Bool,
            _unrecognized => Self::Unspecified,
        }
    }
}

impl From<ColumnType> for String {
    fn from(value: ColumnType) -> Self {
        value.as_wire_name().to_owned()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::rpc::RegistrationRequest;

    #[test]
    fn registration_request_without_the_new_fields_still_deserializes() {
        // Arrange: JSON from a collector built before descriptor/spawn_token existed.
        let json = r#"{"collector_id":"procmond","collector_type":"procmond",
            "hostname":"localhost","version":null,"pid":null,
            "capabilities":[],"attributes":{},"heartbeat_interval_ms":null}"#;

        // Act
        let parsed: RegistrationRequest =
            serde_json::from_str(json).expect("legacy registration JSON must still parse");

        // Assert
        assert!(parsed.descriptor.is_none());
        assert!(parsed.spawn_token.is_none());
    }

    #[test]
    fn unknown_predicate_op_folds_to_unspecified_instead_of_failing() {
        // Arrange: an op name a newer collector might advertise.
        let json = "\"PREDICATE_OP_BETWEEN\"";

        // Act
        let parsed: PredicateOp = serde_json::from_str(json).expect("unknown op must not fail");

        // Assert
        assert_eq!(parsed, PredicateOp::Unspecified);
    }

    #[test]
    fn unknown_column_type_folds_to_unspecified_instead_of_failing() {
        let parsed: ColumnType =
            serde_json::from_str("\"COLUMN_TYPE_BYTES\"").expect("unknown type must not fail");
        assert_eq!(parsed, ColumnType::Unspecified);
    }

    #[test]
    fn every_predicate_op_round_trips_through_its_wire_name() {
        // Arrange
        let ops = [
            PredicateOp::Unspecified,
            PredicateOp::Eq,
            PredicateOp::Ne,
            PredicateOp::Lt,
            PredicateOp::Le,
            PredicateOp::Gt,
            PredicateOp::Ge,
            PredicateOp::In,
            PredicateOp::Like,
            PredicateOp::Regexp,
        ];

        // Act + Assert
        for op in ops {
            let encoded = serde_json::to_string(&op).expect("serialize");
            let decoded: PredicateOp = serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(decoded, op, "round trip failed for {op:?}");
        }
    }

    #[test]
    fn every_column_type_round_trips_through_its_wire_name() {
        let types = [
            ColumnType::Unspecified,
            ColumnType::String,
            ColumnType::Int,
            ColumnType::Uint,
            ColumnType::Float,
            ColumnType::Bool,
        ];
        for column_type in types {
            let encoded = serde_json::to_string(&column_type).expect("serialize");
            let decoded: ColumnType = serde_json::from_str(&encoded).expect("deserialize");
            assert_eq!(
                decoded, column_type,
                "round trip failed for {column_type:?}"
            );
        }
    }
}
