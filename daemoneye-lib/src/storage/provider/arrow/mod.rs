//! Catalog descriptor to Arrow schema, and `ProcessRecord` slices to `RecordBatch`es (KTD12).
//!
//! The descriptor is the authority for names, order and nullability; the encoder only has to
//! back every column it names. [`schema_for`] refuses a descriptor the encoder cannot back, so
//! an advertised column is never silently null.
//!
//! Types: `String` is `Utf8`, `Int` is `Int64`, `Uint` is `UInt64`, `Float` is `Float64`, `Bool`
//! is `Boolean`. `user_id` renders as text, `start_time` is seconds since the Unix epoch and
//! `collection_time` is milliseconds, exactly as the wire carries them, so the pushed and residual
//! halves of a rule agree. Arrow is reached only through `datafusion::arrow`.

use crate::models::process::ProcessRecord;
use crate::proto::{
    ColumnType, METADATA_KEY_ACCESSIBLE, METADATA_KEY_FILE_EXISTS, TableDescriptor,
};
use datafusion::arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::error::ArrowError;
use datafusion::arrow::record_batch::{RecordBatch, RecordBatchOptions};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a descriptor cannot be encoded, or a batch could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArrowEncodeError {
    /// The descriptor names a column the encoder has no source for.
    #[error("descriptor column `{0}` has no encoder")]
    UnbackedColumn(String),
    /// The descriptor declares a type other than the one the encoder produces for the column.
    #[error("column `{column}` is declared as {declared:?} but the encoder produces {backed:?}")]
    TypeMismatch {
        /// Column name.
        column: String,
        /// Type the descriptor declares.
        declared: ColumnType,
        /// Type the encoder produces.
        backed: ColumnType,
    },
    /// The descriptor's type is `Unspecified` or a value this build does not recognize.
    #[error("column `{column}` has unusable declared type {raw}")]
    UnknownColumnType {
        /// Column name.
        column: String,
        /// Raw wire value.
        raw: i32,
    },
    /// A projection index past the end of the schema.
    #[error("projection index {index} out of range for a schema of {width} columns")]
    ProjectionOutOfRange {
        /// Offending index.
        index: usize,
        /// Columns in the schema.
        width: usize,
    },
    /// A `start_time` whose seconds since the epoch do not fit `i64`.
    #[error("start_time seconds since the Unix epoch do not fit in i64")]
    StartTimeOutOfRange,
    /// Arrow rejected the batch, for example a null in a column the descriptor made non-nullable.
    #[error("arrow batch construction failed: {0}")]
    Arrow(#[from] ArrowError),
}

/// The columns the encoder can back, one per `ProcessRecord` source.
#[derive(Debug, Clone, Copy)]
enum Col {
    Pid,
    Ppid,
    Name,
    ExecutablePath,
    CommandLine,
    StartTime,
    CpuUsage,
    MemoryUsage,
    ExecutableHash,
    UserId,
    Accessible,
    FileExists,
    CollectionTime,
}

impl Col {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pid" => Self::Pid,
            "ppid" => Self::Ppid,
            "name" => Self::Name,
            "executable_path" => Self::ExecutablePath,
            "command_line" => Self::CommandLine,
            "start_time" => Self::StartTime,
            "cpu_usage" => Self::CpuUsage,
            "memory_usage" => Self::MemoryUsage,
            "executable_hash" => Self::ExecutableHash,
            "user_id" => Self::UserId,
            "accessible" => Self::Accessible,
            "file_exists" => Self::FileExists,
            "collection_time" => Self::CollectionTime,
            _ => return None,
        })
    }

    const fn column_type(self) -> ColumnType {
        match self {
            Self::Pid | Self::Ppid | Self::MemoryUsage => ColumnType::Uint,
            Self::Name
            | Self::ExecutablePath
            | Self::CommandLine
            | Self::ExecutableHash
            | Self::UserId => ColumnType::String,
            Self::StartTime | Self::CollectionTime => ColumnType::Int,
            Self::CpuUsage => ColumnType::Float,
            Self::Accessible | Self::FileExists => ColumnType::Bool,
        }
    }

    fn build(self, records: &[ProcessRecord], field: &Field) -> Result<ArrayRef, ArrowEncodeError> {
        let flag = |key: &str, r: &ProcessRecord| match r.metadata.get(key).map(String::as_str) {
            Some("true") => Some(true),
            Some("false") => Some(false),
            // Absent (or unreadable): false when the column cannot be null, else NULL.
            _ => (!field.is_nullable()).then_some(false),
        };
        let strings = |f: &dyn for<'r> Fn(&'r ProcessRecord) -> Option<Cow<'r, str>>| -> ArrayRef {
            Arc::new(records.iter().map(f).collect::<StringArray>())
        };
        Ok(match self {
            Self::Pid => Arc::new(
                records
                    .iter()
                    .map(|r| Some(u64::from(r.pid.raw())))
                    .collect::<UInt64Array>(),
            ),
            Self::Ppid => Arc::new(
                records
                    .iter()
                    .map(|r| r.ppid.map(|p| u64::from(p.raw())))
                    .collect::<UInt64Array>(),
            ),
            Self::Name => strings(&|r| Some(Cow::Borrowed(r.name.as_str()))),
            Self::ExecutablePath => {
                strings(&|r| r.executable_path.as_ref().map(|p| p.to_string_lossy()))
            }
            Self::CommandLine => strings(&|r| r.command_line.as_deref().map(Cow::Borrowed)),
            Self::ExecutableHash => strings(&|r| r.executable_hash.as_deref().map(Cow::Borrowed)),
            Self::UserId => strings(&|r| r.user_id.map(|u| Cow::Owned(u.to_string()))),
            Self::StartTime => {
                let secs = records
                    .iter()
                    .map(|r| {
                        r.start_time
                            .map(|t| epoch_seconds(t).ok_or(ArrowEncodeError::StartTimeOutOfRange))
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Arc::new(Int64Array::from(secs))
            }
            Self::CpuUsage => Arc::new(
                records
                    .iter()
                    .map(|r| r.cpu_usage)
                    .collect::<Float64Array>(),
            ),
            Self::MemoryUsage => Arc::new(
                records
                    .iter()
                    .map(|r| r.memory_usage)
                    .collect::<UInt64Array>(),
            ),
            Self::Accessible => Arc::new(
                records
                    .iter()
                    .map(|r| flag(METADATA_KEY_ACCESSIBLE, r))
                    .collect::<BooleanArray>(),
            ),
            Self::FileExists => Arc::new(
                records
                    .iter()
                    .map(|r| flag(METADATA_KEY_FILE_EXISTS, r))
                    .collect::<BooleanArray>(),
            ),
            // `DateTime::timestamp_millis` is `i64` over chrono's whole date range: no overflow.
            Self::CollectionTime => Arc::new(
                records
                    .iter()
                    .map(|r| Some(r.collection_time.timestamp_millis()))
                    .collect::<Int64Array>(),
            ),
        })
    }
}

