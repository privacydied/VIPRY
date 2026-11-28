//! Diagnostics: the time from one of the title's presents to the next,
//! counted in buckets every 5 s. The title paces in ticks of 33.3 ms, so
//! the buckets are tick multiples with slack: up to 40, 70, 103 and 136 ms,
//! and longer. Each line also gives the median and the 90th percentile.
//! It is on in every build with `diagnostics`, and prints at info under
//! `victory_gpu::queue`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The upper edge of each bucket but the last, in ms.
const EDGES: [f64; 4] = [40.0, 70.0, 103.0, 136.0];
/// The buckets' names, for the log line.
const NAMES: [&str; 5] = ["le40", "le70", "le103", "le136", "gt136"];
/// How long a window is.
const WINDOW: Duration = Duration::from_secs(5);

/// The bucket of a present interval of `ms`.
fn bucket(ms: f64) -> usize {
    EDGES
        .iter()
        .position(|&edge| ms <= edge)
        .unwrap_or(EDGES.len())
}

/// The median and the 90th percentile of `ms` (sorted here), nearest rank.
fn median_p90(ms: &mut [f64]) -> (f64, f64) {
    if ms.is_empty() {
        return (0.0, 0.0);
    }
    ms.sort_by(f64::total_cmp);
    let rank = |p: f64| ms[((p * ms.len() as f64).ceil() as usize).clamp(1, ms.len()) - 1];
    (rank(0.5), rank(0.9))
}

struct Window {
    last: Option<Instant>,
    opened: Option<Instant>,
    counts: [u32; 5],
    ms: Vec<f64>,
}

static WINDOW_NOW: Mutex<Window> = Mutex::new(Window {
    last: None,
    opened: None,
    counts: [0; 5],
    ms: Vec::new(),
});

/// The title's present is over.
pub(super) fn presented() {
    let now = Instant::now();
    let mut w = WINDOW_NOW.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = w.last.replace(now) {
        let ms = (now - last).as_secs_f64() * 1000.0;
        w.counts[bucket(ms)] += 1;
        w.ms.push(ms);
    }
    let opened = *w.opened.get_or_insert(now);
    if now - opened < WINDOW {
        return;
    }
    let counts = std::mem::take(&mut w.counts);
    let mut ms = std::mem::take(&mut w.ms);
    w.opened = Some(now);
    drop(w);
    let (median, p90) = median_p90(&mut ms);
    let buckets: Vec<String> = NAMES
        .iter()
        .zip(counts)
        .map(|(n, c)| format!("{n}={c}"))
        .collect();
    tracing::info!(
        frames = ms.len(),
        median_ms = format_args!("{median:.1}"),
        p90_ms = format_args!("{p90:.1}"),
        "present to present, last 5 s: {}",
        buckets.join(" ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_fall_in_tick_buckets_edges_included() {
        assert_eq!(bucket(16.7), 0);
        assert_eq!(bucket(40.0), 0);
        assert_eq!(bucket(40.1), 1);
        assert_eq!(bucket(66.7), 1);
        assert_eq!(bucket(70.0), 1);
        assert_eq!(bucket(100.0), 2);
        assert_eq!(bucket(103.0), 2);
        assert_eq!(bucket(133.3), 3);
        assert_eq!(bucket(136.0), 3);
        assert_eq!(bucket(136.1), 4);
        assert_eq!(bucket(500.0), 4);
    }

    #[test]
    fn median_and_p90_by_nearest_rank() {
        let mut ms: Vec<f64> = (1..=10).rev().map(|i| f64::from(i) * 10.0).collect();
        assert_eq!(median_p90(&mut ms), (50.0, 90.0));
        assert_eq!(median_p90(&mut [100.0]), (100.0, 100.0));
        assert_eq!(median_p90(&mut []), (0.0, 0.0));
    }
}
