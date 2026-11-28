//! Diagnostics only: what the title asked of each fence, what we set it to,
//! and the fence value the title's frame end waits for. For
//! `VICTORY_HANG_LOOK=1` (apps/victory `hang_look.rs`).
//!
//! The title's frame end keeps two fence tokens and, before it starts a
//! frame, waits for the token of two frames back: it reads our fence's
//! value straight from memory (the fence's GPU address) until it reaches
//! the token's value. If it never does, its frame start stops and, 30 s
//! later, its hang check ends the process. [`title_wait`] says which fence
//! and value it waits for, the value now, the highest value the title
//! asked a queue to signal and by which way we took the signal, and what
//! our queues hold.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::object::This;

/// Which way a queue signal of the title went.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Way {
    /// Into the line of our submit thread (`VICTORY_ASYNC_SUBMIT`).
    Line,
    /// Behind what a waiting queue holds.
    Held,
    /// Behind the flights sent to the card and not waited for.
    Late,
    /// Set at once.
    Now,
}

#[derive(Default)]
struct Asks {
    /// Highest value asked by a queue signal, the way, and when.
    asked: Option<(u64, Way, Instant)>,
    asks: u64,
    /// Highest value set, and when.
    set: Option<(u64, Instant)>,
    sets: u64,
}

static FENCES: Mutex<Option<HashMap<usize, Asks>>> = Mutex::new(None);

/// `VICTORY_HANG_LOOK=1`, read once: nothing is recorded without it.
fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HANG_LOOK").is_some_and(|v| v == "1"))
}

fn with(fence: usize, f: impl FnOnce(&mut Asks)) {
    if !on() {
        return;
    }
    let mut map = FENCES.lock().unwrap_or_else(|e| e.into_inner());
    f(map
        .get_or_insert_with(HashMap::new)
        .entry(fence)
        .or_default())
}

/// The title asked queue to signal `fence` to `value`, taken the way `way`.
pub(crate) fn asked(fence: This, value: u64, way: Way) {
    with(fence as usize, |a| {
        a.asks += 1;
        if a.asked.is_none_or(|(v, ..)| value >= v) {
            a.asked = Some((value, way, Instant::now()));
        }
    });
}

/// We set `fence` to `value`.
pub(crate) fn set(fence: This, value: u64) {
    with(fence as usize, |a| {
        a.sets += 1;
        if a.set.is_none_or(|(v, _)| value >= v) {
            a.set = Some((value, Instant::now()));
        }
    });
}

/// The title's ring of two frame fence tokens: the tokens, the index of the
/// next one waited for, the index of the next one written.
const TOKENS: u64 = 0x75d_db60;
const WAITED: u64 = 0x75d_db74;
const WRITTEN: u64 = 0x75d_db78;
/// The title's table of its fence holders, by a token's top byte; a holder
/// keeps our fence at +0x10.
const HOLDERS: u64 = 0x689_29d0;
const LOW_56: u64 = (1 << 56) - 1;

/// What the title's frame end waits for, and the state of that fence and
/// our queues. `base`: the title's image base.
pub fn title_wait(base: usize) -> String {
    let base = base as u64;
    let read = crate::cb_ring::read::<u64>;
    let (Some(waited), Some(written)) = (
        crate::cb_ring::read::<u32>(base + WAITED),
        crate::cb_ring::read::<u32>(base + WRITTEN),
    ) else {
        return "the title's fence ring is not readable".into();
    };
    let mut out = format!("frame fence ring waited {waited} written {written}");
    let now = Instant::now();
    for slot in 0..2u32 {
        let Some(token) = read(base + TOKENS + 8 * u64::from(slot)) else {
            continue;
        };
        let at = if slot == waited & 1 {
            " (waited for next)"
        } else {
            ""
        };
        if token == u64::MAX {
            out += &format!("; slot {slot}{at}: none");
            continue;
        }
        let holder_index = token >> 56;
        let fence = read(base + HOLDERS + 8 * holder_index)
            .and_then(|h| read(h + 0x10))
            .unwrap_or(0);
        let target = token & LOW_56;
        let map = FENCES.lock().unwrap_or_else(|e| e.into_inner());
        let ours = map.as_ref().and_then(|m| m.get(&(fence as usize)));
        let value = ours.map(|_| crate::fence::completed(fence as This) & LOW_56);
        out += &format!(
            "; slot {slot}{at}: holder {holder_index} fence {fence:#x} waits for {target}, value {}",
            value.map_or("? (not a fence of ours seen signalled)".into(), |v| v
                .to_string()),
        );
        if let Some(a) = ours {
            if let Some((v, way, when)) = a.asked {
                out += &format!(
                    ", asked up to {v} by {way:?} {:.1} s ago ({} asks)",
                    (now - when).as_secs_f64(),
                    a.asks
                );
            }
            if let Some((v, when)) = a.set {
                out += &format!(
                    ", set up to {v} {:.1} s ago ({} sets)",
                    (now - when).as_secs_f64(),
                    a.sets
                );
            }
        }
    }
    out + "; " + &crate::queue::held_summary()
}
