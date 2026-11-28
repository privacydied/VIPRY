//! Host pipelines are made by workers, not by the title thread that first
//! records a draw with them. Translating a shader and the host driver's
//! compile take from milliseconds to seconds each (structurisation of the
//! biggest compute programs ~0.4 s, some driver compiles 3.4 s), and
//! the title's first world frame wants over 1,200 new pipelines: made
//! on its recording threads, that frame took more than 30 s and the
//! title's GPU watchdog ended the run (decision 0007, "Pipelines made off
//! the title's threads").
//!
//! A first use queues the pipeline and waits for it within a budget per
//! frame, so the cheap ones (menus, one-off passes such as a colour-grading
//! bake) are there at once as before; past the budget a draw or dispatch
//! whose pipeline is not ready is dropped, as an untranslated one is.
//!
//! With `VICTORY_PIPELINE_STANDINS=1`, a pending graphics draw first tries
//! a finished pipeline with the same shaders and active state. A hit binds
//! immediately. A miss keeps the budget, including the one for one-off passes.
//!
//! The default budget is half a second per title frame. A pass the title
//! runs only once and whose pipeline takes longer is lost. Passes over the
//! screen (no vertex input, no depth) have workers and a longer budget of
//! their own ([`screen_budget`]): the title fills its lighting tables with
//! such draws once, at the world's first frame, and behind a world's worth of
//! pipelines they were dropped; that run had no lamps and no diffuse light to
//! its end (run textures-d2, the first after a new translation; Belobug
//! 18e57db8 found the same).

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::Shader;
use super::host::{GraphicsState, HostPipeline};

/// What a host pipeline is made from, and the host pipeline once made.
pub(crate) struct Pending {
    pub(super) shaders: Vec<Arc<Shader>>,
    pub(crate) graphics: Option<Box<GraphicsState>>,
    queue_state: AtomicU8,
    /// Taken by a worker from [`Queue::slow`].
    was_slow: AtomicBool,
    made: OnceLock<Option<HostPipeline>>,
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    pub(super) key: String,
    used: AtomicBool,
    /// A pass over the screen ([`GraphicsState::screen_pass`]) or a confirmed
    /// opening-scene / bake producer that must never sit behind cold RT shaders.
    screen: bool,
    /// Confirmed first-scene / bake producer prepared early even on a cold cache.
    scene_input: bool,
    /// Why this pipeline is intentionally skipped (e.g. RT resolve without BVH).
    left_out: Option<&'static str>,
    compatibility: OnceLock<blake3::Hash>,
    donor: OnceLock<Arc<Pending>>,
    usage: OnceLock<super::library::Usage>,
    replayed: AtomicBool,
    /// Microseconds first uses waited for it, over all frames ([`super::wait_each`]).
    waited_us: AtomicU64,
}

/// Whether a compute shader of the pipeline carries `name`, a pipeline from
/// pieces by the book's record of its program (`Shader::has_name`).
fn named(shaders: &[Arc<Shader>], name: &[u8]) -> bool {
    shaders.iter().any(|s| s.stage == "compute" && s.has_name(name))
}

/// Shader hashes whose pipelines are left out in every build
/// (`VICTORY_LEAVE_OUT=<16 hex digits>[,..]`; `VICTORY_SKIP_PIPELINES` does
/// the same in diagnostics builds only). The decal heap's
/// `CS_DefragHeap_ComputeRelocations` (02384a60fda6f2d0), translatable for
/// 32-lane hosts through host waves that meet by spinning, took 11.85 ms of
/// the card a frame in the first gameplay area (timing run, 2026-11-26).
/// The hashes are those of the title's own containers, and a pipeline made
/// from pieces (`VICTORY_PIPELINE_PIECES=1`) is left out when the title's
/// container of the same program is named (`Shader::is`).
fn leave_out() -> &'static [String] {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    HASHES.get_or_init(|| {
        std::env::var("VICTORY_LEAVE_OUT")
            .unwrap_or_default()
            .split(',')
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect()
    })
}

/// The light in the water's volume, in tiles (`water_shading:CS_TransmissionVolumetrics_Tiled`
/// and `_Tiled_LocalLights`, version 1.0.2.1): programs of one 64-lane wave a group, which a
/// host of 32-lane waves gets in the wave-wide form only (`victory_shader::emit`, 4 and 7
/// branches taken wave-wide). In that form they leave whole tiles of open water dark, around
/// boats, along shores and under bridges, and take about 15 ms of the card a frame (RTX 5070,
/// runs r_water1 and r_water3 of 2026-11-26: 25 fps with them, 30 without, the blocks gone;
/// fxceless/victory-fork's README of 2026-11-27 names the same two programs and form). Left
/// out there, the water has no light of its own depth (darker, bluer) and no blocks.
/// foxhound 0.2.0-pre.3 fixed the blocks (64-lane programs on 32-lane waves:
/// `VICTORY_LOOPS_WAVE`, `VICTORY_LDS_FIRST_INIT`), so they run now unless
/// `VICTORY_WATER_LIGHT=0` (an empty value, an earlier version's saved OFF, runs them too).
const WATER_LIGHT: [&str; 2] = ["5dc3b25986622949", "6f225b641c5b0708"];

fn water_light_left_out(shaders: &[Arc<Shader>]) -> bool {
    static ASKED: OnceLock<bool> = OnceLock::new();
    *ASKED.get_or_init(|| std::env::var_os("VICTORY_WATER_LIGHT").is_some_and(|v| v == "0"))
        && victory_shader::emit::host_settings()[0] == 32
        && shaders.iter().any(|s| WATER_LIGHT.iter().any(|h| s.is(h)))
}

