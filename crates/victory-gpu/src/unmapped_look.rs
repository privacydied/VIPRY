//! Diagnostics for texture slots passed over unmapped (`VICTORY_UNMAPPED_LOOK=1`;
//! diagnostics builds only, off by default).
//!
//! Tracks each slot passed over with `mapped == 0` in `draw::heap_textures`:
//! whether xmem maps its memory later, how many frames later, whether the slot
//! is looked at again after that map (and whether it is then mapped or still
//! unmapped), or whether a newer view makes it stale or overwrites the slot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_UNMAPPED_LOOK").is_some_and(|v| v == "1"))
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    slot: u32,
    write: u64,
    address: u64,
    len: u64,
    unmapped_frame: u64,
    looks: u32,
    mapped_frame: Option<u64>,
    seen_after_map_frame: Option<u64>,
    seen_after_map_mapped: bool,
    seen_mapped_frame: Option<u64>,
    stale: bool,
    overwritten: bool,
}

#[derive(Default)]
struct Stats {
    events: u64,
    unique: u64,
    mapped_later: u64,
    map_delay_sum: u64,
    map_delay_min: u64,
    map_delay_max: u64,
    seen_after_map_ok: u64,
    seen_after_map_unmapped: u64,
    not_seen_after_map: u64,
    seen_mapped_without_remap: u64,
    stale: u64,
    overwritten: u64,
    samples_logged: usize,
}

impl Stats {
    fn record_map_delay(&mut self, delay: u64) {
        if self.mapped_later == 0 || delay < self.map_delay_min {
            self.map_delay_min = delay;
        }
        self.map_delay_max = self.map_delay_max.max(delay);
        self.map_delay_sum = self.map_delay_sum.saturating_add(delay);
        self.mapped_later += 1;
    }

    fn retire(&mut self, e: Entry) {
        if e.stale {
            self.stale += 1;
        }
        if e.overwritten {
            self.overwritten += 1;
        }
        if e.mapped_frame.is_some() {
            if e.seen_after_map_frame.is_some() {
                if e.seen_after_map_mapped {
                    self.seen_after_map_ok += 1;
                } else {
                    self.seen_after_map_unmapped += 1;
                }
            } else {
                self.not_seen_after_map += 1;
            }
        } else if e.seen_mapped_frame.is_some() {
            self.seen_mapped_without_remap += 1;
        }
    }
}

#[derive(Default)]
struct State {
    active: HashMap<u32, Entry>,
    stats: Stats,
    samples: Vec<Entry>,
}

impl State {
    fn maybe_sample(&mut self, e: Entry) {
        if self.samples.len() < 20 {
            self.samples.push(e);
        }
    }

