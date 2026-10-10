//! The configured page cache reaches redb: observed through redb's own cache statistics, not the
//! field. A cache that was never applied would sit at redb's 1 GiB default and never evict.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use redb::{CacheStats, ReadableDatabase, ReadableTable, TableDefinition};
use tempfile::tempdir;

use super::{DatabaseConfig, EventStore, MIB};

const PROBE: TableDefinition<'static, u64, &[u8]> = TableDefinition::new("page_cache_probe");
const ROWS: u64 = 4_096;
const ROW_BYTES: usize = 4_096;
const SMALL_CACHE_BYTES: usize = 4 * MIB;
const LARGE_CACHE_BYTES: usize = 256 * MIB;

/// Write about 16 MiB through `store`'s database, read it all back, and return the cache stats.
fn churn(store: &EventStore) -> CacheStats {
    let payload = vec![7_u8; ROW_BYTES];
    let write = store.db.begin_write().unwrap();
    {
        let mut table = write.open_table(PROBE).unwrap();
        for key in 0..ROWS {
            table.insert(key, payload.as_slice()).unwrap();
        }
    }
    write.commit().unwrap();
    let read = store.db.begin_read().unwrap();
    let table = read.open_table(PROBE).unwrap();
    assert_eq!(
        table.iter().unwrap().count(),
        usize::try_from(ROWS).unwrap()
    );
    store.db.cache_stats()
}

#[test]
fn new_with_page_cache_caps_what_redb_holds() {
    let dir = tempdir().unwrap();
    let small =
        EventStore::new_with_page_cache(dir.path().join("s.redb"), SMALL_CACHE_BYTES).unwrap();
    let large =
        EventStore::new_with_page_cache(dir.path().join("l.redb"), LARGE_CACHE_BYTES).unwrap();

    let (small_stats, large_stats) = (churn(&small), churn(&large));

    assert!(small_stats.evictions() > 0, "a 4 MiB cache evicted 16 MiB");
    assert!(
        small_stats.used_bytes() <= SMALL_CACHE_BYTES,
        "the cache stayed inside its cap"
    );
    assert_eq!(
        large_stats.evictions(),
        0,
        "a 256 MiB cache evicted nothing"
    );
}

#[test]
fn open_with_page_cache_caps_what_redb_holds() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("o.redb");
    drop(EventStore::new(&path).unwrap());
    let reopened = EventStore::open_with_page_cache(&path, SMALL_CACHE_BYTES).unwrap();
    let stats = churn(&reopened);
    assert!(stats.evictions() > 0, "the reopened store honoured the cap");
    assert!(stats.used_bytes() <= SMALL_CACHE_BYTES);
}

/// `BucketReader` holds one read snapshot for a whole partition scan, so the cap has to hold with
/// a reader open.
///
/// This says nothing about whether a snapshot pins pages elsewhere: `used_bytes` is redb's own
/// cache accounting, and anything held outside the cache would never appear in it.
#[test]
fn the_cap_holds_with_a_reader_open() {
    let dir = tempdir().unwrap();
    let store =
        EventStore::new_with_page_cache(dir.path().join("p.redb"), SMALL_CACHE_BYTES).unwrap();
    let snapshot = store.db.begin_read().unwrap();

    let stats = churn(&store);

    drop(snapshot);
    assert!(
        stats.evictions() > 0,
        "the cache evicted under the snapshot"
    );
    assert!(
        stats.used_bytes() <= SMALL_CACHE_BYTES,
        "a reader did not grow the cache past its cap"
    );
}

/// The step the agent's `open_event_store` relies on: the configured `page_cache_mb` is what
/// reaches redb, not the default.
#[test]
fn new_configured_applies_the_configured_page_cache_mb() {
    let dir = tempdir().unwrap();
    let database = DatabaseConfig {
        page_cache_mb: 4,
        ..DatabaseConfig::default()
    };
    let store = EventStore::new_configured(dir.path().join("c.redb"), &database).unwrap();

    let stats = churn(&store);

    assert!(stats.evictions() > 0, "the configured 4 MiB cache evicted");
    assert!(stats.used_bytes() <= SMALL_CACHE_BYTES);
}