/// Whether the title's own ray tracing is asked for: its structure builder
/// runs (`VICTORY_BVH_BUILDER=1`), which `VICTORY_RT_NATIVE=1` asks for as
/// well (the title's calls of the builder then also go through
/// `victory_hle::modules::bvh_builder`). With neither, nothing changes.
pub(super) fn builder_asked() -> bool {
    static ASKED: OnceLock<bool> = OnceLock::new();
    *ASKED.get_or_init(|| {
        ["VICTORY_BVH_BUILDER", "VICTORY_RT_NATIVE"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|v| v == "1"))
    })
}

fn left_out(shaders: &[Arc<Shader>]) -> Option<&'static str> {
    if !leave_out().is_empty() && shaders.iter().any(|s| leave_out().iter().any(|h| s.is(h))) {
        return Some("named in VICTORY_LEAVE_OUT, left out");
    }
    if water_light_left_out(shaders) {
        return Some("the water's tiled light on a host of 32-lane waves, left out (VICTORY_WATER_LIGHT=0)");
    }
    static SKIP_RT_RESOLVE: OnceLock<bool> = OnceLock::new();
    let skip_rt = *SKIP_RT_RESOLVE.get_or_init(|| {
        if builder_asked() {
            return false;
        }
        // Without BVH, ~1,000 CS_RaytracingResolve shaders shade no hits while
        // clogging the cold compiler queue; skip unless VICTORY_RT_RESOLVE=1.
        std::env::var_os("VICTORY_RT_RESOLVE").is_none_or(|v| v == "0")
    });
    if skip_rt && named(shaders, b"CS_RaytracingResolve") {
        return Some("shades ray-tracing hits without BVH, left out");
    }
    // The screen probes' ray activation runs without the structure builder
    // too: left out, the probes kept stale values and surfaces lit mostly by
    // them took a magenta or violet cast (papers on a wall, irises), at no
    // frame-rate gain (bisected and measured by a tester, issue #2 of the
    // public repository, 2026-11-27). `VICTORY_ACTIVATE_RAYS=0`: left out,
    // as before.
    static SKIP_ACTIVATE_RAYS: OnceLock<bool> = OnceLock::new();
    let skip_activate = *SKIP_ACTIVATE_RAYS.get_or_init(|| {
        if builder_asked() {
            return std::env::var_os("VICTORY_ACTIVATE_RAYS").is_some_and(|v| v == "0");
        }
        std::env::var_os("VICTORY_ACTIVATE_RAYS").is_none_or(|v| v == "0")
    });
    if skip_activate && named(shaders, b"CS_ScreenProbes_ActivateRays") {
        return Some("ray activation shader without BVH, left out");
    }
    None
}

/// The programs of the first scene's producers, by the hashes of the title's
/// own containers (`Shader::is` also matches them when made from pieces).
const SCENE_INPUT: [&str; 6] = [
    "970c4fbb3b017c0e",
    "068c90badc86f1fd",
    "e4a7e088c7780ce8",
    "ebe4e53f031f5249",
    "2cc4c197ee29e291",
    "369cbe906ea72764",
];

/// Confirmed first-scene producers. Prepare these at creation even on a cold
/// launch so cutscene lighting, tiling, and bakes never miss their first frame.
fn scene_input(shaders: &[Arc<Shader>]) -> bool {
    shaders.iter().any(|s| {
        SCENE_INPUT
            .iter()
            .chain(crate::draw::BAKE_PIXEL_SHADERS.iter())
            .any(|h| s.is(h))
    })
}

// SAFETY: D3D12 objects are free-threaded; the graphics state (whose input
// elements point into its own names) is only read after creation.
unsafe impl Send for Pending {}
// SAFETY: as above.
unsafe impl Sync for Pending {}

impl Pending {
    pub(crate) fn new(shaders: Vec<Arc<Shader>>, graphics: Option<Box<GraphicsState>>) -> Pending {
        let mut key = blake3::Hasher::new();
        for s in &shaders {
            key.update(s.stage.as_bytes());
            key.update(s.hash.as_bytes());
        }
        key.update(graphics.as_ref().map(|g| g.describe()).unwrap_or_default().as_bytes());
        let scene_input = scene_input(&shaders);
        let screen = (graphics.as_ref().is_some_and(|g| g.screen_pass())
            && shaders.iter().all(|s| s.stage != "mesh"))
            || scene_input;
        let left_out = left_out(&shaders);
        Pending {
            screen,
            scene_input,
            left_out,
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            was_slow: AtomicBool::new(false),
            made: OnceLock::new(),
            key: key.finalize().to_hex()[..32].to_owned(),
            used: AtomicBool::new(false),
            compatibility: OnceLock::new(),
            donor: OnceLock::new(),
            usage: OnceLock::new(),
            replayed: AtomicBool::new(false),
            waited_us: AtomicU64::new(0),
        }
    }

    pub(super) fn key(&self) -> &str {
        &self.key
    }

    fn make(self: &Arc<Self>) {
        self.make_with(|| {
            let _building = crate::timeline::span("pipeline: build a host pipeline");
            let began = Instant::now();
            let made = HostPipeline::new(&self.shaders, self.graphics.as_deref());
            super::library::note_time(&self.key, began.elapsed().as_millis() as u64);
            made
        });
        if standins()
            && self.left_out.is_none()
            && self.made.get().is_some_and(Option::is_some)
            && let Some(key) = self.compatibility()
        {
            donors()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, Arc::downgrade(self));
        }
    }

    fn make_with(&self, build: impl FnOnce() -> Option<HostPipeline>) {
        self.made.get_or_init(|| {
            // A failed translation must complete this queue entry and wake its waiters:
            // a panic left `made` empty for ever and the worker thread gone with it.
            match catch_unwind(AssertUnwindSafe(build)) {
                Ok(pipeline) => pipeline,
                Err(_) => {
                    tracing::warn!(shaders = ?self.shaders, "host pipeline build panicked");
                    None
                }
            }
        });
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
}

fn standins() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_STANDINS").is_some_and(|v| v == "1"))
}

