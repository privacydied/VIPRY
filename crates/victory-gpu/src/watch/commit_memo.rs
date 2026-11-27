//! Runs of uncommitted blocks the kernel named, kept for the title's frame
//! (`VICTORY_COMMIT_MEMO=1`, off by default).
//!
//! A refresh of a host copy asks the kernel about each block the guards hold
//! as absent that is not a known placeholder hole: is it still uncommitted
//! ([`super::uncommitted_end`], VirtualQuery)? Such a range is never known
//! clean as a whole, so every refresh of it walks its holes again, in each
//! of a frame's ~42 flushes. Under Wine each question takes the process's
//! virtual-memory lock, and the submitting thread and the wide threads take
//! it in turns. In a profile (L4-c361180c, WAIT_EACH on) the questions were
//! 22 % of the submitting thread's preparing and 13 % of the wide threads'.
//!
//! With the switch a run of uncommitted blocks the kernel named answers for
//! every block in it until the title's next frame begins
//! ([`crate::frame::current`]), or until the runtime tells of a change to
//! memory the run touches (`title_memory_*`, a lazy view made real): then
//! only the touched runs are let go of. A question asked across a told
//! change is not kept. Committed answers are not kept: the walk takes such a
//! block out of its absent blocks, so it is not asked about again.
//!
//! What is given up: a commit of such memory that nobody tells us of (the
//! title's own VirtualAlloc reaches Windows directly) is seen at the first
//! walk of the next frame, not at the next walk. The same rule in Xeltax's
//! fork (`VICTORY_GUARD_ABSENT_KNOWN=1`, a7f24db9) re-asked one answer in
//! 64 and found 0 of 372,487 wrong. Before the frame rule this module kept
//! answers for one submission and forgot all of them at any change: that
//! kept 27,000 answers a second but still asked 65,000 (run L2-memo-B-fsync).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

/// Mode of `VICTORY_COMMIT_MEMO`: off, per-frame (`=1`), or across frames
/// until a told memory change touches the run (`=forever` / `=2`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    Frame,
    Forever,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_COMMIT_MEMO").ok().as_deref() {
        Some("1") => Mode::Frame,
        Some("2" | "forever") => Mode::Forever,
        _ => Mode::Off,
    })
}

/// Whether the switch is on.
pub(crate) fn on() -> bool {
    mode() != Mode::Off
}

struct Runs {
    /// The frame the runs were named in (ignored in [`Mode::Forever`]).
    frame: u64,
    /// Moved by every told change: a question asked across one is not kept.
    generation: u64,
    /// First block of a run of uncommitted blocks to the end of the run
    /// (exclusive). The runs do not overlap.
    runs: BTreeMap<usize, usize>,
}

static RUNS: RwLock<Runs> = RwLock::new(Runs {
    frame: u64::MAX,
    generation: 0,
    runs: BTreeMap::new(),
});
/// Questions answered from a run, and asked of the kernel (for the line).
static KEPT: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicU64 = AtomicU64::new(0);

fn runs_read() -> std::sync::RwLockReadGuard<'static, Runs> {
    RUNS.read().unwrap_or_else(|e| e.into_inner())
}

fn runs_write() -> std::sync::RwLockWriteGuard<'static, Runs> {
    RUNS.write().unwrap_or_else(|e| e.into_inner())
}

/// The runtime told of a change to title memory at `[at, at + len)`: the
/// runs touching its blocks are let go of.
pub(crate) fn changed(at: usize, len: usize) {
    if !on() {
        return;
    }
    let (first, last) = (
        at / super::BLOCK,
        at.saturating_add(len.max(1)).div_ceil(super::BLOCK),
    );
    forget(&mut runs_write(), first, last);
}

/// The runs touching blocks `[first, last)` are let go of.
fn forget(r: &mut Runs, first: usize, last: usize) {
    r.generation += 1;
    let mut gone: Vec<usize> = r.runs.range(first..last).map(|(&from, _)| from).collect();
    if let Some((&from, &end)) = r.runs.range(..first).next_back()
        && end > first
    {
        gone.push(from);
    }
    for from in gone {
        r.runs.remove(&from);
    }
}

/// [`super::uncommitted_end`] of `block`, `ask` being the kernel's answer.
pub(super) fn uncommitted_end(block: usize, ask: impl FnOnce(usize) -> usize) -> usize {
    let m = mode();
    if m == Mode::Off {
        return ask(block);
    }
    line();
    let frame = if m == Mode::Forever { 0 } else { crate::frame::current() };
    memo(block, frame, ask)
}

