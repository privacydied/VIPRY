//! The guards' read-only looks under the shared lock without renderer port
//! #6 (`VICTORY_SHARED_LOOKS=1`, off by default).
//!
//! Profile G-profile-95ms-fsync (94.7 ms frame, HELD_ABSENT and
//! FLUSH_NARROW on): the guards' write lock is the first cost left in
//! preparing, 9.5 % of the submitting thread and 35 % of the wide threads'
//! active time. Two places of [`super::guard`] and [`super::publish`] take
//! it only to look: the first look of a guarding (a view? all clean? which
//! holes?) and the publishing of blocks found clean. `VICTORY_PORT_GUARD=1`
//! makes these looks shared, but with it comes `refused_as_view`, which cost
//! more in the write watch than the looks saved (96.8 vs 94.7 ms).
//!
//! Without port #6 everything that changes the guards' sets holds the lock
//! alone, so a look under the shared lock sees them as they are, and many
//! looks go on together. Not while faults that opened blocks without the
//! lock wait to be settled (who holds the lock alone settles them first).
//! A fault that opens a block without the lock clears its published bit
//! after it opens, as it does when the look holds the lock alone.

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_SHARED_LOOKS"))
}

/// Whether a read-only look at the guards is made under the shared lock now.
pub(super) fn now() -> bool {
    (on() || super::port_guard()) && super::TO_SETTLE.load(Ordering::SeqCst) == 0
}

/// The guards held shared or alone, for a look.
pub(super) enum Held {
    Shared(std::sync::RwLockReadGuard<'static, super::Guards>),
    Alone(std::sync::RwLockWriteGuard<'static, super::Guards>),
}

impl std::ops::Deref for Held {
    type Target = super::Guards;
    fn deref(&self) -> &super::Guards {
        match self {
            Held::Shared(g) => g,
            Held::Alone(g) => g,
        }
    }
}
