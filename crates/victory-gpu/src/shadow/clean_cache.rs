//! Remembers refreshes that had nothing to copy while their title blocks
//! stay read-only (`VICTORY_CLEAN_REFRESH=1`, off by default).
//!
//! A frame asks about the same ranges of the same copies tens of times.
//! [`crate::shadow::Shadow::nothing_new`] answers most of those "nothing"
//! from under the journal and the copy's locks. While the blocks those
//! ranges lie in are read-only, no title thread can have written them, so
//! that answer cannot change. Keep the answer in a few slots of the copy
//! and answer from atomics alone until a write clears the blocks' bits or
//! shaders declare a range of the copy.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Whether the cache is used.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_CLEAN_REFRESH"))
}

const SLOTS: usize = 16;

struct Slot {
    /// Even when the other fields can be read, odd while they are written.
    version: AtomicUsize,
    from: AtomicUsize,
    to: AtomicUsize,
    pending: AtomicU64,
    watched: AtomicU8,
    clean: AtomicBool,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            version: AtomicUsize::new(0),
            from: AtomicUsize::new(0),
            to: AtomicUsize::new(0),
            pending: AtomicU64::new(0),
            watched: AtomicU8::new(0),
            clean: AtomicBool::new(false),
        }
    }
}

/// The answers kept for one copy. Writers take one lock; readers do not.
pub(crate) struct CleanCache {
    slots: [Slot; SLOTS],
    next: AtomicUsize,
    writer: Mutex<()>,
}

impl Default for CleanCache {
    fn default() -> CleanCache {
        CleanCache {
            slots: std::array::from_fn(|_| Slot::new()),
            next: AtomicUsize::new(0),
            writer: Mutex::new(()),
        }
    }
}

impl CleanCache {
    /// Whether a range is still known to copy nothing. The caller gives
    /// the copy's number of declared writes and its watched state: a
    /// change of either makes the answer unusable.
    pub(crate) fn hit(&self, from: usize, to: usize, pending: u64, watched: u8) -> bool {
        self.hit_keys(from, to, pending, watched) && crate::watch::published(from, to)
    }

    /// The atomics-only part of [`CleanCache::hit`], without the blocks'
    /// bits.
    fn hit_keys(&self, from: usize, to: usize, pending: u64, watched: u8) -> bool {
        for slot in &self.slots {
            let before = slot.version.load(Ordering::Acquire);
            if before & 1 != 0 || !slot.clean.load(Ordering::Acquire) {
                continue;
            }
            if slot.from.load(Ordering::Relaxed) != from
                || slot.to.load(Ordering::Relaxed) != to
                || slot.pending.load(Ordering::Relaxed) != pending
                || slot.watched.load(Ordering::Relaxed) != watched
            {
                continue;
            }
            if slot.version.load(Ordering::Acquire) == before {
                return true;
            }
        }
        false
    }

    /// Notes that a range has nothing to copy. Called only while its
    /// blocks are read-only, so the answer holds until a write opens them.
    pub(crate) fn remember(&self, from: usize, to: usize, pending: u64, watched: u8) {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &self.slots[self.next.fetch_add(1, Ordering::Relaxed) % SLOTS];
        slot.version.fetch_add(1, Ordering::AcqRel);
        slot.from.store(from, Ordering::Relaxed);
        slot.to.store(to, Ordering::Relaxed);
        slot.pending.store(pending, Ordering::Relaxed);
        slot.watched.store(watched, Ordering::Relaxed);
        slot.clean.store(true, Ordering::Relaxed);
        slot.version.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::CleanCache;

    #[test]
    fn empty_is_not_hit() {
        let cache = CleanCache::default();
        assert!(!cache.hit(0, 4096, 0, 1));
    }

    #[test]
    fn a_remembered_range_is_hit_only_with_its_keys() {
        let cache = CleanCache::default();
        assert!(!cache.hit_keys(0, 4096, 3, 1));
        cache.remember(0, 4096, 3, 1);
        assert!(cache.hit_keys(0, 4096, 3, 1));
        assert!(!cache.hit_keys(0, 4096, 4, 1));
        assert!(!cache.hit_keys(0, 4096, 3, 2));
        assert!(!cache.hit_keys(4096, 8192, 3, 1));
    }
}
