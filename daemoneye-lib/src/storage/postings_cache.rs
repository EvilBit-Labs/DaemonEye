//! Bounded LRU cache of closed-bucket posting lists (requirement R12, KTD8).
//!
//! Only lists of **closed** buckets are cached: a bucket is closed when its id is below the
//! current bucket id, after which its index can no longer change. The open bucket is always read
//! live. The cache is bounded by count (entries, and postings per entry), never by bytes; see
//! `detection_bounds::POSTING_CACHE_MAX_BYTES` for the product that bounds the defaults.
//!
//! The mutex is `parking_lot`'s and is sync-only: it is never held while the loader runs, and
//! `get_or_load` is deliberately not `async`.

use super::read::IndexKind;
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cache key: index, bucket id, term widened to `u128` (`IndexTerm::kind` / `as_u128`).
pub type PostingsKey = (IndexKind, u64, u128);

/// One posting list: `(ts_ms, seq)` pointers into a bucket's event table.
pub type Postings = Arc<[(u64, u32)]>;

/// A bounded, exact-LRU cache of closed-bucket posting lists.
#[derive(Debug)]
pub struct PostingsCache {
    entries: Mutex<LruCache<PostingsKey, Postings>>,
    max_postings: usize,
    hits: AtomicU64,
    misses: AtomicU64,
    bypassed_open: AtomicU64,
    bypassed_long: AtomicU64,
}

