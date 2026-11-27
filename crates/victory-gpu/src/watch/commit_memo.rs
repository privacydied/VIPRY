//! What the kernel said about uncommitted blocks, kept for one
//! submission's preparing (`VICTORY_COMMIT_MEMO=1`).
//!
//! A refresh of a host copy asks the kernel, for each block the guards hold
//! as absent, whether it is still uncommitted ([`super::uncommitted_end`]).
//! A submission refreshes the same buffers in many flushes, so the same
//! blocks are asked again and again; under Wine each question takes the
//! process's virtual-memory lock, which the submitting thread and the wide
//! threads then take in turns (a profile had 13.7 % of their preparing in
//! it, the bytes copied 3 %).
//!
//! With the switch the answers are kept while a submission is prepared
//! ([`Epoch`]) and forgotten when it ends and whenever the title's memory
//! changes in a way we are told of (`title_memory_*`, [`super::materialize`]).
//! What a submission reads was committed before the title submitted it, so
//! before its preparing began: an answer got after that is good for it. An
//! answer asked while a change was told is not kept ([`GENERATION`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Whether the switch is on.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_COMMIT_MEMO"))
}

/// Whether a submission is being prepared now (answers are kept).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Moved by every forgetting: an answer asked across one is not kept.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Answers by the block asked about: the end (exclusive) of the uncommitted
/// blocks from it, or the block itself where it is committed.
static ANSWERS: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());
/// Questions answered from [`ANSWERS`], and asked of the kernel, while a
/// submission was prepared (for the line).
static KEPT: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicU64 = AtomicU64::new(0);

fn forget() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// One submission's preparing: answers are kept until it is dropped.
pub(crate) struct Epoch(bool);

impl Epoch {
    pub(crate) fn begin() -> Epoch {
        if !on() {
            return Epoch(false);
        }
        forget();
        ACTIVE.store(true, Ordering::Release);
        Epoch(true)
    }
}

impl Drop for Epoch {
    fn drop(&mut self) {
        if self.0 {
            ACTIVE.store(false, Ordering::Release);
            forget();
            line();
        }
    }
}

/// The title's memory changed (committed, freed, mapped, unmapped, made
/// real): what was kept may be wrong now.
pub(crate) fn changed() {
    if on() && ACTIVE.load(Ordering::Acquire) {
        forget();
    }
}

/// The answer kept for `block`, if any.
fn kept(block: usize) -> Option<usize> {
    let answers = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    let (&from, &end) = answers.range(..=block).next_back()?;
    if from == block && end == block {
        // Committed.
        return Some(block);
    }
    // Uncommitted from `from` to `end`: so is every block between.
    (from <= block && block < end).then_some(end)
}

/// [`super::uncommitted_end`] of `block`, `ask` being the kernel's answer.
pub(super) fn uncommitted_end(block: usize, ask: impl FnOnce(usize) -> usize) -> usize {
    if !on() {
        return ask(block);
    }
    memo(block, ask)
}

/// [`uncommitted_end`] with the switch on.
fn memo(block: usize, ask: impl FnOnce(usize) -> usize) -> usize {
    if !ACTIVE.load(Ordering::Acquire) {
        return ask(block);
    }
    if let Some(end) = kept(block) {
        KEPT.fetch_add(1, Ordering::Relaxed);
        return end;
    }
    let generation = GENERATION.load(Ordering::Acquire);
    let end = ask(block);
    ASKED.fetch_add(1, Ordering::Relaxed);
    let mut answers = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    // (Not across a forgetting, nor after the submission ended.)
    if GENERATION.load(Ordering::Acquire) == generation && ACTIVE.load(Ordering::Acquire) {
        answers.insert(block, end.max(block));
    }
    end
}

/// Each few seconds, what the memo saved.
fn line() {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let Some(was) = *last else {
        *last = Some(now);
        return;
    };
    let seconds = now.duration_since(was).as_secs_f64();
    if seconds < 10.0 {
        return;
    }
    *last = Some(now);
    let kept = KEPT.swap(0, Ordering::Relaxed);
    let asked = ASKED.swap(0, Ordering::Relaxed);
    tracing::info!(
        target: "victory_gpu::host",
        "uncommitted blocks while preparing (VICTORY_COMMIT_MEMO): {:.0} questions a second answered from what the kernel said earlier in the submission, {:.0} asked of it",
        kept as f64 / seconds,
        asked as f64 / seconds
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // (One test: the memo is global.)
    #[test]
    fn keeps_answers_for_one_submission_only() {
        // The kernel's answers: uncommitted from block 10 to 20.
        let asked = std::cell::Cell::new(0);
        let ask = |b: usize| {
            asked.set(asked.get() + 1);
            if (10..20).contains(&b) { 20 } else { b }
        };
        // Outside a submission nothing is kept.
        assert_eq!(memo(10, ask), 20);
        assert_eq!(memo(10, ask), 20);
        assert_eq!(asked.get(), 2);
        ACTIVE.store(true, Ordering::Release);
        forget();
        asked.set(0);
        assert_eq!(memo(10, ask), 20);
        assert_eq!(memo(15, ask), 20);
        assert_eq!(memo(19, ask), 20);
        assert_eq!(memo(5, ask), 5);
        assert_eq!(memo(5, ask), 5);
        assert_eq!(memo(20, ask), 20);
        assert_eq!(asked.get(), 3);
        // The title's memory changed: asked again.
        forget();
        assert_eq!(memo(15, ask), 20);
        assert_eq!(asked.get(), 4);
        ACTIVE.store(false, Ordering::Release);
        forget();
    }
}