/// [`uncommitted_end`] with the switch on, in frame `frame` (pass a constant
/// frame for cross-frame retention).
fn memo(block: usize, frame: u64, ask: impl FnOnce(usize) -> usize) -> usize {
    let generation = {
        let r = runs_read();
        if r.frame == frame {
            if let Some((_, &end)) = r.runs.range(..=block).next_back()
                && block < end
            {
                KEPT.fetch_add(1, Ordering::Relaxed);
                return end;
            }
            r.generation
        } else {
            drop(r);
            let mut w = runs_write();
            if w.frame != frame {
                w.frame = frame;
                w.runs.clear();
            }
            if let Some((_, &end)) = w.runs.range(..=block).next_back()
                && block < end
            {
                KEPT.fetch_add(1, Ordering::Relaxed);
                return end;
            }
            w.generation
        }
    };
    // (Asked with the lock not held: the kernel's answer takes its own.)
    let end = ask(block);
    ASKED.fetch_add(1, Ordering::Relaxed);
    if end > block {
        let mut r = runs_write();
        if r.generation == generation && r.frame == frame {
            // The runs inside this one were named before it grew.
            let inside: Vec<usize> = r
                .runs
                .range(block + 1..end)
                .map(|(&from, _)| from)
                .collect();
            for from in inside {
                r.runs.remove(&from);
            }
            r.runs.insert(block, end);
        }
    }
    end
}

/// Each ten seconds, what the runs saved (target `victory_gpu::host`).
fn line() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    static SINCE: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
    let now = crate::frame::current();
    if now < NEXT.load(Ordering::Relaxed) {
        return;
    }
    // (Looked at once in 16 frames at most.)
    NEXT.store(now + 16, Ordering::Relaxed);
    let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
    let at = std::time::Instant::now();
    let Some((was, frame_was)) = *since else {
        *since = Some((at, now));
        return;
    };
    if at.duration_since(was).as_secs_f64() < 10.0 || now <= frame_was {
        return;
    }
    *since = Some((at, now));
    let frames = (now - frame_was) as f64;
    tracing::info!(
        target: "victory_gpu::host",
        "uncommitted blocks per frame (VICTORY_COMMIT_MEMO): {:.0} questions answered from a run the kernel named earlier in the frame, {:.0} asked of it",
        KEPT.swap(0, Ordering::Relaxed) as f64 / frames,
        ASKED.swap(0, Ordering::Relaxed) as f64 / frames
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // (One test: the runs are global.)
    #[test]
    fn runs_answer_for_the_frame_until_a_change_touches_them() {
        // The kernel's answers: uncommitted from block 10 to 20 and from 30
        // to 40, committed elsewhere.
        let asked = std::cell::Cell::new(0);
        let ask = |b: usize| {
            asked.set(asked.get() + 1);
            match b {
                10..20 => 20,
                30..40 => 40,
                _ => b,
            }
        };
        assert_eq!(memo(10, 1, ask), 20);
        assert_eq!(memo(15, 1, ask), 20);
        assert_eq!(memo(19, 1, ask), 20);
        assert_eq!(asked.get(), 1);
        // Committed answers are asked again.
        assert_eq!(memo(5, 1, ask), 5);
        assert_eq!(memo(5, 1, ask), 5);
        assert_eq!(memo(20, 1, ask), 20);
        assert_eq!(asked.get(), 4);
        assert_eq!(memo(35, 1, ask), 40);
        assert_eq!(asked.get(), 5);
        // A change touching the second run: only it is asked again.
        forget(&mut runs_write(), 39, 40);
        assert_eq!(memo(12, 1, ask), 20);
        assert_eq!(asked.get(), 5);
        assert_eq!(memo(31, 1, ask), 40);
        assert_eq!(asked.get(), 6);
        // A change just past a run's end leaves it.
        forget(&mut runs_write(), 20, 25);
        assert_eq!(memo(19, 1, ask), 20);
        assert_eq!(asked.get(), 6);
        // The next frame asks again.
        assert_eq!(memo(15, 2, ask), 20);
        assert_eq!(asked.get(), 7);
        // A run named from its middle, then from before it: one run.
        forget(&mut runs_write(), 0, 100);
        assert_eq!(memo(15, 2, ask), 20);
        assert_eq!(memo(12, 2, ask), 20);
        assert_eq!(
            runs_read()
                .runs
                .iter()
                .map(|(&a, &b)| (a, b))
                .collect::<Vec<_>>(),
            vec![(12, 20)]
        );
        // A change told while a question was out: its answer is not kept.
        let ask_changing = |b: usize| {
            forget(&mut runs_write(), 0, 1);
            ask(b)
        };
        assert_eq!(memo(30, 2, ask_changing), 40);
        assert!(runs_read().runs.range(30..31).next().is_none());
    }
}
