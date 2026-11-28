//! The processors the title's threads may run on.
//!
//! The title pins its threads with `SetThreadGroupAffinity` to the logical
//! processors of the console: pools 0-7, 8-12 and 0-13, and single
//! processors such as 13. On the console the two logical processors of a
//! core are neighbours, and 14 of them are the title's. A PC numbers them
//! another way (Linux, and so Wine: 0-7 are eight cores, 8-15 their second
//! threads), and has its own count. So the title's pools landed on the same
//! cores (8-12 are the second threads of 0-4, where pool 0-7 runs), and of
//! 16 processors 14 and 15 had no title thread: in a look 2.2 of 16 were busy.
//!
//! `VICTORY_TITLE_CORES` (on by default; `0`: as the title asks) spreads the
//! title's 14 console processors over every processor of this PC instead, in
//! the console's order: fastest cores first, a core's threads side by side,
//! the cores that share a last-level cache together. Console processor `i`
//! becomes the PC's processors `i*n/14 .. (i+1)*n/14` of that order, so a
//! pool keeps its share of the machine on any count: two processors each on
//! 28 or more, a part of one on fewer. A processor at 14 or above (asked by
//! a title that counted this PC's processors itself) is kept as it is.
//! `GetThreadGroupAffinity` gives back what the title asked.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::SystemInformation::{
    CacheData, CacheUnified, GROUP_AFFINITY, GetLogicalProcessorInformationEx,
    LOGICAL_PROCESSOR_RELATIONSHIP, RelationCache, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows_sys::Win32::System::Threading::{
    GetThreadGroupAffinity as host_get, GetThreadId, SetThreadGroupAffinity as host_set,
};

use crate::{HleModule, hle_export};

/// The console processors a title has.
const CONSOLE: usize = 14;

/// A core of this PC, as the order is made from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Core {
    /// Its logical processors, in group 0.
    pub mask: usize,
    /// Higher is faster.
    pub efficiency: u8,
    /// The lowest processor of the last-level cache it shares.
    pub cache: u32,
}

/// The order of the PC's processors: fastest first, then by shared cache,
/// then by core, each core's threads side by side.
pub(crate) fn order(cores: &[Core]) -> Vec<u32> {
    let mut cores = cores.to_vec();
    cores.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.efficiency),
            c.cache,
            c.mask.trailing_zeros(),
        )
    });
    let mut out = Vec::new();
    for c in cores {
        out.extend((0..usize::BITS).filter(|b| c.mask >> b & 1 == 1));
    }
    out
}

/// The PC's mask for a mask of console processors.
pub(crate) fn spread(asked: usize, order: &[u32]) -> usize {
    let n = order.len();
    if n == 0 {
        return asked;
    }
    let mut out = 0usize;
    for i in (0..usize::BITS as usize).filter(|i| asked >> i & 1 == 1) {
        if i >= CONSOLE {
            out |= 1 << i;
            continue;
        }
        let start = i * n / CONSOLE;
        let end = ((i + 1) * n / CONSOLE).max(start + 1).min(n);
        for p in &order[start.min(n - 1)..end] {
            out |= 1 << p;
        }
    }
    let all = order.iter().fold(0usize, |m, p| m | 1 << p);
    // A kept processor the PC has not: the spread part alone.
    if out & all == 0 { all } else { out & all }
}

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TITLE_CORES").is_none_or(|v| v != "0"))
}

/// Entries of one relation kind from the host.
fn entries(kind: LOGICAL_PROCESSOR_RELATIONSHIP) -> Vec<u8> {
    let mut len = 0u32;
    // SAFETY: a size query, then a buffer of that size.
    unsafe {
        GetLogicalProcessorInformationEx(kind, std::ptr::null_mut(), &mut len);
        let mut buf = vec![0u8; len as usize];
        if len == 0
            || GetLogicalProcessorInformationEx(kind, buf.as_mut_ptr().cast(), &mut len) == 0
        {
            return Vec::new();
        }
        buf.truncate(len as usize);
        buf
    }
}

