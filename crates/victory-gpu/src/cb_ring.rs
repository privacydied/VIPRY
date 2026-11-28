//! `VICTORY_CB_RING_LOOK=1` (diagnostics builds, off): a look at the title's
//! per-frame constant-buffer ring, read out of the title's own memory once a
//! frame, beside what we refresh and fill in its ranges.
//!
//! What the ring is (static reading of the title's program; only addresses
//! relative to its image and field offsets here):
//! - two slot numbers at `+0x0a943a0c` and `+0x0a943a10`, each counting
//!   0..3 (four frames in flight);
//! - two tables of four pointers, `+0x0a943a18..+0x0a943a30` and
//!   `+0x0a943a38..+0x0a943a50`, one allocator a slot;
//! - an allocator: `+0x08` its chunk count (`u32`), `+0x38` active (`u8`),
//!   `+0x40` a pointer to the linear part, whose `+0x30` is the bytes
//!   written this frame (`u32`), `+0x48` a pointer to a byte a chunk, set
//!   when the chunk was written (the title clears it at its frame's start),
//!   `+0x58` where the card reads it, `+0x60` where the CPU writes it,
//!   `+0x68` an offset or stride.
//!
//! The look is taken where the title's frame wait ends (`crate::frame`), so
//! before the title's frame start clears the chunk bytes: it sees what the
//! frame before wrote. What it tells (the per-frame line): per allocator,
//! whether its numbers move from frame to frame, and how much of what our
//! side refreshes (`Shadow::refresh`) and fills (constant views read at
//! submission) lies in its two ranges. Nothing else changes.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

/// Slot numbers, relative to the title's image.
const SLOTS: [u64; 2] = [0x0a94_3a0c, 0x0a94_3a10];
/// The two tables of four allocator pointers, relative to the image.
const TABLES: [u64; 2] = [0x0a94_3a18, 0x0a94_3a38];
/// Allocators looked at: two tables of four.
const N: usize = 8;
/// More chunk bytes than this are not read (a wrong pointer, not a ring).
const MOST_CHUNKS: u32 = 1 << 16;

/// Where the title's main image is mapped (`title_image`).
static IMAGE: AtomicUsize = AtomicUsize::new(0);

/// Whether the look is asked for.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_CB_RING_LOOK"))
}

/// Records where the title's main image is mapped (the loader tells it).
pub fn title_image(base: usize) {
    IMAGE.store(base, Ordering::Release);
}

/// One allocator as read at one frame's end.
#[derive(Clone, Copy, Default, PartialEq)]
struct Read {
    chunks: u32,
    active: u8,
    cursor: u32,
    dirty: u32,
    gpu: u64,
    cpu: u64,
    stride: u64,
}

/// Per allocator, since the last line.
#[derive(Default)]
struct Tally {
    looks: u64,
    active: u64,
    /// Frames whose cursor differed from the frame before's.
    moved: u64,
    /// Frames whose chunk bytes counted fewer than the frame before's
    /// (the title cleared them between).
    cleared: u64,
    cursor_sum: u64,
    cursor_most: u32,
    dirty_sum: u64,
    /// Frames whose ranges differed from the frame before's.
    retargeted: u64,
    last: Read,
}

struct State {
    tally: [Tally; N],
    slots: [u32; 2],
    /// Frames whose slot numbers differed from the frame before's.
    slot_moved: u64,
    /// Looks that found no image or could not read the globals.
    unread: u64,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            tally: Default::default(),
            slots: [u32::MAX; 2],
            slot_moved: 0,
            unread: 0,
        })
    })
}

/// The ring's ranges as last read: (start, end, allocator, 0 the card's
/// side, 1 the CPU's), for [`refreshed`] and [`filled`].
fn ranges() -> &'static RwLock<Vec<(u64, u64, usize, usize)>> {
    static RANGES: OnceLock<RwLock<Vec<(u64, u64, usize, usize)>>> = OnceLock::new();
    RANGES.get_or_init(|| RwLock::new(Vec::new()))
}

