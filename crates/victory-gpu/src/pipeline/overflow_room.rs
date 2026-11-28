//! `VICTORY_PIPELINE_OVERFLOW_ROOM_KB=<n>` (diagnostics, off by default):
//! each pipeline worker keeps `n` KiB of its stack for the handler of a
//! stack overflow (`SetThreadStackGuarantee`).
//!
//! Why: on this PC seven workers die of a stack overflow while they make
//! one compute pipeline each, with 64 MiB stacks and with 512 MiB stacks,
//! so the recursion does not end. The crash handler should name the frames
//! it recurses in, but it runs on what is left of the stack after the
//! overflow: Rust keeps 20 KiB for its own handler, and the report (its
//! text and its walk of the stack) needs more. It dies itself at its first
//! line, and the log has the handler's address, not the frames of the
//! recursion. With more room the report ends and prints its 40 frames. Wine
//! honours the guarantee: at an overflow it hands those bytes to the handler
//! (`GuaranteedStackBytes`). The pipelines are not made any better by it; it
//! is for the one run that names the recursion.

use std::sync::OnceLock;

/// The bytes to keep, from the switch; `None`: off (unset, not a number, 0).
fn room(value: Option<&str>) -> Option<u32> {
    let kib: u32 = value?.trim().parse().ok()?;
    kib.checked_mul(1024).filter(|&b| b > 0)
}

fn bytes() -> Option<u32> {
    static BYTES: OnceLock<Option<u32>> = OnceLock::new();
    *BYTES.get_or_init(|| {
        let bytes = room(std::env::var("VICTORY_PIPELINE_OVERFLOW_ROOM_KB").ok().as_deref());
        if let Some(bytes) = bytes {
            tracing::info!(kib = bytes / 1024, "pipeline workers keep stack for an overflow's report (VICTORY_PIPELINE_OVERFLOW_ROOM_KB)");
        }
        bytes
    })
}

/// On a worker, at its start: keeps the room asked for, if any.
pub(super) fn keep() {
    let Some(mut size) = bytes() else {
        return;
    };
    // SAFETY: a plain call on our own thread with a valid pointer.
    let kept = unsafe { windows::Win32::System::Threading::SetThreadStackGuarantee(&mut size) };
    if let Err(e) = kept {
        tracing::warn!(error = %e, "a pipeline worker could not keep stack for an overflow's report");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_a_positive_number() {
        assert_eq!(room(None), None);
        assert_eq!(room(Some("0")), None);
        assert_eq!(room(Some("many")), None);
        assert_eq!(room(Some(" 1024 ")), Some(1 << 20));
        assert_eq!(room(Some("99999999")), None);
    }

    #[test]
    fn a_thread_keeps_the_room() {
        let mut size = 256 * 1024;
        // SAFETY: as in `keep`.
        let kept = std::thread::spawn(move || unsafe {
            windows::Win32::System::Threading::SetThreadStackGuarantee(&mut size).is_ok()
        });
        assert!(kept.join().unwrap());
    }
}
