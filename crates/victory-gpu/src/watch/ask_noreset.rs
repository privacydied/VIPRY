//! Askings of the write watch that leave the kernel's reset out
//! (`VICTORY_ASK_NORESET=1`; off by default).
//!
//! `GetWriteWatch` with `WRITE_WATCH_FLAG_RESET` resets the watch of the whole
//! range asked. With the switch on, the walk does not reset, and only the
//! pages it found written are reset after, a run of neighbours at a time.
//! What is answered is the same: a page found written is reset before the
//! caller copies it, so a write after the walk is in the copy or seen by the
//! next asking; a page not found is not reset. Under Wine a reset of a range
//! protects every page in it again, so most of a range that was not written
//! is left as it is.
//!
//! After Belobug/VIctory-Belobug-AMD 122d3546a (the switch only; its meter
//! thread and `VICTORY_ASK_SPREAD` are not taken).

use super::KERNEL_PAGE;

/// The flag for `GetWriteWatch`: the reset, unless [`on`].
pub(super) fn flags() -> u32 {
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    if on() { 0 } else { WRITE_WATCH_FLAG_RESET }
}

/// Whether askings reset only the pages they found written.
pub(super) fn on() -> bool {
    #[cfg(feature = "diagnostics")]
    if let Some(on) = crate::ab::pick("VICTORY_ASK_NORESET") {
        return on;
    }
    // Read here: askings that reset only what they found.
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_ASK_NORESET").is_some_and(|v| v == "1"))
}

/// Resets the write watch of the 4 KiB pages at `found` (in order), a run of
/// neighbours at a time. Does nothing unless [`on`]: without it, the asking
/// already reset them.
pub(super) fn reset_found(found: &[usize]) {
    if !on() {
        return;
    }
    for (start, len) in runs(found) {
        // SAFETY: pages the kernel just reported write-watched.
        unsafe { windows_sys::Win32::System::Memory::ResetWriteWatch(start as *const _, len) };
    }
}

/// The runs of neighbouring pages in `found` (in order): start and length in bytes.
fn runs(found: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < found.len() {
        let mut k = i + 1;
        while k < found.len() && found[k] == found[k - 1] + KERNEL_PAGE {
            k += 1;
        }
        out.push((found[i], (k - i) * KERNEL_PAGE));
        i = k;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_are_one_run() {
        let p = KERNEL_PAGE;
        assert_eq!(runs(&[]), vec![]);
        assert_eq!(runs(&[p]), vec![(p, p)]);
        assert_eq!(runs(&[p, 2 * p, 3 * p, 5 * p]), vec![(p, 3 * p), (5 * p, p)]);
    }
}
