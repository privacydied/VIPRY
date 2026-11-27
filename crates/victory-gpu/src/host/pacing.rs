//! Frame pacing: the time from one present to the next (a line every 5 s,
//! which the frame-rate line, sampling one second in five, does not give),
//! the first long waits for new memory with the thread that waited, and
//! `VICTORY_PRESENT_SYNC` (presents wait for the display's refreshes).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Logs one of the first 60 waits for new memory of over 20 ms, with the
/// thread that waited (which of them hold frames up).
pub(super) fn long_wait(what: &str, us: u64) {
    static SAID: AtomicU64 = AtomicU64::new(0);
    if us >= 20_000 && SAID.fetch_add(1, Ordering::Relaxed) < 60 {
        let thread = std::thread::current();
        tracing::info!(
            ms = us as f64 / 1000.0,
            thread = thread.name().unwrap_or("(title)"),
            "a long wait for new memory: {what}"
        );
    }
}

/// `VICTORY_PRESENT_SYNC=1..4`: presents wait for that many refreshes of the
/// display, so that on one without variable refresh every frame is shown
/// for the same time (at 120 Hz: 4 is 30 fps, 2 is 60); 0 or unset: shown
/// at the next refresh, as before.
pub(super) fn present_sync() -> u32 {
    static SYNC: OnceLock<u32> = OnceLock::new();
    *SYNC.get_or_init(|| {
        let n = std::env::var("VICTORY_PRESENT_SYNC").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0).min(4);
        if n > 0 {
            tracing::info!(n, "presents wait for the display's refreshes (VICTORY_PRESENT_SYNC)");
        }
        n
    })
}

/// Microseconds from one present to the next since the last pacing line.
pub(super) struct Pacing {
    intervals: Vec<u32>,
    last_present: Option<Instant>,
}

impl Pacing {
    pub(super) fn new() -> Self {
        Pacing { intervals: Vec::with_capacity(1024), last_present: None }
    }

    /// Notes a present.
    pub(super) fn present(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last_present.replace(now)
            && self.intervals.len() < 1 << 16
        {
            self.intervals.push(now.duration_since(prev).as_micros().min(u128::from(u32::MAX)) as u32);
        }
    }

    /// The intervals since the last call.
    pub(super) fn take(&mut self) -> Vec<u32> {
        std::mem::replace(&mut self.intervals, Vec::with_capacity(1024))
    }
}

/// The frame pacing of the last 5 s: percentiles of the time from one
/// present to the next, the longest, how many took over twice the median
/// or over 50 ms (stutters), and the mean change from one to the next.
pub(super) fn pace_line(intervals: Vec<u32>) {
    if intervals.len() < 2 {
        return;
    }
    let change = intervals.windows(2).map(|w| u64::from(w[0].abs_diff(w[1]))).sum::<u64>() as f64
        / (intervals.len() - 1) as f64
        / 1000.0;
    let mut sorted = intervals;
    sorted.sort_unstable();
    let n = sorted.len();
    let at = |q: f64| f64::from(sorted[((n - 1) as f64 * q).round() as usize]) / 1000.0;
    let median = at(0.5);
    let over = |ms: f64| sorted.iter().filter(|&&us| f64::from(us) / 1000.0 > ms).count();
    tracing::info!(
        "present intervals (frame pacing) over {n} frames: p50 {median:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms; \
         {} over twice the median, {} over 50 ms; mean change from one frame to the next {change:.2} ms",
        at(0.95),
        at(0.99),
        at(1.0),
        over(2.0 * median),
        over(50.0)
    );
}
