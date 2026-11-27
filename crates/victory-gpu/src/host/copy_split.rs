//! How fast title memory is read for copies (`VICTORY_COPY_SPLIT=1`; off by
//! default): every read of title memory into a copy of ours
//! ([`super::copy_mapped`] with title memory as the source) is timed, on
//! whichever thread does it, and the frame line's rate line says how many
//! there were, how many bytes, the threads' summed time, and how many took
//! a millisecond or more.
//!
//! Why: the frame line's "copies" is 25-80 ms for about 80 MB a frame on
//! this PC, 1.3-2.8 GB/s, where a copy in memory that is there runs at
//! 10 GB/s and more on one thread, and the same megabytes take twice as
//! long in one second as in the next. If the reads are slow and many take
//! a millisecond, the pages were not in memory (swap): then copies get
//! shorter with more free memory, not with new copy code.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Whether reads are timed.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COPY_SPLIT").is_some_and(|v| v == "1"))
}

/// Since the last frame line: reads, bytes, nanoseconds summed over the
/// threads, reads of a millisecond or more, their nanoseconds.
static READS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// A read of `len` bytes of title memory that began at `began` has ended.
pub(super) fn read(began: std::time::Instant, len: usize) {
    let ns = began.elapsed().as_nanos() as u64;
    READS[0].fetch_add(1, Ordering::Relaxed);
    READS[1].fetch_add(len as u64, Ordering::Relaxed);
    READS[2].fetch_add(ns, Ordering::Relaxed);
    if ns >= 1_000_000 {
        READS[3].fetch_add(1, Ordering::Relaxed);
        READS[4].fetch_add(ns, Ordering::Relaxed);
    }
}

/// For the frame line: the reads per frame.
pub(super) fn line(frames: f64) {
    if !on() {
        return;
    }
    let [reads, bytes, ns, slow, slow_ns] = [0, 1, 2, 3, 4].map(|i| READS[i].swap(0, Ordering::Relaxed));
    let mb = bytes as f64 / 1e6;
    tracing::info!(
        "title memory read for copies per frame (VICTORY_COPY_SPLIT=1): {:.0} reads, {:.1} MB, {:.1} ms summed over the \
         threads ({:.2} GB/s a thread); {:.1} reads of 1 ms or more, {:.1} ms of them",
        reads as f64 / frames,
        mb / frames,
        ns as f64 / 1e6 / frames,
        if ns > 0 { bytes as f64 / ns as f64 } else { 0.0 },
        slow as f64 / frames,
        slow_ns as f64 / 1e6 / frames
    );
}
