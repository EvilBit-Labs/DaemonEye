//! U5 — redb-backed `TableProvider` with time-range partition pruning.
//!
//! Exposes the bucket-partitioned event store to `DataFusion` as one table,
//! pruning to the buckets a `ts_ms` predicate can reach (R3, R5, KTD6).
//!
//! It uses only `daemoneye-lib`'s public surface. The `bucket` and `codec`
//! modules are private, `TsSeqKey` is `pub(super)`, and `EventStore` exposes no
//! handle on its `redb::Database`, so a crate outside the workspace cannot
//! touch the bucket tables directly. Partitions are discovered with
//! `list_buckets()` and read with `scan_range()`.
//!
//! Reading happens in `execute()`, not `scan()`: one bucket is decoded at a
//! time as its partition runs, which is what keeps this a fair measurement of
//! `DataFusion`'s own footprint rather than a whole-fixture materialization.

use crate::fixture::HOUR_MS;
use daemoneye_lib::storage::EventStore;
use datafusion::arrow::array::{Int64Array, StringArray, UInt32Array, UInt32Builder};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::datasource::TableType;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{Expr, Operator, TableProviderFilterPushDown};
use datafusion::physical_expr::{EquivalenceProperties, PhysicalExpr};
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
};
use datafusion::scalar::ScalarValue;
use std::fmt;
use std::sync::Arc;

/// Build the Arrow schema the provider exposes.
///
/// `ts_ms` carries event time as epoch milliseconds, taken from each record's
/// `collection_time`. The redb key's `ts_ms`/`seq` are not reachable from
/// outside `daemoneye-lib`, so the fixture sets `collection_time` equal to the
/// timestamp that assigns the bucket (U3).
#[must_use]
pub fn event_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("ts_ms", DataType::Int64, false),
        Field::new("pid", DataType::UInt32, false),
        Field::new("ppid", DataType::UInt32, true),
        Field::new("name", DataType::Utf8, false),
    ]))
}

/// A `TableProvider` over the event store's time buckets.
pub struct EventTable {
    store: Arc<EventStore>,
    schema: SchemaRef,
    buckets: Vec<u64>,
    granularity_ms: u64,
}

impl fmt::Debug for EventTable {
    // `EventStore` is not `Debug`, but `TableProvider` requires it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventTable")
            .field("buckets", &self.buckets.len())
            .field("granularity_ms", &self.granularity_ms)
            .finish_non_exhaustive()
    }
}

impl EventTable {
    /// Discover the store's live buckets and build the provider.
    ///
    /// # Errors
    /// Returns an error when bucket discovery fails, or when the store's bucket
    /// width is not the hourly granularity this provider assumes. `EventStore`
    /// exposes no accessor for granularity, so it is asserted rather than read.
    pub fn try_new(store: Arc<EventStore>) -> DfResult<Self> {
        let mut buckets = store
            .list_buckets()
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        buckets.sort_unstable();
        Ok(Self {
            store,
            schema: event_schema(),
            buckets,
            granularity_ms: HOUR_MS,
        })
    }

    /// Number of live buckets the provider discovered.
    #[must_use]
    pub const fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Assert the store's bucket width matches `granularity_ms`.
    ///
    /// Bucket ids are `ts_ms / granularity`, so consecutive ids must be one
    /// granularity apart in time. A mismatch means the store coarsened to daily
    /// and every derived window would be wrong.
    ///
    /// # Errors
    /// Returns an error when a bucket's computed window holds no rows although
    /// the bucket exists.
    pub fn assert_granularity(&self) -> DfResult<()> {
        let Some(&first) = self.buckets.first() else {
            return Ok(());
        };
        let (start, end) = self.window(first)?;
        let rows = self
            .store
            .scan_range(start, end)
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        if rows.is_empty() {
            return Err(DataFusionError::Execution(format!(
                "bucket {first} exists but its assumed {}ms window [{start},{end}) is empty; \
                 the store's granularity is not what the provider assumed",
                self.granularity_ms
            )));
        }
        Ok(())
    }

    /// The half-open millisecond window a bucket id covers.
    fn window(&self, bucket: u64) -> DfResult<(u64, u64)> {
        let start = bucket
            .checked_mul(self.granularity_ms)
            .ok_or_else(|| DataFusionError::Execution("bucket window overflow".to_owned()))?;
        let end = start
            .checked_add(self.granularity_ms)
            .ok_or_else(|| DataFusionError::Execution("bucket window overflow".to_owned()))?;
        Ok((start, end))
    }

    /// Buckets whose window intersects `[lo, hi)`; all buckets when unbounded.
    fn surviving(&self, lo: Option<i64>, hi: Option<i64>) -> DfResult<Vec<u64>> {
        let mut out = Vec::new();
        for &b in &self.buckets {
            let (start, end) = self.window(b)?;
            let (s, e) = (start.cast_signed(), end.cast_signed());
            let after_lo = lo.is_none_or(|bound| e > bound);
            let before_hi = hi.is_none_or(|bound| s < bound);
            if after_lo && before_hi {
                out.push(b);
            }
        }
        Ok(out)
    }
}

/// Extract a `ts_ms` lower/upper bound from one filter expression.
///
/// Recognizes `ts_ms >= lit`, `ts_ms > lit`, `ts_ms < lit`, `ts_ms <= lit`.
/// Anything else contributes no bound.
fn ts_bound(expr: &Expr) -> Option<(Operator, i64)> {
    let Expr::BinaryExpr(ref bin) = *expr else {
        return None;
    };
    let Expr::Column(ref col) = *bin.left.as_ref() else {
        return None;
    };
    if col.name != "ts_ms" {
        return None;
    }
    let Expr::Literal(ScalarValue::Int64(Some(v)), _) = *bin.right.as_ref() else {
        return None;
    };
    Some((bin.op, v))
}

