//! `VICTORY_PIPELINE_SPARE_S=<seconds>`: a worker that has been on one
//! pipeline for that long gets a spare worker in its place, so the others
//! in the queue do not wait for it.
//!
//! Why: on this PC (RTX 3090, Proton) the same seven compute pipelines end
//! their worker with a stack overflow inside the host's pipeline create,
//! in every run (18 of 18 logs of 2026-11-27). Seven of the eight workers
//! were gone within 3 s of the start: the world loaded with one worker
//! and the two that only make passes over the screen, and its first
//! 25 s ran at 0.6-1.2 s a frame while the queue drained. The seven
//! pipelines stay "being made" to the end, as their workers never return.
//! A compile that only takes long (some driver compiles take seconds) is
//! treated the same: its worker goes on, and leaves when it is done.
//!
//! The pipeline itself is given up for the run: a first use no longer
//! waits for it (a wait for a pipeline whose worker is gone took the whole
//! half second budget of each frame that used it), and draws with it are
//! dropped as before.
//!
//! Off by default (unset or 0). At most [`SPARES`] spares in a run.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::make::Pending;

/// Spares started in a run, at most.
const SPARES: usize = 16;

/// How often the watch looks at the workers.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// After how long on one pipeline a worker gets a spare; `None`: off.
pub(super) fn after() -> Option<Duration> {
    static AFTER: OnceLock<Option<Duration>> = OnceLock::new();
    *AFTER.get_or_init(|| {
        let seconds: f64 = std::env::var("VICTORY_PIPELINE_SPARE_S")
            .ok()?
            .trim()
            .parse()
            .ok()?;
        (seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
    })
}

#[derive(Default)]
struct Job {
    began: Option<Instant>,
    pipeline: Option<Arc<Pending>>,
    /// Whether the pipeline came from the slow list (the watch gives its
    /// place in [`super::make`]'s slow count back when it replaces the worker).
    slow: bool,
    replaced: bool,
}

/// What one worker is making.
pub(super) struct Slot {
    screens_only: bool,
    job: Mutex<Job>,
}

static SLOTS: Mutex<Vec<Arc<Slot>>> = Mutex::new(Vec::new());

/// The pipelines given up for the run (by address; they stay alive, as
/// their worker holds them).
static GIVEN_UP: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Whether `p`'s worker was replaced while on it: a first use does not
/// wait for it.
pub(super) fn given_up(p: &Pending) -> bool {
    after().is_some()
        && GIVEN_UP
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(std::ptr::from_ref(p) as usize))
}
static STARTED: AtomicUsize = AtomicUsize::new(0);

impl Slot {
    /// A slot for a new worker; watched when the switch is on.
    pub(super) fn new(screens_only: bool) -> Arc<Slot> {
        let slot = Arc::new(Slot {
            screens_only,
            job: Mutex::new(Job::default()),
        });
        if after().is_some() {
            SLOTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(slot.clone());
            watch();
        }
        slot
    }

    /// The worker begins `p` (`slow`: it came from the slow list).
    pub(super) fn begin(&self, p: &Arc<Pending>, slow: bool) {
        if after().is_none() {
            return;
        }
        let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
        *job = Job {
            began: Some(Instant::now()),
            pipeline: Some(p.clone()),
            slow,
            replaced: false,
        };
    }

    /// The worker is done with its pipeline. Whether a spare took its place
    /// meanwhile: then the worker leaves, and the slow count was given back.
    pub(super) fn end(&self) -> bool {
        if after().is_none() {
            return false;
        }
        let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
        let replaced = job.replaced;
        *job = Job::default();
        replaced
    }
}

/// Starts the watch once.
fn watch() {
    static WATCH: OnceLock<()> = OnceLock::new();
    WATCH.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("victory-pipelines-watch".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(LOOK_EVERY);
                    look();
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "no watch of the pipeline workers");
        }
    });
}

/// Gives each worker that has been on one pipeline too long a spare.
fn look() {
    let Some(after) = after() else { return };
    let slots: Vec<Arc<Slot>> = SLOTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for slot in slots {
        if STARTED.load(Ordering::Relaxed) >= SPARES {
            return;
        }
        let Ok(mut job) = slot.job.try_lock() else {
            continue;
        };
        let Some(began) = job.began else { continue };
        if job.replaced || began.elapsed() < after {
            continue;
        }
        job.replaced = true;
        let slow = std::mem::take(&mut job.slow);
        let shaders = job
            .pipeline
            .as_deref()
            .map(super::make::hashes)
            .unwrap_or_default();
        if let Some(p) = job.pipeline.as_deref() {
            GIVEN_UP
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(std::ptr::from_ref(p) as usize);
        }
        drop(job);
        if slow {
            super::make::slow_released();
        }
        let n = STARTED.fetch_add(1, Ordering::Relaxed) + 1;
        let started =
            super::make::spawn_worker(format!("victory-pipelines-spare-{n}"), slot.screens_only);
        tracing::warn!(
            shaders,
            seconds = began.elapsed().as_secs(),
            started,
            spares = n,
            "a pipeline worker is still on one pipeline: a spare worker takes its place (VICTORY_PIPELINE_SPARE_S)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_off_the_switch_is_never_replaced() {
        if after().is_some() {
            return;
        }
        let slot = Slot::new(false);
        let p = Arc::new(Pending::new(Vec::new(), None));
        slot.begin(&p, true);
        assert!(!slot.end());
        assert!(SLOTS.lock().unwrap().is_empty());
    }
}
