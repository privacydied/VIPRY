//! Renderer port #6 (`VICTORY_PORT_GUARD`, off by default): the guards' lock
//! shared by the title's write faults and the refreshes' looks at view memory,
//! and a block's claim held while its state changes. The sets and the lock
//! are in [`super`]; what is only for the switch is here.

#![allow(clippy::too_many_lines)]

use super::*;

/// Whether the kernel would refuse to ask about `[start, end)` for its view
/// memory (renderer port #6): read-only by our hand, or known a view
/// ([`VIEWS`]). Such a range is left to [`writes`], which goes to the guards
/// at once; asked with others it failed their asking too, which was then
/// asked again by halves.
pub(super) fn refused_as_view(start: usize, end: usize) -> bool {
    if !port_guard() || !light() {
        return false;
    }
    let (first, last) = (start / BLOCK, end.div_ceil(BLOCK));
    let refused = PROTECTED.any(first, last) || VIEWS.any(first, last);
    if refused {
        port6_count(P6_NOT_ASKED, 1);
    }
    refused
}

/// Renderer port #6 (`VICTORY_PORT_GUARD=1`, off by default; read here, and
/// A/B-able in one run with `VICTORY_AB=<s>,VICTORY_PORT_GUARD`): the guards'
/// one lock no longer makes the title's write faults, and the refreshes'
/// looks at view memory, wait for one another.
///
/// What it was (run best-1, Series S, ports 1, 2 and 5 on): some 350 write
/// faults a frame, each taking [`GUARDS`] alone to give one block its write
/// access back, and some 220 refreshes a frame guarding view memory block by
/// block under the same lock (every `VirtualProtect` of theirs under it):
/// 450 waits for the lock, 58-62 ms a frame of all threads' time, 20 of it the
/// refreshes' (2.5-4.5 ms of the submitting thread's), the rest the title's
/// threads, mostly queued behind one another.
///
/// What it is with the switch: [`GUARDS`] is a lock for readers and one
/// writer. The sets of blocks in it are of atomic words, so what changes a
/// block's state needs no lock of all of them: a block's own claim does
/// ([`BUSY`]). A fault, a refresh guarding its range and a block made known
/// by its bits ([`publish`]) take the lock shared and the claims of the
/// blocks they change, so they wait only for those who change the same
/// block (a `VirtualProtect` of it) and for the few things that still take
/// the lock alone (a mapping changed, a kernel read, memory freed, the holes
/// looked at, ...), which take no claims: holding the lock alone, nobody
/// holds one. Who waits for a claim holds no other claim, or only claims of
/// lower blocks (faults take theirs in order); who holds claims and wants
/// more only tries for them (a refresh: what it cannot claim it leaves as it
/// is, written, told so at the next asking), so no two wait for each other.
///
/// Also with the switch:
/// - view memory is not asked of the kernel, which refuses it
///   ([`VIEWS`]; ~300 refused askings a frame).
pub(crate) fn port_guard() -> bool {
    #[cfg(feature = "diagnostics")]
    if let Some(on) = crate::ab::pick("VICTORY_PORT_GUARD") {
        return on;
    }
    #[cfg(test)]
    if PORT_GUARD_IN_TEST.get() {
        return true;
    }
    // Read here: the switch of renderer port #6.
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_PORT_GUARD").is_ok_and(|v| v.trim() == "1"))
}

