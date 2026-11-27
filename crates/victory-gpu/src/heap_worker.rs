//! The looks of the texture pass (`State::heap_textures` in `crate::draw`)
//! taken on a thread of ours instead of the recording one:
//! `VICTORY_HEAP_WORKER=1`. Off by default.
//!
//! A slot uploaded whole is looked at again a few times: the pass reads up
//! to 48 KiB of the texture's memory and compares a fingerprint with the one
//! of its upload. Most looks find nothing changed, and every one of them
//! runs on the thread that records the frame. Here the pass asks for the
//! fingerprint instead, leaves the slot due, and takes the answer at a later
//! pass, if it is fresh enough; what the answer then leads to (another look,
//! an upload) is done by the pass as before.
//!
//! Only a pure reading of title memory moves: a fingerprint of `(at, len)`,
//! stamped with the frame it was read in. The thread holds no heap, slot,
//! descriptor, list, staging buffer or texture: nothing it does is seen by
//! the card, and the pass takes every decision itself.
//!
//! The idea of moving the pass's work off the recording thread comes from
//! the `VICTORY_HEAP_WORKER` of fxceless/victory-fork PR 1007 (233a65f, also
//! in soldsawn/victory-nvidia-8gb 85a6ef0); none of its code is here, and
//! its asynchronous uploads are not done.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Whether the looks are taken aside (`VICTORY_HEAP_WORKER=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_WORKER").is_some_and(|v| v == "1"))
}

/// What the pass does with a slot's look.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// The fingerprint, read after the upload and at most a frame ago.
    Print(u64),
    /// Asked for: the slot stays due and is looked at by a later pass.
    Later,
    /// Taken here and now, as without the thread.
    Here,
}

/// Since the start: looks answered from the thread, slots left due for
/// one, looks taken on the recording thread after all.
pub(crate) static COUNTS: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// A look asked for that has waited this many frames is taken here (the
/// thread is slow, gone, or panicked on it).
const WAIT_FRAMES: u64 = 2;
/// At most this many memory ranges asked for or answered at once.
const CAP: usize = 1 << 16;

#[derive(Clone, Copy, Debug)]
enum Entry {
    Asked { since: u64 },
    Seen { print: u64, read_at: u64 },
}

/// The ranges asked for and their answers, and those the thread is still
/// to read.
#[derive(Default)]
struct Table {
    entries: HashMap<(u64, u64), Entry>,
    queue: Vec<(u64, u64)>,
    frame: u64,
}

impl Table {
    /// The look of `[at, at + len)` at `frame`, for a slot uploaded at
    /// frame `uploaded`; `last` for its last look, which a missed write
    /// would leave missed until the slot is written again.
    fn look(&mut self, at: u64, len: u64, uploaded: u64, frame: u64, last: bool) -> Look {
        if frame != self.frame {
            self.frame = frame;
            self.entries.retain(|_, e| match *e {
                Entry::Asked { since } => since + 2 * WAIT_FRAMES >= frame,
                Entry::Seen { read_at, .. } => read_at + 2 >= frame,
            });
        }
        let key = (at, len);
        if last {
            self.entries.remove(&key);
            return Look::Here;
        }
        match self.entries.get(&key).copied() {
            // Read in this frame or the last one, and after the upload (one
            // read in the upload's frame may be of the memory before it).
            // Kept for other slots of the same memory; old ones go above.
            Some(Entry::Seen { print, read_at }) if read_at > uploaded && read_at + 1 >= frame => Look::Print(print),
            Some(Entry::Asked { since }) if frame >= since + WAIT_FRAMES => {
                self.entries.remove(&key);
                Look::Here
            }
            Some(Entry::Asked { .. }) => Look::Later,
            // Too old, or from before the upload: taken here. (Asked again,
            // a slot the pass reaches every other frame never had an answer.)
            Some(Entry::Seen { .. }) => {
                self.entries.remove(&key);
                Look::Here
            }
            None if self.entries.len() >= CAP => Look::Here,
            None => self.ask(key, frame),
        }
    }

    fn ask(&mut self, key: (u64, u64), frame: u64) -> Look {
        self.entries.insert(key, Entry::Asked { since: frame });
        self.queue.push(key);
        Look::Later
    }

    /// The thread read `key` at frame `read_at`.
    fn seen(&mut self, key: (u64, u64), print: u64, read_at: u64) {
        // Not for a range let go meanwhile: the table stays bounded.
        if let Some(e) = self.entries.get_mut(&key) {
            *e = Entry::Seen { print, read_at };
        }
    }
}

struct Worker {
    table: Mutex<Table>,
    wake: Condvar,
}

type Probe = fn(u64, u64) -> u64;
type Now = fn() -> u64;

impl Worker {
    fn look(&self, at: u64, len: u64, uploaded: u64, frame: u64, last: bool) -> Look {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let look = table.look(at, len, uploaded, frame, last);
        let wake = !table.queue.is_empty();
        drop(table);
        if wake {
            self.wake.notify_one();
        }
        look
    }

    /// The thread's loop: reads what is asked for, with no lock held.
    fn run(&self, probe: Probe, now: Now) {
        loop {
            let asked = {
                let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
                while table.queue.is_empty() {
                    table = self.wake.wait(table).unwrap_or_else(|e| e.into_inner());
                }
                std::mem::take(&mut table.queue)
            };
            for (at, len) in asked {
                // The frame before the read: the memory is at least this new.
                let read_at = now();
                // A panic leaves it asked: the pass takes it itself.
                if let Ok(print) = std::panic::catch_unwind(|| probe(at, len)) {
                    self.table.lock().unwrap_or_else(|e| e.into_inner()).seen((at, len), print, read_at);
                }
            }
        }
    }

