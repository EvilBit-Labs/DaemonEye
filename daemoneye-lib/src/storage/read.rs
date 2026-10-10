//! Bucket-at-a-time read primitives.
//!
//! The `DataFusion` provider reads one time bucket per partition, under one redb
//! MVCC snapshot, instead of materialising a window through
//! [`EventStore::scan_range`]. This module supplies those reads: chunked key-range
//! walks, plain (uncached) index postings, and keyed row fetches.
//!
//! Index lookups are bucket- or hash-granular (`idx:name` is a lowercase 128-bit
//! hash), so [`BucketReader::postings`] may return candidates that do not match;
//! callers verify against the primary row.

use super::bucket::{bucket_id, bucket_table_name};
use super::codec::{TsSeqKey, decode_value};
use super::index::{
    exe_index_name, hash_index_def, name_index_name, pid_index_name, ppid_index_name, u32_index_def,
};
use super::{EventStore, StorageError, bucket_def, collect_bucket_ids};
use redb::{MultimapTableDefinition, ReadOnlyTable, ReadableDatabase};
use std::ops::Bound;

/// A primary key `(ts_ms, seq)` paired with its decoded row.
pub type KeyedRecord = (Key, crate::models::ProcessRecord);

/// A primary key `(ts_ms, seq)`.
pub type Key = (u64, u32);

/// Which secondary index a posting list belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IndexKind {
    /// `idx:pid`.
    Pid,
    /// `idx:ppid`.
    Ppid,
    /// `idx:name` (lowercase BLAKE3 hash).
    Name,
    /// `idx:exe_hash` (128-bit SHA-256 prefix).
    ExeHash,
}

/// An index term, tagged with its index so a mismatched kind/term pair cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IndexTerm {
    /// A pid.
    Pid(u32),
    /// A parent pid.
    Ppid(u32),
    /// A name hash, from [`IndexTerm::name`].
    Name(u128),
    /// An executable-hash prefix, from [`IndexTerm::exe_hash`].
    ExeHash(u128),
}

impl IndexTerm {
    /// Term for a process name; case-insensitive, like the index.
    #[must_use]
    pub fn name(name: &str) -> Self {
        Self::Name(super::index::name_hash128(name))
    }

    /// Term for a hex SHA-256 executable hash, or `None` if too short or not hex.
    #[must_use]
    pub fn exe_hash(hex: &str) -> Option<Self> {
        super::index::exe_hash_prefix(hex).map(Self::ExeHash)
    }

    /// The index this term addresses.
    #[must_use]
    pub const fn kind(self) -> IndexKind {
        match self {
            Self::Pid(_) => IndexKind::Pid,
            Self::Ppid(_) => IndexKind::Ppid,
            Self::Name(_) => IndexKind::Name,
            Self::ExeHash(_) => IndexKind::ExeHash,
        }
    }

    /// The term widened to `u128`; with [`IndexTerm::kind`] it is a lossless cache key.
    #[must_use]
    pub fn as_u128(self) -> u128 {
        match self {
            Self::Pid(v) | Self::Ppid(v) => u128::from(v),
            Self::Name(v) | Self::ExeHash(v) => v,
        }
    }
}

impl EventStore {
    /// Bucket width in milliseconds.
    #[must_use]
    pub const fn granularity_ms(&self) -> u64 {
        self.granularity_ms
    }

    /// Ids of existing buckets overlapping `[start_ms, end_ms)`, ascending.
    pub fn bucket_ids_in(&self, start_ms: u64, end_ms: u64) -> Result<Vec<u64>, StorageError> {
        if end_ms <= start_ms {
            return Ok(Vec::new());
        }
        let first = bucket_id(start_ms, self.granularity_ms)?;
        let last = bucket_id(end_ms.saturating_sub(1), self.granularity_ms)?;
        let txn = self.db.begin_read()?;
        let mut ids = collect_bucket_ids(&txn)?;
        ids.retain(|id| (first..=last).contains(id));
        Ok(ids)
    }

    /// Open one MVCC snapshot for bucket-at-a-time reads.
    pub fn open_read(&self) -> Result<BucketReader, StorageError> {
        Ok(BucketReader {
            txn: self.db.begin_read()?,
        })
    }
}

/// Reads against a single redb snapshot, so every call sees the same data.
pub struct BucketReader {
    txn: redb::ReadTransaction,
}

