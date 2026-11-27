//! The card's own time for the title's submissions, for the frame line's
//! "on the GPU" (`VICTORY_GPU_TIME`; on unless `0`).
//!
//! Before, that field was the time the submitting thread stood waiting for
//! the card. With submissions sent ahead the thread hardly ever waits, so a
//! release build said 0.0 and nobody could tell whether the card is a wall.
//!
//! Now every submission gets a timestamp list ahead of it and one behind it,
//! in the same `ExecuteCommandLists` calls (no extra calls). The one behind
//! also resolves both into memory the CPU reads. A ring of [`SLOTS`] pairs of
//! lists is recorded once and used again; a pair is used again only after
//! the card has written both of its times, so a list is never sent while it
//! may still run. Times are read in the order the submissions were sent, and
//! the busy time is the union of their spans: a submission that starts
//! before the one ahead of it ends counts only from that end.
//!
//! What it does not count: work of ours sent outside a title submission (the
//! present's copy, fetches of pieces between submissions, the copy queue).
//! Work another thread sends between the parts of one submission that is
//! sent in parts counts as part of it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::Host;

/// Pairs of lists in the ring (a frame has some forty submissions).
pub(super) const SLOTS: usize = 1024;

/// The slot [`take`] gave last (`usize::MAX`: the submission is not timed).
static LAST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Whether submissions are timed on the card.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_GPU_TIME").as_deref() != Ok("0"))
}

struct Ring {
    /// First the lists that go ahead, then those that go behind.
    lists: Vec<ID3D12CommandList>,
    /// The mapping the lists behind resolve into: two tick counts a slot.
    back: usize,
    /// Ticks a second.
    frequency: u64,
    /// Keeps it all.
    _kept: (ID3D12CommandAllocator, ID3D12QueryHeap, ID3D12Resource),
}

// SAFETY: free-threaded objects, and a mapping read behind `STATE`.
unsafe impl Send for Ring {}
// SAFETY: as above.
unsafe impl Sync for Ring {}

/// Pairs taken so far, pairs read so far, and where the last span read ended.
struct State {
    taken: u64,
    read: u64,
    last_end: u64,
}

static RING: OnceLock<Option<Ring>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State {
    taken: 0,
    read: 0,
    last_end: 0,
});

/// Since the last frame line: busy ticks, submissions timed, submissions not
/// timed as every pair was still on its way.
static TOTALS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// The two tick counts of `slot` in the mapping at `back`, once the card has
/// written both.
fn times(back: usize, slot: usize) -> Option<(u64, u64)> {
    // SAFETY: the mapping holds two tick counts for each of `SLOTS`.
    let (from, to) = unsafe {
        let at = (back as *const u64).add(2 * slot);
        (std::ptr::read_volatile(at), std::ptr::read_volatile(at.add(1)))
    };
    (from != 0 && to != 0).then_some((from, to))
}

/// Reads every pair the card has finished, oldest first: the busy ticks
/// they add and how many were read.
fn read_finished(back: usize, state: &mut State) -> (u64, u64) {
    let (mut busy, mut read) = (0, 0);
    while state.read < state.taken {
        let slot = (state.read % SLOTS as u64) as usize;
        let Some((from, to)) = times(back, slot) else {
            break;
        };
        if to >= from {
            busy += to - from.max(state.last_end).min(to);
            state.last_end = state.last_end.max(to);
        }
        read += 1;
        state.read += 1;
    }
    (busy, read)
}

/// [`read_finished`] into [`TOTALS`].
fn count_finished(ring: &Ring, state: &mut State) {
    let (busy, read) = read_finished(ring.back, state);
    TOTALS[0].fetch_add(busy, Ordering::Relaxed);
    TOTALS[1].fetch_add(read, Ordering::Relaxed);
}

