//! Where the submitting thread's wait for the kept threads goes in the
//! refreshes in front of a submission's steps (`VICTORY_FRONT_WAIT=1`, off by
//! default; a measurement only, it changes no work).
//!
//! Profile G-047814cb: `refresh_front` is 40 % of the submitting thread's
//! preparing, 25 % of it standing at the join of `wide_each`. This times every
//! piece of a front flush (a texture or a host copy's span) and says, per
//! frame: how long the pieces were on all threads and on this one, the longest
//! piece of each flush, how long this thread stood while other threads' pieces
//! were still running, and how long after the last piece ended it took to
//! return. One slow piece, too few threads and an uneven split read apart:
//! the longest pieces near the stand, the pieces' sum over the threads near
//! the flushes' time, or the stand after the last piece (the join itself).

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_FRONT_WAIT"))
}

/// One front flush being timed.
pub(super) struct Probe {
    started: Instant,
    caller: std::thread::ThreadId,
    /// Nanoseconds of pieces: [0] textures, [1] copies refreshed, [2] on the
    /// caller, [3] copies a 'nothing new' check let go.
    sums: [AtomicU64; 4],
    /// The longest piece, and 1 if it was a texture.
    longest: AtomicU64,
    longest_texture: AtomicU64,
    /// From the start: the end of the caller's last piece, the end of the
    /// last piece of any thread, the first start of a piece on another thread.
    caller_end: AtomicU64,
    last_end: AtomicU64,
    first_other: AtomicU64,
    pieces: AtomicU64,
    pieces_nothing: AtomicU64,
}

/// Per frame: [0] flushes, [1] their ns, [2] texture ns, [3] copy ns, [4]
/// ns on the caller, [5] longest pieces' ns summed, [6] flushes whose longest
/// was a texture, [7] ns the caller stood while others' pieces ran, [8] ns
/// after the last piece, [9] ns until another thread took a first piece, [10]
/// flushes another thread took a piece in, [11] pieces, [12] ns of 'nothing
/// new' checks, [13] how many.
static FRAME: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];

fn ns(since: Instant) -> u64 {
    since.elapsed().as_nanos() as u64
}

/// A probe for the flush about to start; none with the switch off.
pub(super) fn begin() -> Option<Arc<Probe>> {
    on().then(|| {
        Arc::new(Probe {
            started: Instant::now(),
            caller: std::thread::current().id(),
            sums: [const { AtomicU64::new(0) }; 4],
            longest: AtomicU64::new(0),
            longest_texture: AtomicU64::new(0),
            caller_end: AtomicU64::new(0),
            last_end: AtomicU64::new(0),
            first_other: AtomicU64::new(u64::MAX),
            pieces: AtomicU64::new(0),
            pieces_nothing: AtomicU64::new(0),
        })
    })
}

/// `work`, timed piece by piece under `probe`; the numbers under `textures`
/// are textures, and `work` says false for a copy a 'nothing new' check let
/// go. With no probe, `work` as it is.
pub(super) fn wrap(
    probe: Option<Arc<Probe>>,
    textures: usize,
    work: impl Fn(usize) -> bool + Send + Sync + 'static,
) -> impl Fn(usize) + Send + Sync + 'static {
    move |i| {
        let Some(p) = &probe else {
            work(i);
            return;
        };
        let from = ns(p.started);
        let mine = std::thread::current().id() == p.caller;
        if !mine {
            p.first_other.fetch_min(from, Ordering::Relaxed);
        }
        let did = work(i);
        let to = ns(p.started);
        let took = to - from;
        let texture = i < textures;
        p.sums[if texture {
            0
        } else if did {
            1
        } else {
            3
        }]
        .fetch_add(took, Ordering::Relaxed);
        if !did {
            p.pieces_nothing.fetch_add(1, Ordering::Relaxed);
        }
        if mine {
            p.sums[2].fetch_add(took, Ordering::Relaxed);
            p.caller_end.fetch_max(to, Ordering::Relaxed);
        }
        if p.longest.fetch_max(took, Ordering::Relaxed) < took {
            p.longest_texture
                .store(u64::from(texture), Ordering::Relaxed);
        }
        p.last_end.fetch_max(to, Ordering::Relaxed);
        p.pieces.fetch_add(1, Ordering::Relaxed);
    }
}

/// The flush under `probe` is done (all its pieces have run).
pub(super) fn end(probe: Option<Arc<Probe>>) {
    let Some(p) = probe else {
        return;
    };
    let wall = ns(p.started);
    let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let (last, caller_end) = (get(&p.last_end), get(&p.caller_end));
    let first_other = get(&p.first_other);
    let add = |i: usize, v: u64| FRAME[i].fetch_add(v, Ordering::Relaxed);
    add(0, 1);
    add(1, wall);
    add(2, get(&p.sums[0]));
    add(3, get(&p.sums[1]));
    add(4, get(&p.sums[2]));
    add(5, get(&p.longest));
    add(6, get(&p.longest_texture));
    add(7, last.saturating_sub(caller_end));
    add(8, wall.saturating_sub(last));
    if first_other != u64::MAX {
        add(9, first_other);
        add(10, 1);
    }
    add(11, get(&p.pieces));
    add(12, get(&p.sums[3]));
    add(13, get(&p.pieces_nothing));
}

/// Logs and clears the counters, per frame; nothing with the switch off.
pub(super) fn line(frames: f64) {
    if !on() {
        return;
    }
    let c: [f64; 14] = std::array::from_fn(|i| FRAME[i].swap(0, Ordering::Relaxed) as f64);
    if c[0] == 0.0 {
        return;
    }
    let f = frames.max(1.0);
    let ms = |i: usize| c[i] / 1e6 / f;
    let helper_start = if c[10] > 0.0 { c[9] / 1e3 / c[10] } else { 0.0 };
    tracing::info!(
        "front refreshes and the kept threads, per frame (VICTORY_FRONT_WAIT=1): {:.1} flushes in {:.2} ms, {:.0} pieces; \
         pieces' time on all threads {:.2} ms textures + {:.2} ms copies refreshed + {:.2} ms in {:.0} 'nothing new' \
         checks, {:.2} ms of it on this thread; the longest piece \
         of each flush summed {:.2} ms (a texture in {:.1} of the flushes); this thread stood {:.2} ms while pieces of other \
         threads still ran and {:.2} ms after the last piece ended; another thread took its first piece {:.0} us after a \
         flush began ({:.1} flushes had one)",
        c[0] / f,
        ms(1),
        c[11] / f,
        ms(2),
        ms(3),
        ms(12),
        c[13] / f,
        ms(4),
        ms(5),
        c[6] / f,
        ms(7),
        ms(8),
        helper_start,
        c[10] / f
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_without_probe_calls_work() {
        let n = Arc::new(AtomicU64::new(0));
        let m = n.clone();
        let w = wrap(None, 1, move |i| {
            m.fetch_add(i as u64 + 1, Ordering::Relaxed);
            true
        });
        w(0);
        w(2);
        assert_eq!(n.load(Ordering::Relaxed), 4);
    }
}