#[cfg(test)]
thread_local! {
    /// [`port_guard`], for what this thread does.
    pub(crate) static PORT_GUARD_IN_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The blocks a thread holding [`GUARDS`] shared is changing the state of
/// (renderer port #6, [`port_guard`]): a bit each, set by who changes it
/// ([`Claims`]).
pub(super) static BUSY: Bits = Bits::new();

/// Claims of blocks ([`BUSY`]), let go when dropped.
pub(super) struct Claims(Vec<usize>);

impl Claims {
    /// The claims of `blocks` (in order, each once), waited for one after
    /// the other: who waits holds only claims of lower blocks.
    pub(super) fn wait_for(blocks: &[usize]) -> Claims {
        debug_assert!(blocks.windows(2).all(|w| w[0] < w[1]));
        let mut spun = 0u64;
        for &block in blocks {
            let mut tries = 0u32;
            while !BUSY.try_set(block) {
                tries += 1;
                spun += 1;
                if tries < 64 {
                    std::hint::spin_loop();
                } else {
                    // Another thread's `VirtualProtect` of the block.
                    std::thread::yield_now();
                }
            }
        }
        if spun != 0 {
            port6_count(P6_CLAIM_SPINS, spun);
        }
        Claims(blocks.to_vec())
    }

    /// The claim of `block` if it is free; nothing waited for.
    pub(super) fn try_one(&mut self, block: usize) -> bool {
        let got = BUSY.try_set(block);
        if got {
            self.0.push(block);
        } else {
            port6_count(P6_CLAIMS_MISSED, 1);
        }
        got
    }

    pub(super) fn none() -> Claims {
        Claims(Vec::new())
    }

    pub(super) fn holds(&self, block: usize) -> bool {
        self.0.contains(&block)
    }
}

impl Drop for Claims {
    fn drop(&mut self) {
        for &block in &self.0 {
            BUSY.clear(block);
        }
    }
}

/// Renderer port #6's counts since the last line: faults opened under the
/// shared lock; nanoseconds they waited for the lock; refreshes' guards
/// done shared; blocks a refresh could not claim (left written); spins for
/// claims; kernel askings left out for view memory.
pub(super) static PORT6: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];
pub(super) const P6_FAULTS: usize = 0;
pub(super) const P6_FAULT_WAIT_NS: usize = 1;
pub(super) const P6_GUARDS: usize = 2;
pub(super) const P6_CLAIMS_MISSED: usize = 3;
pub(super) const P6_CLAIM_SPINS: usize = 4;
pub(super) const P6_NOT_ASKED: usize = 5;

pub(super) fn port6_count(what: usize, n: u64) {
    PORT6[what].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// Renderer port #6's line of the frame lines, per frame.
pub(crate) fn port_guard_line(frames: f64) {
    let n: Vec<f64> = PORT6.iter().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / frames).collect();
    if !port_guard() && n.iter().all(|&v| v == 0.0) {
        return;
    }
    tracing::info!(
        "renderer port 6 (VICTORY_PORT_GUARD=1) per frame: faults opened under the shared lock {:.1} ({:.2} ms waiting for it), \
         refreshes' guards done shared {:.1}, blocks left written for want of their claim {:.1}, spins for claims {:.0}; \
         kernel askings left out for view memory {:.1}",
        n[P6_FAULTS],
        n[P6_FAULT_WAIT_NS] / 1e6,
        n[P6_GUARDS],
        n[P6_CLAIMS_MISSED],
        n[P6_CLAIM_SPINS],
        n[P6_NOT_ASKED],
    );
}

/// The blocks known to be view memory (renderer port #6): guarded by us
/// once, or found a view by [`guard`]. The kernel refuses to ask about them
/// (its write watch is not kept for views), and with the switch such a
/// range goes to the guards at once. Forgotten when the memory is unmapped
/// or freed; a block wrongly here is only looked at by the guards, which
/// find it no view ([`is_view`]) and say the range is not watched.
pub(super) static VIEWS: Bits = Bits::new();

/// Whether a block of `[first, last)` is known view memory and the kernel
/// need not be asked ([`VIEWS`]).
pub(super) fn known_views(first: usize, last: usize) -> bool {
    port_guard() && light() && VIEWS.any(first, last)
}

/// Opens the guarded block at `at` for a write of the title's that faulted
/// there (or is about to be made), and notes it opened: how many blocks were
/// opened (0: it was not guarded, or another thread opened it first), and how
/// long the guards' lock was waited for.
///
/// With renderer port #6 the lock is taken shared and the claims of the block
/// and of its page's other views are waited for, in order: the block's state
/// is changed by nobody else meanwhile, and other faults go on beside this.
/// (Not while faults that took no lock are not settled yet ([`settle`]): that
/// is for who holds the lock alone, as before.)
pub(super) fn open_faulted(at: usize) -> (usize, std::time::Duration) {
    let block = at / BLOCK;
    let began = std::time::Instant::now();
    if !port_guard() || TO_SETTLE.load(std::sync::atomic::Ordering::SeqCst) != 0 || unsettled(block, block + 1) {
        let g = guards();
        let waited = began.elapsed();
        let opened = open(&g, at, at + 1);
        if opened != 0 {
            g.opened(block);
        }
        return (opened, waited);
    }
    let g = guards_shared();
    let waited = began.elapsed();
    port6_count(P6_FAULT_WAIT_NS, waited.as_nanos() as u64);
    let mut views: Vec<usize> = vec![block];
    loop {
        let _claims = Claims::wait_for(&views);
        if !g.protected.contains(block) {
            return (0, waited);
        }
        let given = if g.aliased.contains(block) {
            // (The views as the table has them now, as `open` looks them up;
            // a view mapped after this opens what is guarded of the page
            // again, holding the lock alone: `views_mapped`.)
            let mut now: Vec<usize> = crate::inplace::pages_of(block * BLOCK).into_iter().map(|p| p / BLOCK).collect();
            now.push(block);
            now.sort_unstable();
            now.dedup();
            if now.iter().any(|b| views.binary_search(b).is_err()) {
                // Their claims too, in order: again.
                views = now;
                continue;
            }
            Some(now)
        } else {
            None
        };
        let opened = open_given(&g, at, at + 1, given.as_deref());
        if opened != 0 {
            g.opened(block);
            port6_count(P6_FAULTS, 1);
        }
        return (opened, waited);
    }
}

