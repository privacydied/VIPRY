//! Coalesced `ahead` interval search and shared `ReadJournal` guard
//! (`VICTORY_READ_JOURNAL=1`, off by default, flippable via `VICTORY_AB`).
//!
//! Ported from CHECK6960/Victory-Nvidia-optimized 4d51f6b4b (GPL-3.0-or-later).

/// Whether coalesced `ahead` intervals and shared `ReadJournal` are enabled.
#[inline]
pub(crate) fn on() -> bool {
    crate::asked_for!("VICTORY_READ_JOURNAL")
}

/// Sorts and merges overlapping or adjacent page intervals `[a, b)` in place so
/// interval containment queries can use binary search.
pub(crate) fn coalesce_intervals(intervals: &mut Vec<(usize, usize)>) {
    if intervals.len() < 2 {
        return;
    }
    intervals.sort_unstable();
    let mut write = 0usize;
    for read in 1..intervals.len() {
        let next = intervals[read];
        if next.0 <= intervals[write].1 {
            intervals[write].1 = intervals[write].1.max(next.1);
        } else {
            write += 1;
            intervals[write] = next;
        }
    }
    intervals.truncate(write + 1);
}

/// Returns true when sorted, coalesced `intervals` contain all of `[first, last)`.
#[inline]
pub(crate) fn contains_interval(intervals: &[(usize, usize)], first: usize, last: usize) -> bool {
    let idx = intervals.partition_point(|&(a, _)| a <= first);
    idx > 0 && intervals[idx - 1].1 >= last
}

#[cfg(test)]
mod tests {
    use super::{coalesce_intervals, contains_interval};

    #[test]
    fn ahead_intervals_coalesce_and_match() {
        let mut intervals = vec![(20, 24), (10, 14), (14, 16), (12, 18), (30, 32)];
        coalesce_intervals(&mut intervals);
        assert_eq!(intervals, vec![(10, 18), (20, 24), (30, 32)]);
        assert!(contains_interval(&intervals, 10, 18));
        assert!(contains_interval(&intervals, 12, 16));
        assert!(contains_interval(&intervals, 21, 23));
        assert!(!contains_interval(&intervals, 17, 21));
        assert!(!contains_interval(&intervals, 5, 11));
        assert!(!contains_interval(&intervals, 30, 33));
        assert!(!contains_interval(&[], 0, 1));
    }
}