/// Walks the variable-size entries of a buffer.
fn each(buf: &[u8], mut f: impl FnMut(&SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)) {
    let mut at = 0usize;
    while at + 8 <= buf.len() {
        // SAFETY: the host wrote whole entries; `Size` steps to the next.
        let e = unsafe {
            &*buf
                .as_ptr()
                .add(at)
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()
        };
        if e.Size == 0 {
            break;
        }
        f(e);
        at += e.Size as usize;
    }
}

/// This PC's cores in group 0.
fn host_cores() -> Vec<Core> {
    let mut caches: Vec<(u8, usize)> = Vec::new();
    each(&entries(RelationCache), |e| {
        // SAFETY: a cache entry.
        let c = unsafe { &e.Anonymous.Cache };
        if (c.Type == CacheUnified || c.Type == CacheData) && c.Level >= 2 {
            // SAFETY: GroupCount 0 or 1 holds one mask.
            let g = unsafe { c.Anonymous.GroupMask };
            if g.Group == 0 {
                caches.push((c.Level, g.Mask));
            }
        }
    });
    let last = caches.iter().map(|c| c.0).max().unwrap_or(0);
    let mut cores = Vec::new();
    each(&entries(RelationProcessorCore), |e| {
        // SAFETY: a core entry.
        let p = unsafe { &e.Anonymous.Processor };
        let g = p.GroupMask[0];
        if g.Group != 0 || g.Mask == 0 {
            return;
        }
        let cache = caches
            .iter()
            .filter(|c| c.0 == last && c.1 & g.Mask != 0)
            .map(|c| c.1.trailing_zeros())
            .next()
            .unwrap_or(0);
        cores.push(Core {
            mask: g.Mask,
            efficiency: p.EfficiencyClass,
            cache,
        });
    });
    cores
}

fn host_order() -> &'static [u32] {
    static ORDER: OnceLock<Vec<u32>> = OnceLock::new();
    ORDER.get_or_init(|| {
        let order = order(&host_cores());
        tracing::info!(
            processors = order.len(),
            ?order,
            "title cores: console processors spread over this order"
        );
        order
    })
}

/// What each thread asked, for [`GetThreadGroupAffinity`].
fn asked() -> &'static Mutex<HashMap<u32, GROUP_AFFINITY>> {
    static ASKED: OnceLock<Mutex<HashMap<u32, GROUP_AFFINITY>>> = OnceLock::new();
    ASKED.get_or_init(Default::default)
}

/// See the module.
pub extern "system" fn SetThreadGroupAffinity(
    thread: *mut c_void,
    affinity: *const GROUP_AFFINITY,
    previous: *mut GROUP_AFFINITY,
) -> i32 {
    if !on() || affinity.is_null() {
        // SAFETY: the title's arguments, as given.
        return unsafe { host_set(thread, affinity, previous) };
    }
    // SAFETY: the title passes a valid affinity.
    let want = unsafe { *affinity };
    if want.Group != 0 {
        // SAFETY: as given.
        return unsafe { host_set(thread, affinity, previous) };
    }
    let mut ours = want;
    ours.Mask = spread(want.Mask, host_order());
    // SAFETY: a thread handle from the title.
    let id = unsafe { GetThreadId(thread) };
    let mut map = asked().lock().unwrap_or_else(|e| e.into_inner());
    let before = map.get(&id).copied();
    // SAFETY: our copy of the title's affinity.
    let ok = unsafe { host_set(thread, &ours, previous) };
    if ok != 0 {
        if let (Some(b), false) = (before, previous.is_null()) {
            // SAFETY: the title's out pointer.
            unsafe { *previous = b };
        }
        if before.is_none_or(|b| b.Mask != want.Mask) {
            tracing::debug!(
                thread = id,
                asked = format_args!("{:#x}", want.Mask),
                set = format_args!("{:#x}", ours.Mask),
                "title cores"
            );
        }
        map.insert(id, want);
    }
    ok
}