/// [`refreshed`] and [`filled`] per frame: calls and bytes in the card's
/// side of a ring range, the same in the CPU's side, and all calls and
/// bytes (refreshes 0..6, fills 6..12).
static SEEN: [AtomicU64; 12] = [const { AtomicU64::new(0) }; 12];

/// Reads a `T` at `at` through the kernel: a page that is not there fails
/// the read instead of faulting.
pub(crate) fn read<T: Copy + Default>(at: u64) -> Option<T> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    if at == 0 {
        return None;
    }
    let mut value = T::default();
    let mut done = 0;
    // SAFETY: the destination is `value`, of the size given; the kernel
    // checks the source.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            at as *const _,
            (&raw mut value).cast(),
            size_of::<T>(),
            &mut done,
        ) != 0
    };
    (ok && done == size_of::<T>()).then_some(value)
}

/// The chunk bytes at `at` that are set, of `chunks`.
fn set_bytes(at: u64, chunks: u32) -> u32 {
    if at == 0 || chunks == 0 || chunks > MOST_CHUNKS {
        return 0;
    }
    let mut bytes = vec![0u8; chunks as usize];
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut done = 0;
    // SAFETY: as in `read`, into a buffer of `chunks` bytes.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            at as *const _,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut done,
        ) != 0
    };
    if !ok {
        return 0;
    }
    bytes[..done].iter().filter(|&&b| b != 0).count() as u32
}

fn read_one(at: u64) -> Option<Read> {
    let linear: u64 = read(at + 0x40)?;
    let chunks: u32 = read(at + 0x08)?;
    let dirty_at: u64 = read(at + 0x48).unwrap_or(0);
    Some(Read {
        chunks,
        active: read(at + 0x38)?,
        cursor: if linear == 0 {
            0
        } else {
            read(linear + 0x30).unwrap_or(0)
        },
        dirty: set_bytes(dirty_at, chunks),
        gpu: read(at + 0x58)?,
        cpu: read(at + 0x60)?,
        stride: read(at + 0x68).unwrap_or(0),
    })
}

/// The look, once a frame, where the title's frame wait ends.
pub(crate) fn look() {
    if !on() {
        return;
    }
    let base = IMAGE.load(Ordering::Acquire) as u64;
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    let slots: Option<[u32; 2]> = (|| Some([read(base + SLOTS[0])?, read(base + SLOTS[1])?]))();
    let Some(slots) = slots.filter(|_| base != 0) else {
        state.unread += 1;
        return;
    };
    if slots != state.slots {
        state.slot_moved += 1;
        state.slots = slots;
    }
    let mut spans = Vec::with_capacity(2 * N);
    for n in 0..N {
        let at: u64 = read(base + TABLES[n / 4] + 8 * (n % 4) as u64).unwrap_or(0);
        let Some(now) = (at != 0).then(|| read_one(at)).flatten() else {
            continue;
        };
        let t = &mut state.tally[n];
        t.looks += 1;
        t.active += u64::from(now.active != 0);
        t.moved += u64::from(now.cursor != t.last.cursor);
        t.cleared += u64::from(now.dirty < t.last.dirty);
        t.retargeted += u64::from(t.looks > 1 && (now.gpu, now.cpu) != (t.last.gpu, t.last.cpu));
        t.cursor_sum += u64::from(now.cursor);
        t.cursor_most = t.cursor_most.max(now.cursor);
        t.dirty_sum += u64::from(now.dirty);
        t.last = now;
        // The range a frame writes: up to the most bytes written seen.
        let len = u64::from(t.cursor_most.max(now.cursor));
        if len > 0 {
            if now.gpu != 0 {
                spans.push((now.gpu, now.gpu + len, n, 0));
            }
            if now.cpu != 0 {
                spans.push((now.cpu, now.cpu + len, n, 1));
            }
        }
    }
    drop(state);
    *ranges().write().unwrap_or_else(|e| e.into_inner()) = spans;
}