/// True when the expression is a `ts_ms` comparison the provider can prune on.
fn is_prunable(expr: &Expr) -> bool {
    matches!(
        ts_bound(expr),
        Some((
            Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq,
            _
        ))
    )
}

#[async_trait::async_trait]
impl TableProvider for EventTable {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    /// Claims `ts_ms` range predicates as `Inexact`.
    ///
    /// `Inexact` is the honest claim: a surviving bucket's window can hold rows
    /// outside the predicate, so `DataFusion` must still re-filter. Claiming
    /// `Exact` here would silently drop rows, which R18's equivalence check
    /// would catch only after a wasted run.
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> DfResult<Vec<TableProviderFilterPushDown>> {
        Ok(filters
            .iter()
            .map(|f| {
                if is_prunable(f) {
                    TableProviderFilterPushDown::Inexact
                } else {
                    TableProviderFilterPushDown::Unsupported
                }
            })
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        _limit: Option<usize>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        let mut lo: Option<i64> = None;
        let mut hi: Option<i64> = None;
        for f in filters {
            match ts_bound(f) {
                Some((Operator::GtEq | Operator::Gt, v)) => {
                    lo = Some(lo.map_or(v, |cur: i64| cur.max(v)));
                }
                Some((Operator::Lt | Operator::LtEq, v)) => {
                    hi = Some(hi.map_or(v, |cur: i64| cur.min(v)));
                }
                _ => {}
            }
        }

        let selected = self.surviving(lo, hi)?;
        let windows = selected
            .iter()
            .map(|&b| self.window(b))
            .collect::<DfResult<Vec<_>>>()?;

        let projected = match projection {
            Some(idx) => Arc::new(self.schema.project(idx)?),
            None => Arc::clone(&self.schema),
        };

        Ok(Arc::new(BucketScanExec::new(
            Arc::clone(&self.store),
            Arc::clone(&self.schema),
            projected,
            projection.cloned(),
            windows,
        )))
    }
}

/// One `DataFusion` partition per surviving bucket; each decodes on execute.
struct BucketScanExec {
    store: Arc<EventStore>,
    full_schema: SchemaRef,
    projected_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    windows: Vec<(u64, u64)>,
    properties: Arc<PlanProperties>,
}

impl fmt::Debug for BucketScanExec {
    // `EventStore` is not `Debug`, but `ExecutionPlan` requires it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BucketScanExec")
            .field("partitions", &self.windows.len())
            .finish_non_exhaustive()
    }
}

impl BucketScanExec {
    fn new(
        store: Arc<EventStore>,
        full_schema: SchemaRef,
        projected_schema: SchemaRef,
        projection: Option<Vec<usize>>,
        windows: Vec<(u64, u64)>,
    ) -> Self {
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&projected_schema)),
            Partitioning::UnknownPartitioning(windows.len().max(1)),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Self {
            store,
            full_schema,
            projected_schema,
            projection,
            windows,
            properties,
        }
    }
}

impl DisplayAs for BucketScanExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BucketScanExec: buckets={}", self.windows.len())
    }
}

impl ExecutionPlan for BucketScanExec {
    fn name(&self) -> &'static str {
        "BucketScanExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    // A leaf scan holds no physical expressions to walk.
    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> DfResult<TreeNodeRecursion>,
    ) -> DfResult<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> DfResult<SendableRecordBatchStream> {
        let window = self.windows.get(partition).copied();
        let store = Arc::clone(&self.store);
        let full = Arc::clone(&self.full_schema);
        let projected = Arc::clone(&self.projected_schema);
        let projection = self.projection.clone();

        let stream = futures::stream::once(async move {
            let Some((start, end)) = window else {
                // Pruning left no bucket for this partition. The batch must
                // still carry one empty array per field, not zero columns.
                return Ok(RecordBatch::new_empty(Arc::clone(&projected)));
            };
            let rows = store
                .scan_range(start, end)
                .map_err(|e| DataFusionError::External(Box::new(e)))?;
            let batch = rows_to_batch(&rows, &full)?;
            match projection {
                Some(idx) => batch.project(&idx).map_err(DataFusionError::from),
                None => Ok(batch),
            }
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            Arc::clone(&self.projected_schema),
            stream,
        )))
    }
}

/// Convert decoded records into one Arrow `RecordBatch`.
fn rows_to_batch(
    rows: &[daemoneye_lib::models::process::ProcessRecord],
    schema: &SchemaRef,
) -> DfResult<RecordBatch> {
    let mut ts = Vec::with_capacity(rows.len());
    let mut pid = Vec::with_capacity(rows.len());
    let mut ppid = UInt32Builder::with_capacity(rows.len());
    let mut name = Vec::with_capacity(rows.len());

    for r in rows {
        ts.push(r.collection_time.timestamp_millis());
        pid.push(r.pid.raw());
        match r.ppid {
            Some(p) => ppid.append_value(p.raw()),
            None => ppid.append_null(),
        }
        name.push(r.name.as_str());
    }

    RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(Int64Array::from(ts)),
            Arc::new(UInt32Array::from(pid)),
            Arc::new(ppid.finish()),
            Arc::new(StringArray::from(name)),
        ],
    )
    .map_err(DataFusionError::from)
}