/// Approach after XCL 625a0f3: share an existing driver pipeline across
/// compatible title objects. Here the donor actually draws. The same interned
/// shaders give it the same root bindings, resource accesses and mesh launch
/// convention. Only state that D3D12 ignores can differ.
impl Pending {
    fn compatibility(&self) -> Option<blake3::Hash> {
        let graphics = self.graphics.as_ref()?;
        if self.shaders.is_empty()
            || self
                .shaders
                .iter()
                .any(|s| !matches!(s.stage, "vertex" | "mesh" | "pixel"))
        {
            return None;
        }
        Some(*self.compatibility.get_or_init(|| {
            let mut hash = blake3::Hasher::new();
            hash.update(&self.shaders.len().to_le_bytes());
            for shader in &self.shaders {
                hash.update(&(Arc::as_ptr(shader) as usize).to_le_bytes());
            }
            hash.update(&graphics.cache_bytes(true));
            hash.finalize()
        }))
    }

    fn standin(&self) -> Option<&HostPipeline> {
        if let Some(real) = self.made.get() {
            return real.as_ref();
        }
        let key = self.compatibility()?;
        let donor = if let Some(p) = self.donor.get() {
            p
        } else {
            let mut table = donors().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(real) = self.made.get() {
                return real.as_ref();
            }
            let Some(p) = table.get(&key)?.upgrade() else {
                table.remove(&key);
                return None;
            };
            self.donor.get_or_init(|| p)
        };
        let host = donor.made.get()?.as_ref()?;
        STANDIN_HITS.fetch_add(1, Ordering::Relaxed);
        Some(host)
    }

    /// The short key in the old early list was only a scheduling hint.
    /// Sharing a built object also requires matching the complete inputs.
    pub(super) fn same_inputs(&self, other: &Self) -> bool {
        self.shaders.len() == other.shaders.len()
            && self
                .shaders
                .iter()
                .zip(&other.shaders)
                .all(|(a, b)| Arc::ptr_eq(a, b))
            && match (&self.graphics, &other.graphics) {
                (None, None) => true,
                (Some(a), Some(b)) => a.cache_bytes(false) == b.cache_bytes(false),
                _ => false,
            }
    }
}

fn donors() -> &'static Mutex<HashMap<blake3::Hash, Weak<Pending>>> {
    static DONORS: OnceLock<Mutex<HashMap<blake3::Hash, Weak<Pending>>>> = OnceLock::new();
    DONORS.get_or_init(Mutex::default)
}

pub(super) static STANDIN_HITS: AtomicU64 = AtomicU64::new(0);
pub(super) static STANDIN_MISSES: AtomicU64 = AtomicU64::new(0);

/// Why a pipeline has no host pipeline (diagnostics): left out on purpose
/// ([`left_out`]), its shaders that do not translate, else "refused" or
/// "being made"; `None` if it has one. (A pipeline left out is never made:
/// it was counted as "being made" in every frame, some 630 indirect
/// ray-tracing resolves a frame in gameplay, while no pipeline was queued.)
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    if let Some(why) = p.left_out {
        return Some(format!("left out on purpose: {why}"));
    }
    let p = p.usage.get().and_then(|u| u.shared.as_deref()).unwrap_or(p);
    match p.made.get() {
        Some(Some(_)) => None,
        None => Some("being made".into()),
        Some(None) => {
            let failed: Vec<String> = p
                .shaders
                .iter()
                .filter(|s| matches!(s.translated.get(), Some(None)))
                .map(|s| format!("{} {}", s.stage, s.hash))
                .collect();
            Some(if failed.is_empty() {
                format!("refused {:?}", p.shaders)
            } else {
                failed.join(" + ")
            })
        }
    }
}

/// Where a pipeline's host pipeline is, for [`super::drop_why`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Stage {
    LeftOut,
    NotQueued,
    Queued,
    Taken,
    Refused,
    Made,
}

pub(super) fn stage(p: &Pending) -> Stage {
    if p.left_out.is_some() {
        return Stage::LeftOut;
    }
    let p = p.usage.get().and_then(|u| u.shared.as_deref()).unwrap_or(p);
    match p.made.get() {
        Some(Some(_)) => Stage::Made,
        Some(None) => Stage::Refused,
        None => match p.queue_state.load(Ordering::Relaxed) {
            NOT_QUEUED => Stage::NotQueued,
            MAKING => Stage::Taken,
            _ => Stage::Queued,
        },
    }
}

/// The hashes of the shaders `p` is made of.
pub(crate) fn hashes(p: &Pending) -> String {
    let hashes: Vec<&str> = p.shaders.iter().map(|s| s.hash.as_str()).collect();
    hashes.join("+")
}

/// Signalled whenever a worker has made a pipeline.
static DONE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

/// Waiting allowed per title frame, over all threads.
fn wait_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_PIPELINE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500);
        tracing::info!(millis, "host pipeline wait budget per frame");
        Duration::from_millis(millis)
    })
}

/// As [`wait_budget`], for passes over the screen (`VICTORY_SCREEN_WAIT_MS`,
/// 10 s; 0: like the others). Below the title's 30 s watchdog.
fn screen_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_SCREEN_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(10_000);
        Duration::from_millis(millis)
    })
}

/// Whether passes over the screen go before the others.
fn screens_first() -> bool {
    !screen_budget().is_zero()
}

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Draws and dispatches (direct and indirect) asked of a pipeline left out on
/// purpose ([`left_out`]), since the last [`take_left_out`].
static LEFT_OUT: AtomicU64 = AtomicU64::new(0);

