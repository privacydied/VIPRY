//! How late the relay tells the title that the card is done
//! (`VICTORY_RELAY_LAG=1`; off by default).
//!
//! The title spins on a fence word in its memory, and only the relay
//! (`victory-relay`, [`super::relay_worker`]) writes it (`crate::fence::set`
//! in [`super::Host::relay_land`]). All the time from the card finishing a
//! submission to that write is in the frame. With the switch, each title
//! submission timed on the card ([`super::card_time`]) is tied to the value
//! the host's fence gets behind it; when the relay lands that flight, the
//! card's end time of it is put on the PC's clock (`GetClockCalibration`)
//! and set against the PC's clock now: once when the landing begins (the
//! relay has seen the card is done) and once at each fence the landing sets.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;

use super::card_time::SLOTS;

/// Whether the relay's lateness is timed.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RELAY_LAG").is_some_and(|v| v == "1"))
}

/// By slot of [`super::card_time`]: the host fence's value behind the
/// submission timed in it (0: none).
static FENCES: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

/// Since the last line: flights seen late, their summed and largest
/// lateness at the landing's begin (PC ticks); fences set, their summed and
/// largest lateness (PC ticks).
static STATS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// The submission timed in `slot` is followed by the host fence's `value`.
pub(super) fn sent(slot: usize, value: u64) {
    FENCES[slot].store(value, Relaxed);
}

fn qpc() -> u64 {
    let mut now = 0i64;
    // SAFETY: a plain query.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now as u64
}

fn qpc_frequency() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: a plain query.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut f) };
        f.max(1) as u64
    })
}

/// One landing's clocks and where it stands.
pub(super) struct Landing {
    /// The card's ticks and the PC's counter at one moment.
    card: u64,
    pc: u64,
    /// PC ticks per card tick.
    rate: f64,
    /// When the card ended the last flight of this landing seen so far, on
    /// the PC's counter.
    ended: Option<u64>,
    /// Whether the landing's first flight was counted.
    first: bool,
}

/// A landing begins. `None` when the switch is off or the clocks cannot be
/// read.
pub(super) fn landing(queue: &ID3D12CommandQueue) -> Option<Landing> {
    if !on() {
        return None;
    }
    let card_frequency = super::card_time::frequency()?;
    let (mut card, mut pc) = (0, 0);
    // SAFETY: plain queries.
    unsafe { queue.GetClockCalibration(&mut card, &mut pc) }.ok()?;
    Some(Landing {
        card,
        pc,
        rate: qpc_frequency() as f64 / card_frequency as f64,
        ended: None,
        first: true,
    })
}

impl Landing {
    /// The flight followed by the host fence's `value` is landed.
    pub(super) fn ran(&mut self, value: u64) {
        let slot = (0..SLOTS).find(|&s| FENCES[s].load(Relaxed) == value);
        let Some(end) = slot.and_then(super::card_time::end_ticks) else {
            return;
        };
        let before = self.card.saturating_sub(end) as f64 * self.rate;
        let ended = self.pc.saturating_sub(before as u64);
        self.ended = Some(ended);
        if std::mem::take(&mut self.first) {
            let late = self.pc.saturating_sub(ended);
            STATS[0].fetch_add(1, Relaxed);
            STATS[1].fetch_add(late, Relaxed);
            STATS[2].fetch_max(late, Relaxed);
        }
    }

    /// A fence is set behind the flights landed so far.
    pub(super) fn told(&self) {
        if let Some(ended) = self.ended {
            let late = qpc().saturating_sub(ended);
            STATS[3].fetch_add(1, Relaxed);
            STATS[4].fetch_add(late, Relaxed);
            STATS[5].fetch_max(late, Relaxed);
        }
    }
}

/// For the frame line.
pub(super) fn line(frames: f64) {
    if !on() {
        return;
    }
    let [seen, seen_sum, seen_most, told, told_sum, told_most] = [0, 1, 2, 3, 4, 5].map(|i| STATS[i].swap(0, Relaxed));
    let ms = |ticks: u64| ticks as f64 * 1e3 / qpc_frequency() as f64;
    tracing::info!(
        "the relay's lateness (VICTORY_RELAY_LAG=1): {:.1} landings a frame began {:.2} ms (most {:.2}) after the card ended \
         their first flight; {:.1} fences a frame were set {:.2} ms (most {:.2}) after the card ended the flight before them",
        seen as f64 / frames,
        ms(seen_sum) / seen.max(1) as f64,
        ms(seen_most),
        told as f64 / frames,
        ms(told_sum) / told.max(1) as f64,
        ms(told_most)
    );
}