/// The rest of [`guard`] with renderer port #6: the lock shared, and a
/// block's claim held while its state changes. What a fault opens meanwhile
/// is opened before or after the block's state is looked at and changed,
/// never in between; a block whose claim another holds (a fault, a cover of
/// another range's) is left as it is, written: told so now, and looked at
/// again at the next asking.
pub(super) fn guard_shared(first: usize, last: usize, came: &[(usize, bool)], rest: std::time::Duration) -> Vec<usize> {
    if !came.is_empty() {
        // (Changes no block's protection: the sets of what is where.)
        let mut g = guards_timed();
        for &(block, view) in came {
            if g.absent.remove(block) && !view {
                g.foreign.insert(block);
            }
        }
    }
    port6_count(P6_GUARDS, 1);
    let g = guards_timed_shared();
    #[cfg(feature = "diagnostics")]
    tally(4, 1);
    let mut written = Vec::new();
    for word in first / 64..last.div_ceil(64) {
        let known = (g.clean.word(word) & !BESIDE.word(word) | g.absent.word(word)) & !OPENING.word(word) & !TOLD.word(word);
        written.extend(BitsOf(!known & in_word(word * 64, first, last)).map(|bit| word * 64 + bit));
    }
    let mut run: Option<(usize, usize)> = None;
    for &block in &written {
        if let Some((from, to)) = run
            && to != block
        {
            protect_claimed(&g, from, to);
            run = None;
        }
        // Opened by a fault that took no lock, or another view of such a
        // block's page, and not settled yet: told as written, and left as it
        // is (`OPENING`).
        if unsettled(block, block + 1) {
            if let Some((from, to)) = run.take() {
                protect_claimed(&g, from, to);
            }
            continue;
        }
        // Told as written by this, and read by the caller after.
        if BESIDE.take(block) {
            put_count(block).store(0, std::sync::atomic::Ordering::Relaxed);
        }
        if g.protected.contains(block) {
            if let Some((from, to)) = run.take() {
                protect_claimed(&g, from, to);
            }
            if g.clean.contains(block) {
                continue;
            }
            // (No other claim is held here.)
            let mut claims = Claims::wait_for(&[block]);
            if g.protected.contains(block) && !g.clean.contains(block) && cover(&g, block, block + 1, Some(&mut claims)) {
                g.clean.insert(block);
            }
        } else if !g.foreign.contains(&block) && !g.left_alone(block) && !g.rests(block, rest) {
            run = Some((run.map_or(block, |r| r.0), block + 1));
        } else if let Some((from, to)) = run.take() {
            protect_claimed(&g, from, to);
        }
    }
    if let Some((from, to)) = run {
        protect_claimed(&g, from, to);
    }
    drop(g);
    #[cfg(feature = "diagnostics")]
    crate::faultbook::note(crate::faultbook::What::Asked, first, last, written.len() as u64);
    written
}

/// [`protect`] of `[from, to)` with the lock shared: of the blocks whose
/// claims can be had now, each run, its other views' blocks only where their
/// claims can be had too.
pub(super) fn protect_claimed(g: &Guards, from: usize, to: usize) {
    let mut claims = Claims::none();
    let mut start = None;
    for block in from..to {
        if claims.try_one(block) {
            start.get_or_insert(block);
        } else if let Some(s) = start.take() {
            protect_run(g, s, block, &mut claims);
        }
    }
    if let Some(s) = start {
        protect_run(g, s, to, &mut claims);
    }
}

pub(super) fn protect_run(g: &Guards, from: usize, to: usize, claims: &mut Claims) {
    #[cfg(feature = "diagnostics")]
    crate::faultbook::note(crate::faultbook::What::Taken, from, to, 0);
    protect_here(g, from, to);
    cover(g, from, to, Some(claims));
}

/// What is known of `[first, last)` as view memory ([`VIEWS`]) is forgotten.
pub(super) fn forget_views(first: usize, last: usize) {
    if !VIEWS.any(first, last) {
        return;
    }
    let set: Vec<usize> = VIEWS.set_in(first, last).collect();
    for block in set {
        VIEWS.clear(block);
    }
}

