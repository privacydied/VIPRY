//! The title's bytes under flights' writes, compared and put in in one pass
//! (`VICTORY_UNDER_FUSED`; off unless asked for): [`Shadow::merge_under_writes`]
//! found the changed word runs of a page and then copied each run on its
//! own into the CPU's side of a copy in the card's memory. With the switch
//! every 32-byte block in which a word differs takes the title's bytes as
//! it is compared (the words of a block that do not differ are equal, so the
//! same bytes end up there). Also the time that step takes, by part, for the
//! frame-rate line.

use super::*;

/// Since the last frame-rate line, where [`Shadow::merge_under_writes`]'s
/// time went, in ns: 0 asking which pages were written, 1 finding the
/// stale ones, 2 waiting for the locks, 3 the pages compared (the wall
/// time), 4 noting the runs; summed over threads: 5 reading title memory,
/// 6 comparing and putting in. Then 7 (unused), 8 stale pages, 9 calls.
pub(crate) static UNDER_NS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];

/// Since the last frame-rate line, ns of the host's look through the lists'
/// refreshes for copies shaders may be writing (`stale_under_flights`).
pub(crate) static UNDER_SCAN: AtomicU64 = AtomicU64::new(0);

pub(super) fn under_ns(i: usize, since: std::time::Instant) -> std::time::Instant {
    let now = std::time::Instant::now();
    UNDER_NS[i].fetch_add((now - since).as_nanos() as u64, Ordering::Relaxed);
    now
}

/// [`UNDER_NS`], taken.
pub(crate) fn under_stats() -> [u64; 10] {
    std::array::from_fn(|i| UNDER_NS[i].swap(0, Ordering::Relaxed))
}

/// The frame-rate line of where the time of [`Shadow::merge_under_writes`]
/// goes.
pub(crate) fn line(frames: f64) {
    let u = under_stats().map(|n| n as f64 / frames);
    let ms = |i: usize| u[i] / 1e6;
    tracing::info!(
        "of those, ms a frame: {:.2} finding copies in the lists' refreshes, {:.0} calls: {:.2} asking what was written, {:.2} finding the pages, {:.2} waiting for the locks, {:.2} comparing pages (threads together: {:.2} reading title memory, {:.2} comparing and putting in), {:.2} noting the runs; {:.0} stale pages",
        UNDER_SCAN.swap(0, Ordering::Relaxed) as f64 / 1e6 / frames,
        u[9],
        ms(0),
        ms(1),
        ms(2),
        ms(3),
        ms(5),
        ms(6),
        ms(4),
        u[8],
    );
}

/// Adds the time since it was made to [`UNDER_NS`]`[6]` when dropped.
pub(super) struct Compared(pub(super) std::time::Instant);

impl Drop for Compared {
    fn drop(&mut self) {
        under_ns(6, self.0);
    }
}

/// Whether [`Shadow::merge_under_writes`] puts the title's bytes into the
/// CPU's side of a copy in the card's memory as it compares, a block of 32
/// bytes where one of its words differs (`VICTORY_UNDER_FUSED=1`; off
/// otherwise, or flipped while the title runs by `VICTORY_AB`: the runs are found first and copied one by one afterwards,
/// as before). The same bytes end up there: the words of a block that do not
/// differ are equal. In a session of play a frame had 190,000 runs in 400
/// pages, each a copy of a few bytes of its own.
pub(super) fn under_fused() -> bool {
    crate::asked_for!("VICTORY_UNDER_FUSED")
}

/// [`word_runs`], and every 32-byte block of `held` in which a word differs
/// takes `title`'s bytes (the words of the tail that differ, one by one):
/// `held` is `title` afterwards.
pub(super) fn word_runs_put(at: usize, title: &[u8], held: &mut [u8]) -> Vec<(usize, usize)> {
    #[cfg(target_arch = "x86_64")]
    if at % 4 == 0 && title.len() % 4 == 0 && title.len() == held.len() && std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2, and the lengths are the same.
        return unsafe { word_runs_put_avx2(title, held) };
    }
    let runs = dword_runs(at, title, held, 0);
    for &(r, m) in &runs {
        held[r..r + m].copy_from_slice(&title[r..r + m]);
    }
    runs
}