    fn retire_entry(&mut self, e: Entry) {
        self.stats.retire(e);
        self.maybe_sample(e);
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Called when `draw::heap_textures` passes over `slot` with `mapped == 0`.
pub(crate) fn passed_over(slot: u32, write: u64, address: u64, len: u64, frame: u64) {
    if !on() {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(State::default);
    state.stats.events += 1;
    if let Some(existing) = state.active.get_mut(&slot) {
        if existing.write == write && existing.address == address && existing.len == len {
            existing.looks = existing.looks.saturating_add(1);
            if existing.mapped_frame.is_some() && existing.seen_after_map_frame.is_none() {
                existing.seen_after_map_frame = Some(frame);
                existing.seen_after_map_mapped = false;
            }
            return;
        }
        let mut old = state.active.remove(&slot).unwrap();
        old.overwritten = true;
        state.retire_entry(old);
    }
    state.stats.unique += 1;
    state.active.insert(
        slot,
        Entry {
            slot,
            write,
            address,
            len,
            unmapped_frame: frame,
            looks: 1,
            mapped_frame: None,
            seen_after_map_frame: None,
            seen_after_map_mapped: false,
            seen_mapped_frame: None,
            stale: false,
            overwritten: false,
        },
    );
    ACTIVE.store(state.active.len(), Relaxed);
}

/// Called when `draw::heap_textures` sees `slot` with `mapped > 0`.
pub(crate) fn looked_mapped(slot: u32, write: u64, frame: u64) {
    if !on() || ACTIVE.load(Relaxed) == 0 {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    let matches = state.active.get(&slot).is_some_and(|e| e.write == write);
    if !matches {
        return;
    }
    if let Some(mut e) = state.active.remove(&slot) {
        e.seen_mapped_frame = Some(frame);
        if e.mapped_frame.is_some() && e.seen_after_map_frame.is_none() {
            e.seen_after_map_frame = Some(frame);
            e.seen_after_map_mapped = true;
        }
        state.retire_entry(e);
        ACTIVE.store(state.active.len(), Relaxed);
    }
}

/// Called from `watch::title_memory_remapped(at, len)` when xmem maps a range.
pub(crate) fn remapped(at: usize, len: usize) {
    if !on() || ACTIVE.load(Relaxed) == 0 || len == 0 {
        return;
    }
    let start = at as u64;
    let end = at.saturating_add(len) as u64;
    let frame = crate::frame::current();
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    let mut delays = Vec::new();
    for e in state.active.values_mut() {
        if e.mapped_frame.is_none() && e.address < end && e.address.saturating_add(e.len) > start {
            e.mapped_frame = Some(frame);
            delays.push(frame.saturating_sub(e.unmapped_frame));
        }
    }
    for d in delays {
        state.stats.record_map_delay(d);
    }
}

/// Called when newer views make `slots` stale in `descriptor::host`.
pub(crate) fn stale(slots: &[(u32, u64)]) {
    if !on() || ACTIVE.load(Relaxed) == 0 || slots.is_empty() {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    for &(slot, _) in slots {
        if let Some(mut e) = state.active.remove(&slot) {
            e.stale = true;
            state.retire_entry(e);
        }
    }
    ACTIVE.store(state.active.len(), Relaxed);
}

/// Periodic 5-second summary from `host.rs`.
pub(crate) fn log() {
    if !on() {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    if state.stats.events == 0 {
        return;
    }
    let mut active_mapped = 0u64;
    let mut active_seen_after = 0u64;
    let mut active_not_seen_after = 0u64;
    for e in state.active.values() {
        if e.mapped_frame.is_some() {
            active_mapped += 1;
            if e.seen_after_map_frame.is_some() {
                active_seen_after += 1;
            } else {
                active_not_seen_after += 1;
            }
        }
    }
    let s = &state.stats;
    let avg_delay = if s.mapped_later > 0 { s.map_delay_sum as f64 / s.mapped_later as f64 } else { 0.0 };
    tracing::info!(
        events = s.events,
        unique = s.unique,
        active = state.active.len(),
        mapped_later = s.mapped_later,
        delay_min = s.map_delay_min,
        delay_avg = format_args!("{avg_delay:.1}"),
        delay_max = s.map_delay_max,
        seen_after_ok = s.seen_after_map_ok,
        seen_after_unmapped = s.seen_after_map_unmapped + active_seen_after,
        not_seen_after = s.not_seen_after_map + active_not_seen_after,
        mapped_no_remap = s.seen_mapped_without_remap,
        stale = s.stale,
        overwritten = s.overwritten,
        active_mapped,
        "unmapped texture slots (VICTORY_UNMAPPED_LOOK=1)"
    );
    while state.stats.samples_logged < state.samples.len() {
        let e = state.samples[state.stats.samples_logged];
        state.stats.samples_logged += 1;
        log_sample(state.stats.samples_logged, e);
    }
    if state.stats.samples_logged < 20 && state.stats.events >= 20 {
        let need = 20 - state.stats.samples_logged;
        let extras: Vec<Entry> = state.active.values().take(need).copied().collect();
        for e in extras {
            state.stats.samples_logged += 1;
            log_sample(state.stats.samples_logged, e);
        }
    }
}

fn log_sample(n: usize, e: Entry) {
    let map_delta = e.mapped_frame.map(|m| m.saturating_sub(e.unmapped_frame));
    tracing::info!(
        sample = n,
        slot = e.slot,
        address = format_args!("{:#x}", e.address),
        size = e.len,
        unmapped_frame = e.unmapped_frame,
        mapped_frame = ?e.mapped_frame,
        map_delta = ?map_delta,
        seen_after_frame = ?e.seen_after_map_frame,
        seen_after_mapped = e.seen_after_map_mapped,
        seen_mapped_frame = ?e.seen_mapped_frame,
        looks = e.looks,
        stale = e.stale,
        overwritten = e.overwritten,
        "unmapped texture sample (VICTORY_UNMAPPED_LOOK=1)"
    );
}
