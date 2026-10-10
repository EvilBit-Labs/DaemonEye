//! Sorted key-set algebra over `(ts_ms, seq)` posting lists (R12).

use std::cmp::Ordering;

pub(super) use crate::storage::read::Key;

/// Keys present in both ascending lists, ascending.
pub(super) fn intersect_sorted(a: &[Key], b: &[Key]) -> Vec<Key> {
    let (mut i, mut j) = (0_usize, 0_usize);
    let mut out = Vec::new();
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        match x.cmp(y) {
            Ordering::Less => i = i.saturating_add(1),
            Ordering::Greater => j = j.saturating_add(1),
            Ordering::Equal => {
                out.push(*x);
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
        }
    }
    out
}

/// Keys present in any of the ascending lists, ascending and deduplicated.
// ponytail: concat + sort, O(n log n); a k-way merge only matters for IN lists far past MAX_IN_VALUES.
pub(super) fn union_sorted(lists: &[&[Key]]) -> Vec<Key> {
    let mut out: Vec<Key> = lists.iter().flat_map(|l| l.iter().copied()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn provider_intersect_keeps_only_common_keys_in_order() {
        let a = [(1, 0), (1, 1), (2, 0), (5, 3), (9, 9)];
        let b = [(1, 1), (2, 0), (2, 1), (9, 9), (10, 0)];
        assert_eq!(intersect_sorted(&a, &b), vec![(1, 1), (2, 0), (9, 9)]);
        assert!(intersect_sorted(&a, &[]).is_empty());
        assert!(intersect_sorted(&[], &b).is_empty());
    }

    #[test]
    fn provider_union_merges_sorted_and_drops_duplicates() {
        let a = [(1, 0), (3, 0)];
        let b = [(1, 0), (2, 0), (3, 1)];
        assert_eq!(
            union_sorted(&[&a, &b]),
            vec![(1, 0), (2, 0), (3, 0), (3, 1)]
        );
        assert!(union_sorted(&[]).is_empty());
    }
}
