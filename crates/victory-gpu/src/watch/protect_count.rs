//! Our changes of protection of title memory, counted by who makes them and
//! how many blocks each takes (diagnostics builds), and made fewer
//! (`VICTORY_PROTECT_MERGE=1`, off by default; flippable with `VICTORY_AB`).
//!
//! Under Wine every change of protection, every write fault, every asking of
//! the write watch and the runtime's mapping of physical pages take one lock
//! of the whole process (ntdll's `virtual_mutex`): 11-18 ms a frame of
//! `VirtualProtect` on the present chain (timeline L3-tl-plain). Which of
//! ours they are, and how many fail, says what to cut.
//!
//! With the switch:
//! - a run of blocks to make read-only is cut where the title's views meet
//!   ([`crate::inplace::view_pieces`]), one call each piece. A call over two
//!   views fails (the host takes a range within one mapping only), and the
//!   whole run was then done block by block: one failed call and one a block.
//! - blocks given write access back together (a fault through a page shown
//!   at several views, a view mapped anew, our own writes) are one call for
//!   each run of neighbours within one view, not one a block.
//!
//! Each piece and run is all title view memory of one mapping, which the
//! guards made read-only: the protection found at its first block (`old`)
//! is checked as before, and a call that fails is done block by block, as
//! before. The switch only groups the calls, so it may change while the
//! title runs.

use super::BLOCK;
#[cfg(feature = "diagnostics")]
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub(super) fn on() -> bool {
    crate::asked_for!("VICTORY_PROTECT_MERGE")
}

/// Who changes a protection.
#[derive(Clone, Copy)]
pub(super) enum By {
    /// Blocks made read-only by a refresh's guard, more than one a call.
    GuardRun,
    /// The same, one block a call (a run of one, or block by block after a
    /// run's call failed).
    GuardBlock,
    /// Another view of a page a copy takes, made read-only.
    Alias,
    /// Blocks given write access back: our own writes, the runtime's
    /// changes, a view mapped anew (a title fault's are [`FaultOpen`],
    /// told by [`in_fault`]).
    Open,
    /// A fault opening its block without the lock (`VICTORY_FAULT_UNLOCKED`).
    FaultUnlocked,
    /// Blocks given write access back by a title fault, under the lock.
    FaultOpen,
}

const BYS: usize = 6;
/// Each with whether the guards' lock is held across the call: all but
/// unlocked faults hold it (written, or shared with renderer port #6).
const NAMES: [&str; BYS] = [
    "guard runs [held]",
    "guard blocks [held]",
    "other views [held]",
    "other opens [held]",
    "unlocked faults [no lock]",
    "fault opens [held]",
];

thread_local! {
    static FAULTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `f`, its opens counted as a title fault's.
pub(super) fn in_fault<T>(f: impl FnOnce() -> T) -> T {
    FAULTING.with(|c| c.set(true));
    let out = f();
    FAULTING.with(|c| c.set(false));
    out
}

/// Per [`By`]: calls, calls that failed, blocks, nanoseconds, and calls of
/// 1, 2-4, 5-16 and more blocks.
#[cfg(feature = "diagnostics")]
static COUNTS: [[AtomicU64; 8]; BYS] = [const { [const { AtomicU64::new(0) }; 8] }; BYS];

/// `VirtualProtect` of blocks `[from, to)` to `new`, counted for `by`
/// (a protection put back that was not ours counts as its caller's):
/// whether it was done, and the protection found at `from`.
pub(super) fn set(by: By, from: usize, to: usize, new: u32) -> (bool, u32) {
    use windows_sys::Win32::System::Memory::VirtualProtect;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    let mut old = 0;
    // SAFETY: title view memory (or a block a caller made read-only); only
    // its protection changes.
    let done = unsafe { VirtualProtect((from * BLOCK) as *const _, (to - from) * BLOCK, new, &mut old) } != 0;
    #[cfg(feature = "diagnostics")]
    {
        let by = match by {
            By::Open if FAULTING.with(|c| c.get()) => By::FaultOpen,
            by => by,
        };
        let c = &COUNTS[by as usize];
        let n = (to - from) as u64;
        c[0].fetch_add(1, Relaxed);
        c[1].fetch_add(u64::from(!done), Relaxed);
        c[2].fetch_add(n, Relaxed);
        c[3].fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
        c[4 + match n {
            0..=1 => 0,
            2..=4 => 1,
            5..=16 => 2,
            _ => 3,
        }]
        .fetch_add(1, Relaxed);
    }
    #[cfg(not(feature = "diagnostics"))]
    let _ = by;
    (done, old)
}

/// `blocks` (sorted, no repeats) cut in runs of neighbours within one of
/// the title's views; a block in no view is a run of its own.
pub(super) fn runs_in_views(blocks: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let first = blocks[i];
        let mut last = first + 1;
        let mut j = i + 1;
        while j < blocks.len() && blocks[j] == last {
            last += 1;
            j += 1;
        }
        // A run of neighbours: cut where views meet.
        let pieces = crate::inplace::view_pieces(first * BLOCK, (last - first) * BLOCK);
        let mut at = first;
        for (start, len) in pieces {
            let (a, b) = (start / BLOCK, (start + len) / BLOCK);
            for block in at..a.min(last) {
                out.push((block, block + 1));
            }
            if a < b {
                out.push((a, b));
            }
            at = b.max(at);
        }
        for block in at..last {
            out.push((block, block + 1));
        }
        i = j;
    }
    out
}

