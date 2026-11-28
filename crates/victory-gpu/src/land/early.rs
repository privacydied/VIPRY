//! Counts for landing early (docs/landing-early.md, step 2): what closing
//! the pages a landing writes, before the title is told, would cost and
//! give. Nothing here changes what a landing does.
//!
//! Per landing, on the thread that lands flights: the title ranges its
//! merges may write ([`will_write`]: the pieces fetched from copies in the
//! card's memory, and the ranges declared of copies in the PC's memory),
//! joined into runs of whole pages with gaps up to
//! `VICTORY_LAND_EARLY_GAP_KB` (64 by default) closed; and the time from
//! the start of the landing (the card is done with its flights) to the
//! first fence the title is told of ([`told`]). A landing of at most
//! `VICTORY_LAND_EARLY_RUNS` runs (2 by default) is one that could go
//! early: its time to the first fence is what early telling could give,
//! at two protection calls a run.
//!
//! Counted only with `VICTORY_LAND_EARLY_COUNT=1` (off by default).

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

const PAGE: usize = 4096;

/// Whether landings are counted (`VICTORY_LAND_EARLY_COUNT=1`).
fn counting() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LAND_EARLY_COUNT").is_some_and(|v| v == "1"))
}

fn gap() -> usize {
    static GAP: OnceLock<usize> = OnceLock::new();
    *GAP.get_or_init(|| {
        std::env::var("VICTORY_LAND_EARLY_GAP_KB")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(64)
            << 10
    })
}

fn most_runs() -> usize {
    static MOST: OnceLock<usize> = OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("VICTORY_LAND_EARLY_RUNS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(2)
    })
}

/// The landing under way on this thread.
struct Landing {
    began: std::time::Instant,
    ranges: Vec<(usize, usize)>,
    told: Option<std::time::Duration>,
}

thread_local! {
    static NOW: RefCell<Option<Landing>> = const { RefCell::new(None) };
}

mod at {
    /// Landings; those with nothing to merge; those of few runs.
    pub(super) const LANDINGS: usize = 0;
    pub(super) const EMPTY: usize = 1;
    pub(super) const FEW: usize = 2;
    /// Runs and pages of all landings; of those with few runs.
    pub(super) const RUNS: usize = 3;
    pub(super) const PAGES: usize = 4;
    pub(super) const FEW_RUNS: usize = 5;
    pub(super) const FEW_PAGES: usize = 6;
    /// Microseconds to the first fence told: all landings that told one,
    /// how many; those of few runs, how many.
    pub(super) const TOLD_US: usize = 7;
    pub(super) const TOLD: usize = 8;
    pub(super) const FEW_TOLD_US: usize = 9;
    pub(super) const FEW_TOLD: usize = 10;
    /// The most runs of one landing.
    pub(super) const MOST: usize = 11;
    /// Microseconds from the first fence told to the end of the landing
    /// (read-backs and lists of later flights, then what is dropped).
    pub(super) const AFTER_US: usize = 12;
    pub(super) const COUNT: usize = 13;
}

static STATS: [AtomicU64; at::COUNT] = [const { AtomicU64::new(0) }; at::COUNT];

/// A landing begins on this thread.
pub(crate) fn begin() {
    if !counting() {
        return;
    }
    start();
}

fn start() {
    NOW.with(|now| {
        *now.borrow_mut() = Some(Landing {
            began: std::time::Instant::now(),
            ranges: Vec::new(),
            told: None,
        });
    });
}

/// The landing may write `len` bytes of title memory at `at`.
pub(crate) fn will_write(at: usize, len: usize) {
    if len == 0 {
        return;
    }
    NOW.with(|now| {
        if let Some(l) = now.borrow_mut().as_mut() {
            l.ranges.push((at, at + len));
        }
    });
}

/// The landing tells the title of a fence: the first such is kept.
pub(crate) fn told() {
    NOW.with(|now| {
        if let Some(l) = now.borrow_mut().as_mut()
            && l.told.is_none()
        {
            l.told = Some(l.began.elapsed());
        }
    });
}

/// The landing is over: counted.
pub(crate) fn end() {
    let Some(mut l) = NOW.with(|now| now.borrow_mut().take()) else {
        return;
    };
    let (runs, pages) = runs(&mut l.ranges, gap());
    let add = |i: usize, n: u64| STATS[i].fetch_add(n, Relaxed);
    add(at::LANDINGS, 1);
    add(at::RUNS, runs as u64);
    add(at::PAGES, pages as u64);
    STATS[at::MOST].fetch_max(runs as u64, Relaxed);
    let few = runs <= most_runs();
    if runs == 0 {
        add(at::EMPTY, 1);
    } else if few {
        add(at::FEW, 1);
        add(at::FEW_RUNS, runs as u64);
        add(at::FEW_PAGES, pages as u64);
    }
    if let Some(told) = l.told {
        add(at::AFTER_US, l.began.elapsed().saturating_sub(told).as_micros() as u64);
        let us = told.as_micros() as u64;
        add(at::TOLD_US, us);
        add(at::TOLD, 1);
        if few && runs > 0 {
            add(at::FEW_TOLD_US, us);
            add(at::FEW_TOLD, 1);
        }
    }
}

