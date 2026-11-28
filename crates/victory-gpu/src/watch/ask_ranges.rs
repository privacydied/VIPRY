//! Range-granular `ASKING` holds (`VICTORY_ASK_RANGES=1`, off by default,
//! flippable via `VICTORY_AB`).
//!
//! Replaces `Asking::hold_all()` (which locked `[0, usize::MAX)` across all
//! threads during `GetWriteWatch`) with holding only the coalesced page ranges
//! being asked, so disjoint ranges on other threads (heap sync, texture pass,
//! early asking) do not block `victory-submit`.

use super::{ASKING, read_journal, waited};

/// Whether range-granular `ASKING` holds are enabled.
#[inline]
pub(super) fn on() -> bool {
    crate::asked_for!("VICTORY_ASK_RANGES")
}

/// Multiple disjoint page ranges held in [`ASKING`] until dropped.
pub(super) struct AskingMany(Vec<(usize, usize)>);

fn any_overlap(held: &[(usize, usize)], want: &[(usize, usize)]) -> bool {
    for &(first, last) in want {
        if held.iter().any(|&(a, b)| a < last && first < b) {
            return true;
        }
    }
    false
}

impl AskingMany {
    /// Holds `ranges` in [`ASKING`] when `VICTORY_ASK_RANGES=1`, or
    /// `[0, usize::MAX)` when off.
    pub(super) fn hold(mut ranges: Vec<(usize, usize)>) -> Self {
        if !on() {
            ranges.clear();
            ranges.push((0, usize::MAX));
        } else {
            ranges.retain(|&(a, b)| a < b);
            if ranges.is_empty() {
                return Self(ranges);
            }
            read_journal::coalesce_intervals(&mut ranges);
        }
        let mut held = ASKING.0.lock().unwrap_or_else(|e| e.into_inner());
        if any_overlap(&held, &ranges) {
            held = waited(0, || {
                let mut held = held;
                while any_overlap(&held, &ranges) {
                    held = ASKING.1.wait(held).unwrap_or_else(|e| e.into_inner());
                }
                held
            });
        }
        held.extend_from_slice(&ranges);
        Self(ranges)
    }
}

impl Drop for AskingMany {
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }
        let mut held = ASKING.0.lock().unwrap_or_else(|e| e.into_inner());
        for &r in &self.0 {
            if let Some(i) = held.iter().position(|&h| h == r) {
                held.swap_remove(i);
            }
        }
        drop(held);
        ASKING.1.notify_all();
    }
}

/// Computes the coalesced `[first, last)` runs that `ask_ahead` will pass to
/// `ask_run` for sorted `pieces`, using `gap` and `max_span`.
pub(super) fn ahead_runs(pieces: &[(usize, usize)], gap: usize, max_span: usize) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let first = pieces[i].0;
        let mut last = pieces[i].1;
        let mut k = i + 1;
        while k < pieces.len()
            && pieces[k].0 <= last.saturating_add(gap)
            && pieces[k].1.max(last) - first <= max_span
        {
            last = last.max(pieces[k].1);
            k += 1;
        }
        if k > i + 1 {
            runs.push((first, last));
        }
        i = k;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::ahead_runs;

    #[test]
    fn ahead_runs_groups_nearby_pieces_and_skips_singles() {
        let pieces = vec![(10, 12), (14, 18), (100, 102), (200, 205), (206, 210)];
        assert_eq!(ahead_runs(&pieces, 5, 100), vec![(10, 18), (200, 210)]);
    }
}
