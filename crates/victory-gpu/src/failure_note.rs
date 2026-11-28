//! Who hears of the failures our D3D12 and DXGI answer with (methods we
//! have not written, `E_NOTIMPL`): the crash report's ring of last calls
//! (`VICTORY_HLE_LAST`, diagnostics builds) when the HLE side asks, else
//! nobody.

use std::sync::OnceLock;

/// Told what failed (an object kind or method), its table slot in bytes
/// (0 where there is none) and the `HRESULT`.
pub type Note = fn(&'static str, usize, i32);

static NOTE: OnceLock<Note> = OnceLock::new();

/// Sets who hears of the failures (once; later calls do nothing).
pub fn on_failure_note(note: Note) {
    let _ = NOTE.set(note);
}

/// A failure we answer: `what` at `slot` gave `hr`.
pub(crate) fn note(what: &'static str, slot: usize, hr: i32) {
    if let Some(note) = NOTE.get() {
        note(what, slot, hr);
    }
}
