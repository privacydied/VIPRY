//! The guards' read-only looks with no lock at all (`VICTORY_FREE_LOOKS=1`,
//! off by default).
//!
//! The guards' lock costs the submitting thread up to ~30 ms a frame: ~11 ms
//! waiting for it itself, and ~20 ms waiting for the refresh helpers, which
//! do CPU work 0.35 of 4 threads because they wait for it too (agent-1's
//! numbers, profile G-047814cb). Most of what they wait for it for is a look:
//! is the range's first block guarded, is the whole range clean, which holes
//! to ask about, is anything left written; and [`super::publish`]'s note of
//! blocks made clean. `VICTORY_SHARED_LOOKS` makes those looks shared, but a
//! shared look still waits for every holder alone (faults of the title,
//! another refresh protecting what it found written, the runtime's changes).
//!
//! The guards' block sets are of atomic words already (renderer port #6), so
//! a look can read them with no lock. With the switch, the four sets live in
//! the statics here and [`super::Guards`] refers to them, so who holds the
//! lock alone changes them as before, and a look reads them as they are.
//! Why that sees no less than a shared look:
//!
//! - Whatever the title did before it submitted (a write fault, a mapping or
//!   unmapping told to us) is done, with its notes, before we prepare that
//!   submission: a look sees it as the lock's holder left it.
//! - What happens meanwhile happens as it would just after a shared look.
//!   Every change that makes a block not clean takes its clean note (and its
//!   held note) away before it clears [`super::PUBLISHED`], and before the
//!   block can be written; a look that saw the note came before the write.
//! - A look that sees a half-made change (a run being protected, its clean
//!   notes going in) sees fewer clean blocks, and goes the locked way.
//! - [`publish`] sets a block's published bit, then looks again: if the
//!   clean and held notes are gone by then, or the block was put in beside
//!   its guard or opened by a fault without the lock, it clears the bit
//!   itself; else whoever takes the note after clears the bit after it.
//!
//! Not with renderer port #6 (its looks are shared already, and its faults
//! change the sets under the shared lock and claims), nor while faults that
//! opened blocks without the lock wait to be settled.

use super::{BESIDE, Blocks, OPENING, PUBLISHED, TOLD, in_word, unsettled};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// [`super::Guards`]' block sets: read-only by our hand, clean, absent, held.
pub(super) static PROTECTED_BLOCKS: Blocks = Blocks::new();
pub(super) static CLEAN_BLOCKS: Blocks = Blocks::new();
pub(super) static ABSENT_BLOCKS: Blocks = Blocks::new();
pub(super) static HELD_BLOCKS: Blocks = Blocks::new();

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_FREE_LOOKS"))
}

/// Whether the guards' looks are made with no lock now.
pub(super) fn now() -> bool {
    on() && !super::port_guard() && super::TO_SETTLE.load(Ordering::SeqCst) == 0
}

/// Since the last frame line: [0] first looks, [1] guards that needed no
/// lock at all, [2] publishes.
static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Whether `first` is guarded or noted absent (a view, then).
pub(super) fn known(first: usize) -> bool {
    COUNTS[0].fetch_add(1, Ordering::Relaxed);
    PROTECTED_BLOCKS.contains(first) || ABSENT_BLOCKS.contains(first)
}

/// `None` if `[first, last)` is wholly clean, nothing was put in beside its
/// guards and no fault opened any of it without the lock; else its absent
/// blocks not held (the holes to ask about).
pub(super) fn unsure(first: usize, last: usize) -> Option<Vec<usize>> {
    if CLEAN_BLOCKS.range(first..last).count() == last - first
        && !BESIDE.any(first, last)
        && !unsettled(first, last)
    {
        return None;
    }
    Some(
        ABSENT_BLOCKS
            .range(first..last)
            .filter(|block| !HELD_BLOCKS.contains(*block))
            .collect(),
    )
}

/// Whether no block of `[first, last)` would be told as written: each clean
/// (and not put in beside its guard) or absent, and none being opened. The
/// guard has nothing to do under the lock then.
pub(super) fn nothing_written(first: usize, last: usize) -> bool {
    let none = (first / 64..last.div_ceil(64)).all(|word| {
        let known = (CLEAN_BLOCKS.word(word) & !BESIDE.word(word) | ABSENT_BLOCKS.word(word))
            & !OPENING.word(word)
            & !TOLD.word(word);
        !known & in_word(word * 64, first, last) == 0
    });
    if none {
        COUNTS[1].fetch_add(1, Ordering::Relaxed);
    }
    none
}

/// [`super::publish`] with no lock.
pub(super) fn publish(pages: &[usize]) {
    COUNTS[2].fetch_add(1, Ordering::Relaxed);
    let noted = |page: usize| CLEAN_BLOCKS.contains(page) || HELD_BLOCKS.contains(page);
    for &page in pages {
        if noted(page) && !unsettled(page, page + 1) {
            PUBLISHED.set(page);
            // (The notes taken since, put there since, or opened since: the
            // bit goes again.)
            if !noted(page) || BESIDE.any(page, page + 1) || unsettled(page, page + 1) {
                PUBLISHED.clear(page);
            }
        }
    }
}

/// Logs and clears the counts, per frame; nothing with the switch off.
pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    let c: [f64; 3] =
        std::array::from_fn(|i| COUNTS[i].swap(0, Ordering::Relaxed) as f64 / frames.max(1.0));
    tracing::info!(
        target: "victory_gpu::host",
        "the guards' looks with no lock, per frame (VICTORY_FREE_LOOKS=1): {:.0} first looks, {:.0} guards that needed no lock \
         at all, {:.0} publishes",
        c[0],
        c[1],
        c[2]
    );
}
