//! Refreshes done before a title write only where the write lands
//! (`VICTORY_FLUSH_NARROW=0` turns it off; on by default).
//!
//! A refresh step (`Step::Refresh`) waits in the submission's [`Batch`]
//! until something needs the copy up to date. Before each step that writes
//! title memory on the CPU, a title buffer copy (`Step::Copy`) or a copy done
//! at its place (`Step::Placed`), [`Batch::title_written`] did every pending
//! refresh of every copy, and forgot every range refreshed ahead of the
//! steps, so that each later refresh step of those copies was done again.
//! On this PC that is part of the 39 refresh flushes a frame (76.6 ms of
//! refreshes a frame in the EXACT_FAST bench row), for some 25 title copies
//! of 123 KB in all.
//!
//! A copy writes `[dst, dst + len)` only. A pending refresh of a range it
//! does not touch reads the same title bytes before the copy and after it,
//! so here only the pending refreshes that overlap the write are done before
//! it, and only the ranges refreshed ahead that overlap it are forgot. The
//! rest stay pending, for the next write that touches them or the
//! submission's last flush. Work that may write anywhere (`Step::Run`) still
//! flushes all. The textures are handled as before.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::Batch;

/// Whether it is on (`=0` turns it off). Each step asks again, so in
/// diagnostics builds `VICTORY_AB` may turn it on and off while the title runs.
pub(super) fn on() -> bool {
    crate::toggled!("VICTORY_FLUSH_NARROW")
}

/// What `toggled!` reads the variable as: unset or anything but `0` is on.
#[cfg(test)]
fn parse(val: Option<&str>) -> bool {
    !matches!(val, Some("0"))
}

/// Since the last frame line: title writes seen, of them those that had
/// refreshes to do, the refreshes done before them, the refreshes left
/// pending, the ranges refreshed ahead that were kept.
static STATS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Whether `[from, to)` and `[dst, end)` share a byte.
fn overlaps(from: usize, to: usize, dst: usize, end: usize) -> bool {
    from < end && dst < to
}

impl Batch {
    /// [`Batch::title_written`] for a step that writes `[dst, dst + len)` of
    /// title memory only.
    pub(super) fn title_written_at(&mut self, by: super::lean::Cause, dst: usize, len: usize) {
        let end = dst.saturating_add(len);
        STATS[0].fetch_add(1, Relaxed);
        // What the write touches is not as the refresh sets left it
        // (`VICTORY_REFRESH_SET`), and nothing declared before it holds.
        self.sets.dropped(dst, end);
        self.declared.clear();
        self.ahead
            .retain(|_, &mut (from, to)| !overlaps(from, to, dst, end));
        STATS[4].fetch_add(self.ahead.len() as u64, Relaxed);
        self.textures.clear();
        if !self
            .refreshes
            .iter()
            .any(|&(_, from, to)| overlaps(from, to, dst, end))
        {
            // Nothing pending lies under the write: the textures as before,
            // the refreshes (and their index) left as they are.
            self.refresh_textures();
            STATS[3].fetch_add(self.refreshes.len() as u64, Relaxed);
            return;
        }
        let (due, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.refreshes)
            .into_iter()
            .partition(|&(_, from, to)| overlaps(from, to, dst, end));
        STATS[1].fetch_add(1, Relaxed);
        STATS[2].fetch_add(due.len() as u64, Relaxed);
        STATS[3].fetch_add(kept.len() as u64, Relaxed);
        self.refreshes = due;
        self.flush(by);
        self.refreshes = kept;
        self.index.clear();
        for (at, (shadow, ..)) in self.refreshes.iter().enumerate() {
            self.index
                .insert(std::sync::Arc::as_ptr(shadow) as usize, at);
        }
    }
}

/// The frame line's line, when on.
pub(super) fn line(frames: f64) {
    if !on() {
        return;
    }
    let [seen, flushed, done, kept, ahead] =
        [0, 1, 2, 3, 4].map(|i| STATS[i].swap(0, Relaxed) as f64 / frames);
    tracing::info!(
        "title writes in submissions' steps per frame (VICTORY_FLUSH_NARROW=1): {seen:.1}, {flushed:.1} of them with \
         refreshes under them done first ({done:.0} refreshes), {kept:.0} refreshes left pending past them, {ahead:.0} \
         ranges refreshed ahead kept"
    );
}

#[cfg(test)]
mod tests {
    use super::{overlaps, parse};

    #[test]
    fn overlap_is_a_shared_byte() {
        assert!(overlaps(0, 10, 9, 20));
        assert!(!overlaps(0, 10, 10, 20));
        assert!(!overlaps(20, 30, 10, 20));
        assert!(overlaps(5, 6, 0, 100));
    }

    #[test]
    fn default_on_and_explicit_off() {
        assert!(parse(None));
        assert!(parse(Some("1")));
        assert!(!parse(Some("0")));
    }
}
