//! `VICTORY_BORDERLESS=1`: the title's top-level window covers the whole
//! primary monitor without a frame (what it shows is scaled to it), instead of
//! a framed window fitted to the work area. Off unless set to `1`.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_BORDERLESS").is_some_and(|v| v == "1"))
}

/// The style and place for a top-level window of `width` x `height`, when the
/// switch is on: no frame, a popup at (0, 0) over the primary monitor.
pub(super) fn place(parent: HWND, style: u32, width: i32, height: i32) -> Option<(u32, i32, i32, i32, i32)> {
    if !parent.is_null() || !enabled() || width <= 0 || height <= 0 {
        return None;
    }
    let frame = wm::WS_CAPTION | wm::WS_THICKFRAME | wm::WS_SYSMENU | wm::WS_MINIMIZEBOX | wm::WS_MAXIMIZEBOX;
    // SAFETY: plain queries.
    let (w, h) = unsafe { (wm::GetSystemMetrics(wm::SM_CXSCREEN), wm::GetSystemMetrics(wm::SM_CYSCREEN)) };
    tracing::info!(width, height, to = ?(w, h), "title window borderless over the whole screen (VICTORY_BORDERLESS)");
    Some(((style & !frame) | wm::WS_POPUP, 0, 0, w, h))
}