/// See [`LEFT_OUT`] (for the frame line).
pub(crate) fn take_left_out() -> u64 {
    LEFT_OUT.swap(0, Ordering::Relaxed)
}

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    if p.left_out.is_some() {
        LEFT_OUT.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let p = if super::library::warm_enabled() {
        let usage = p.usage.get_or_init(|| super::library::usage(p));
        usage.calls.fetch_add(1, Ordering::Relaxed);
        usage.shared.as_ref().unwrap_or(p)
    } else {
        p
    };
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
        if p.replayed.load(Ordering::Relaxed) {
            match p.made.get() {
                Some(Some(_)) => {
                    REPLAY_READY.fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    REPLAY_LATE.fetch_add(1, Ordering::Relaxed);
                }
                Some(None) => {} // Reported separately by missing().
            }
        }
    }
    if p.made.get().is_none() {
        start(p, REQUESTED);
        if standins() && p.compatibility().is_some() {
            if let Some(real) = p.made.get() {
                return real.as_ref();
            }
            if let Some(host) = p.standin() {
                return Some(host);
            }
            STANDIN_MISSES.fetch_add(1, Ordering::Relaxed);
        }
        wait(p, crate::frame::current());
    }
    p.made.get()?.as_ref()
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    if p.left_out.is_some() {
        return false;
    }
    let p = if super::library::warm_enabled() {
        let usage = p.usage.get_or_init(|| super::library::usage(p));
        usage.shared.as_ref().unwrap_or(p)
    } else {
        p
    };
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it),
/// or if it is a confirmed opening-scene / bake producer.
pub(crate) fn early(mut p: Arc<Pending>) -> Arc<Pending> {
    if p.left_out.is_some() {
        return p;
    }
    if super::library::warm_enabled() {
        super::library::start_warmup();
    }
    if !p.shaders.is_empty() && (p.scene_input || super::library::wanted_early(&p.key)) {
        if super::library::warm_enabled() {
            p = super::library::shared(p);
        }
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(&p, if p.scene_input { REQUESTED } else { EARLY });
    } else if super::library::warm_enabled()
        && let Some(shared) = super::library::replayed(&p)
    {
        return shared;
    }
    p
}

pub(super) fn replay(p: &Arc<Pending>) {
    p.replayed.store(true, Ordering::Relaxed);
    start(p, REPLAY);
}

pub(super) static REPLAY_READY: AtomicU64 = AtomicU64::new(0);
pub(super) static REPLAY_LATE: AtomicU64 = AtomicU64::new(0);

/// Queues `p` for the workers at once, ahead of any draw or dispatch: for
/// pipelines that are all needed in the same instant (the structure
/// builder's, which run all or none). Nothing waits here; [`now`] waits
/// for one a worker has begun and makes one no worker has taken yet.
///
/// The builder's were made by [`now`] alone at first: one after the other
/// on the title's recording thread at the first builder dispatch, some
/// fifty programs at two to four a second on a cache that had none of
/// them. The frame stood for 32 s and the title's own watchdog (30 s)
/// ended the run.
pub(super) fn ahead(p: &Arc<Pending>) {
    // (One left out on purpose is never made; one without shaders has
    // nothing to make.)
    if p.left_out.is_some() || p.shaders.is_empty() {
        return;
    }
    start(p, REQUESTED);
}

/// Whether the frame's budget is one for all threads together: a thread
/// that waits takes what is left of it and gives back what it did not
/// use. Counted only when a wait had ended, the budget was one per thread:
/// each of the title's five recording threads began its first wait of a
/// frame with nothing counted yet and waited the whole half second, 2.5 s
/// of waiting in every frame in which pipelines were being made (runs
/// 34-49: "2504 ms waiting for pipelines" in each such frame).
/// On by default; `VICTORY_PIPELINE_WAIT_SHARED=0` counts as before.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0"))
}

/// Waits for `p` while frame `frame` has budget left.
fn wait(p: &Pending, frame: u64) {
    let screen = p.screen && screens_first();
    let budget = if screen { screen_budget() } else { wait_budget() };
    if budget.is_zero() || super::spare::given_up(p) {
        return;
    }
    let own = if screen {
        Duration::MAX
    } else {
        super::wait_each::left(Duration::from_micros(p.waited_us.load(Ordering::Relaxed)))
    };
    if own.is_zero() {
        return;
    }
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now); of the passes over the screen too.
    static SPENT: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let spent_of = &SPENT[usize::from(screen)];
    let shared = shared_budget();
    let left = {
        let mut spent = spent_of.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1).min(own);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        return;
    }
    let waiting = crate::timeline::span("pipeline: first use waits for a host pipeline");
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    waiting.end();
    if !screen {
        p.waited_us.fetch_add(waited.as_micros() as u64, Ordering::Relaxed);
    }
    super::drop_why::waited(started.elapsed(), p.made.get().is_some());
    if screen && waited > Duration::from_millis(500) {
        tracing::info!(
            ms = waited.as_millis() as u64,
            made = p.made.get().is_some(),
            shaders = ?p.shaders,
            "a pass over the screen was waited for (VICTORY_SCREEN_WAIT_MS=0: dropped past half a second a frame)"
        );
    }
    {
        let mut spent = spent_of.lock().unwrap_or_else(|e| e.into_inner());
        // A frame that began meanwhile has its own budget.
        if spent.0 == frame {
            if shared {
                spent.1 = spent.1.saturating_sub(left - waited);
            } else {
                spent.1 += waited;
            }
        }
    }
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
}

const NOT_QUEUED: u8 = 0;
const EARLY: u8 = 1;
const REPLAY: u8 = 2;
const REQUESTED: u8 = 3;
const MAKING: u8 = 4;

/// How many workers may make pipelines at once that are known to take long
/// and that no draw has asked for: a quarter of them. On a fresh cache (a
/// change of the translator or the driver) the early list begins with
/// programs that take tens of seconds each: all 16 workers were in them
/// through the whole of the title's loading, 97 pipelines made in 20 s, and
/// the first minute and a half of the world ran at 0.5 to 1.6 s a frame,
/// dropping up to 2,900 draws a frame (run shaders-1).
/// `VICTORY_PIPELINE_SLOW=0`: in the order the title creates them, as before.
fn slow_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if std::env::var_os("VICTORY_PIPELINE_SLOW").is_some_and(|v| v == "0") {
            usize::MAX
        } else {
            (worker_count(None, std::thread::available_parallelism().map_or(2, |n| n.get())) / 4).max(1)
        }
    })
}

