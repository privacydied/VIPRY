//! Treat every uncommitted block found in a view range as a held hole
//! (`VICTORY_HELD_ABSENT=0` turns it off; on by default).
//!
//! Under `VICTORY_KNOWN_HOLES` (on by default), [`super::placeholder`] asks
//! `NtQueryVirtualMemory(MemoryRegionInformationEx = 7)` whether an
//! uncommitted block lies in a placeholder reservation so it can be added to
//! `Guards::held`, published in `PUBLISHED`, and skipped by
//! `Shadow::nothing_new` and `guard_writes`. On Wine/Proton, class 7 either
//! fails or does not report `PLACEHOLDER_RESERVATION`, so `g.held` stays
//! empty, `PUBLISHED` is never set on sparse view ranges, and every refresh
//! walks `g.absent` and calls `VirtualQuery` (~14,000 times a frame).
//!
//! With `VICTORY_HELD_ABSENT=1`, any uncommitted block found in a guarded
//! view range is inserted into `g.held` without waiting for
//! `MemoryRegionInformationEx`. `title_memory_allocated` and
//! `title_memory_remapped` already clear `g.held` and `PUBLISHED` on told
//! changes, and `look_at_holes` still audits 1/100th of `g.held` each frame.

use std::sync::OnceLock;

/// Whether `VICTORY_HELD_ABSENT` is enabled (`=0` turns it off).
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| parse(std::env::var("VICTORY_HELD_ABSENT").ok().as_deref()))
}

fn parse(val: Option<&str>) -> bool {
    !matches!(val, Some("0"))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn default_on_and_explicit_off() {
        assert!(parse(None));
        assert!(parse(Some("1")));
        assert!(!parse(Some("0")));
    }
}
