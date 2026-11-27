//! Wall time the title spends in each call of a frame handoff
//! (`VICTORY_ASYNC_PRESENT=1`). Title times sum completed calls across
//! threads, and are shown per title thread as well, as two blocked threads
//! can sum to more than a whole frame.
//!
//! Ported from stellarr-studio/kelvaroq 0046-async-present-frame-waits (its
//! per-worker stage clocks are left out): the timers do nothing unless the
//! switch is on.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Call {
    Present,
    Drain,
    Pacing,
    QueueFence,
    FenceLanding,
    Show,
    PresentTurn,
}

static NS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

// Title thread ids from the same window as NS: present, queue fence call,
// blocking fence.
static TITLE_THREADS: Mutex<Vec<(u32, [u64; 3])>> = Mutex::new(Vec::new());

pub(crate) struct Timer(Option<(Call, Instant)>);

impl Drop for Timer {
    fn drop(&mut self) {
        let Some((call, began)) = self.0 else {
            return;
        };
        let ns = began.elapsed().as_nanos() as u64;
        NS[call as usize].fetch_add(ns, Ordering::Relaxed);
        let field = match call {
            Call::Present => 0,
            Call::QueueFence => 1,
            Call::FenceLanding => 2,
            _ => return,
        };
        let id = crate::host::thread_id();
        let mut threads = TITLE_THREADS.lock().unwrap_or_else(|e| e.into_inner());
        match threads.iter_mut().find(|t| t.0 == id) {
            Some((_, sums)) => sums[field] += ns,
            None => {
                let mut sums = [0; 3];
                sums[field] = ns;
                threads.push((id, sums));
            }
        }
    }
}

/// Times the call until the returned value is dropped (nothing when the
/// switch is off).
pub(crate) fn time(call: Call) -> Timer {
    Timer(crate::queue::async_present().then(|| (call, Instant::now())))
}

static QUEUED: AtomicU64 = AtomicU64::new(0);

/// A present returned before its frame was shown.
pub(crate) fn queued() {
    QUEUED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn line(frames: f64) {
    if !crate::queue::async_present() {
        return;
    }
    let queued = QUEUED.swap(0, Ordering::Relaxed) as f64 / frames;
    let ms = |ns: u64| ns as f64 / 1e6 / frames;
    let [present, drain, pacing, queue_fence, fence, show, turn] =
        NS.each_ref().map(|n| ms(n.swap(0, Ordering::Relaxed)));
    tracing::info!(
        "title calls per frame (VICTORY_ASYNC_PRESENT; {queued:.2} presents returned early per displayed frame), in ms: present {present:.1}, \
         of which a drain {drain:.1}, waiting for a frame slot {pacing:.1} and the previous caller {turn:.1}, host present {show:.1}; \
         queue fence wait call {queue_fence:.1}, blocking fence {fence:.1}"
    );
    let callers = std::mem::take(&mut *TITLE_THREADS.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, ns) in callers {
        let times = ns.map(ms);
        if times.iter().any(|&t| t >= 0.5) {
            let [present, queue, fence] = times;
            tracing::info!(
                "title thread {id} per frame in ms: present {present:.1}, queue fence call {queue:.1}, blocking fence {fence:.1}"
            );
        }
    }
}
