//! Marks beside a card copy's lists, read without their locks
//! (`VICTORY_REFRESH_DIRTY=1`, with [`super::dirty`]).
//!
//! Each merge of shader writes into a card copy ([`super::Shadow::card_merge_relay`],
//! [`super::Shadow::land_card`]) took the `dirty` and `sent` locks to learn
//! which offsets to leave alone, though most copies have neither. The marks
//! say whether each list holds anything; they are written under the list's
//! lock and read without it. They are kept up with the switch off too.
//!
//! Hand-ported from mandy f6fa3a4e1, which ports TaynanGT/Vice-Tay db28bb8
//! and c289a95.

use super::Card;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct Marks {
    /// Whether `dirty` holds a range. Unlike `touched` it goes down only
    /// after `sent` and `has_sent` hold what `dirty` held, so one that finds
    /// it down and then `has_sent` up misses no range on its way from the
    /// one list to the other.
    has_dirty: AtomicBool,
    /// Whether `sent` holds a send.
    has_sent: AtomicBool,
}

impl Marks {
    /// Something was put into `dirty` (under its lock).
    pub(super) fn dirtied(&self) {
        self.has_dirty.store(true, Ordering::Release);
    }

    /// `dirty` was taken (under its lock), after `sent` got what went up
    /// ([`Marks::sent`]) if anything did.
    pub(super) fn taken(&self) {
        self.has_dirty.store(false, Ordering::Release);
    }

    /// `sent` holds a send or not (under its lock).
    pub(super) fn sent(&self, any: bool) {
        self.has_sent.store(any, Ordering::Release);
    }
}

/// The ranges of offsets `[from, to)` of `card` whose bytes a fetch sent
/// after `fetched_after` did not see: those not gone up yet (`dirty`) and
/// those sent after it (`sent`). With the switch, a copy with neither is
/// told without either lock: `has_dirty` is read first, and `has_sent` goes
/// up before `has_dirty` goes down.
pub(super) fn skip(card: &Card, from: usize, to: usize, fetched_after: u64) -> Vec<(usize, usize)> {
    let fast = super::dirty::on();
    if fast && !card.marks.has_dirty.load(Ordering::Acquire) && !card.marks.has_sent.load(Ordering::Acquire) {
        return Vec::new();
    }
    let dirty = card.dirty.lock().unwrap_or_else(|e| e.into_inner());
    let sent = card.sent.lock().unwrap_or_else(|e| e.into_inner());
    let mut skip: Vec<(usize, usize)> = dirty.iter().copied().filter(|&(a, b)| a < to && b > from).collect();
    sent.near(fetched_after, from, to, &mut skip);
    skip
}
