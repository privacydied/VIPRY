//! What the line of submissions ([`super::behind`]) stands at while the
//! title waits for it (`VICTORY_LINE_STANDS=1`; off by default).
//!
//! The title's threads wait about 200 ms a frame at their presents (the
//! present waits until the one before it was sent), while our thread that
//! serves the line works about 80 ms. The rest of the time the line is
//! empty, or its oldest entry stands at something: a fence wait, a memory
//! value, lists still being translated, or a present behind them. With the
//! switch each wait of the serving thread is timed and put on what the
//! oldest entry was at that moment, and the time it spent looking at the
//! line and doing what was free is timed too.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// What the oldest entry of the line is.
pub(super) const EMPTY: usize = 0;
pub(super) const FENCE: usize = 1;
pub(super) const VALUE: usize = 2;
pub(super) const LISTS: usize = 3;
pub(super) const PRESENT: usize = 4;
pub(super) const OTHER: usize = 5;
/// The serving thread looking at the line and doing what is free to run.
const LOOKING: usize = 6;

/// Nanoseconds per kind since the last line, and the waits of each kind.
static NS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
static WAITS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// Whether the line's waits are timed.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LINE_STANDS").is_some_and(|v| v == "1"))
}

/// A wait of the serving thread begins, the oldest entry being of `kind`
/// (`None`: the line is empty). `None` back when the switch is off.
pub(super) fn begin(kind: Option<usize>) -> Option<(usize, Instant)> {
    on().then(|| (kind.unwrap_or(EMPTY), Instant::now()))
}

/// The serving thread looks at the line and does what is free.
pub(super) fn looking() -> Option<(usize, Instant)> {
    on().then(|| (LOOKING, Instant::now()))
}

/// What [`begin`] or [`looking`] began has ended.
pub(super) fn end(begun: Option<(usize, Instant)>) {
    if let Some((kind, at)) = begun {
        NS[kind].fetch_add(at.elapsed().as_nanos() as u64, Relaxed);
        WAITS[kind].fetch_add(1, Relaxed);
    }
}

/// For the frame-rate line.
pub(super) fn line(frames: f64) {
    if !on() {
        return;
    }
    let ms: Vec<f64> = NS.iter().map(|n| n.swap(0, Relaxed) as f64 / 1e6 / frames).collect();
    let n: Vec<f64> = WAITS.iter().map(|w| w.swap(0, Relaxed) as f64 / frames).collect();
    tracing::info!(
        "the line's serving thread per frame (VICTORY_LINE_STANDS=1): {:.1} ms looking at it and doing what was free \
         ({:.0} times); waiting {:.1} ms with the line empty, and with its oldest entry standing {:.1} ms at a fence \
         wait ({:.0} waits), {:.1} ms at a memory value ({:.0}), {:.1} ms at lists still being translated ({:.0}), \
         {:.1} ms at a present ({:.0}), {:.1} ms at other work ({:.0})",
        ms[LOOKING],
        n[LOOKING],
        ms[EMPTY],
        ms[FENCE],
        n[FENCE],
        ms[VALUE],
        n[VALUE],
        ms[LISTS],
        n[LISTS],
        ms[PRESENT],
        n[PRESENT],
        ms[OTHER],
        n[OTHER]
    );
}
