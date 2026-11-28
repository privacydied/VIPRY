//! `VICTORY_CRASH_NEST=1` (diagnostics, off by default): an exception that
//! comes while its thread is already reporting one is not reported again.
//! It gets one short line that names both exceptions, and nothing more.
//!
//! Why: seven pipeline workers die of a stack overflow in the crash report
//! itself, at its first line, with 64 MiB, 512 MiB and with 1 MiB kept for
//! the handler (`VICTORY_PIPELINE_OVERFLOW_ROOM_KB`). That much stack used
//! inside the handler means the report faults and the handler reports that
//! fault in turn, without an end, so the size of the stack does not matter
//! and not one line is printed. The line here names the exception that
//! started it (the first one, which the title or the driver may handle
//! itself) and the fault inside the report. It writes into a buffer on the
//! stack and allocates nothing, because the report may have faulted inside
//! the allocator.

use std::cell::Cell;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);

/// Nested lines printed per thread at most: the first ones name the loop.
const LINES: u8 = 4;

thread_local! {
    /// The exception this thread is reporting: code and address.
    static OUTER: Cell<Option<(u32, usize)>> = const { Cell::new(None) };
    static SAID: Cell<u8> = const { Cell::new(0) };
}

/// Reads the switch; call outside any handler (reading it allocates).
pub(crate) fn init() {
    let on = std::env::var("VICTORY_CRASH_NEST").is_ok_and(|v| v.trim() == "1");
    ON.store(on, Ordering::Relaxed);
}

/// Before a report of exception `code` at `at`: whether to report it. With
/// the switch off, always. With it on, not when this thread is already
/// reporting one; then the nested one gets its line.
pub(crate) fn enter(code: u32, at: usize) -> bool {
    enter_if(ON.load(Ordering::Relaxed), code, at)
}

fn enter_if(on: bool, code: u32, at: usize) -> bool {
    if !on {
        return true;
    }
    let Some(outer) = OUTER.try_with(Cell::get).ok() else {
        return true;
    };
    if let Some(outer) = outer {
        say(outer, (code, at));
        return false;
    }
    let _ = OUTER.try_with(|o| o.set(Some((code, at))));
    true
}

/// After the report that `enter` let through.
pub(crate) fn leave() {
    let _ = OUTER.try_with(|o| o.set(None));
}

fn say(outer: (u32, usize), inner: (u32, usize)) {
    let Ok(n) = SAID.try_with(|s| {
        let n = s.get();
        s.set(n.saturating_add(1));
        n
    }) else {
        return;
    };
    if n >= LINES {
        return;
    }
    let mut buf = [0u8; 200];
    let len = line(&mut buf, outer, inner);
    let _ = std::io::stderr().write_all(&buf[..len]);
}

/// The line, in `buf`; its length.
fn line(buf: &mut [u8], outer: (u32, usize), inner: (u32, usize)) -> usize {
    let mut w = std::io::Cursor::new(buf);
    let _ = writeln!(
        w,
        "VIctory: exception {:#010x} at {:#x} while reporting exception {:#010x} at {:#x}, not reported (VICTORY_CRASH_NEST)",
        inner.0, inner.1, outer.0, outer.1
    );
    w.position() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_lets_every_report_through() {
        assert!(enter_if(false, 0xc000_0005, 1));
        assert!(enter_if(false, 0xc000_0005, 2));
    }

    #[test]
    fn on_stops_the_nested_report_only() {
        let r = std::thread::spawn(|| {
            let first = enter_if(true, 0xc000_0005, 0x10);
            let nested = enter_if(true, 0xc000_00fd, 0x20);
            leave();
            let next = enter_if(true, 0xc000_0005, 0x30);
            leave();
            (first, nested, next)
        })
        .join()
        .unwrap();
        assert_eq!(r, (true, false, true));
    }

    #[test]
    fn the_line_names_both() {
        let mut buf = [0u8; 200];
        let n = line(&mut buf, (0xc000_0005, 0x1234), (0xc000_00fd, 0x5678));
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(
            s.contains("0xc00000fd at 0x5678 while reporting exception 0xc0000005 at 0x1234"),
            "{s}"
        );
    }
}