struct Queue {
    /// Requested passes over the screen: before all others, and the only ones
    /// [`SCREEN_WORKERS`] take.
    screen: VecDeque<Arc<Pending>>,
    early: VecDeque<Arc<Pending>>,
    /// Those of the early ones known to take long, and how many of them are
    /// being made ([`slow_limit`]).
    slow: VecDeque<Arc<Pending>>,
    slow_now: usize,
    replay: VecDeque<Arc<Pending>>,
    requested: VecDeque<Arc<Pending>>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            screen: VecDeque::new(),
            early: VecDeque::new(),
            slow: VecDeque::new(),
            slow_now: 0,
            replay: VecDeque::new(),
            requested: VecDeque::new(),
        }
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        self.push_as(p, priority, slow_limit() != usize::MAX && super::library::slow(&p.key))
    }

    fn push_as(&mut self, p: &Arc<Pending>, priority: u8, slow: bool) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED && p.screen && screens_first() {
            self.screen.push_back(p.clone());
        } else if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else if priority == REPLAY {
            self.replay.push_back(p.clone());
        } else if slow {
            self.slow.push_back(p.clone());
        } else {
            self.early.push_back(p.clone());
        }
        Some(prev == NOT_QUEUED)
    }

    fn pop(&mut self, screens_only: bool, slow_limit: usize) -> Option<Arc<Pending>> {
        // An early entry can have been promoted and claimed already.
        let unclaimed = |list: &mut VecDeque<Arc<Pending>>, state: u8| {
            std::iter::from_fn(|| list.pop_front()).find(|p| p.queue_state.load(Ordering::Relaxed) == state)
        };
        let mut p = self.screen.pop_front();
        if p.is_none() && !screens_only {
            p = self
                .requested
                .pop_front()
                .or_else(|| unclaimed(&mut self.replay, REPLAY))
                .or_else(|| unclaimed(&mut self.early, EARLY));
            if p.is_none() && self.slow_now < slow_limit {
                p = unclaimed(&mut self.slow, EARLY);
                if let Some(p) = &p {
                    p.was_slow.store(true, Ordering::Relaxed);
                    self.slow_now += 1;
                }
            }
        }
        let p = p?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }

    /// For a thread of [`slow_extras`]: the first slow pipeline still
    /// unclaimed, only while nothing is requested and no pass over the
    /// screen waits. It does not count in `slow_now` and is not marked
    /// `was_slow`: the workers' own limit and places stay as they are.
    fn take_slow(&mut self) -> Option<Arc<Pending>> {
        if !self.requested.is_empty() || !self.screen.is_empty() {
            return None;
        }
        // (One promoted since it was queued is skipped, as in `pop`.)
        let p = std::iter::from_fn(|| self.slow.pop_front()).find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static AVAILABLE: Condvar = Condvar::new();

/// Start or promote work for a draw without waiting behind the entire early
/// list. Queue changes and worker claims happen under the same lock.
fn start(p: &Arc<Pending>, priority: u8) {
    if p.queue_state.load(Ordering::Relaxed) >= priority {
        return;
    }
    if !workers() {
        if p.queue_state.swap(MAKING, Ordering::Relaxed) == NOT_QUEUED {
            p.make();
        }
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push(p, priority) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        // (One woken could be a worker that takes passes over the screen
        // alone: it would leave the pipeline to nobody.)
        AVAILABLE.notify_all();
    }
}

/// Workers that make passes over the screen alone, besides the others.
const SCREEN_WORKERS: usize = 2;

fn next(screens_only: bool) -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop(screens_only, slow_limit()) {
            // Nothing asked for is left: the extra threads may take slow
            // ones now ([`slow_extras`]).
            if EXTRAS.load(Ordering::Relaxed) > 0
                && queue.requested.is_empty()
                && queue.screen.is_empty()
                && !queue.slow.is_empty()
            {
                AVAILABLE.notify_all();
            }
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// For a thread of [`slow_extras`]: the next slow pipeline, waited for.
fn next_slow() -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.take_slow() {
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// The threads of [`slow_extras`] that were started.
static EXTRAS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// More threads that make early pipelines known to take long, only while
/// nothing is requested (`VICTORY_PIPELINE_SLOW_WORKERS=<n>`, 0 by
/// default): at most `processors - workers - 4`, none where the PC's memory
/// is short or [`slow_limit`] puts nothing on the slow list. They take
/// nothing a draw asks for and do not count in the workers' slow limit, so
/// the workers keep their places for requested work.
///
/// It helps when the slow list is known but its pipelines are not cheap in
/// this run yet: after the shaders were translated again, a driver change,
/// or a run that ended before they were made. Runs that showed the queue
/// at the world's first frame (2026-11-26): -221426, -222450, -223904 and
/// -224808, 485 and 526 queued in the last two, where four workers took
/// slow ones while twelve stood idle through menu and loading; runs with
/// settled caches showed none (-220729, -225818, -230328, -231606), and
/// there the extra threads stay idle. Expected from the measured 3.5-4 a
/// second with sixteen threads: the queue at the world's first frame falls
/// to between 0 and about 150; not verified in a run. The risk to measure
/// in a pair of runs on an unsettled cache: in the world up to `n` slow
/// compiles run beside the workers at the same priority; if requested
/// pipelines come later with the switch ("dropped N without a host
/// pipeline", the time to "host pipelines all made"), the extras need a
/// further gate before this is recommended.
fn slow_extras(workers: usize, processors: usize) -> usize {
    if crate::residency::limited_host_ram() || slow_limit() == usize::MAX {
        return 0;
    }
    let asked = std::env::var("VICTORY_PIPELINE_SLOW_WORKERS").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
    extras_for(asked, workers, processors)
}

/// `asked` extra threads, capped at the processors left after `workers`
/// and four more.
fn extras_for(asked: usize, workers: usize, processors: usize) -> usize {
    asked.min(processors.saturating_sub(workers + 4))
}

/// How many workers make pipelines: `asked` (`VICTORY_PIPELINE_WORKERS`, 1 to
/// 64), else two thirds of the logical processors, 2 to 16. They run below the
/// title's own threads, and a cold cache queues hundreds at once.
fn worker_count(asked: Option<usize>, processors: usize) -> usize {
    worker_count_for(asked, processors, false)
}

fn worker_count_for(asked: Option<usize>, processors: usize, limited_ram: bool) -> usize {
    match asked {
        Some(n) => n.clamp(1, 64),
        None if limited_ram => (processors * 3 / 4).clamp(2, 8),
        None => (processors * 2 / 3).clamp(2, 16),
    }
}

/// Starts the workers ([`worker_count`]) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        let workers = worker_count_for(
            std::env::var("VICTORY_PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            std::thread::available_parallelism().map_or(2, |n| n.get()),
            crate::residency::limited_host_ram(),
        );
        tracing::info!(workers, "pipeline workers");
        let screens = if screens_first() { SCREEN_WORKERS } else { 0 };
        for i in 0..workers + screens {
            running |= spawn_worker(format!("victory-pipelines-{i}"), i >= workers);
        }
        // Read once, here ([`slow_extras`]); none with it unset. the screen
        // workers count among the threads already making pipelines.
        let extras = if running { slow_extras(workers + screens, std::thread::available_parallelism().map_or(2, |n| n.get())) } else { 0 };
        for i in 0..extras {
            let spawned = std::thread::Builder::new().name(format!("victory-pipelines-slow-{i}")).spawn(|| {
                super::overflow_room::keep();
                let lowest = std::env::var_os("VICTORY_PIPELINE_LOWEST").is_none_or(|v| v != "0");
                // SAFETY: a plain call on our own thread.
                unsafe {
                    use windows::Win32::System::Threading::*;
                    let priority = if lowest { THREAD_PRIORITY_LOWEST } else { THREAD_PRIORITY_BELOW_NORMAL };
                    let _ = SetThreadPriority(GetCurrentThread(), priority);
                }
                loop {
                    let p = next_slow();
                    p.make();
                    // As the workers do: "(N being made)" comes back to zero
                    // and the cache is saved.
                    if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                        drained();
                    } else {
                        super::library::save(false);
                    }
                }
            });
            match spawned {
                Ok(_) => {
                    EXTRAS.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => tracing::warn!(error = %e, "no extra pipeline thread for slow ones"),
            }
        }
        if extras > 0 {
            tracing::info!(
                extras = EXTRAS.load(Ordering::Relaxed),
                "threads that make slow pipelines while nothing is requested (VICTORY_PIPELINE_SLOW_WORKERS)"
            );
        }
        running
    })
}

