//! Tests for the event-store `TableProvider` and `BucketScanExec` (U5; R9 to R12).
//!
//! Fixtures write `collection_time` equal to the primary key's `ts_ms`: the provider prunes by the
//! key and the `FilterExec` re-checks the column, so the two have to agree for pruning to be sound.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::produce::estimated_row_bytes;
use super::*;
use crate::detection_bounds::EXECUTOR_BATCH_MAX_BYTES;
use crate::models::ProcessRecord;
use crate::proto::{ColumnDescriptor, ColumnType};
use crate::storage::ingest::IngestRecord;
use crate::storage::read::IndexTerm;
use chrono::{TimeZone, Utc};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::logical_expr::{BinaryExpr, Operator, TableProviderFilterPushDown as Verdict};
use datafusion::physical_plan::{collect, displayable};
use datafusion::prelude::{SessionContext, col, lit};
use datafusion::scalar::ScalarValue;
use proptest::prelude::*;
use tempfile::TempDir;

const HOUR: u64 = 3_600_000;
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn column(name: &str, ty: ColumnType, nullable: bool) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(ty),
        nullable,
        supported_ops: vec![],
    }
}

/// Mirrors `procmond::pushdown_eval::process_schema_descriptor`'s `processes` table.
fn process_table() -> TableDescriptor {
    TableDescriptor {
        name: "processes".to_owned(),
        columns: vec![
            column("pid", ColumnType::Uint, false),
            column("ppid", ColumnType::Uint, true),
            column("name", ColumnType::String, false),
            column("executable_path", ColumnType::String, true),
            column("command_line", ColumnType::String, true),
            column("start_time", ColumnType::Int, true),
            column("cpu_usage", ColumnType::Float, true),
            column("memory_usage", ColumnType::Uint, true),
            column("executable_hash", ColumnType::String, true),
            column("user_id", ColumnType::String, true),
            column("accessible", ColumnType::Bool, false),
            column("file_exists", ColumnType::Bool, false),
            column("collection_time", ColumnType::Int, false),
        ],
    }
}

struct Fixture {
    _dir: TempDir,
    store: Arc<EventStore>,
    cache: Arc<PostingsCache>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("provider.redb")).unwrap());
    Fixture {
        _dir: dir,
        store,
        cache: Arc::new(PostingsCache::new(256, 1024)),
    }
}

fn record(ts_ms: u64, pid: u32, name: &str) -> ProcessRecord {
    let mut r = ProcessRecord::new(pid, name.to_owned());
    r.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
        .unwrap();
    r
}

/// One group commit, so a 20,000-row fixture is not 20,000 fsyncs.
fn put_all(fx: &Fixture, rows: Vec<(u64, ProcessRecord)>) {
    let batch: Vec<IngestRecord> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (ts_ms, record))| IngestRecord {
            collector_id: "test".to_owned(),
            source_seq: u64::try_from(i).unwrap(),
            ts_ms,
            seq: u32::try_from(i).unwrap(),
            record,
        })
        .collect();
    fx.store.put_batch(&batch).unwrap();
}

fn provider_with(fx: &Fixture, limits: ScanLimits) -> Arc<EventStoreTableProvider> {
    Arc::new(
        EventStoreTableProvider::new(
            Arc::clone(&fx.store),
            Arc::clone(&fx.cache),
            &process_table(),
            limits,
        )
        .unwrap(),
    )
}

fn provider(fx: &Fixture) -> Arc<EventStoreTableProvider> {
    provider_with(fx, ScanLimits::default())
}

async fn plan_for(
    p: &EventStoreTableProvider,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
) -> Arc<dyn ExecutionPlan> {
    let ctx = SessionContext::new();
    p.scan(&ctx.state(), projection, filters, None)
        .await
        .unwrap()
}

async fn run(plan: Arc<dyn ExecutionPlan>) -> Vec<RecordBatch> {
    collect(plan, SessionContext::new().task_ctx())
        .await
        .unwrap()
}

fn rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
}

fn ms(value: u64) -> i64 {
    i64::try_from(value).unwrap()
}

fn runs_of(plan: &Arc<dyn ExecutionPlan>) -> Vec<Vec<u64>> {
    plan.downcast_ref::<BucketScanExec>()
        .expect("plan should be a BucketScanExec")
        .runs()
        .to_vec()
}

mod execution;
mod pushdown;
