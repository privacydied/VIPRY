//! `VICTORY_DROP_WHY=1`: why draws and dispatches are dropped, in every
//! build. The frame line counts the drops; only diagnostics builds named a
//! reason for each, so a release run could not tell a pipeline still in the
//! workers' queue from one that is never made (the baseline of 2026-11-27:
//! 292 drops a frame, 7 pipelines "being made" for the whole run).
//!
//! One line beside the frame line: drops per frame by where their pipeline
//! is, then the pipelines with the most drops (their shaders' hashes).
//! Off by default; a drop costs a lock and a map entry with it on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::make::{self, Stage};
use crate::object::This;

/// What each class of drop says on the line ([`classify`]).
const CLASSES: [&str; 8] = [
    "no pipeline set",
    "left out on purpose",
    "never queued",
    "queued, no worker on it",
    "taken by a worker, not made",
    "refused or not translated",
    "has a host pipeline (binding, rectangles, mesh test)",
    "ray-tracing builder off",
];
const BUILDER: usize = 7;

static COUNTS: [AtomicU64; CLASSES.len()] = [const { AtomicU64::new(0) }; CLASSES.len()];

/// Drops since the last line by title pipeline: its class, its drops, its
/// shaders' hashes (read at the first drop: the title may release it).
static BY_PIPELINE: Mutex<Option<HashMap<usize, (usize, u64, String)>>> = Mutex::new(None);

/// How many pipelines the line names.
const NAMED: usize = 8;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_DROP_WHY"))
}

fn classify(this: This) -> usize {
    if this.is_null() {
        return 0;
    }
    match make::stage(&super::pipeline(this).host) {
        Stage::LeftOut => 1,
        Stage::NotQueued => 2,
        Stage::Queued => 3,
        Stage::Taken => 4,
        Stage::Refused => 5,
        Stage::Made => 6,
    }
}

fn count(this: This, class: usize) {
    COUNTS[class].fetch_add(1, Ordering::Relaxed);
    let mut map = BY_PIPELINE.lock().unwrap_or_else(|e| e.into_inner());
    let entry = map
        .get_or_insert_with(HashMap::new)
        .entry(this as usize)
        .or_insert_with(|| {
            let hashes = if this.is_null() {
                "-".to_owned()
            } else {
                make::hashes(&super::pipeline(this).host)
            };
            (class, 0, hashes)
        });
    entry.0 = class;
    entry.1 += 1;
}

/// A draw or dispatch of `this` was dropped.
pub(crate) fn note(this: This) {
    if on() {
        count(this, classify(this));
    }
}

/// A dispatch of the ray-tracing structure builder was dropped (it is off).
pub(crate) fn note_builder(this: This) {
    if on() {
        count(this, BUILDER);
    }
}

/// Microseconds first uses waited for host pipelines since the last line:
/// all waits, and those that ended with no pipeline made.
static WAITED_US: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// A first use waited `waited` for a pipeline; `made`: it was made by then.
pub(crate) fn waited(waited: std::time::Duration, made: bool) {
    if on() {
        let us = waited.as_micros() as u64;
        WAITED_US[0].fetch_add(us, Ordering::Relaxed);
        if !made {
            WAITED_US[1].fetch_add(us, Ordering::Relaxed);
        }
    }
}

/// Frames since the last line ([`frames`]); the counts run over all of them.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// The frame line's window of `frames` frames has ended. The frame line
/// counts every second and is printed every five; this line is printed
/// with it and gives the mean over the frames of all those windows.
pub(crate) fn frames(frames: u32) {
    if on() {
        FRAMES.fetch_add(u64::from(frames), Ordering::Relaxed);
    }
}

/// Classes that are not dropped on purpose (by the ray-tracing switches).
fn unplanned(class: usize) -> bool {
    class != 1 && class != BUILDER
}

/// The line, over the frames since the last ([`frames`]).
pub(crate) fn line() {
    if !on() {
        return;
    }
    let frames = FRAMES.swap(0, Ordering::Relaxed) as f64;
    let counts = COUNTS.each_ref().map(|c| c.swap(0, Ordering::Relaxed));
    let waited = WAITED_US
        .each_ref()
        .map(|w| w.swap(0, Ordering::Relaxed) as f64 / 1e3 / frames.max(1.0));
    let map = BY_PIPELINE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default();
    let per_frame = |n: u64| n as f64 / frames.max(1.0);
    let classes: Vec<String> = CLASSES
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(name, n)| format!("{name} {:.1}", per_frame(n)))
        .collect();
    let unplanned_count: u64 = (0..CLASSES.len())
        .filter(|&c| unplanned(c))
        .map(|c| counts[c])
        .sum();
    tracing::info!(
        "dropped per frame by why (VICTORY_DROP_WHY=1): {:.1} not on purpose, over {frames:.0} frames: {}; \
         waited for pipelines {:.1} ms a frame, {:.1} ms of it in vain; in {} pipelines, the most: {}",
        per_frame(unplanned_count),
        if classes.is_empty() {
            "none".to_owned()
        } else {
            classes.join(", ")
        },
        waited[0],
        waited[1],
        map.len(),
        most(map, NAMED)
            .iter()
            .map(|(class, n, hashes)| format!(
                "{hashes} ({}) {:.1}",
                CLASSES[*class],
                per_frame(*n)
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// The `named` entries with the most drops, most first (ties by hashes, so
/// the line does not depend on the map's order).
fn most(map: HashMap<usize, (usize, u64, String)>, named: usize) -> Vec<(usize, u64, String)> {
    let mut all: Vec<(usize, u64, String)> = map.into_values().collect();
    all.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    all.truncate(named);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_the_pipelines_with_most_drops_first() {
        let map: HashMap<usize, (usize, u64, String)> = [
            (1, (4, 30, "b".to_owned())),
            (2, (6, 90, "c".to_owned())),
            (3, (1, 30, "a".to_owned())),
            (4, (0, 5, "-".to_owned())),
        ]
        .into_iter()
        .collect();
        let top = most(map, 3);
        let names: Vec<&str> = top.iter().map(|e| e.2.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
    }

    #[test]
    fn drops_on_purpose_are_not_counted_as_unplanned() {
        assert!(!unplanned(1));
        assert!(!unplanned(BUILDER));
        assert!((0..CLASSES.len()).filter(|&c| unplanned(c)).count() == CLASSES.len() - 2);
    }

    #[test]
    fn a_draw_without_a_pipeline_is_its_own_class() {
        assert_eq!(classify(std::ptr::null()), 0);
    }
}
