//! Window lookups under the shared lock first (`VICTORY_SHARED_LOOKUPS=1`,
//! off by default; `VICTORY_AB` can flip it).
//!
//! An unsized ask with tails, or a fine ask for any window, picked its window
//! under [`super::LOOSE`]'s write lock even when that window was already
//! there (nearly always), so the recording threads stood in line for it. With
//! the switch the same window is picked under the read lock; only a window
//! still to be made takes the write lock, which picks again as before.
//!
//! After Project-507 (507KAZ) c0856a9, GPL-3.0, reimplemented here.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::Shadow;

/// Whether the switch is on.
pub(super) fn on() -> bool {
    crate::asked_for!("VICTORY_SHARED_LOOKUPS")
}

/// The window an unsized (`unsized_ask`) or fine ask at `[address, end)`
/// would be given by the write-locked path, if it is in `map` already:
/// the same choice that path makes (`cell`: the ask's cell, `named`: its
/// window by name).
pub(super) fn find(
    map: &BTreeMap<(u64, u64), Arc<Shadow>>,
    cell: u64,
    address: u64,
    end: u64,
    unsized_ask: bool,
    named: (u64, u64),
) -> Option<Arc<Shadow>> {
    let there = map.range((cell, 0)..=(address, u64::MAX)).map(|(key, _)| *key);
    let key = if unsized_ask && super::tails() {
        super::longest_tail(there, address, end).unwrap_or_else(|| super::tail_of(address, end))
    } else {
        super::holder(there, address, end).unwrap_or(named)
    };
    map.get(&key).cloned()
}