impl BucketReader {
    /// Up to `limit` rows of `bucket_id` with `ts_ms` in `[start_ms, end_ms)`,
    /// ascending, strictly after `after` when given. A missing bucket, an empty
    /// range, `limit == 0`, or an `after` at or past the end yield no rows.
    pub fn range_chunk(
        &self,
        bucket_id: u64,
        start_ms: u64,
        end_ms: u64,
        after: Option<Key>,
        limit: usize,
    ) -> Result<Vec<KeyedRecord>, StorageError> {
        let lower_key = (start_ms, 0_u32);
        let upper_key = (end_ms, 0_u32);
        let resume = after.filter(|key| *key >= lower_key);
        let lower = resume.map_or(Bound::Included(lower_key), Bound::Excluded);
        let starts_at = resume.unwrap_or(lower_key);
        if limit == 0 || starts_at >= upper_key {
            return Ok(Vec::new());
        }
        let Some(table) = self.event_table(bucket_id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in table
            .range((lower, Bound::Excluded(upper_key)))?
            .take(limit)
        {
            let (key, value) = entry?;
            out.push((key.value(), decode_value(value.value())?));
        }
        Ok(out)
    }

    /// Ascending `(ts_ms, seq)` postings for `term` in `bucket_id`; empty if the
    /// index table is absent. A plain read: hash-keyed indexes can return
    /// candidates that do not match, so callers verify against the row.
    pub fn postings(&self, bucket_id: u64, term: IndexTerm) -> Result<Vec<Key>, StorageError> {
        let found = match term {
            IndexTerm::Pid(v) => {
                self.collect_postings(u32_index_def(&pid_index_name(bucket_id)), v)
            }
            IndexTerm::Ppid(v) => {
                self.collect_postings(u32_index_def(&ppid_index_name(bucket_id)), v)
            }
            IndexTerm::Name(h) => {
                self.collect_postings(hash_index_def(&name_index_name(bucket_id)), h)
            }
            IndexTerm::ExeHash(h) => {
                self.collect_postings(hash_index_def(&exe_index_name(bucket_id)), h)
            }
        };
        found.map(Option::unwrap_or_default)
    }

    /// Rows for `keys` in `bucket_id`, in input order (duplicates repeat);
    /// keys absent from this bucket are skipped.
    pub fn fetch(&self, bucket_id: u64, keys: &[Key]) -> Result<Vec<KeyedRecord>, StorageError> {
        let Some(table) = self.event_table(bucket_id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(keys.len());
        for &key in keys {
            if let Some(value) = table.get(key)? {
                out.push((key, decode_value(value.value())?));
            }
        }
        Ok(out)
    }

    fn event_table(
        &self,
        bucket_id: u64,
    ) -> Result<Option<ReadOnlyTable<TsSeqKey, &'static [u8]>>, StorageError> {
        match self
            .txn
            .open_table(bucket_def(&bucket_table_name(bucket_id)))
        {
            Ok(table) => Ok(Some(table)),
            Err(redb::TableError::TableDoesNotExist(_)) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    fn collect_postings<'a, K: redb::Key + 'static>(
        &self,
        def: MultimapTableDefinition<'a, K, TsSeqKey>,
        term: K::SelfType<'a>,
    ) -> Result<Option<Vec<Key>>, StorageError> {
        let idx = match self.txn.open_multimap_table(def) {
            Ok(idx) => idx,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(other) => return Err(other.into()),
        };
        let mut out = Vec::new();
        for posting in idx.get(term)? {
            out.push(posting?.value());
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::bucket::HOURLY_MS;
    use super::*;
    use crate::models::ProcessRecord;
    use tempfile::{TempDir, tempdir};

    const BASE_HOUR: u64 = 100;

    fn open_store() -> (TempDir, EventStore) {
        let dir = tempdir().unwrap();
        let store = EventStore::new(dir.path().join("read.redb")).unwrap();
        (dir, store)
    }

    fn put(store: &EventStore, ts: u64, seq: u32, pid: u32, name: &str) {
        store
            .put_event(ts, seq, &ProcessRecord::new(pid, name.to_owned()))
            .unwrap();
    }

    fn keys(rows: &[KeyedRecord]) -> Vec<Key> {
        rows.iter().map(|&(key, _)| key).collect()
    }

    #[test]
    fn granularity_matches_default_retention_choice() {
        let (_d, st) = open_store();
        assert_eq!(st.granularity_ms(), HOURLY_MS);
    }

    #[test]
    fn range_chunk_threads_after_without_overlap_or_gap() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        for i in 0..20_u32 {
            put(&st, t0 + 1_000 + u64::from(i) * 10, i, i, "p");
        }
        // Outside [start, end): before and at the exclusive end, same bucket.
        put(&st, t0 + 10, 99, 900, "early");
        put(&st, t0 + 1_000 + 20 * 10, 98, 901, "at-end");
        let id = BASE_HOUR;
        let (start, end) = (t0 + 1_000, t0 + 1_000 + 20 * 10);
        let rd = st.open_read().unwrap();

        let c1 = rd.range_chunk(id, start, end, None, 8).unwrap();
        let c2 = rd
            .range_chunk(id, start, end, c1.last().map(|x| x.0), 8)
            .unwrap();
        let c3 = rd
            .range_chunk(id, start, end, c2.last().map(|x| x.0), 8)
            .unwrap();
        let c4 = rd
            .range_chunk(id, start, end, c3.last().map(|x| x.0), 8)
            .unwrap();

        let all: Vec<_> = [c1, c2, c3].iter().flat_map(|chunk| keys(chunk)).collect();
        let expected: Vec<_> = (0..20_u32)
            .map(|i| (start + u64::from(i) * 10, i))
            .collect();
        assert_eq!(all, expected);
        assert!(c4.is_empty());
    }

    #[test]
    fn range_chunk_chunk_sizes_are_8_8_4() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        for i in 0..20_u32 {
            put(&st, t0 + u64::from(i), i, i, "p");
        }
        let rd = st.open_read().unwrap();
        let c1 = rd.range_chunk(BASE_HOUR, t0, t0 + 20, None, 8).unwrap();
        let c2 = rd
            .range_chunk(BASE_HOUR, t0, t0 + 20, c1.last().map(|x| x.0), 8)
            .unwrap();
        let c3 = rd
            .range_chunk(BASE_HOUR, t0, t0 + 20, c2.last().map(|x| x.0), 8)
            .unwrap();
        assert_eq!((c1.len(), c2.len(), c3.len()), (8, 8, 4));
    }

    #[test]
    fn range_chunk_edges_are_empty_not_errors() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        put(&st, t0 + 5, 1, 1, "p");
        put(&st, t0 + 5, u32::MAX, 2, "max");
        let rd = st.open_read().unwrap();
        let id = BASE_HOUR;
        assert!(
            rd.range_chunk(id, t0, t0 + 100, None, 0)
                .unwrap()
                .is_empty(),
            "limit 0"
        );
        assert!(
            rd.range_chunk(id + 7, t0, t0 + 100, None, 8)
                .unwrap()
                .is_empty(),
            "missing bucket"
        );
        assert!(
            rd.range_chunk(id, t0 + 100, t0, None, 8)
                .unwrap()
                .is_empty(),
            "inverted range"
        );
        assert!(
            rd.range_chunk(id, t0, t0 + 100, Some((t0 + 5, u32::MAX)), 8)
                .unwrap()
                .is_empty(),
            "after = last key, seq MAX"
        );
        assert!(
            rd.range_chunk(id, t0, t0 + 100, Some((t0 + 90_000, 0)), 8)
                .unwrap()
                .is_empty(),
            "after past end"
        );
        // after below the window start is clamped, not an error or a row outside the window.
        let rows = rd
            .range_chunk(id, t0 + 5, t0 + 100, Some((0, 0)), 8)
            .unwrap();
        assert_eq!(keys(&rows), vec![(t0 + 5, 1), (t0 + 5, u32::MAX)]);
    }

    #[test]
    fn name_postings_are_case_insensitive_and_unmatched_term_is_empty() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        put(&st, t0 + 1, 1, 10, "bash");
        put(&st, t0 + 2, 2, 11, "Bash");
        put(&st, t0 + 3, 3, 12, "zsh");
        let rd = st.open_read().unwrap();
        let lower = rd.postings(BASE_HOUR, IndexTerm::name("bash")).unwrap();
        let upper = rd.postings(BASE_HOUR, IndexTerm::name("Bash")).unwrap();
        assert_eq!(lower, vec![(t0 + 1, 1), (t0 + 2, 2)]);
        assert_eq!(lower, upper);
        assert!(
            rd.postings(BASE_HOUR, IndexTerm::name("nope"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pid_ppid_exe_postings_and_missing_tables() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        let sha = "abcdef0123456789abcdef0123456789ffffffffffffffffffffffffffffffff";
        let mut rec = ProcessRecord::new(77, "x".to_owned());
        rec.ppid = Some(crate::models::process::ProcessId::new(5));
        rec.executable_hash = Some(sha.to_owned());
        st.put_event(t0 + 1, 1, &rec).unwrap();
        put(&st, t0 + 2, 2, 78, "y"); // no ppid, no hash
        let rd = st.open_read().unwrap();
        assert_eq!(
            rd.postings(BASE_HOUR, IndexTerm::Pid(77)).unwrap(),
            vec![(t0 + 1, 1)]
        );
        assert_eq!(
            rd.postings(BASE_HOUR, IndexTerm::Ppid(5)).unwrap(),
            vec![(t0 + 1, 1)]
        );
        let exe = IndexTerm::exe_hash(sha).unwrap();
        assert_eq!(rd.postings(BASE_HOUR, exe).unwrap(), vec![(t0 + 1, 1)]);
        // Bucket with no tables at all, and a bucket whose ppid/exe tables were never created.
        assert!(
            rd.postings(BASE_HOUR + 9, IndexTerm::Pid(77))
                .unwrap()
                .is_empty()
        );
        let (_d2, st2) = open_store();
        put(&st2, t0 + 1, 1, 1, "only-pid-name");
        let rd2 = st2.open_read().unwrap();
        assert!(
            rd2.postings(BASE_HOUR, IndexTerm::Ppid(5))
                .unwrap()
                .is_empty()
        );
        assert!(rd2.postings(BASE_HOUR, exe).unwrap().is_empty());
    }

    #[test]
    fn index_term_kind_and_u128_round_trip() {
        assert_eq!(IndexTerm::Pid(7).kind(), IndexKind::Pid);
        assert_eq!(IndexTerm::Ppid(7).kind(), IndexKind::Ppid);
        assert_eq!(IndexTerm::name("a").kind(), IndexKind::Name);
        assert_eq!(IndexTerm::Pid(u32::MAX).as_u128(), u128::from(u32::MAX));
        assert_eq!(
            IndexTerm::name("a").as_u128(),
            crate::storage::index::name_hash128("a")
        );
        assert!(IndexTerm::exe_hash("short").is_none());
    }

    #[test]
    fn fetch_returns_input_order_and_skips_foreign_or_absent_keys() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        let other = (BASE_HOUR + 1) * HOURLY_MS;
        put(&st, t0 + 1, 1, 10, "a");
        put(&st, t0 + 2, 2, 11, "b");
        put(&st, other + 1, 1, 12, "c");
        let rd = st.open_read().unwrap();
        // Unsorted, a duplicate, an absent key, and a key that lives in the next bucket.
        let got = rd
            .fetch(
                BASE_HOUR,
                &[
                    (t0 + 2, 2),
                    (t0 + 1, 1),
                    (t0 + 2, 2),
                    (t0 + 3, 3),
                    (other + 1, 1),
                ],
            )
            .unwrap();
        assert_eq!(keys(&got), vec![(t0 + 2, 2), (t0 + 1, 1), (t0 + 2, 2)]);
        assert_eq!(got.get(1).map(|row| row.1.name.as_str()), Some("a"));
        assert!(rd.fetch(BASE_HOUR + 50, &[(t0 + 1, 1)]).unwrap().is_empty());
        assert!(rd.fetch(BASE_HOUR, &[]).unwrap().is_empty());
    }

    #[test]
    fn bucket_ids_in_prunes_to_span_and_inverted_is_empty() {
        let (_d, st) = open_store();
        for h in [1_u64, 3, 5] {
            put(&st, h * HOURLY_MS + 1, 1, 1, "p");
        }
        assert_eq!(
            st.bucket_ids_in(HOURLY_MS, 4 * HOURLY_MS).unwrap(),
            vec![1, 3]
        );
        assert_eq!(
            st.bucket_ids_in(3 * HOURLY_MS, 3 * HOURLY_MS + 1).unwrap(),
            vec![3]
        );
        assert_eq!(st.bucket_ids_in(0, 6 * HOURLY_MS).unwrap(), vec![1, 3, 5]);
        assert!(
            st.bucket_ids_in(4 * HOURLY_MS, 4 * HOURLY_MS)
                .unwrap()
                .is_empty()
        );
        assert!(
            st.bucket_ids_in(5 * HOURLY_MS, HOURLY_MS)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reader_keeps_its_snapshot_across_later_writes() {
        let (_d, st) = open_store();
        let t0 = BASE_HOUR * HOURLY_MS;
        put(&st, t0 + 1, 1, 1, "a");
        let rd = st.open_read().unwrap();
        put(&st, t0 + 2, 2, 2, "b");
        assert_eq!(
            keys(&rd.range_chunk(BASE_HOUR, t0, t0 + 100, None, 10).unwrap()),
            vec![(t0 + 1, 1)]
        );
        assert_eq!(
            keys(
                &st.open_read()
                    .unwrap()
                    .range_chunk(BASE_HOUR, t0, t0 + 100, None, 10)
                    .unwrap()
            )
            .len(),
            2
        );
    }
}