/// Starts a worker named `name`; whether it started. `screens_only`: it
/// makes passes over the screen alone.
pub(super) fn spawn_worker(name: String, screens_only: bool) -> bool {
    let mut builder = std::thread::Builder::new().name(name);
    if let Some(bytes) = worker_stack() {
        builder = builder.stack_size(bytes);
    }
    let spawned = builder.spawn(move || {
        super::overflow_room::keep();
        // Below the title's own threads (loading, recording),
        // and the streaming one, which is below normal itself
        // (`VICTORY_PIPELINE_LOWEST=0`: level with it, as before).
        let lowest = !screens_only && std::env::var_os("VICTORY_PIPELINE_LOWEST").is_none_or(|v| v != "0");
        // SAFETY: a plain call on our own thread.
        unsafe {
            use windows::Win32::System::Threading::*;
            let priority = if lowest { THREAD_PRIORITY_LOWEST } else { THREAD_PRIORITY_BELOW_NORMAL };
            let _ = SetThreadPriority(GetCurrentThread(), priority);
        }
        let slot = super::spare::Slot::new(screens_only);
        loop {
            let p = next(screens_only);
            let slow = p.was_slow.load(Ordering::Relaxed);
            slot.begin(&p, slow);
            p.make();
            // A spare that took this worker's place gave the slow count back.
            let replaced = slot.end();
            if slow && !replaced {
                slow_released();
            }
            if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                drained();
            } else {
                super::library::save(false);
            }
            if replaced {
                return;
            }
        }
    });
    if let Err(e) = &spawned {
        tracing::warn!(error = %e, "no pipeline worker");
    }
    spawned.is_ok()
}

/// `VICTORY_PIPELINE_STACK_MB=<n>`: the workers' stack, reserved only
/// (unset: Rust's default, which `RUST_MIN_STACK` sets). vkd3d-proton
/// converts DXIL on the thread that makes the pipeline, recursively over its
/// control flow; on this PC seven workers die of a stack overflow while they
/// make one compute pipeline each, with 64 MiB stacks.
fn worker_stack() -> Option<usize> {
    static BYTES: OnceLock<Option<usize>> = OnceLock::new();
    *BYTES.get_or_init(|| {
        let mb: usize = std::env::var("VICTORY_PIPELINE_STACK_MB").ok()?.trim().parse().ok()?;
        let bytes = mb.checked_mul(1 << 20).filter(|&b| b > 0)?;
        tracing::info!(mb, "pipeline worker stack (VICTORY_PIPELINE_STACK_MB)");
        Some(bytes)
    })
}

/// A pipeline from the slow list is no longer being made ([`slow_limit`]).
pub(super) fn slow_released() {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    queue.slow_now -= 1;
    AVAILABLE.notify_all();
}

