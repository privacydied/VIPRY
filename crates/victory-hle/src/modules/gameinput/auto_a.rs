//! `VICTORY_AUTO_A=<seconds>` (off unless set above 24): taps A (`Enter` on the
//! stand-in keyboard, `A` on the first pad) for a quarter of every four
//! seconds, from second 24 (past the title's initial load) up to `<seconds>`
//! after input starts, with or without focus. Works in release builds too.

use std::sync::LazyLock;
use std::time::Instant;

/// Seconds the taps wait for: the title's initial load.
const DELAY_SECS: u64 = 24;

pub(super) fn pressed() -> bool {
    static AUTO_A: LazyLock<(Instant, u64)> = LazyLock::new(|| {
        let secs = std::env::var("VICTORY_AUTO_A").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        (Instant::now(), secs)
    });
    let (start, secs) = &*AUTO_A;
    if *secs <= DELAY_SECS {
        return false;
    }
    let elapsed = start.elapsed();
    (DELAY_SECS..*secs).contains(&elapsed.as_secs()) && elapsed.as_millis() % 4000 < 250
}
