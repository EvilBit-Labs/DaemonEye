//! The schema procmond advertises, and how one process record's columns are read (R20).
//!
//! The descriptor here is the single source of what this collector will evaluate. Registration
//! advertises it and [`super::PushdownEvaluator`] validates against it, so a column or operation
//! appears in exactly one place and cannot drift between what is claimed and what is served.

use super::PushdownError;
use daemoneye_eventbus::rpc::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::proto::ProcessRecord;

/// The only table procmond serves.
pub const PROCESS_TABLE: &str = "processes";

/// Opaque version of the descriptor below.
///
/// Bump it whenever a column, type, nullability or advertised operation changes: compiled rules
/// and conformance results are bound to this string, and a silent change would leave a rule
/// planned against a schema the collector no longer serves.
pub const DESCRIPTOR_VERSION: &str = "processes-v1";

/// One process column's value, normalised to the descriptor's declared type.
///
/// Comparison is same-kind only. A literal of another kind is a refusal rather than a coercion,
/// because a coercion the agent would perform differently is exactly what loses rows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FieldValue {
    /// A signed 64-bit column.
    Int(i64),
    /// An unsigned 64-bit column.
    Uint(u64),
    /// A 64-bit floating-point column.
    Float(f64),
    /// A boolean column.
    Bool(bool),
    /// A UTF-8 text column.
    Str(String),
}

/// Ordering operations every scalar column supports.
const SCALAR_OPS: [PredicateOp; 7] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::In,
];

/// Operations a text column supports. Ordering comparisons on text are byte-wise and rarely what
/// a rule means, so they are not advertised.
const TEXT_OPS: [PredicateOp; 5] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::In,
    PredicateOp::Like,
    PredicateOp::Regexp,
];

/// Operations a boolean column supports.
const BOOL_OPS: [PredicateOp; 2] = [PredicateOp::Eq, PredicateOp::Ne];

/// Builds one column descriptor.
fn column(
    name: &str,
    column_type: ColumnType,
    nullable: bool,
    supported_ops: &[PredicateOp],
) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type,
        nullable,
        supported_ops: supported_ops.to_vec(),
    }
}

/// Everything procmond claims it can serve, owned by `collector_id`.
///
/// The same value is advertised at registration and used to validate pushed tasks; binding a rule
/// to one descriptor and validating against another is how a task gets accepted for a schema the
/// collector never had.
#[must_use]
pub fn process_schema_descriptor(collector_id: &str) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: collector_id.to_owned(),
        descriptor_version: DESCRIPTOR_VERSION.to_owned(),
        tables: vec![TableDescriptor {
            name: PROCESS_TABLE.to_owned(),
            columns: vec![
                column("pid", ColumnType::Uint, false, &SCALAR_OPS),
                column("ppid", ColumnType::Uint, true, &SCALAR_OPS),
                column("name", ColumnType::String, false, &TEXT_OPS),
                column("executable_path", ColumnType::String, true, &TEXT_OPS),
                column("command_line", ColumnType::String, true, &TEXT_OPS),
                column("start_time", ColumnType::Int, true, &SCALAR_OPS),
                column("cpu_usage", ColumnType::Float, true, &SCALAR_OPS),
                column("memory_usage", ColumnType::Uint, true, &SCALAR_OPS),
                column("executable_hash", ColumnType::String, true, &TEXT_OPS),
                column("user_id", ColumnType::String, true, &TEXT_OPS),
                column("accessible", ColumnType::Bool, false, &BOOL_OPS),
                column("file_exists", ColumnType::Bool, false, &BOOL_OPS),
                column("collection_time", ColumnType::Int, false, &SCALAR_OPS),
            ],
        }],
    }
}

/// Reads one declared column off a process record.
///
/// `Ok(None)` is SQL NULL, which is a value the record genuinely lacks. A column this build does
/// not serve is an error rather than `None`, so an unadvertised name can never be read as NULL and
/// quietly turn every comparison over it into a non-match.
///
/// # Errors
///
/// Returns [`PushdownError::Identity`] naming the unadvertised column.
pub fn field_value(
    record: &ProcessRecord,
    column_name: &str,
) -> Result<Option<FieldValue>, PushdownError> {
    let value = match column_name {
        "pid" => Some(FieldValue::Uint(u64::from(record.pid))),
        "ppid" => record.ppid.map(|ppid| FieldValue::Uint(u64::from(ppid))),
        "name" => Some(FieldValue::Str(record.name.clone())),
        "executable_path" => record.executable_path.clone().map(FieldValue::Str),
        // A process with no readable argument vector has no command line, which is NULL rather
        // than the empty string: an empty string would match `command_line = ''`.
        "command_line" => {
            if record.command_line.is_empty() {
                None
            } else {
                Some(FieldValue::Str(record.command_line.join(" ")))
            }
        }
        "start_time" => record.start_time.map(FieldValue::Int),
        "cpu_usage" => record.cpu_usage.map(FieldValue::Float),
        "memory_usage" => record.memory_usage.map(FieldValue::Uint),
        "executable_hash" => record.executable_hash.clone().map(FieldValue::Str),
        "user_id" => record.user_id.clone().map(FieldValue::Str),
        "accessible" => Some(FieldValue::Bool(record.accessible)),
        "file_exists" => Some(FieldValue::Bool(record.file_exists)),
        "collection_time" => Some(FieldValue::Int(record.collection_time)),
        unadvertised => return Err(PushdownError::unknown_column(unadvertised)),
    };
    Ok(value)
}
