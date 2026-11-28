//! Per-pass stop-reason accounting for `heap_textures` (`VICTORY_TEXTURE_STAGE_WHY=1`, off by default).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Whether stop-reason accounting is enabled (`VICTORY_TEXTURE_STAGE_WHY=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_STAGE_WHY").is_some_and(|v| v != "0"))
}

/// Why a `heap_textures` pass stopped.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Stop {
    QueueEmpty = 0,
    QueueExhausted = 1,
    DueCap = 2,
    Time = 3,
    Bytes = 4,
    NoRoomRefused = 5,
    AdmitRefused = 6,
    UploadRefusedPlaced = 7,
    UploadRefusedResource = 8,
    UploadRefusedAhead = 9,
    StagingDeferred = 10,
    DeviceRemoved = 11,
}

const STOP_COUNT: usize = 12;
static STOPS: [AtomicU64; STOP_COUNT] = [const { AtomicU64::new(0) }; STOP_COUNT];

/// Per-interval pass summary counters:
/// 0: passes, 1: due_total, 2: staged_count, 3: staged_bytes, 4: room_false_passes,
/// 5: last_span_ms, 6: last_bytes_limit_mb, 7: last_waiting, 8: last_on_time.
static META: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

pub(crate) fn record_pass(
    stop: Stop,
    due_len: usize,
    staged_count: u64,
    staged_bytes: u64,
    room: bool,
    span_ms: u64,
    bytes_limit: u64,
    waiting: u64,
    on_time: usize,
) {
    if !on() {
        return;
    }
    STOPS[stop as usize].fetch_add(1, Relaxed);
    META[0].fetch_add(1, Relaxed);
    META[1].fetch_add(due_len as u64, Relaxed);
    META[2].fetch_add(staged_count, Relaxed);
    META[3].fetch_add(staged_bytes, Relaxed);
    if !room {
        META[4].fetch_add(1, Relaxed);
    }
    META[5].store(span_ms, Relaxed);
    META[6].store(bytes_limit >> 20, Relaxed);
    META[7].store(waiting, Relaxed);
    META[8].store(on_time as u64, Relaxed);
}

/// Logs and resets the interval counters from the 5-second stats block in `host.rs`.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let passes = META[0].swap(0, Relaxed);
    if passes == 0 {
        return;
    }
    let s = STOPS.each_ref().map(|c| c.swap(0, Relaxed));
    let due_total = META[1].swap(0, Relaxed);
    let staged_count = META[2].swap(0, Relaxed);
    let staged_mb = META[3].swap(0, Relaxed) as f64 / (1 << 20) as f64;
    let room_false = META[4].swap(0, Relaxed);
    let span_ms = META[5].load(Relaxed);
    let limit_mb = META[6].load(Relaxed);
    let waiting = META[7].load(Relaxed);
    let on_time = META[8].load(Relaxed);
    let per_frame_mb = staged_mb / frames.max(1.0);
    tracing::info!(
        "texture stage why (VICTORY_TEXTURE_STAGE_WHY=1): passes={passes} due_total={due_total} \
         staged={staged_count} ({staged_mb:.1} MB, {per_frame_mb:.1} MB/frame) room_false={room_false} \
         last(span={span_ms}ms limit={limit_mb}MB waiting={waiting} on_time={on_time}) stops: \
         empty={} exhausted={} due_cap={} time={} bytes={} no_room={} admit={} \
         refused_placed={} refused_res={} refused_ahead={} staging_deferred={} dev_removed={}",
        s[Stop::QueueEmpty as usize],
        s[Stop::QueueExhausted as usize],
        s[Stop::DueCap as usize],
        s[Stop::Time as usize],
        s[Stop::Bytes as usize],
        s[Stop::NoRoomRefused as usize],
        s[Stop::AdmitRefused as usize],
        s[Stop::UploadRefusedPlaced as usize],
        s[Stop::UploadRefusedResource as usize],
        s[Stop::UploadRefusedAhead as usize],
        s[Stop::StagingDeferred as usize],
        s[Stop::DeviceRemoved as usize],
    );
}