/// The lists to put ahead of and behind the next submission, if it is timed.
/// Both must be sent, the first ahead of everything, the second behind.
pub(super) fn take(host: &Host) -> Option<(ID3D12CommandList, ID3D12CommandList)> {
    LAST.store(usize::MAX, Ordering::Relaxed);
    if !on() {
        return None;
    }
    let ring = RING
        .get_or_init(|| match host.make_card_time() {
            Ok(ring) => Some(ring),
            Err(e) => {
                tracing::warn!(error = %e, "submissions are not timed on the card: the frame line's GPU time is the wait for it");
                None
            }
        })
        .as_ref()?;
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    count_finished(ring, &mut state);
    if state.taken - state.read >= SLOTS as u64 {
        TOTALS[2].fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let slot = (state.taken % SLOTS as u64) as usize;
    // SAFETY: as in `times`; the card has written this pair and nothing
    // sent will write it again until it is sent again.
    unsafe { std::ptr::write_bytes((ring.back as *mut u64).add(2 * slot), 0, 2) };
    state.taken += 1;
    LAST.store(slot, Ordering::Relaxed);
    Some((ring.lists[slot].clone(), ring.lists[SLOTS + slot].clone()))
}

/// The slot of the submission [`take`] timed last, if it timed it
/// ([`super::relay_lag`]).
pub(super) fn last_slot() -> Option<usize> {
    let slot = LAST.load(Ordering::Relaxed);
    (slot < SLOTS).then_some(slot)
}

/// The card's ticks a second, once the ring is made.
pub(super) fn frequency() -> Option<u64> {
    RING.get()?.as_ref().map(|ring| ring.frequency)
}

/// When the submission in `slot` ended on the card, in its ticks, once the
/// card has written it.
pub(super) fn end_ticks(slot: usize) -> Option<u64> {
    let ring = RING.get()?.as_ref()?;
    times(ring.back, slot).map(|(_, to)| to)
}

/// For the frame line: the card's busy time per frame in ms, or `waited`
/// (the old measure: the wait for the card) when submissions are not timed.
/// Also says what the measure was on a line of its own.
pub(super) fn line(frames: f64, waited: f64) -> f64 {
    let Some(Some(ring)) = RING.get() else {
        return waited;
    };
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        count_finished(ring, &mut state);
    }
    let [busy, timed, untimed] = [0, 1, 2].map(|i| TOTALS[i].swap(0, Ordering::Relaxed));
    let ms = busy as f64 * 1e3 / ring.frequency.max(1) as f64 / frames;
    tracing::info!(
        "the card's own time for the title's submissions per frame (VICTORY_GPU_TIME=0: off): {ms:.1} ms busy, \
         {:.1} submissions timed, {:.1} not timed (every pair on its way); the submitting thread waited for the card \
         {waited:.1} ms",
        timed as f64 / frames,
        untimed as f64 / frames
    );
    ms
}

impl Host {
    fn make_card_time(&self) -> windows::core::Result<Ring> {
        let bytes = 16 * SLOTS as u64;
        let Some((buffer, back)) = self.shared_buffer(bytes) else {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
        };
        // SAFETY: plain object creation; a fresh mapping of that size.
        unsafe {
            std::ptr::write_bytes(back, 0, bytes as usize);
            let frequency = self.queue.GetTimestampFrequency()?;
            let mut heap: Option<ID3D12QueryHeap> = None;
            self.device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC {
                    Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                    Count: 2 * SLOTS as u32,
                    NodeMask: 0,
                },
                &mut heap,
            )?;
            let heap = heap.ok_or_else(windows::core::Error::empty)?;
            let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let mut ahead = Vec::with_capacity(SLOTS);
            let mut behind = Vec::with_capacity(SLOTS);
            for slot in 0..SLOTS as u32 {
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * slot);
                list.Close()?;
                ahead.push(list.cast()?);
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * slot + 1);
                list.ResolveQueryData(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * slot, 2, &buffer, u64::from(slot) * 16);
                list.Close()?;
                behind.push(list.cast()?);
            }
            ahead.extend(behind);
            Ok(Ring {
                lists: ahead,
                back: back as usize,
                frequency,
                _kept: (allocator, heap, buffer),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spans are read in the order sent, each counted only past the end of
    /// the one before, and reading stops at the first the card has not
    /// written yet.
    #[test]
    fn spans_count_once_and_without_overlap() {
        let mut back = vec![0u64; 2 * SLOTS];
        for (slot, (from, to)) in [(10, 20), (15, 30), (40, 45), (0, 0), (50, 60)].into_iter().enumerate() {
            back[2 * slot] = from;
            back[2 * slot + 1] = to;
        }
        let mut state = State {
            taken: 5,
            read: 0,
            last_end: 0,
        };
        assert_eq!(read_finished(back.as_ptr() as usize, &mut state), (10 + 10 + 5, 3));
        assert_eq!(state.read, 3);
        back[6] = 46;
        back[7] = 48;
        assert_eq!(read_finished(back.as_ptr() as usize, &mut state), (2 + 10, 2));
    }
}