/// `ranges` (title addresses, `[start, end)`) as runs of whole pages, a
/// gap of up to `gap` bytes between two closed: how many runs, and how
/// many pages they hold (gaps closed counted).
pub(crate) fn runs(ranges: &mut [(usize, usize)], gap: usize) -> (usize, usize) {
    ranges.sort_unstable();
    let mut runs = 0;
    let mut pages = 0;
    let mut open: Option<(usize, usize)> = None;
    for &(start, end) in ranges.iter() {
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        match &mut open {
            Some((_, to)) if first * PAGE <= *to * PAGE + gap => *to = (*to).max(last),
            _ => {
                if let Some((from, to)) = open.replace((first, last)) {
                    runs += 1;
                    pages += to - from;
                }
            }
        }
    }
    if let Some((from, to)) = open {
        runs += 1;
        pages += to - from;
    }
    (runs, pages)
}

/// For the frame-rate line, per frame.
pub(crate) fn log(frames: f64) {
    let s: Vec<f64> = STATS.iter().map(|s| s.swap(0, Relaxed) as f64).collect();
    if s[at::LANDINGS] == 0.0 {
        return;
    }
    let per = |i: usize| s[i] / frames;
    let mean_ms = |us: usize, n: usize| if s[n] > 0.0 { s[us] / s[n] / 1000.0 } else { 0.0 };
    tracing::info!(
        "landing early, counted only (docs/landing-early.md) per frame: {:.1} landings, {:.1} with nothing to merge, \
         {:.1} of at most {} runs ({:.1} runs, {:.0} pages); all landings {:.1} runs, {:.0} pages, the most in one {}; \
         from the card done to the first fence told {:.2} ms a landing ({:.2} ms a frame), in the landings of few runs \
         {:.2} ms a landing ({:.2} ms a frame); after it, still landing, {:.2} ms a frame (gap {} KiB)",
        per(at::LANDINGS),
        per(at::EMPTY),
        per(at::FEW),
        most_runs(),
        per(at::FEW_RUNS),
        per(at::FEW_PAGES),
        per(at::RUNS),
        per(at::PAGES),
        s[at::MOST],
        mean_ms(at::TOLD_US, at::TOLD),
        per(at::TOLD_US) / 1000.0,
        mean_ms(at::FEW_TOLD_US, at::FEW_TOLD),
        per(at::FEW_TOLD_US) / 1000.0,
        per(at::AFTER_US) / 1000.0,
        gap() >> 10,
    );
    STATS[at::MOST].store(0, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_no_run() {
        assert_eq!(runs(&mut [], 64 << 10), (0, 0));
    }

    #[test]
    fn ranges_in_one_page_are_one_run() {
        let mut r = [(0x1010, 0x1020), (0x1800, 0x1900)];
        assert_eq!(runs(&mut r, 0), (1, 1));
    }

    #[test]
    fn a_range_takes_every_page_it_touches() {
        let mut r = [(0x1ff0, 0x3010)];
        assert_eq!(runs(&mut r, 0), (1, 3));
    }

    #[test]
    fn a_gap_up_to_the_limit_is_closed_and_counted() {
        // Pages 0 and 17: 16 pages (64 KiB) between them.
        let mut r = [(17 * PAGE, 18 * PAGE), (0, PAGE)];
        assert_eq!(runs(&mut r, 64 << 10), (1, 18));
        let mut r = [(17 * PAGE, 18 * PAGE), (0, PAGE)];
        assert_eq!(runs(&mut r, 60 << 10), (2, 2));
    }

    #[test]
    fn overlapping_and_unsorted_ranges_join() {
        let mut r = [(5 * PAGE, 9 * PAGE), (0, 3 * PAGE), (2 * PAGE, 6 * PAGE), (100 * PAGE, 101 * PAGE)];
        assert_eq!(runs(&mut r, 0), (2, 10));
    }

    #[test]
    fn a_landing_is_counted_with_its_time_to_the_first_fence() {
        for s in &STATS {
            s.store(0, Relaxed);
        }
        start();
        will_write(0, PAGE);
        will_write(0, 0);
        told();
        told();
        end();
        start();
        end();
        assert_eq!(STATS[at::LANDINGS].load(Relaxed), 2);
        assert_eq!(STATS[at::EMPTY].load(Relaxed), 1);
        assert_eq!(STATS[at::FEW].load(Relaxed), 1);
        assert_eq!(STATS[at::RUNS].load(Relaxed), 1);
        assert_eq!(STATS[at::TOLD].load(Relaxed), 1);
        assert_eq!(STATS[at::FEW_TOLD].load(Relaxed), 1);
        // Outside a landing: nothing.
        will_write(0, PAGE);
        told();
        end();
        assert_eq!(STATS[at::LANDINGS].load(Relaxed), 2);
    }
}