/// Seconds since the Unix epoch, flooring toward negative infinity.
///
/// A pre-epoch time is negative (a fraction rounds away from zero, so `-0.5s` is `-1`), matching
/// the floor a post-epoch time gets from `as_secs`. `None` when the magnitude exceeds `i64`.
#[must_use]
pub fn epoch_seconds(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => {
            let gap = before.duration();
            let whole = i64::try_from(gap.as_secs()).ok()?;
            whole
                .checked_add(i64::from(gap.subsec_nanos() > 0))?
                .checked_neg()
        }
    }
}

const fn arrow_type(column_type: ColumnType) -> Option<DataType> {
    match column_type {
        ColumnType::String => Some(DataType::Utf8),
        ColumnType::Int => Some(DataType::Int64),
        ColumnType::Uint => Some(DataType::UInt64),
        ColumnType::Float => Some(DataType::Float64),
        ColumnType::Bool => Some(DataType::Boolean),
        ColumnType::Unspecified => None,
    }
}

/// Build the Arrow schema for a catalog table, in descriptor order, with the descriptor's
/// nullability.
///
/// # Errors
///
/// [`ArrowEncodeError::UnbackedColumn`] for a column the encoder cannot back,
/// [`ArrowEncodeError::TypeMismatch`] when the declared type differs from what the encoder
/// produces, and [`ArrowEncodeError::UnknownColumnType`] for an unspecified or unrecognized type.
/// Call this once at provider construction so a bad descriptor surfaces there, not as a NULL
/// column at scan time.
pub fn schema_for(table: &TableDescriptor) -> Result<SchemaRef, ArrowEncodeError> {
    let fields = table
        .columns
        .iter()
        .map(|c| {
            let col = Col::from_name(&c.name)
                .ok_or_else(|| ArrowEncodeError::UnbackedColumn(c.name.clone()))?;
            let declared = ColumnType::try_from(c.column_type).map_err(|_unknown| {
                ArrowEncodeError::UnknownColumnType {
                    column: c.name.clone(),
                    raw: c.column_type,
                }
            })?;
            let data_type =
                arrow_type(declared).ok_or_else(|| ArrowEncodeError::UnknownColumnType {
                    column: c.name.clone(),
                    raw: c.column_type,
                })?;
            if declared != col.column_type() {
                return Err(ArrowEncodeError::TypeMismatch {
                    column: c.name.clone(),
                    declared,
                    backed: col.column_type(),
                });
            }
            Ok(Field::new(&c.name, data_type, c.nullable))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Arc::new(Schema::new(fields)))
}

/// Encode `records` as one `RecordBatch`, writing only the projected columns.
///
/// `projection` indexes into `schema` (as `DataFusion` hands it to a scan); `None` is every
/// column. The batch's row count always equals `records.len()`, including for an empty
/// projection. A repeated index yields the column twice.
///
/// # Errors
///
/// [`ArrowEncodeError::ProjectionOutOfRange`] for a bad index,
/// [`ArrowEncodeError::StartTimeOutOfRange`] for an unrepresentable `start_time`,
/// [`ArrowEncodeError::UnbackedColumn`] if `schema` was not built by [`schema_for`], and
/// [`ArrowEncodeError::Arrow`] when Arrow rejects the batch (a null in a non-nullable column).
pub fn encode(
    records: &[ProcessRecord],
    schema: &SchemaRef,
    projection: Option<&[usize]>,
) -> Result<RecordBatch, ArrowEncodeError> {
    let width = schema.fields().len();
    let indices: Vec<usize> = projection.map_or_else(|| (0..width).collect(), <[usize]>::to_vec);
    if let Some(&index) = indices.iter().find(|&&i| i >= width) {
        return Err(ArrowEncodeError::ProjectionOutOfRange { index, width });
    }
    let projected = Arc::new(schema.project(&indices)?);
    let columns = indices
        .iter()
        .map(|&i| {
            let field = schema.field(i);
            Col::from_name(field.name())
                .ok_or_else(|| ArrowEncodeError::UnbackedColumn(field.name().clone()))?
                .build(records, field)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let options = RecordBatchOptions::new().with_row_count(Some(records.len()));
    Ok(RecordBatch::try_new_with_options(
        projected, columns, &options,
    )?)
}

#[cfg(test)]
mod tests;
