//! Read-only opens go through the host's file cache (`VICTORY_FILE_CACHE=0`
//! restores the old behaviour; on by default).
//!
//! On the host `FILE_FLAG_NO_BUFFERING` keeps every read out of the system
//! file cache, so a title that opens its packs with it reads the same bytes
//! from the drive on every launch. A non-overlapped open with `GENERIC_READ`
//! and no other access loses the flag on the way to the host. An overlapped
//! open keeps it, because `ReadFileScatter` requires both.
//! (After Marvisak/VIctory#2667, imjustprism#2427, avatarmanz/VIctory-7900XTX#423.)

use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED};

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_FILE_CACHE").is_none_or(|v| v != "0");
        tracing::info!(on, "files: read-only opens go through the host file cache (VICTORY_FILE_CACHE)");
        on
    })
}

/// The flags the host's open does not get.
fn dropped_flags(access: u32, flags: u32) -> u32 {
    if access == GENERIC_READ && flags & FILE_FLAG_OVERLAPPED == 0 { FILE_FLAG_NO_BUFFERING } else { 0 }
}

/// `flags` as the host's open gets them.
pub(super) fn host_flags(call: &str, access: u32, flags: u32) -> u32 {
    static UNBUFFERED: AtomicU64 = AtomicU64::new(0);
    if flags & FILE_FLAG_NO_BUFFERING == 0 {
        return flags;
    }
    let dropped = if enabled() { dropped_flags(access, flags) } else { 0 };
    let n = UNBUFFERED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_multiple_of(256) {
        tracing::info!(
            call,
            opens = n,
            access = format_args!("{access:#x}"),
            flags = format_args!("{flags:#x}"),
            cached = dropped != 0,
            "files: the title opened a file unbuffered (VICTORY_FILE_CACHE)"
        );
    }
    flags & !dropped
}