/// The most pipelines waiting at once since the workers last ran out.
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The workers ran out of work: logged after a batch of 10 or more, and
/// the cache saved (at most every 20 s).
fn drained() {
    use super::library::{EARLY, LOADED, MADE};
    if PEAK.swap(0, Ordering::Relaxed) >= 10 {
        tracing::info!(
            made = MADE.load(Ordering::Relaxed),
            loaded = LOADED.load(Ordering::Relaxed),
            started_early = EARLY.load(Ordering::Relaxed),
            "host pipelines all made"
        );
    }
    super::library::save(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_follows_the_processors_and_the_switch() {
        assert_eq!(worker_count(None, 1), 2);
        assert_eq!(worker_count(None, 24), 16);
        assert_eq!(worker_count(None, 64), 16);
        assert_eq!(worker_count(Some(0), 24), 1);
        assert_eq!(worker_count(Some(20), 24), 20);
        assert_eq!(worker_count_for(None, 8, true), 6);
        assert_eq!(worker_count_for(None, 16, true), 8);
        assert_eq!(worker_count_for(None, 24, true), 8);
        assert_eq!(worker_count_for(Some(12), 8, true), 12);
    }

    /// A first use queues the pipeline and, within the budget, gets the
    /// worker's answer (here: no host pipeline for no shaders).
    #[test]
    fn first_use_is_made_by_a_worker() {
        let p = Arc::new(Pending::new(Vec::new(), None));
        assert!(ready(&p).is_none());
        assert_ne!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        let started = Instant::now();
        while p.made.get().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)));
    }

    /// A panic inside a build must not take the worker thread with it, nor
    /// leave the queue entry open for ever.
    #[test]
    fn a_panicking_pipeline_is_refused_and_the_worker_continues() {
        let bad = Arc::new(Pending::new(Vec::new(), None));
        let good = Arc::new(Pending::new(Vec::new(), None));
        let failed = bad.clone();
        let next = good.clone();
        let worker = std::thread::spawn(move || {
            failed.make_with(|| panic!("synthetic pipeline build panic"));
            next.make_with(|| None);
        });

        worker.join().expect("the pipeline worker should survive");
        assert!(matches!(bad.made.get(), Some(None)));
        assert!(matches!(good.made.get(), Some(None)));

        bad.make_with(|| panic!("the refused pipeline should stay complete"));
        assert!(matches!(bad.made.get(), Some(None)));
    }

    /// A pipeline left out on purpose (a ray-tracing resolve without the structure builder)
    /// is never queued, so never made: it is counted as left out, and named so, not as
    /// "being made" (some 630 indirect dispatches a frame were, in every gameplay frame).
    #[test]
    fn pipelines_left_out_are_counted_and_never_queued() {
        let mut p = Pending::new(Vec::new(), None);
        p.left_out = Some("shades ray-tracing hits without BVH, left out");
        let p = Arc::new(p);
        let before = LEFT_OUT.load(Ordering::Relaxed);
        assert!(ready(&p).is_none());
        assert!(ready(&p).is_none());
        assert!(!now(&p));
        assert_eq!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        assert!(p.made.get().is_none());
        assert!(LEFT_OUT.load(Ordering::Relaxed) >= before + 2);
        #[cfg(feature = "diagnostics")]
        assert_eq!(
            missing(&p).as_deref(),
            Some("left out on purpose: shades ray-tracing hits without BVH, left out")
        );
    }

    #[test]
    fn pending_pipeline_is_recorded_for_the_next_launch() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "pending-pipeline-test".to_owned();
        // Simulate a worker that has taken the job but cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let p = Arc::new(p);

        assert!(!super::super::library::wanted_early(&p.key));
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none());
        assert!(super::super::library::wanted_early(&p.key));
    }

    #[test]
    fn draws_promote_early_pipelines_without_claiming_them_twice() {
        let earlier = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&earlier, EARLY), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert_eq!(queue.push(&promoted, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &promoted));
        assert_eq!(queue.push(&promoted, REQUESTED), None);
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &earlier));
        assert!(queue.pop(false, 1).is_none());
    }

    #[test]
    fn pipelines_known_to_take_long_leave_workers_for_the_others() {
        let new = || Arc::new(Pending::new(Vec::new(), None));
        let (long, longer, asked, quick) = (new(), new(), new(), new());
        let mut queue = Queue::new();
        for p in [&long, &longer, &asked] {
            assert_eq!(queue.push_as(p, EARLY, true), Some(true));
        }
        assert_eq!(queue.push_as(&quick, EARLY, false), Some(true));
        // The quick one first, then one long one at a time; one a draw asks
        // for waits for nobody.
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &quick));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &long));
        assert!(queue.pop(false, 1).is_none());
        assert_eq!(queue.push_as(&asked, REQUESTED, true), Some(false));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &asked));
        assert!(!asked.was_slow.load(Ordering::Relaxed));
        assert!(queue.pop(false, 1).is_none());
        queue.slow_now -= 1;
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &longer));
        assert!(queue.pop(false, 1).is_none());
    }

    /// An extra thread takes a slow pipeline only while nothing is requested
    /// and no pass over the screen waits, skips one promoted since, and
    /// leaves the workers' slow count alone; the extras are capped at the
    /// processors left.
    #[test]
    fn extra_threads_take_slow_ones_only_when_nothing_is_asked_for() {
        let new = || Arc::new(Pending::new(Vec::new(), None));
        let (long, promoted, longer, asked) = (new(), new(), new(), new());
        let mut queue = Queue::new();
        for p in [&long, &promoted, &longer] {
            assert_eq!(queue.push_as(p, EARLY, true), Some(true));
        }
        assert_eq!(queue.push_as(&asked, REQUESTED, false), Some(true));
        assert!(queue.take_slow().is_none());
        assert!(Arc::ptr_eq(&queue.pop(false, 0).unwrap(), &asked));
        let screen = {
            let mut p = Pending::new(Vec::new(), None);
            p.screen = true;
            Arc::new(p)
        };
        if screens_first() {
            assert_eq!(queue.push(&screen, REQUESTED), Some(true));
            assert!(queue.take_slow().is_none());
            assert!(Arc::ptr_eq(&queue.pop(true, 0).unwrap(), &screen));
        }
        assert!(Arc::ptr_eq(&queue.take_slow().unwrap(), &long));
        assert_eq!(long.queue_state.load(Ordering::Relaxed), MAKING);
        assert!(!long.was_slow.load(Ordering::Relaxed));
        assert_eq!(queue.slow_now, 0);
        // A draw asked for this one meanwhile, and a worker has it.
        assert_eq!(queue.push_as(&promoted, REQUESTED, true), Some(false));
        assert!(Arc::ptr_eq(&queue.pop(false, 0).unwrap(), &promoted));
        assert!(Arc::ptr_eq(&queue.take_slow().unwrap(), &longer));
        assert!(queue.take_slow().is_none());
        assert_eq!(queue.slow_now, 0);
        assert_eq!(extras_for(12, 16, 32), 12);
        assert_eq!(extras_for(20, 16, 32), 12);
        assert_eq!(extras_for(4, 16, 8), 0);
        assert_eq!(extras_for(0, 2, 32), 0);
    }

    /// Passes over the screen go before all others, and workers of their own
    /// take nothing else.
    #[test]
    fn passes_over_the_screen_go_first() {
        let of = |screen| {
            let mut p = Pending::new(Vec::new(), None);
            p.screen = screen;
            Arc::new(p)
        };
        let (world, screen) = (of(false), of(true));
        let mut queue = Queue::new();
        assert_eq!(queue.push(&world, REQUESTED), Some(true));
        assert_eq!(queue.push(&screen, REQUESTED), Some(true));
        assert!(Arc::ptr_eq(&queue.pop(true, 1).unwrap(), &screen));
        assert!(queue.pop(true, 1).is_none());
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &world));
    }

    #[test]
    fn requested_work_precedes_ranked_replay_then_creation_order() {
        let pending = || Arc::new(Pending::new(Vec::new(), None));
        let (early, hot, cold, live) = (pending(), pending(), pending(), pending());
        let mut q = Queue::new();
        q.push(&early, EARLY);
        q.push(&live, EARLY);
        q.push(&hot, REPLAY);
        q.push(&cold, REPLAY);
        assert_eq!(q.push(&live, REPLAY), Some(false));
        assert_eq!(q.push(&live, REQUESTED), Some(false));
        for p in [&live, &hot, &cold, &early] {
            assert!(Arc::ptr_eq(&q.pop(false, 1).unwrap(), p));
        }
        assert!(q.pop(false, 1).is_none());
    }

    #[test]
    fn pending_graphics_can_bind_the_donor_then_the_real_pipeline() {
        use victory_shader::{HwStage, emit::HostShader};
        use windows::Win32::Graphics::Direct3D12::*;
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_D32_FLOAT;
        use windows::core::s;

        if crate::host::host().is_none() {
            return;
        }
        let dxil = crate::host::compile(
            "float4 main(uint i : SV_VertexID) : SV_Position { return float4(i,0,0,1); }",
            s!("main"),
            s!("vs_5_1"),
        )
        .unwrap();
        let shader = Arc::new(Shader {
            stage: "vertex",
            hash: "synthetic-standin".into(),
            container: vec![],
            ambient: None,
            program: None,
            stock: OnceLock::new(),
            translated: OnceLock::from(Some(super::super::Translated {
                threads: None,
                used: vec![],
                tables: crate::draw::TablePlan::new(&[], &[]),
                stores: false,
                stores_by_address: false,
                picks: Default::default(),
                host: HostShader {
                    stage: HwStage::Vs,
                    dxil,
                    user_data: vec![],
                    root_cbvs: vec![],
                    root_srvs: vec![],
                    root_uavs: vec![],
                    root_indexed_views: vec![],
                    samplers: vec![],
                    static_samplers: vec![],
                },
            })),
        });
        let make = |stencil_mask| {
            let mut g = GraphicsState::default();
            g.topology = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
            let format = [16u32, DXGI_FORMAT_D32_FLOAT.0 as u32];
            #[repr(C)]
            struct Depth(u32, D3D12_DEPTH_STENCIL_DESC);
            let keep = D3D12_DEPTH_STENCILOP_DESC {
                StencilFailOp: D3D12_STENCIL_OP_KEEP,
                StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
                StencilPassOp: D3D12_STENCIL_OP_KEEP,
                StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
            };
            let depth = Depth(
                11,
                D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: true.into(),
                    DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                    DepthFunc: D3D12_COMPARISON_FUNC_LESS,
                    StencilWriteMask: stencil_mask,
                    FrontFace: keep,
                    BackFace: keep,
                    ..Default::default()
                },
            );
            // SAFETY: a complete depth-format subobject.
            unsafe { g.read(16, format.as_ptr().cast()) };
            // SAFETY: a complete depth-stencil subobject.
            unsafe { g.read(11, (&raw const depth).cast()) };
            Arc::new(Pending::new(vec![shader.clone()], Some(Box::new(g))))
        };
        let donor = make(0xff);
        assert!(now(&donor));
        let p = make(1);
        assert!(!p.same_inputs(&donor));
        let key = p.compatibility().unwrap();
        assert_eq!(Some(key), donor.compatibility());
        donors().lock().unwrap().insert(key, Arc::downgrade(&donor));
        let fallback = p.standin().unwrap() as *const HostPipeline;
        assert!(std::ptr::eq(
            fallback,
            donor.made.get().unwrap().as_ref().unwrap()
        ));
        if standins() {
            p.queue_state.store(MAKING, Ordering::Relaxed);
            assert!(std::ptr::eq(fallback, ready(&p).unwrap()));
            assert!(p.made.get().is_none());
        }
        drop(donor);
        assert!(std::ptr::eq(fallback, p.standin().unwrap()));
        assert!(now(&p));
        assert!(!std::ptr::eq(fallback, p.standin().unwrap()));
        let refused = make(2);
        assert!(refused.made.set(None).is_ok());
        assert!(refused.standin().is_none());
        assert!(Pending::new(vec![shader], None).compatibility().is_none());
    }
}
