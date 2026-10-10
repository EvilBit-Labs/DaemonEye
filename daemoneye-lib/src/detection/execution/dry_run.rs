//! A load-time planning pass (R17).
//!
//! The load-time gates refuse what they can name: the function allowlist, the parser-level
//! constructs, the regex operators, the literal kinds. What they cannot enumerate is everything
//! the executor's session will refuse to *plan*: a registered function over a column of the wrong
//! type (`length(pid)`), a `BETWEEN` whose bounds do not coerce. So the rule is planned once here,
//! over an empty table of the catalog's declared shape, through the same `derive` the executor
//! runs every cycle, and a rule that cannot be planned is refused at load instead of degrading
//! every evaluation with an execution error.

use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;

use crate::detection::RegexCache;
use crate::detection::execution::derive::{CycleWindow, DeriveError, TIME_COLUMN, derive};
use crate::detection::execution::session::{ExecutorRuntime, LatencySink, session_state};
use crate::detection::planner::CompiledRule;
use crate::proto::{ColumnType, TableDescriptor};
use crate::storage::provider::arrow::arrow_type;

/// Plan `compiled` over an empty `table` under the executor's session, reading no row.
///
/// # Errors
///
/// [`DeriveError`] when the session cannot derive or plan the rule, or a column of `table` has
/// no plannable declared type.
pub fn check_plannable(
    compiled: &CompiledRule,
    table: &TableDescriptor,
    regex_cache: Arc<RegexCache>,
) -> Result<(), DeriveError> {
    let provider = Arc::new(MemTable::try_new(
        declared_schema(table)?,
        vec![Vec::new()],
    )?);
    let state = session_state(
        ExecutorRuntime::new()?.env(),
        Arc::new(LatencySink::default()),
        regex_cache,
    )?;
    let ctx = SessionContext::new_with_state(state);
    let empty = CycleWindow {
        after_ms: 0,
        through_ms: 0,
    };
    let frame = derive(&ctx, provider, compiled, empty, 1)?;
    // The analyzer (type coercion included) runs here, so a predicate the logical builder admits
    // but the planner would refuse is caught too.
    let _plan = frame.into_optimized_plan()?;
    Ok(())
}

/// The table's Arrow schema from its declared column types alone.
///
/// Not `schema_for`: that one also checks each column is backed by the record encoder, which is
/// the provider's concern. Planning needs only the declared types a rule is written against,
/// plus the window column every derived plan filters on, which the provider always carries
/// whether or not a descriptor names it.
fn declared_schema(table: &TableDescriptor) -> Result<SchemaRef, DeriveError> {
    let mut fields = table
        .columns
        .iter()
        .map(|column| {
            let data_type = ColumnType::try_from(column.column_type)
                .ok()
                .and_then(arrow_type)
                .ok_or_else(|| DeriveError::UnknownColumnType {
                    column: column.name.clone(),
                })?;
            Ok(Field::new(&column.name, data_type, column.nullable))
        })
        .collect::<Result<Vec<Field>, DeriveError>>()?;
    if !fields.iter().any(|field| field.name() == TIME_COLUMN) {
        fields.push(Field::new(TIME_COLUMN, DataType::Int64, false));
    }
    Ok(Arc::new(Schema::new(fields)))
}