impl PostingsCache {
    /// Create an empty cache of at most `max_entries` lists of at most `max_postings` postings.
    ///
    /// `max_entries` of 0 is raised to 1 so the cache is always constructible; the caller owns
    /// validating configured values. A `max_postings` of 0 caches only empty lists.
    #[must_use]
    pub fn new(max_entries: usize, max_postings: usize) -> Self {
        Self {
            entries: Mutex::new(LruCache::new(
                NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::MIN),
            )),
            max_postings,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            bypassed_open: AtomicU64::new(0),
            bypassed_long: AtomicU64::new(0),
        }
    }

    /// Return the posting list for `key`, calling `loader` unless a resident entry serves it.
    ///
    /// A key whose bucket is `>= now_bucket` is open: `loader` runs every time and nothing is
    /// stored. A loaded list longer than `max_postings` is returned but not stored. A hit
    /// promotes the entry to most-recently-used; inserting into a full cache evicts the
    /// least-recently-used one. Empty lists are cached like any other.
    ///
    /// Two threads missing on one key may both load; the second insert replaces the first and the
    /// lists are equal. Holding the lock across `loader` would serialise every redb read.
    ///
    /// # Errors
    ///
    /// Returns the loader's error unchanged. A failed load stores nothing, so the next call
    /// loads again; the cache is never poisoned.
    pub fn get_or_load<E>(
        &self,
        key: PostingsKey,
        now_bucket: u64,
        loader: impl FnOnce() -> Result<Vec<(u64, u32)>, E>,
    ) -> Result<Postings, E> {
        if key.1 >= now_bucket {
            self.bypassed_open.fetch_add(1, Ordering::Relaxed);
            return loader().map(Postings::from);
        }
        if let Some(found) = self.entries.lock().get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::clone(found));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let list = Postings::from(loader()?);
        if list.len() > self.max_postings {
            self.bypassed_long.fetch_add(1, Ordering::Relaxed);
        } else {
            self.entries.lock().put(key, Arc::clone(&list));
        }
        Ok(list)
    }

    /// Number of resident lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Whether no list is resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Whether `key` is resident, without promoting it.
    #[must_use]
    pub fn contains(&self, key: &PostingsKey) -> bool {
        self.entries.lock().contains(key)
    }

    /// Lookups of closed buckets served from a resident entry.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Lookups of closed buckets that called the loader, including failed and over-long loads.
    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Lookups served live because the bucket was open.
    #[must_use]
    pub fn bypassed_open(&self) -> u64 {
        self.bypassed_open.load(Ordering::Relaxed)
    }

    /// Loads returned but not stored because the list exceeded `max_postings`.
    #[must_use]
    pub fn bypassed_long(&self) -> u64 {
        self.bypassed_long.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::detection_bounds::{POSTING_CACHE_MAX_ENTRIES, POSTING_CACHE_MAX_POSTINGS};
    use proptest::prelude::*;
    use std::cell::Cell;

    const NOW: u64 = 100;

    fn key(term: u128) -> PostingsKey {
        (IndexKind::Name, 5, term)
    }

    fn list(n: usize) -> Vec<(u64, u32)> {
        (0..n).map(|i| (u64::try_from(i).unwrap(), 0)).collect()
    }

    /// Calls `get_or_load` with a loader that counts its own invocations in `calls`.
    fn load(
        cache: &PostingsCache,
        k: PostingsKey,
        now: u64,
        n: usize,
        calls: &Cell<u32>,
    ) -> Postings {
        cache
            .get_or_load::<()>(k, now, || {
                calls.set(calls.get().saturating_add(1));
                Ok(list(n))
            })
            .unwrap()
    }

    #[test]
    fn second_lookup_of_closed_bucket_skips_loader() {
        let cache = PostingsCache::new(4, 8);
        let calls = Cell::new(0);
        let a = load(&cache, key(1), NOW, 3, &calls);
        let b = load(&cache, key(1), NOW, 3, &calls);
        assert_eq!(calls.get(), 1);
        assert_eq!((cache.hits(), cache.misses()), (1, 1));
        assert_eq!(a, b);
    }

    #[test]
    fn open_bucket_always_loads_and_never_stores() {
        let cache = PostingsCache::new(4, 8);
        let calls = Cell::new(0);
        let open = (IndexKind::Pid, NOW, 9);
        load(&cache, open, NOW, 2, &calls);
        load(&cache, open, NOW, 2, &calls);
        assert_eq!(calls.get(), 2);
        assert_eq!(cache.bypassed_open(), 2);
        assert!(cache.is_empty());
    }

    #[test]
    fn bucket_after_now_is_also_treated_as_open() {
        let cache = PostingsCache::new(4, 8);
        let calls = Cell::new(0);
        load(&cache, (IndexKind::Pid, NOW + 1, 1), NOW, 1, &calls);
        assert_eq!(cache.bypassed_open(), 1);
        assert!(cache.is_empty());
    }

    #[test]
    fn list_at_cap_is_cached_and_one_over_is_returned_but_not() {
        let cache = PostingsCache::new(4, POSTING_CACHE_MAX_POSTINGS);
        let calls = Cell::new(0);
        load(&cache, key(1), NOW, POSTING_CACHE_MAX_POSTINGS, &calls);
        load(&cache, key(1), NOW, POSTING_CACHE_MAX_POSTINGS, &calls);
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.bypassed_long(), 0);

        let over = load(&cache, key(2), NOW, POSTING_CACHE_MAX_POSTINGS + 1, &calls);
        assert_eq!(over.len(), POSTING_CACHE_MAX_POSTINGS + 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bypassed_long(), 1);
        load(&cache, key(2), NOW, POSTING_CACHE_MAX_POSTINGS + 1, &calls);
        assert_eq!(calls.get(), 3);
        assert_eq!(cache.bypassed_long(), 2);
    }

    #[test]
    fn long_key_that_shrinks_becomes_cacheable() {
        let cache = PostingsCache::new(4, 4);
        let calls = Cell::new(0);
        load(&cache, key(1), NOW, 5, &calls);
        assert!(cache.is_empty());
        load(&cache, key(1), NOW, 4, &calls);
        load(&cache, key(1), NOW, 4, &calls);
        assert_eq!(calls.get(), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn exceeding_max_entries_evicts_the_oldest() {
        let cache = PostingsCache::new(POSTING_CACHE_MAX_ENTRIES, 8);
        let calls = Cell::new(0);
        for term in 0..=u128::try_from(POSTING_CACHE_MAX_ENTRIES).unwrap() {
            load(&cache, key(term), NOW, 1, &calls);
        }
        assert_eq!(cache.len(), POSTING_CACHE_MAX_ENTRIES);
        assert!(!cache.contains(&key(0)));
        assert!(cache.contains(&key(1)));
    }

    #[test]
    fn hit_promotes_so_the_untouched_entry_is_evicted() {
        let cache = PostingsCache::new(2, 8);
        let calls = Cell::new(0);
        load(&cache, key(1), NOW, 1, &calls);
        load(&cache, key(2), NOW, 1, &calls);
        load(&cache, key(1), NOW, 1, &calls);
        load(&cache, key(3), NOW, 1, &calls);
        assert!(cache.contains(&key(1)));
        assert!(!cache.contains(&key(2)));
    }

    #[test]
    fn loader_error_is_returned_and_leaves_the_slot_clean() {
        let cache = PostingsCache::new(4, 8);
        let err = cache.get_or_load(key(1), NOW, || Err("boom"));
        assert_eq!(err.unwrap_err(), "boom");
        assert!(cache.is_empty());
        let calls = Cell::new(0);
        load(&cache, key(1), NOW, 2, &calls);
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn empty_list_is_cached() {
        let cache = PostingsCache::new(4, 8);
        let calls = Cell::new(0);
        assert!(load(&cache, key(1), NOW, 0, &calls).is_empty());
        load(&cache, key(1), NOW, 0, &calls);
        assert_eq!(calls.get(), 1);
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn zero_and_one_entry_bounds_hold() {
        let zero = PostingsCache::new(0, 8);
        let one = PostingsCache::new(1, 8);
        let calls = Cell::new(0);
        for term in 0..3 {
            load(&zero, key(term), NOW, 1, &calls);
            load(&one, key(term), NOW, 1, &calls);
        }
        assert_eq!((zero.len(), one.len()), (1, 1));
        assert!(one.contains(&key(2)));
    }

    #[test]
    fn max_postings_zero_caches_only_empty_lists() {
        let cache = PostingsCache::new(4, 0);
        let calls = Cell::new(0);
        load(&cache, key(1), NOW, 1, &calls);
        assert!(cache.is_empty());
        load(&cache, key(2), NOW, 0, &calls);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn keys_differing_in_any_component_do_not_collide() {
        let cache = PostingsCache::new(8, 8);
        let calls = Cell::new(0);
        load(&cache, (IndexKind::Pid, 1, 7), NOW, 1, &calls);
        load(&cache, (IndexKind::Ppid, 1, 7), NOW, 1, &calls);
        load(&cache, (IndexKind::Pid, 2, 7), NOW, 1, &calls);
        load(&cache, (IndexKind::Pid, 1, 8), NOW, 1, &calls);
        assert_eq!(calls.get(), 4);
    }

    proptest! {
        /// Residency and hit/miss outcomes match an independently written LRU model.
        #[test]
        fn residency_matches_independent_lru_model(
            cap in 1_usize..6,
            terms in proptest::collection::vec(0_u128..10, 0..80),
        ) {
            let cache = PostingsCache::new(cap, 8);
            // Model: front = least recently used.
            let mut model: Vec<u128> = Vec::new();
            for term in terms {
                let calls = Cell::new(0);
                load(&cache, key(term), NOW, 1, &calls);
                let was_resident = model.contains(&term);
                prop_assert_eq!(calls.get(), u32::from(!was_resident));
                model.retain(|t| *t != term);
                model.push(term);
                if model.len() > cap {
                    model.remove(0);
                }
                prop_assert!(cache.len() <= cap);
                prop_assert_eq!(cache.len(), model.len());
                for t in 0..10_u128 {
                    prop_assert_eq!(cache.contains(&key(t)), model.contains(&t));
                }
            }
        }
    }
}