    fn start(name: &str, probe: Probe, now: Now) -> Option<Arc<Worker>> {
        let worker = Arc::new(Worker { table: Mutex::new(Table::default()), wake: Condvar::new() });
        let theirs = worker.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                crate::cores::keep_on_fast_cores("victory-heap-worker");
                theirs.run(probe, now)
            })
            .ok()?;
        Some(worker)
    }
}

/// See [`Look`]. Without the thread (it could not be started), `Here`.
pub(crate) fn look(at: u64, len: u64, uploaded: u64, frame: u64, last: bool) -> Look {
    static WORKER: OnceLock<Option<Arc<Worker>>> = OnceLock::new();
    let look = match WORKER.get_or_init(|| Worker::start("victory-heap-worker", crate::texture::fingerprint, crate::frame::current)) {
        Some(worker) => worker.look(at, len, uploaded, frame, last),
        None => Look::Here,
    };
    let n = COUNTS[match look {
        Look::Print(_) => 0,
        Look::Later => 1,
        Look::Here => 2,
    }]
    .fetch_add(1, Ordering::Relaxed)
        + 1;
    if matches!(look, Look::Print(_)) && n.is_power_of_two() && n >= 64 {
        let [served, left, here] = COUNTS.each_ref().map(|c| c.load(Ordering::Relaxed));
        tracing::info!(
            served,
            left_due = left,
            taken_here = here,
            "texture looks taken on the heap worker (VICTORY_HEAP_WORKER=1)"
        );
    }
    look
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_is_asked_for_then_answered_once() {
        let mut t = Table::default();
        assert_eq!(t.look(0x1000, 64, 5, 10, false), Look::Later);
        assert_eq!(t.queue, vec![(0x1000, 64)]);
        // Asked again before the answer: still later, not asked twice.
        assert_eq!(t.look(0x1000, 64, 5, 10, false), Look::Later);
        assert_eq!(t.queue.len(), 1);
        t.queue.clear();
        t.seen((0x1000, 64), 77, 10);
        assert_eq!(t.look(0x1000, 64, 5, 11, false), Look::Print(77));
        // Another slot of the same memory has it too.
        assert_eq!(t.look(0x1000, 64, 3, 11, false), Look::Print(77));
        assert!(t.queue.is_empty());
    }

    #[test]
    fn a_reading_from_before_or_at_the_upload_is_not_used() {
        let mut t = Table::default();
        t.look(0x2000, 64, 0, 10, false);
        t.seen((0x2000, 64), 1, 10);
        // Uploaded again at frame 10 (a bake): read in that frame, maybe before.
        assert_eq!(t.look(0x2000, 64, 10, 10, false), Look::Here);
        assert!(!t.entries.contains_key(&(0x2000, 64)));
    }

    #[test]
    fn a_reading_two_frames_old_is_not_used() {
        let mut t = Table::default();
        t.look(0x3000, 64, 0, 10, false);
        t.seen((0x3000, 64), 1, 10);
        assert_eq!(t.look(0x3000, 64, 0, 12, false), Look::Here);
        // And the next look asks anew.
        assert_eq!(t.look(0x3000, 64, 0, 12, false), Look::Later);
    }

    #[test]
    fn a_look_waited_for_too_long_and_the_last_look_are_taken_here() {
        let mut t = Table::default();
        assert_eq!(t.look(0x4000, 64, 0, 10, false), Look::Later);
        assert_eq!(t.look(0x4000, 64, 0, 11, false), Look::Later);
        assert_eq!(t.look(0x4000, 64, 0, 12, false), Look::Here);
        t.seen((0x5000, 64), 1, 10);
        t.look(0x5000, 64, 0, 12, false);
        t.seen((0x5000, 64), 9, 12);
        assert_eq!(t.look(0x5000, 64, 0, 12, true), Look::Here);
        assert!(!t.entries.contains_key(&(0x5000, 64)));
    }

    #[test]
    fn old_entries_go_and_the_table_is_bounded() {
        let mut t = Table::default();
        t.look(0x6000, 64, 0, 10, false);
        t.look(0x7000, 64, 0, 20, false);
        assert!(!t.entries.contains_key(&(0x6000, 64)));
        // An answer for a range let go is not kept.
        t.seen((0x6000, 64), 1, 20);
        assert!(!t.entries.contains_key(&(0x6000, 64)));
        for i in 0..CAP as u64 {
            t.entries.insert((i << 20, 1), Entry::Asked { since: 20 });
        }
        assert_eq!(t.look(0x8000, 64, 0, 20, false), Look::Here);
    }

    #[test]
    fn the_thread_answers_and_a_panic_leaves_the_look_to_the_pass() {
        fn probe(at: u64, len: u64) -> u64 {
            assert!(at != 0xdead, "a panic of the probe");
            at ^ len
        }
        fn now() -> u64 {
            10
        }
        let worker = Worker::start("heap-worker-test", probe, now).unwrap();
        assert_eq!(worker.look(0x10, 1, 0, 10, false), Look::Later);
        assert_eq!(worker.look(0xdead, 1, 0, 10, false), Look::Later);
        let began = std::time::Instant::now();
        loop {
            let seen = matches!(worker.table.lock().unwrap().entries.get(&(0x10, 1)), Some(Entry::Seen { .. }));
            if seen || began.elapsed().as_secs() > 10 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(worker.look(0x10, 1, 0, 11, false), Look::Print(0x11));
        // The panicked one stays asked, and is taken here once it waited.
        assert_eq!(worker.look(0xdead, 1, 0, 11, false), Look::Later);
        assert_eq!(worker.look(0xdead, 1, 0, 12, false), Look::Here);
    }
}
