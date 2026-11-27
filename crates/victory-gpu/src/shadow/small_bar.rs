//! Small write-combined title buffers in card memory the CPU maps
//! (`VICTORY_SMALL_BUFFERS_BAR_MB`, default 0: off). Like `VICTORY_WC_BAR`,
//! but only for buffers under 1 MiB and up to a total of that many MiB: a
//! single `D3D12_HEAP_TYPE_GPU_UPLOAD` buffer has no CPU's side to send up,
//! so no copy-up race, and shader reads and atomics stay in the card's own
//! memory instead of crossing the bus. What shaders write stays there, as
//! with [`wc_bar`]; a refresh writes whole pages into the mapping.

use super::*;

/// Buffers of this length and over are not small.
const SMALL: u64 = 1 << 20;

/// The most bytes of such buffers alive at once
/// (`VICTORY_SMALL_BUFFERS_BAR_MB` MiB; 0 or unset: none).
fn most() -> u64 {
    static MOST: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("VICTORY_SMALL_BUFFERS_BAR_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0) << 20
    })
}

/// Bytes alive, and how many such buffers were made.
static BYTES: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);

/// Called when such a copy is dropped.
pub(super) fn released(size: u64) {
    BYTES.fetch_sub(size, Ordering::Relaxed);
}

/// A buffer of `size` bytes for the host copy of write-combined title memory
/// `[start, start + len)`, if the switch is on, the memory is small and
/// write-combined, at least 15% of the card's budget is free and the cap
/// holds. Also counted with the card sides of copies ([`card_limits`]), as
/// the drop of the copy gives it back there.
pub(super) fn mapped_for(start: u64, len: u64, size: u64) -> Option<(ID3D12Resource, *mut u8)> {
    let cap = most();
    if cap == 0 || len >= SMALL || !is_write_combined(start, len) {
        return None;
    }
    let host = host()?;
    if let Some([budget, used, ..]) = host.video_memory()
        && used + size > budget / 100 * 85
    {
        return None;
    }
    if BYTES.fetch_add(size, Ordering::Relaxed) + size > cap {
        BYTES.fetch_sub(size, Ordering::Relaxed);
        return None;
    }
    CARD_BYTES[0].fetch_add(size, Ordering::Relaxed);
    let made = host.mapped_card_buffer(size, true);
    if made.is_none() {
        BYTES.fetch_sub(size, Ordering::Relaxed);
        CARD_BYTES[0].fetch_sub(size, Ordering::Relaxed);
    } else {
        let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            tracing::info!(
                count = n,
                mib = BYTES.load(Ordering::Relaxed) >> 20,
                "VICTORY_SMALL_BUFFERS_BAR_MB: small write-combined title buffers placed in card memory the CPU maps"
            );
        }
    }
    made
}
