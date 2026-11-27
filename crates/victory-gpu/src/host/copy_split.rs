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
//!
//! The same line also gives the refreshes of copies (`Batch::update`, on
//! whichever thread): how many, how many copied something, their summed
//! time, and of it the write watch's answers (`crate::watch::writes` in
//! `Shadow::update_free`). Set against the frame line's wall-time "copies",
//! it says whether the time is in the refreshes themselves or in the
//! submitting thread waiting for the threads beside it.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Whether reads are timed.
pub(crate) fn on() -> bool {
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

/// Since the last frame line: refreshes, those that copied something, their
/// nanoseconds summed over the threads, askings of the write watch in them,
/// their nanoseconds.
static REFRESHES: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// A refresh that began at `began` has ended, having copied `copied` bytes.
pub(super) fn refreshed(began: std::time::Instant, copied: usize) {
    let ns = began.elapsed().as_nanos() as u64;
    REFRESHES[0].fetch_add(1, Ordering::Relaxed);
    REFRESHES[1].fetch_add(u64::from(copied > 0), Ordering::Relaxed);
    REFRESHES[2].fetch_add(ns, Ordering::Relaxed);
}

/// An asking of the write watch in a refresh that began at `began` has ended.
pub(crate) fn asked(began: std::time::Instant) {
    REFRESHES[3].fetch_add(1, Ordering::Relaxed);
    REFRESHES[4].fetch_add(began.elapsed().as_nanos() as u64, Ordering::Relaxed);
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
    let [refreshes, copying, ns, asks, ask_ns] = [0, 1, 2, 3, 4].map(|i| REFRESHES[i].swap(0, Ordering::Relaxed));
    tracing::info!(
        "refreshes of copies per frame (VICTORY_COPY_SPLIT=1): {:.0}, {:.0} of them copied something, {:.1} ms summed \
         over the threads; of that {:.0} askings of the write watch, {:.1} ms",
        refreshes as f64 / frames,
        copying as f64 / frames,
        ns as f64 / 1e6 / frames,
        asks as f64 / frames,
        ask_ns as f64 / 1e6 / frames
    );
}