/// See the module.
pub extern "system" fn GetThreadGroupAffinity(
    thread: *mut c_void,
    affinity: *mut GROUP_AFFINITY,
) -> i32 {
    // SAFETY: the title's arguments, as given.
    let ok = unsafe { host_get(thread, affinity) };
    if ok != 0 && on() && !affinity.is_null() {
        // SAFETY: a thread handle from the title.
        let id = unsafe { GetThreadId(thread) };
        if let Some(a) = asked().lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            // SAFETY: the title's out pointer, just written by the host.
            unsafe { *affinity = *a };
        }
    }
    ok
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "title_cores",
        dlls: &[
            "kernel32.dll",
            "kernelbase.dll",
            "api-ms-win-core-processtopology-*",
        ],
        exports: vec![
            hle_export!(SetThreadGroupAffinity),
            hle_export!(GetThreadGroupAffinity),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop part numbered as Linux does: 0-7 the cores, 8-15 their
    /// second threads, two caches of four cores.
    fn smt16() -> Vec<Core> {
        (0..8)
            .map(|c| Core {
                mask: 1 << c | 1 << (c + 8),
                efficiency: 0,
                cache: if c < 4 { 0 } else { 4 },
            })
            .collect()
    }

    #[test]
    fn order_puts_a_cores_threads_side_by_side() {
        assert_eq!(
            order(&smt16()),
            vec![0, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15]
        );
    }

    #[test]
    fn the_titles_pools_no_longer_share_cores_and_use_every_processor() {
        let o = order(&smt16());
        let a = spread(0xff, &o); // 0-7
        let b = spread(0x1f00, &o); // 8-12
        let c = spread(1 << 13, &o);
        assert_eq!(a & b, 0, "pools 0-7 and 8-12 on separate processors");
        assert_eq!(a | b | c, 0xffff, "all 16 processors");
        assert_eq!(spread(0x3fff, &o), 0xffff);
        // A pool's two processors of a core are both its own.
        assert_eq!(a, 0x0f1f, "cores 0-3 whole and core 4's first thread");
    }

    #[test]
    fn more_processors_give_each_pool_more() {
        let cores: Vec<Core> = (0..16)
            .map(|c| Core {
                mask: 1 << c | 1 << (c + 16),
                efficiency: 0,
                cache: c / 8 * 8,
            })
            .collect();
        let o = order(&cores);
        assert_eq!(spread(0x3fff, &o).count_ones(), 32);
        assert!(spread(0xff, &o).count_ones() >= 16);
    }

    #[test]
    fn few_processors_still_give_every_thread_one() {
        let cores: Vec<Core> = (0..4)
            .map(|c| Core {
                mask: 1 << c | 1 << (c + 4),
                efficiency: 0,
                cache: 0,
            })
            .collect();
        let o = order(&cores);
        for i in 0..CONSOLE {
            assert_eq!(spread(1 << i, &o).count_ones(), 1);
        }
        assert_eq!(spread(0x3fff, &o), 0xff);
    }

    #[test]
    fn fast_cores_come_first() {
        let cores = vec![
            Core {
                mask: 0b0011,
                efficiency: 0,
                cache: 0,
            },
            Core {
                mask: 0b1100,
                efficiency: 1,
                cache: 0,
            },
        ];
        assert_eq!(order(&cores), vec![2, 3, 0, 1]);
    }

    /// The host's own cores: every processor once.
    #[test]
    fn this_pc_gives_every_processor_once() {
        let cores = host_cores();
        let o = order(&cores);
        println!("cores {cores:?}\norder {o:?}");
        let mut seen = o.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), o.len());
        assert_eq!(
            o.len(),
            std::thread::available_parallelism()
                .map_or(0, |n| n.get())
                .min(64)
        );
    }

    #[test]
    fn processors_past_the_console_are_kept() {
        let o = order(&smt16());
        assert_eq!(spread(1 << 15, &o), 1 << 15);
        assert_eq!(spread(1 << 40, &o), 0xffff);
    }
}
