//! `DataFusion` `TableProvider` over the event store (ADR-0006, ADR-0008, KTD2).
//!
//! The provider reads a bucket at a time and emits `RecordBatch`es bounded in
//! both rows and bytes, so peak resident set tracks one batch rather than the
//! retention window. It reports `TableProviderFilterPushDown::Inexact` for
//! every predicate it consumes and never `Exact`, so a `FilterExec` always
//! re-checks the rows an index lookup admitted (R10).
//!
//! Empty at this unit: U1 declares the module so the `detection-engine` feature
//! gate is exercised from the first commit. `EventStoreTableProvider` and
//! `BucketScanExec` land in U5, over the read primitives of U2 and the Arrow
//! encoding of U4.

pub mod arrow;