/// Logs and clears the counts, per frame; diagnostics builds.
#[cfg(feature = "diagnostics")]
pub(crate) fn line(frames: f64) {
    let f = frames.max(1.0);
    let mut parts = Vec::new();
    let mut total = [0f64; 4];
    for (by, name) in NAMES.iter().enumerate() {
        let c: [u64; 8] = std::array::from_fn(|i| COUNTS[by][i].swap(0, Relaxed));
        if c[0] == 0 {
            continue;
        }
        for i in 0..4 {
            total[i] += c[i] as f64;
        }
        parts.push(format!(
            "{name} {:.0} ({:.0} failed, {:.0} blocks, {:.2} ms; of 1/2-4/5-16/more blocks {:.0}/{:.0}/{:.0}/{:.0})",
            c[0] as f64 / f,
            c[1] as f64 / f,
            c[2] as f64 / f,
            c[3] as f64 / 1e6 / f,
            c[4] as f64 / f,
            c[5] as f64 / f,
            c[6] as f64 / f,
            c[7] as f64 / f
        ));
    }
    tracing::info!(
        target: "victory_gpu::host",
        "our changes of protection per frame{}: {:.0} calls, {:.0} failed, {:.0} blocks, {:.2} ms; {}",
        if on() { " (VICTORY_PROTECT_MERGE=1)" } else { "" },
        total[0] / f,
        total[1] / f,
        total[2] / f,
        total[3] / 1e6 / f,
        parts.join(", ")
    );
}

#[cfg(not(feature = "diagnostics"))]
pub(crate) fn line(_: f64) {}

#[cfg(test)]
mod tests {
    use super::{BLOCK, runs_in_views};

    #[test]
    fn runs_are_cut_where_views_meet() {
        // Two views side by side of blocks 0-2 and 3-4 of a made-up place
        // far from anything mapped; block 7 in none.
        let base = 0x7e00_0000_0000usize;
        let b0 = base / BLOCK;
        crate::inplace::title_memory_view(base, 3 * BLOCK, 0xdead_0001, 0);
        crate::inplace::title_memory_view(base + 3 * BLOCK, 2 * BLOCK, 0xdead_0002, 0);
        let blocks: Vec<usize> = [0, 1, 2, 3, 4, 7].iter().map(|b| b0 + b).collect();
        let runs = runs_in_views(&blocks);
        crate::inplace::views_gone(base, 8 * BLOCK);
        assert_eq!(runs, vec![(b0, b0 + 3), (b0 + 3, b0 + 5), (b0 + 7, b0 + 8)]);
        // A gap inside one view: two runs.
        crate::inplace::title_memory_view(base, 4 * BLOCK, 0xdead_0003, 0);
        let runs = runs_in_views(&[b0, b0 + 2, b0 + 3]);
        crate::inplace::views_gone(base, 4 * BLOCK);
        assert_eq!(runs, vec![(b0, b0 + 1), (b0 + 2, b0 + 4)]);
    }
}
