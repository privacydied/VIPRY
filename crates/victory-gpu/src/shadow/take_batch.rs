//! Batched 64 KiB `Shadow::take` reads with AVX2 `word_runs` comparison
//! (`VICTORY_TAKE_BATCH=1`, off by default, flippable via `VICTORY_AB`).
//!
//! Ported from CHECK6960/Victory-Nvidia-optimized c51df9f76 (GPL-3.0-or-later).

use std::sync::atomic::Ordering;

use super::{SEND, Shadow, dword_runs};

const TAKE_CHUNK: usize = 64 << 10;

/// Whether batched 64 KiB `Shadow::take` reads and AVX2 `word_runs` are enabled.
#[inline]
pub(super) fn on() -> bool {
    crate::asked_for!("VICTORY_TAKE_BATCH")
}

/// Finds changed 4-byte-aligned runs between `title` and `held`, using AVX2
/// when available and aligned.
pub(super) fn word_runs(at: usize, title: &[u8], held: &[u8]) -> Vec<(usize, usize)> {
    #[cfg(target_arch = "x86_64")]
    if at % 4 == 0
        && title.len() % 4 == 0
        && title.len() == held.len()
        && std::arch::is_x86_feature_detected!("avx2")
    {
        // SAFETY: the processor has AVX2, and the lengths are the same.
        return unsafe { word_runs_avx2(title, held) };
    }
    dword_runs(at, title, held, 0)
}

/// # Safety
/// AVX2, and two slices of one length, whole words.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn word_runs_avx2(title: &[u8], held: &[u8]) -> Vec<(usize, usize)> {
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
            // SAFETY: inside both slices (`i + 256 <= n`).
            let same = unsafe {
                _mm256_cmpeq_epi32(
                    _mm256_loadu_si256(title.as_ptr().add(i + 32 * k).cast()),
                    _mm256_loadu_si256(held.as_ptr().add(i + 32 * k).cast()),
                )
            };
            differ |= u64::from(!_mm256_movemask_ps(_mm256_castsi256_ps(same)) as u8) << (8 * k);
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
            push(i, 4);
        }
        i += 4;
    }
    runs
}

fn push_coalesced(card_runs: &mut Vec<(usize, usize)>, c0: usize, c1: usize) {
    match card_runs.last_mut() {
        Some(last) if c0 <= last.1 => last.1 = last.1.max(c1),
        _ => card_runs.push((c0, c1)),
    }
}

/// Batched 64 KiB implementation of `Shadow::take` for `changes_only` / `send_changed`.
pub(super) fn take_batched(shadow: &Shadow, from: usize, to: usize, whole: bool) -> usize {
    thread_local! {
        static TITLE: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
        static CARD_RUNS: std::cell::RefCell<Vec<(usize, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let start = shadow.start as usize;
    TITLE.with_borrow_mut(|title| {
        CARD_RUNS.with_borrow_mut(|card_runs| {
            card_runs.clear();
            title.resize(TAKE_CHUNK.min(to - from), 0);
            let mut at = from;
            while at < to {
                let n = (TAKE_CHUNK - at % TAKE_CHUNK).min(to - at);
                let chunk = &mut title[..n];
                crate::host::copy_mapped(chunk.as_mut_ptr(), at as *const u8, n, false);
                // SAFETY: inside the mapping of the copy's CPU side, disjoint per part.
                let held = unsafe { std::slice::from_raw_parts_mut(shadow.data.add(at - start), n) };
                let was = crate::host::upcheck::on().then(|| held.to_vec());
                if whole {
                    held.copy_from_slice(chunk);
                    shadow.changed(at - start, at - start + n);
                    push_coalesced(card_runs, at - start, at - start + n);
                    SEND[0].fetch_add(n as u64, Ordering::Relaxed);
                    SEND[2].fetch_add(n as u64, Ordering::Relaxed);
                    SEND[3].fetch_add(1, Ordering::Relaxed);
                } else {
                    let runs = word_runs(at, chunk, held);
                    for &(a, len) in &runs {
                        held[a..a + len].copy_from_slice(&chunk[a..a + len]);
                        shadow.changed(at - start + a, at - start + a + len);
                        push_coalesced(card_runs, at - start + a, at - start + a + len);
                    }
                    SEND[0].fetch_add(n as u64, Ordering::Relaxed);
                    if runs.is_empty() {
                        SEND[1].fetch_add(n as u64, Ordering::Relaxed);
                    }
                    SEND[2].fetch_add(runs.iter().map(|r| r.1 as u64).sum(), Ordering::Relaxed);
                    SEND[3].fetch_add(runs.len() as u64, Ordering::Relaxed);
                }
                if let Some(was) = was {
                    crate::host::upcheck::note(shadow.id, at - start, &was, held);
                }
                at += n;
            }
            if !card_runs.is_empty() {
                shadow.card_changed_runs(card_runs.drain(..));
            }
        });
    });
    to - from
}

#[cfg(test)]
mod tests {
    use super::{dword_runs, word_runs};

    #[test]
    fn word_runs_match_dword_runs_across_64kib_chunk() {
        let was = vec![0x3Au8; 64 << 10];
        let mut now = was.clone();
        for at in [0, 3, 4, 31, 32, 33, 63, 64, 4095, 4096, 32767, 32768, 65532, 65535] {
            now[at] ^= 0x81;
        }
        assert_eq!(word_runs(0x10000, &now, &was), dword_runs(0x10000, &now, &was, 0));
    }
}
