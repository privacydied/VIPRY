//! What the title holds through our memory calls, for
//! `XMemGetWorkingSetStatistics`'s "title used" (`VICTORY_TITLE_USED=1`, off
//! by default). Off, the title is told all our process has committed (about
//! 27 GB in the world against a limit of 13.5 GB: the layer's memory and the
//! card's included). On, it is told the bytes it holds through
//! `XMemAllocDefault`, `XMemVirtualAlloc` commits and physical pages, at most
//! its limit. Asked at every call, so `VICTORY_AB` can turn it on and off
//! while the title runs. The books are kept whether it is on or not, so they
//! are right the moment it turns on.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

const MEM_COMMIT: u32 = 0x1000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;

/// Whether the title is told what it holds rather than our process's commit.
pub(crate) fn on() -> bool {
    #[cfg(all(windows, feature = "diagnostics"))]
    if let Some(on) = victory_gpu::switched("VICTORY_TITLE_USED") {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TITLE_USED").is_some_and(|v| v == "1"))
}

/// Whether 5 s have gone by since this last said yes: how often the title's
/// asking is logged, so that each `VICTORY_AB` phase shows it.
pub(crate) fn due() -> bool {
    use std::sync::atomic::AtomicU64;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = START.get_or_init(std::time::Instant::now).elapsed().as_secs() + 5;
    let last = LAST.load(Relaxed);
    now >= last + 5 && LAST.compare_exchange(last, now, Relaxed, Relaxed).is_ok()
}

/// One reservation the title made with `XMemVirtualAlloc`: its size and how
/// much of it is committed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Region {
    reserved: usize,
    committed: usize,
}

/// The title's memory, by where it came from.
#[derive(Default)]
pub(crate) struct Books {
    heap: AtomicUsize,
    physical: AtomicUsize,
    regions: Mutex<BTreeMap<usize, Region>>,
    committed: AtomicUsize,
}

impl Books {
    pub(crate) const fn new() -> Self {
        Self {
            heap: AtomicUsize::new(0),
            physical: AtomicUsize::new(0),
            regions: Mutex::new(BTreeMap::new()),
            committed: AtomicUsize::new(0),
        }
    }

    /// All the title holds.
    pub(crate) fn used(&self) -> usize {
        self.heap.load(Relaxed) + self.physical.load(Relaxed) + self.committed.load(Relaxed)
    }

    pub(crate) fn heap_allocated(&self, size: usize) {
        self.heap.fetch_add(size, Relaxed);
    }

    pub(crate) fn heap_freed(&self, size: usize) {
        sub(&self.heap, size);
    }

    pub(crate) fn physical_allocated(&self, bytes: usize) {
        self.physical.fetch_add(bytes, Relaxed);
    }

    pub(crate) fn physical_freed(&self, bytes: usize) {
        sub(&self.physical, bytes);
    }

    /// `XMemVirtualAlloc` gave `at` for `size` bytes of `kind`; `base` is what
    /// the title passed (null: a new reservation).
    pub(crate) fn virtual_allocated(&self, base: usize, at: usize, size: usize, kind: u32) {
        let commit = kind & MEM_COMMIT != 0;
        let mut regions = self.regions.lock().unwrap_or_else(|e| e.into_inner());
        if base == 0 {
            let committed = if commit { size } else { 0 };
            if let Some(old) = regions.insert(at, Region { reserved: size, committed }) {
                sub(&self.committed, old.committed);
            }
            self.committed.fetch_add(committed, Relaxed);
            return;
        }
        if !commit {
            return;
        }
        // Committing pages of a reservation it made.
        // shortcut: committing pages that already are counts them again, up
        // to the reservation's size.
        if let Some(region) = containing(&mut regions, at) {
            let grown = size.min(region.reserved - region.committed);
            region.committed += grown;
            self.committed.fetch_add(grown, Relaxed);
        }
    }

    /// The title's `VirtualFree` of `address`.
    pub(crate) fn virtual_freed(&self, address: usize, size: usize, kind: u32) {
        let mut regions = self.regions.lock().unwrap_or_else(|e| e.into_inner());
        if kind & MEM_RELEASE != 0 {
            if let Some(gone) = regions.remove(&address) {
                sub(&self.committed, gone.committed);
            }
        } else if kind & MEM_DECOMMIT != 0
            && let Some(region) = containing(&mut regions, address)
        {
            let size = if size == 0 { region.reserved } else { size };
            let shrunk = size.min(region.committed);
            region.committed -= shrunk;
            sub(&self.committed, shrunk);
        }
    }
}

/// The region `at` lies in.
fn containing(regions: &mut BTreeMap<usize, Region>, at: usize) -> Option<&mut Region> {
    let (&base, region) = regions.range_mut(..=at).next_back()?;
    (at < base + region.reserved).then_some(region)
}

fn sub(counter: &AtomicUsize, n: usize) {
    let _ = counter.fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(n)));
}

/// The title's books.
pub(crate) static BOOKS: Books = Books::new();

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    #[test]
    fn heap_blocks_count_until_freed() {
        let books = Books::new();
        books.heap_allocated(3 * MIB);
        books.heap_allocated(MIB);
        assert_eq!(books.used(), 4 * MIB);
        books.heap_freed(3 * MIB);
        assert_eq!(books.used(), MIB);
        // A free we never saw the allocation of leaves no debt.
        books.heap_freed(8 * MIB);
        assert_eq!(books.used(), 0);
    }

    #[test]
    fn physical_pages_count_until_freed() {
        let books = Books::new();
        books.physical_allocated(64 * MIB);
        books.physical_freed(16 * MIB);
        assert_eq!(books.used(), 48 * MIB);
    }

    #[test]
    fn a_reservation_counts_what_is_committed_of_it() {
        let books = Books::new();
        let at = 0x1_0000_0000;
        books.virtual_allocated(0, at, 64 * MIB, 0x2000);
        assert_eq!(books.used(), 0);
        books.virtual_allocated(at + MIB, at + MIB, 4 * MIB, MEM_COMMIT);
        assert_eq!(books.used(), 4 * MIB);
        // Committing more than is reserved counts the reservation at most.
        books.virtual_allocated(at, at, 100 * MIB, MEM_COMMIT);
        assert_eq!(books.used(), 64 * MIB);
        books.virtual_freed(at, 16 * MIB, MEM_DECOMMIT);
        assert_eq!(books.used(), 48 * MIB);
        books.virtual_freed(at, 0, MEM_RELEASE);
        assert_eq!(books.used(), 0);
    }

    #[test]
    fn committed_at_once_and_released() {
        let books = Books::new();
        books.virtual_allocated(0, 0x2000_0000, 2 * MIB, MEM_COMMIT | 0x2000);
        books.virtual_allocated(0, 0x3000_0000, 6 * MIB, MEM_COMMIT | 0x2000);
        assert_eq!(books.used(), 8 * MIB);
        // Memory not from our calls, and commits outside any reservation.
        books.virtual_freed(0x5000_0000, 0, MEM_RELEASE);
        books.virtual_allocated(0x6000_0000, 0x6000_0000, MIB, MEM_COMMIT);
        assert_eq!(books.used(), 8 * MIB);
        books.virtual_freed(0x2000_0000, 0, MEM_RELEASE);
        assert_eq!(books.used(), 6 * MIB);
        // Decommit of 0 bytes: all of it.
        books.virtual_freed(0x3000_0000, 0, MEM_DECOMMIT);
        assert_eq!(books.used(), 0);
    }
}
