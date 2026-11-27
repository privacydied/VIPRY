//! Copies of single buffer views' own bytes ([`super::exact`]) found and
//! made without one lock held over everything (`VICTORY_EXACT_FAST=1`, off
//! by default).
//!
//! Why: the threads that translate lists (`list/thin.rs`) ask for such a copy
//! for every buffer view off its copy's elements, when a slot is made and
//! when it is refreshed. `exact` holds one mutex over the lookup, over the
//! walk that lets idle copies go (a COM reference count asked of each one, on
//! every new copy once 256 are kept), and over making the new buffer
//! (`CreateCommittedResource`, residency, `Map`). In the L2 profile of the
//! base exe 16 % of all samples of the eight list threads stood in that
//! mutex, and 2 % were making a buffer inside it: about one list thread at
//! any moment, which holds its recording, and so its submission, back.
//!
//! Here a lookup takes a read lock only; a new buffer is made with no lock
//! held; the walk over idle copies runs once a frame, or when the copies
//! kept have doubled since the last walk. What is kept, and for how long,
//! is as before.

use super::*;

/// Whether it is asked for.
pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_EXACT_FAST").is_some_and(|v| v == "1"))
}

/// A copy and the asking that last took it.
struct Kept {
    shadow: Arc<Shadow>,
    used: AtomicU64,
}

/// The copies by (start, bytes).
static KEPT: RwLock<BTreeMap<(u64, u64), Kept>> = RwLock::new(BTreeMap::new());
/// Askings so far.
static TICK: AtomicU64 = AtomicU64::new(0);
/// The frame of the last walk over idle copies, and the copies kept after it.
static SWEPT_FRAME: AtomicU64 = AtomicU64::new(u64::MAX);
static SWEPT_LEFT: AtomicUsize = AtomicUsize::new(0);

/// The copies kept now (for the frame line).
pub(super) fn kept() -> usize {
    KEPT.read().unwrap_or_else(|e| e.into_inner()).len()
}

fn take(kept: &Kept, now: u64) -> Arc<Shadow> {
    kept.used.fetch_max(now, Ordering::Relaxed);
    kept.shadow
        .used
        .store(crate::host::moment(), Ordering::Relaxed);
    kept.shadow.clone()
}

/// [`super::exact`] for `[address, address + len)`, already checked; `many`
/// copies are kept before idle ones are looked for, and an idle one stays
/// for `lately` askings.
pub(super) fn exact(address: u64, len: u64, many: usize, lately: u64) -> Option<Arc<Shadow>> {
    let now = TICK.fetch_add(1, Ordering::Relaxed) + 1;
    let key = (address, len);
    if let Some(kept) = KEPT.read().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(take(kept, now));
    }
    // Made with no lock held; one made by another thread meanwhile wins, and
    // this one is let go after the lock (its drop counts copies).
    let (buffer, data) = host()?.shared_buffer(len.next_multiple_of(256))?;
    crate::host::name(&buffer, || format!("title view {address:#x}+{len:#x}"));
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() };
    let made = Arc::new(Shadow::new(buffer, data, gpu, address, len));
    let mut map = KEPT.write().unwrap_or_else(|e| e.into_inner());
    if let Some(kept) = map.get(&key) {
        return Some(take(kept, now));
    }
    if map.len() >= many {
        let frame = crate::frame::current();
        let left = SWEPT_LEFT.load(Ordering::Relaxed).max(many);
        if SWEPT_FRAME.load(Ordering::Relaxed) != frame || map.len() >= 2 * left {
            // Held by the map alone (no list), its buffer by the copy alone
            // (no slot shows it), and not asked for lately.
            map.retain(|_, kept| {
                Arc::strong_count(&kept.shadow) > 1
                    || crate::host::references(&kept.shadow.buffer) > 1
                    || kept.used.load(Ordering::Relaxed) + lately > now
            });
            SWEPT_FRAME.store(frame, Ordering::Relaxed);
            SWEPT_LEFT.store(map.len(), Ordering::Relaxed);
        }
    }
    map.insert(
        key,
        Kept {
            shadow: made.clone(),
            used: AtomicU64::new(now),
        },
    );
    drop(map);
    EXACT_STATS[1].fetch_add(1, Ordering::Relaxed);
    OWN_MADE.fetch_add(1, Ordering::Relaxed);
    Some(made)
}
