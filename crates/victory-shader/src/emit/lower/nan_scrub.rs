//! `VICTORY_NAN_SCRUB=1` (off by default): the colours a pixel program writes
//! and the floats an image store writes become 0 where they are NaN or an
//! infinity.
//!
//! On NVIDIA one bad value that reaches the title's bloom, blur or temporal
//! upscaler spreads into black patches that stay in its history (black blobs
//! on faces and in the air). An infinity through bloom or the exposure turns
//! the whole picture white at some angles, so infinities are made 0 too.
//! Not what the console does, so it is a switch.
//!
//! Ported from trustno1-coder/VIctory-NVIDIA 77e13a974 (`Lower::scrub_nan`,
//! GPL-3.0), which merge fb9d45559 dropped. Colour exports already have a
//! finite filter under `VICTORY_LEAN_MATH` bit 16 ([`Lower::finite_or_zero`]);
//! this switch uses the same filter and adds the image stores.
//!
//! Part of the translation: the shader cache keys on every `VICTORY_*` this
//! crate reads (`build.rs`), so a program is translated once per value. The
//! switch is read once; it cannot change programs already translated.

use super::*;

/// Whether `VICTORY_NAN_SCRUB=1`.
pub(super) fn nan_scrub() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_SCRUB.get() {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_NAN_SCRUB").is_some_and(|v| v == "1"))
}

#[cfg(test)]
thread_local! {
    /// A test's choice for [`nan_scrub`], for its thread only.
    pub(crate) static TEST_SCRUB: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

impl Lower<'_> {
    /// `f`, or 0 where it is NaN or an infinity, with the switch on.
    pub(super) fn scrub_nan(&mut self, f: V) -> V {
        if nan_scrub() { self.finite_or_zero(f) } else { f }
    }
}
