//! A list's small first scratch chunk may be a kept 4 MiB one (`VICTORY_SCRATCH_ANY_SIZE=1`,
//! off by default).
//!
//! The spare chunks ([`super::SPARE`]) are handed out by size only. Upstream measured the spares
//! at their bound the whole run, all of them 4 MiB ones, while some 35 small first chunks a frame
//! were made new and as many freed past the bound (about 1 ms each, a committed upload buffer).
//! With the switch a small first chunk asked for while no small one is kept is a kept 4 MiB one.
//!
//! After fxceless/VIctory-NVIDIA 5a83e5a2 (the any-size part; the spare bound stays as it is).

/// Whether a small first chunk may be a kept chunk of the regular size.
pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SCRATCH_ANY_SIZE").is_some_and(|v| v == "1"))
}

/// A kept thing of `size` bytes, or, when `size` is `small` and the switch is on, one of
/// `regular` bytes; with the size it has.
pub(super) fn take<T>(spare: &mut super::Spare<T>, size: u64, small: u64, regular: u64) -> Option<(T, u64)> {
    spare
        .take(size)
        .map(|t| (t, size))
        .or_else(|| (size == small && on()).then(|| spare.take(regular).map(|t| (t, regular))).flatten())
}

#[cfg(test)]
mod tests {
    /// Without the switch a small ask takes only a small thing.
    #[test]
    fn a_small_ask_takes_a_small_one_first() {
        let mut spare = super::super::Spare::new();
        assert!(spare.give('r', 64, 1000).is_none());
        assert_eq!(super::take(&mut spare, 4, 4, 64).map(|t| t.1), if super::on() { Some(64) } else { None });
        let mut spare = super::super::Spare::new();
        assert!(spare.give('r', 64, 1000).is_none() && spare.give('s', 4, 1000).is_none());
        assert_eq!(super::take(&mut spare, 4, 4, 64), Some(('s', 4)));
        assert_eq!(super::take(&mut spare, 64, 4, 64), Some(('r', 64)));
    }
}