/// # Safety
/// AVX2, and two slices of one length, whole words.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn word_runs_put_avx2(title: &[u8], held: &mut [u8]) -> Vec<(usize, usize)> {
    use std::arch::x86_64::*;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut push = |from: usize, len: usize| match runs.last_mut() {
        Some((a, n)) if *a + *n == from => *n += len,
        _ => runs.push((from, len)),
    };
    let n = title.len();
    let mut i = 0;
    while i + 256 <= n {
        let mut differ = 0u64;
        for k in 0..8 {
            let at = i + 32 * k;
            // SAFETY: inside both slices (`i + 256 <= n`).
            unsafe {
                let now = _mm256_loadu_si256(title.as_ptr().add(at).cast());
                let old = _mm256_loadu_si256(held.as_ptr().add(at).cast());
                let block = !_mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(now, old))) as u8;
                if block != 0 {
                    _mm256_storeu_si256(held.as_mut_ptr().add(at).cast(), now);
                }
                differ |= u64::from(block) << (8 * k);
            }
        }
        while differ != 0 {
            let first = differ.trailing_zeros();
            let count = (differ >> first).trailing_ones();
            push(i + 4 * first as usize, 4 * count as usize);
            if first + count >= 64 {
                break;
            }
            differ &= u64::MAX << (first + count);
        }
        i += 256;
    }
    while i + 4 <= n {
        if title[i..i + 4] != held[i..i + 4] {
            held[i..i + 4].copy_from_slice(&title[i..i + 4]);
            push(i, 4);
        }
        i += 4;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::word_runs_put;
    use crate::shadow::{PAGE, word_runs};

    #[test]
    fn word_runs_put_finds_the_word_runs_and_puts_the_title_in() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..300 {
            let n = [PAGE, 4096, 256, 260, 4, 0, 1000][round % 7];
            let held: Vec<u8> = (0..n).map(|_| next() as u8).collect();
            let mut title = held.clone();
            // Sparse words, dense stretches, single bytes of words.
            let changes = (next() % 600) as usize;
            for _ in 0..changes.min(n) {
                let at = (next() as usize) % n.max(1);
                let len = [1, 3, 4, 40, 300][(next() % 5) as usize].min(n - at);
                for b in &mut title[at..at + len] {
                    *b = b.wrapping_add(1 + (next() % 255) as u8);
                }
            }
            let want = word_runs(0, &title, &held);
            let mut merged = held.clone();
            let runs = word_runs_put(0, &title, &mut merged);
            assert_eq!(runs, want, "round {round}");
            assert_eq!(merged, title, "round {round}");
            // At most this many runs of more than 256 bytes (the page's
            // copies of their own, `merge_page_under_writes`).
            assert!(runs.iter().filter(|r| r.1 > 256).count() <= n / 260 + 1, "round {round}");
        }
    }
}

/// With the switch, for a page of `n` bytes: takes from `left` the most
/// copies of their own the page can have (`n / 260 + 1`; whole words at word
/// offsets, a copy of its own is a run of more than 256 bytes), so that no
/// byte need be put back. Where they are not there to take, `None`: the pass
/// is as before. Else how many were taken, for [`give_back`].
pub(super) fn take_most(left: &std::sync::atomic::AtomicI64, n: usize) -> Option<i64> {
    let most = (n / 260 + 1) as i64;
    if left.fetch_sub(most, Ordering::Relaxed) >= most {
        Some(most)
    } else {
        left.fetch_add(most, Ordering::Relaxed);
        None
    }
}

/// What [`take_most`] took and the page did not use.
pub(super) fn give_back(left: &std::sync::atomic::AtomicI64, most: i64, copies: i64) {
    debug_assert!(copies <= most);
    left.fetch_add(most - copies, Ordering::Relaxed);
}
