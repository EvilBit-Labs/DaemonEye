//! `BucketScanExec`: the physical scan over the event store (R9, R11).
//!
//! One `DataFusion` partition is a contiguous run of buckets, read by one blocking producer
//! under one redb MVCC snapshot. The producer feeds a two-slot channel, so a partition holds at
//! most a few batches at a time no matter how large its buckets are.

use super::filters::PushedFilters;
use super::produce::PartitionJob;
use super::{ScanCounters, ScanLimits};
use crate::storage::EventStore;
use crate::storage::postings_cache::PostingsCache;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::TaskContext;
use datafusion::physical_expr::{EquivalenceProperties, PhysicalExpr};
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::stream::RecordBatchReceiverStreamBuilder;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use std::fmt;
use std::sync::Arc;

/// Batches a partition's producer may queue ahead of its consumer (KTD2).
pub const STREAM_CAPACITY: usize = 2;

/// Everything a scan needs, fixed at `scan()` time.
#[derive(Debug)]
pub(super) struct ScanPlan {
    pub(super) store: Arc<EventStore>,
    pub(super) cache: Arc<PostingsCache>,
    /// Schema of the whole table; the projection indexes into it.
    pub(super) table_schema: SchemaRef,
    pub(super) projection: Option<Vec<usize>>,
    pub(super) filters: PushedFilters,
    /// Contiguous bucket runs, one per partition.
    pub(super) runs: Vec<Vec<u64>>,
    pub(super) limits: ScanLimits,
    /// First bucket id that is still open at plan time.
    pub(super) now_bucket: u64,
    pub(super) counters: Arc<ScanCounters>,
}

/// Physical scan of the event store, one partition per bucket run.
///
/// Rows are read in key order inside a partition, so output order is ascending by
/// `(ts_ms, seq)` within a partition and unspecified across partitions.
#[derive(Debug)]
pub struct BucketScanExec {
    plan: ScanPlan,
    schema: SchemaRef,
    properties: Arc<PlanProperties>,
}

impl BucketScanExec {
    pub(super) fn new(plan: ScanPlan, projected: SchemaRef) -> Self {
        let properties = PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&projected)),
            Partitioning::UnknownPartitioning(plan.runs.len()),
            EmissionType::Incremental,
            Boundedness::Bounded,
        );
        Self {
            plan,
            schema: projected,
            properties: Arc::new(properties),
        }
    }

    /// The bucket ids each partition reads, in partition order.
    #[must_use]
    pub fn runs(&self) -> &[Vec<u64>] {
        &self.plan.runs
    }

    /// Counters shared with the provider that planned this scan.
    #[must_use]
    pub const fn counters(&self) -> &Arc<ScanCounters> {
        &self.plan.counters
    }
}

impl DisplayAs for BucketScanExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => write!(
                f,
                "BucketScanExec: partitions={}, window_ms=[{}, {}), index_terms={}",
                self.plan.runs.len(),
                self.plan.filters.start_ms(),
                self.plan.filters.end_ms(),
                self.plan.filters.term_set_count(),
            ),
            DisplayFormatType::TreeRender => Ok(()),
        }
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

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.is_empty() {
            Ok(self)
        } else {
            Err(DataFusionError::Internal(
                "BucketScanExec is a leaf and takes no children".to_owned(),
            ))
        }
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let Some(buckets) = self.plan.runs.get(partition) else {
            return Err(DataFusionError::Internal(format!(
                "BucketScanExec partition {partition} out of range for {} partitions",
                self.plan.runs.len()
            )));
        };
        let job = PartitionJob::new(&self.plan, buckets.clone());
        let mut builder =
            RecordBatchReceiverStreamBuilder::new(Arc::clone(&self.schema), STREAM_CAPACITY);
        let tx = builder.tx();
        builder.spawn_blocking(move || job.run(&tx));
        Ok(builder.build())
    }
}