/// Counts `[address, address + len)` against the ring's ranges, into
/// `SEEN[first..first + 6]`.
fn count(first: usize, address: u64, len: u64) {
    SEEN[first + 4].fetch_add(1, Ordering::Relaxed);
    SEEN[first + 5].fetch_add(len, Ordering::Relaxed);
    let end = address.saturating_add(len);
    let spans = ranges().read().unwrap_or_else(|e| e.into_inner());
    let mut hit = [0u64; 2];
    for &(from, to, _, side) in spans.iter() {
        let over = end.min(to).saturating_sub(address.max(from));
        hit[side] = hit[side].max(over);
    }
    for side in 0..2 {
        if hit[side] > 0 {
            SEEN[first + 2 * side].fetch_add(1, Ordering::Relaxed);
            SEEN[first + 2 * side + 1].fetch_add(hit[side], Ordering::Relaxed);
        }
    }
}

/// A refresh asked of a copy of title memory (`Shadow::refresh`).
pub(crate) fn refreshed(address: u64, len: u64) {
    if on() {
        count(0, address, len);
    }
}

/// A constant view filled at submission from title memory.
pub(crate) fn filled(address: u64, len: u64) {
    if on() {
        count(6, address, len);
    }
}

/// For the frame-rate line.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<f64> = SEEN
        .iter()
        .map(|s| s.swap(0, Ordering::Relaxed) as f64 / frames)
        .collect();
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    let spans = ranges().read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut each = String::new();
    for n in 0..N {
        let t = &mut state.tally[n];
        if t.looks == 0 {
            continue;
        }
        let l = t.last;
        // Whether the ranges lie in what renderer port 1 has in place, or
        // in a copy of ours.
        let kind = |at: u64| match crate::shadow::peek(at, 1) {
            Some(s) if s.in_place() => "in place",
            Some(_) => "copied",
            None => "no copy",
        };
        each.push_str(&format!(
            "; {}{}: {} looks, {} active, cursor moved {} cleared {} retargeted {}, cursor mean {:.0} most {}, chunks {} set mean {:.1}, \
             card {:#x} ({}) cpu {:#x} ({}) +0x68 {:#x}",
            if n < 4 { 'a' } else { 'b' },
            n % 4,
            t.looks,
            t.active,
            t.moved,
            t.cleared,
            t.retargeted,
            t.cursor_sum as f64 / t.looks as f64,
            t.cursor_most,
            l.chunks,
            t.dirty_sum as f64 / t.looks as f64,
            l.gpu,
            kind(l.gpu),
            l.cpu,
            kind(l.cpu),
            l.stride,
        ));
        *t = Tally {
            last: l,
            cursor_most: t.cursor_most,
            ..Tally::default()
        };
    }
    tracing::info!(
        target: "victory_gpu::host::cb_ring",
        "constant-buffer ring per frame (VICTORY_CB_RING_LOOK=1): slots {:?} moved {} times, {} looks unread, {} ranges; \
         refreshes in its card side {:.1} ({:.1} KB), in its CPU side {:.1} ({:.1} KB), of {:.0} ({:.1} MB); \
         constant fills in its card side {:.1} ({:.1} KB), in its CPU side {:.1} ({:.1} KB), of {:.0} ({:.1} KB){}",
        state.slots,
        state.slot_moved,
        state.unread,
        spans.len(),
        s[0],
        s[1] / 1024.0,
        s[2],
        s[3] / 1024.0,
        s[4],
        s[5] / (1 << 20) as f64,
        s[6],
        s[7] / 1024.0,
        s[8],
        s[9] / 1024.0,
        s[10],
        s[11] / 1024.0,
        each,
    );
    state.slot_moved = 0;
    state.unread = 0;
}
