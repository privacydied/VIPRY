//! `VICTORY_LANE_CORE=1` (off by default): the thread that prepares and sends
//! submissions (`victory-submit`, `queue::behind`) gets one core, both of its
//! processors, to itself as far as this process goes. The other threads of the
//! process are kept off that core, so nothing of ours shares the core's units
//! with it.
//!
//! After Slaktarn700/SlaktarnVI `cores.rs` (`VICTORY_LANE_CORE`), with these
//! changes. That form asks with processor sets
//! (`SetThreadSelectedCpuSets`, `SetProcessDefaultCpuSets`), and Proton's
//! kernelbase only has stubs for both. It also needs "fast" cores, which a
//! processor whose cores are all alike does not have. Here affinity masks do
//! the work, and Wine applies them. A thread's mask must stay inside the
//! process's, so the process mask is left as it is. Instead a thread of ours
//! sets each thread of the process, at the start and every few seconds for
//! threads made since then. Threads that Wine does not know (made by native
//! libraries) are not seen. Nothing else of ours sets affinity, so turning it
//! off gives each thread the process's whole mask again.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::Duration;

/// How often threads made since the last pass are kept off the core too.
const EVERY: Duration = Duration::from_secs(5);

fn on() -> bool {
    crate::asked_for!("VICTORY_LANE_CORE")
}

/// The submitting thread's id, 0 before it starts.
static LANE: AtomicU32 = AtomicU32::new(0);

/// The core for the lane and all the other processors, as masks within the
/// process's `process` mask; `cores` holds each core's processors. The core is
/// the one with the highest processor (the last). `None` with fewer than six
/// cores: one of them would be too large a part of the rest.
fn split(cores: &[usize], process: usize) -> Option<(usize, usize)> {
    let cores: Vec<usize> = cores
        .iter()
        .map(|m| m & process)
        .filter(|m| *m != 0)
        .collect();
    if cores.len() < 6 {
        return None;
    }
    let lane = *cores
        .iter()
        .max_by_key(|m| usize::BITS - m.leading_zeros())?;
    let rest = process & !lane;
    (rest != 0).then_some((lane, rest))
}

/// Each core's processors, as the system tells them.
fn core_masks() -> Vec<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    let mut bytes = 0u32;
    // SAFETY: a size query, then the call into a buffer at least that large.
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut bytes) };
    let size = size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let mut records =
        vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); (bytes as usize).div_ceil(size)];
    let mut bytes = (records.len() * size) as u32;
    // SAFETY: as above.
    if unsafe { GetLogicalProcessorInformation(records.as_mut_ptr(), &mut bytes) } == 0 {
        return Vec::new();
    }
    records.truncate(bytes as usize / size);
    records
        .iter()
        .filter(|r| r.Relationship == RelationProcessorCore)
        .map(|r| r.ProcessorMask)
        .collect()
}

/// The process's own processors.
fn process_mask() -> usize {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
    let (mut process, mut system) = (0usize, 0usize);
    // SAFETY: our own process, into two locals.
    unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) };
    process
}

/// Sets each thread of the process that Wine knows: the lane thread on
/// `lane`, the others on `rest`. Gives how many threads were set.
fn sweep(lane_thread: u32, lane: usize, rest: usize) -> usize {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenThread, SetThreadAffinityMask, THREAD_QUERY_INFORMATION,
        THREAD_SET_INFORMATION,
    };
    // SAFETY: a snapshot of the system's threads, closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return 0;
    }
    // SAFETY: our own process id.
    let us = unsafe { GetCurrentProcessId() };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut set = 0;
    // SAFETY: the snapshot is open and `entry` says its own size.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == us {
            let mask = if entry.th32ThreadID == lane_thread {
                lane
            } else {
                rest
            };
            // SAFETY: a thread of our own process; the handle is closed at once.
            unsafe {
                let thread = OpenThread(
                    THREAD_SET_INFORMATION | THREAD_QUERY_INFORMATION,
                    0,
                    entry.th32ThreadID,
                );
                if !thread.is_null() {
                    set += usize::from(SetThreadAffinityMask(thread, mask) != 0);
                    CloseHandle(thread);
                }
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot opened above.
    unsafe { CloseHandle(snapshot) };
    set
}

/// The submitting thread starts: called on it. With the switch (or with
/// `VICTORY_AB` naming it) a thread of ours keeps the core for it.
pub(crate) fn take() {
    // SAFETY: this thread's own id.
    let me = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    if LANE.swap(me, Relaxed) != 0 {
        return;
    }
    #[cfg(feature = "diagnostics")]
    let asked = on() || crate::ab::bit("VICTORY_LANE_CORE").is_some();
    #[cfg(not(feature = "diagnostics"))]
    let asked = on();
    if !asked {
        return;
    }
    let process = process_mask();
    let Some((lane, rest)) = split(&core_masks(), process) else {
        tracing::info!(
            process = format_args!("{process:#x}"),
            "VICTORY_LANE_CORE: fewer than six cores, the submitting thread shares them"
        );
        return;
    };
    let keep = move || {
        let mut was = false;
        let mut last = std::time::Instant::now();
        loop {
            let now = on();
            if now != was || (now && last.elapsed() >= EVERY) {
                let (l, r) = if now {
                    (lane, rest)
                } else {
                    (process, process)
                };
                let threads = sweep(LANE.load(Relaxed), l, r);
                if now != was {
                    tracing::info!(
                        on = now,
                        lane = format_args!("{lane:#x}"),
                        rest = format_args!("{rest:#x}"),
                        threads,
                        "the submitting thread has a core of its own (VICTORY_LANE_CORE)"
                    );
                }
                (was, last) = (now, std::time::Instant::now());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("victory-lane-core".into())
        .spawn(keep)
    {
        tracing::warn!(%error, "VICTORY_LANE_CORE: the thread that keeps the core could not start");
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    /// Eight cores of two processors each (n and n + 8): the last core is
    /// processors 7 and 15, the rest is everything else.
    #[test]
    fn the_last_core_goes_to_the_lane() {
        let cores: Vec<usize> = (0..8).map(|n| (1 << n) | (1 << (n + 8))).collect();
        assert_eq!(split(&cores, 0xffff), Some((0x8080, 0x7f7f)));
        // Within a smaller process mask, only its processors count.
        assert_eq!(split(&cores, 0x7f7f), Some((0x4040, 0x3f3f)));
        // Fewer than six cores: nothing.
        assert_eq!(split(&cores[..5], 0xffff), None);
        assert_eq!(split(&[], 0xffff), None);
    }
}
