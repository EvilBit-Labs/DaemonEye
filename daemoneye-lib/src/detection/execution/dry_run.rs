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

use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;

use crate::detection::RegexCache;
use crate::detection::execution::derive::{CycleWindow, DeriveError, derive};
use crate::detection::execution::session::{ExecutorRuntime, LatencySink, session_state};
use crate::detection::planner::CompiledRule;
use crate::proto::TableDescriptor;
use crate::storage::provider::arrow::schema_for;

/// Plan `compiled` over an empty `table` under the executor's session, reading no row.
///
/// # Errors
///
/// [`DeriveError`] when the session cannot derive or plan the rule, or the provider could not
/// build `table`'s schema.
pub fn check_plannable(
    compiled: &CompiledRule,
    table: &TableDescriptor,
    regex_cache: Arc<RegexCache>,
) -> Result<(), DeriveError> {
    // Exactly the provider's schema, so load-time and runtime agree on every column: a descriptor
    // the provider cannot encode, or one without the window column, is refused here too.
    let schema = schema_for(table).map_err(|error| DeriveError::Schema {
        message: error.to_string(),
    })?;
    let provider = Arc::new(MemTable::try_new(schema, vec![Vec::new()])?);
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
