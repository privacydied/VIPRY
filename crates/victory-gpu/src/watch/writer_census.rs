//! Census of title instructions that fault on guarded GPU-read memory
//! (`VICTORY_WRITER_CENSUS=1`, off by default).
//!
//! Records each faulting instruction's offset in the title image (`RIP` minus
//! module base), the faulted 64 KiB block, and the host copy kind (`texture`,
//! `buffer`, or `other`). Every 30 seconds, prints the top 30 offsets and the
//! share of refreshes and nothing-new checks that land on blocks those offsets
//! wrote.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const BLOCK: usize = 64 << 10;
const REPORT_INTERVAL: Duration = Duration::from_secs(30);
const TOP_N: usize = 30;
const TARGET: &str = "victory_gpu::host::writer_census";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyKind {
    Other = 0,
    Buffer = 1,
    Texture = 2,
}

impl CopyKind {
    fn label(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Buffer => "buffer",
            Self::Other => "other",
        }
    }
}

#[derive(Default)]
struct OffsetStats {
    faults: u64,
    blocks: HashSet<usize>,
    kind_hits: [u64; 3],
}

impl OffsetStats {
    fn dominant_kind(&self) -> CopyKind {
        if self.kind_hits[CopyKind::Texture as usize] >= self.kind_hits[CopyKind::Buffer as usize]
            && self.kind_hits[CopyKind::Texture as usize] > 0
        {
            CopyKind::Texture
        } else if self.kind_hits[CopyKind::Buffer as usize] > 0 {
            CopyKind::Buffer
        } else {
            CopyKind::Other
        }
    }
}

struct State {
    last_report: Instant,
    by_offset: HashMap<usize, OffsetStats>,
    /// 64 KiB block -> last observed host copy kind.
    block_kind: HashMap<usize, CopyKind>,
    /// 64 KiB blocks written by any recorded fault offset.
    faulted_blocks: HashSet<usize>,
    refreshes_total: u64,
    refreshes_on_faulted: u64,
    nothing_new_total: u64,
    nothing_new_on_faulted: u64,
}

impl State {
    fn new() -> Self {
        Self {
            last_report: Instant::now(),
            by_offset: HashMap::new(),
            block_kind: HashMap::new(),
            faulted_blocks: HashSet::new(),
            refreshes_total: 0,
            refreshes_on_faulted: 0,
            nothing_new_total: 0,
            nothing_new_on_faulted: 0,
        }
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static FAULT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether the writer census is enabled (`VICTORY_WRITER_CENSUS=1`).
#[inline]
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("VICTORY_WRITER_CENSUS").is_ok_and(|v| v.trim() == "1")
    })
}

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::new()))
}

fn rip_to_offset(rip: usize) -> usize {
    let (base, size) = crate::port_descriptors::image_span();
    if base != 0 && rip >= base && rip < base.saturating_add(size) {
        rip - base
    } else {
        rip
    }
}

/// Records a guard fault at `at` caused by instruction `rip`.
pub(crate) fn note_fault(at: usize, rip: usize) {
    if !on() {
        return;
    }
    let block = at / BLOCK;
    let offset = rip_to_offset(rip);
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    let kind = s.block_kind.get(&block).copied().unwrap_or(CopyKind::Other);
    s.faulted_blocks.insert(block);
    let entry = s.by_offset.entry(offset).or_default();
    entry.faults += 1;
    entry.blocks.insert(block);
    entry.kind_hits[kind as usize] += 1;
    let n = FAULT_COUNTER.fetch_add(1, Ordering::Relaxed);
    if n & 0xff == 0 && s.last_report.elapsed() >= REPORT_INTERVAL {
        report_locked(&mut s);
    }
}

/// Records a host copy refresh (`Shadow::update`) over `[from, to)`.
pub(crate) fn note_refresh(from: usize, to: usize, kind: CopyKind) {
    if !on() || from >= to {
        return;
    }
    let first = from / BLOCK;
    let last = (to - 1) / BLOCK;
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    s.refreshes_total += 1;
    let mut hit = false;
    for b in first..=last {
        s.block_kind.insert(b, kind);
        if s.faulted_blocks.contains(&b) {
            hit = true;
        }
    }
    if hit {
        s.refreshes_on_faulted += 1;
    }
    if s.last_report.elapsed() >= REPORT_INTERVAL {
        report_locked(&mut s);
    }
}

/// Records a `Shadow::nothing_new` check over `[from, to)`.
pub(crate) fn note_nothing_new(from: usize, to: usize, kind: CopyKind) {
    if !on() || from >= to {
        return;
    }
    let first = from / BLOCK;
    let last = (to - 1) / BLOCK;
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    s.nothing_new_total += 1;
    let mut hit = false;
    for b in first..=last {
        s.block_kind.entry(b).or_insert(kind);
        if s.faulted_blocks.contains(&b) {
            hit = true;
        }
    }
    if hit {
        s.nothing_new_on_faulted += 1;
    }
}

fn report_locked(s: &mut State) {
    s.last_report = Instant::now();
    let mut rows: Vec<(usize, u64, usize, CopyKind)> = s
        .by_offset
        .iter()
        .map(|(&off, st)| (off, st.faults, st.blocks.len(), st.dominant_kind()))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));

    let ref_pct = pct(s.refreshes_on_faulted, s.refreshes_total);
    let nn_pct = pct(s.nothing_new_on_faulted, s.nothing_new_total);
    let total_faults: u64 = rows.iter().map(|r| r.1).sum();

    tracing::info!(
        target: TARGET,
        "writer_census: offsets={} total_faults={} distinct_blocks={} ({:.1} MB) refreshes_on_faulted={}/{} ({:.1}%) nothing_new_on_faulted={}/{} ({:.1}%)",
        rows.len(),
        total_faults,
        s.faulted_blocks.len(),
        (s.faulted_blocks.len() * BLOCK) as f64 / (1024.0 * 1024.0),
        s.refreshes_on_faulted,
        s.refreshes_total,
        ref_pct,
        s.nothing_new_on_faulted,
        s.nothing_new_total,
        nn_pct,
    );
    for (rank, &(off, faults, blocks, kind)) in rows.iter().take(TOP_N).enumerate() {
        let mb = (blocks * BLOCK) as f64 / (1024.0 * 1024.0);
        tracing::info!(
            target: TARGET,
            "  #{:02} offset=+{:#x} faults={} pages={} mb={:.2} kind={}",
            rank + 1,
            off,
            faults,
            blocks,
            mb,
            kind.label(),
        );
    }
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64) * 100.0 / (whole as f64)
    }
}
