//! `VICTORY_HLE_LAST=1` (diagnostics builds): the last calls our side
//! answered on each thread, for the crash report.
//!
//! When the switch is on, every import bound to one of our implementations
//! (`Provenance::Hle`) goes through a 32-byte stub:
//!
//! ```text
//! mov r10, <&'static Site>   ; 49 BA imm64
//! mov r11, <our function>    ; 49 BB imm64
//! mov rax, <hle_last_enter>  ; 48 B8 imm64
//! jmp rax                    ; FF E0
//! ```
//!
//! `hle_last_enter` saves the argument registers, notes the call (the four
//! register arguments and the caller's return address) in the thread's
//! ring, puts the address of `hle_last_leave` in place of the return address
//! and jumps on to our function with the registers as they came. Our
//! function's `ret` lands in `hle_last_leave`, which notes the result (`rax`)
//! and returns to the caller. The real return addresses are kept on a small
//! stack of the thread's, each with the stack slot it came from, so a call
//! left by an exception or a stack switch is dropped when an outer one
//! returns.
//!
//! The D3D12 methods we have not written (and the other `E_NOTIMPL`
//! answers of our device) are noted in the same ring through
//! [`note_failure`].
//!
//! The crash report prints the faulting thread's ring, oldest first.
//!
//! Limits: calls the title makes through addresses it asked
//! `GetProcAddress` for, and calls through COM tables, are not in the ring
//! (only the failures noted through [`note_failure`]). An exception that
//! unwinds through one of our functions while it runs (a title callback we
//! call that throws) meets `hle_last_leave`'s address with no unwind data;
//! that is why this is a diagnostics switch, off by default.

use std::cell::UnsafeCell;
use std::sync::Mutex;
use std::sync::OnceLock;

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
};

/// Whether `VICTORY_HLE_LAST` is on (`=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HLE_LAST").is_some_and(|v| v != "0"))
}

/// One wrapped import. Leaked: the stub holds its address for the life of
/// the process.
struct Site {
    dll: String,
    symbol: String,
    /// Whether its return is caught for the result. Off for calls that may
    /// not come back the usual way.
    caught: bool,
}

/// Imports that may leave other than by `ret` to their caller.
const NOT_CAUGHT: &[&str] = &[
    "SwitchToFiber",
    "ExitThread",
    "FreeLibraryAndExitThread",
    "ExitProcess",
    "RaiseException",
    "RtlRaiseException",
    "RtlUnwind",
    "RtlUnwindEx",
    "RtlRestoreContext",
    "NtContinue",
    "longjmp",
    "_CxxThrowException",
];

/// What one ring entry is.
#[derive(Clone, Copy)]
enum What {
    Nothing,
    Call(&'static Site),
    /// A failure our D3D12 or DXGI answered: the object kind, the method's
    /// table slot (bytes) and the `HRESULT`.
    Failure(&'static str, usize, i32),
}

#[derive(Clone, Copy)]
struct Entry {
    what: What,
    /// The ring's count when it was noted.
    seq: u64,
    args: [u64; 4],
    /// The caller's return address.
    from: u64,
    result: Option<u64>,
}

const EMPTY: Entry = Entry {
    what: What::Nothing,
    seq: 0,
    args: [0; 4],
    from: 0,
    result: None,
};

/// How many calls each thread keeps.
const RING: usize = 32;
/// How deep calls of ours may nest on one thread and still have their
/// result caught.
const DEPTH: usize = 64;

struct State {
    ring: [Entry; RING],
    count: u64,
    /// (stack slot of the return address, the return address, ring seq).
    stack: [(u64, u64, u64); DEPTH],
    depth: usize,
}

thread_local! {
    static STATE: UnsafeCell<State> = const {
        UnsafeCell::new(State {
            ring: [EMPTY; RING],
            count: 0,
            stack: [(0, 0, 0); DEPTH],
            depth: 0,
        })
    };
}

impl State {
    fn push(&mut self, mut entry: Entry) -> u64 {
        self.count += 1;
        entry.seq = self.count;
        self.ring[(self.count % RING as u64) as usize] = entry;
        self.count
    }
}

/// Runs `f` on the thread's state. Not re-entered: nothing in here calls
/// out to anything that is wrapped.
fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    // SAFETY: the state is the current thread's own, and no function here
    // gives a reference to it out or calls back in.
    STATE.with(|s| f(unsafe { &mut *s.get() }))
}

/// A failure our D3D12 or DXGI answered, for the ring (any build; it does
/// nothing while the switch is off).
pub fn note_failure(object: &'static str, slot: usize, hr: i32) {
    if on() {
        note_failure_always(object, slot, hr);
    }
}

fn note_failure_always(object: &'static str, slot: usize, hr: i32) {
    with(|s| {
        s.push(Entry {
            what: What::Failure(object, slot, hr),
            ..EMPTY
        })
    });
}

/// From the stub: the saved registers are at `frame` (r11, r9, r8, rdx,
/// rcx), then the caller's return address.
extern "C" fn enter(site: &'static Site, frame: *mut u64) {
    // SAFETY: `frame` points at the words `hle_last_enter` pushed, followed
    // by the return address the caller's `call` pushed.
    let (args, slot) = unsafe {
        (
            [*frame.add(4), *frame.add(3), *frame.add(2), *frame.add(1)],
            frame.add(5),
        )
    };
    // SAFETY: as above.
    let from = unsafe { *slot };
    with(|s| {
        let seq = s.push(Entry {
            what: What::Call(site),
            seq: 0,
            args,
            from,
            result: None,
        });
        if site.caught && s.depth < DEPTH {
            s.stack[s.depth] = (slot as u64, from, seq);
            s.depth += 1;
            // SAFETY: the caller's return address slot, written back by
            // `leave` before anything returns through it.
            unsafe { *slot = hle_last_leave as *const () as u64 };
        }
    });
}

/// From `hle_last_leave`: our function returned `result`, with the stack
/// pointer at `rsp` (one word above the slot its return address was in).
/// Gives the caller's return address.
extern "C" fn leave(result: u64, rsp: u64) -> u64 {
    let slot = rsp - 8;
    let found = with(|s| {
        // Calls left without returning (an exception, a stack switch) lie
        // deeper on the stack than the one returning now.
        while s.depth > 0 && s.stack[s.depth - 1].0 < slot {
            s.depth -= 1;
        }
        if s.depth == 0 || s.stack[s.depth - 1].0 != slot {
            return None;
        }
        s.depth -= 1;
        let (_, from, seq) = s.stack[s.depth];
        let entry = &mut s.ring[(seq % RING as u64) as usize];
        if entry.seq == seq {
            entry.result = Some(result);
        }
        Some(from)
    });
    match found {
        Some(from) => from,
        None => {
            crate::log_crash("VIctory: VICTORY_HLE_LAST lost a return address; the process ends");
            std::process::abort();
        }
    }
}

unsafe extern "C" {
    fn hle_last_enter();
    fn hle_last_leave();
}

// Entered by `jmp` from a stub, with the stack as the caller's `call` left
// it (rsp = 8 mod 16) and the arguments in their registers.
std::arch::global_asm!(
    ".globl hle_last_enter",
    "hle_last_enter:",
    "push rcx",
    "push rdx",
    "push r8",
    "push r9",
    "push r11",
    "sub rsp, 0x60",
    "movups [rsp + 0x20], xmm0",
    "movups [rsp + 0x30], xmm1",
    "movups [rsp + 0x40], xmm2",
    "movups [rsp + 0x50], xmm3",
    "mov rcx, r10",
    "lea rdx, [rsp + 0x60]",
    "call {enter}",
    "movups xmm0, [rsp + 0x20]",
    "movups xmm1, [rsp + 0x30]",
    "movups xmm2, [rsp + 0x40]",
    "movups xmm3, [rsp + 0x50]",
    "add rsp, 0x60",
    "pop r11",
    "pop r9",
    "pop r8",
    "pop rdx",
    "pop rcx",
    "jmp r11",
    // Reached by our function's `ret` (rsp = 0 mod 16), result in rax
    // (and xmm0 for floating point).
    ".globl hle_last_leave",
    "hle_last_leave:",
    "push rax",
    "push rdx",
    "sub rsp, 0x30",
    "movups [rsp + 0x20], xmm0",
    "mov rcx, rax",
    "lea rdx, [rsp + 0x40]",
    "call {leave}",
    "mov r11, rax",
    "movups xmm0, [rsp + 0x20]",
    "add rsp, 0x30",
    "pop rdx",
    "pop rax",
    "jmp r11",
    enter = sym enter,
    leave = sym leave,
);

const STUB: usize = 32;
const CHUNK: usize = 64 << 10;

/// Executable memory the stubs are written in: (next free byte, end).
static STUBS: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// The address to bind `dll!symbol` to instead of `address`, our
/// implementation: a stub that notes each call. `address` itself if no
/// stub could be made.
pub(crate) fn wrap(dll: &str, symbol: &str, address: usize) -> usize {
    let site: &'static Site = Box::leak(Box::new(Site {
        dll: dll.to_owned(),
        symbol: symbol.to_owned(),
        caught: !NOT_CAUGHT.contains(&symbol),
    }));
    let mut stubs = STUBS.lock().unwrap_or_else(|e| e.into_inner());
    if stubs.0 + STUB > stubs.1 {
        // SAFETY: a fresh anonymous allocation.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                CHUNK,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        } as usize;
        if base == 0 {
            tracing::warn!(
                dll,
                symbol,
                "VICTORY_HLE_LAST: no memory for a stub; the import is not noted"
            );
            return address;
        }
        *stubs = (base, base + CHUNK);
    }
    let mut code = [0xCC_u8; STUB];
    code[0..2].copy_from_slice(&[0x49, 0xBA]);
    code[2..10].copy_from_slice(&(site as *const Site as u64).to_le_bytes());
    code[10..12].copy_from_slice(&[0x49, 0xBB]);
    code[12..20].copy_from_slice(&(address as u64).to_le_bytes());
    code[20..22].copy_from_slice(&[0x48, 0xB8]);
    code[22..30].copy_from_slice(&(hle_last_enter as *const () as u64).to_le_bytes());
    code[30..32].copy_from_slice(&[0xFF, 0xE0]);
    let at = stubs.0;
    // SAFETY: `at..at + STUB` is inside the chunk, which is writable.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, STUB) };
    stubs.0 += STUB;
    at
}

/// The current thread's ring for the crash report, oldest first; nothing
/// while the switch is off.
pub(crate) fn lines() -> Vec<String> {
    if !on() {
        return Vec::new();
    }
    ring_lines()
}

fn ring_lines() -> Vec<String> {
    let (ring, count) = with(|s| (s.ring, s.count));
    let mut out = vec![format!(
        "  our last calls on this thread (VICTORY_HLE_LAST), {} of {count}, oldest first:",
        count.min(RING as u64)
    )];
    let first = count.saturating_sub(RING as u64 - 1).max(1);
    for seq in first..=count {
        let e = ring[(seq % RING as u64) as usize];
        if e.seq != seq {
            continue;
        }
        let back = count - seq;
        out.push(match e.what {
            What::Nothing => continue,
            What::Call(site) => format!(
                "    -{back:<2} {}!{}({:#x}, {:#x}, {:#x}, {:#x}) -> {} from {}",
                site.dll,
                site.symbol,
                e.args[0],
                e.args[1],
                e.args[2],
                e.args[3],
                match (e.result, site.caught) {
                    (Some(r), _) => format!("{r:#x}"),
                    (None, true) => "(not returned)".to_owned(),
                    (None, false) => "(not caught)".to_owned(),
                },
                crate::locate(e.from as usize)
            ),
            What::Failure(object, slot, hr) => format!(
                "    -{back:<2} our D3D12: {object}, slot {slot:#x}, answered {:#010x}",
                hr as u32
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six integer and two floating arguments: four in registers, the rest
    /// on the stack, where the stub must leave them.
    extern "system" fn probe(
        a: u64,
        b: f64,
        c: u64,
        d: f32,
        e: u64,
        f: f64,
        g: u64,
        h: u64,
    ) -> f64 {
        (a + c + e + g + h) as f64 + b + f64::from(d) + f
    }

    extern "system" fn sum(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
        a + b * 10 + c * 100 + d * 1000 + e * 10_000 + f * 100_000
    }

    /// Calls `inner` through its stub, from inside a wrapped call.
    extern "system" fn outer(inner: u64, x: u64) -> u64 {
        // SAFETY: `inner` is the stub of `sum`.
        let inner: extern "system" fn(u64, u64, u64, u64, u64, u64) -> u64 =
            unsafe { std::mem::transmute(inner) };
        inner(x, 2, 3, 4, 5, 6) + 1
    }

    #[test]
    fn calls_through_a_stub_keep_arguments_and_results_and_are_noted() {
        type Probe = extern "system" fn(u64, f64, u64, f32, u64, f64, u64, u64) -> f64;
        type Sum = extern "system" fn(u64, u64, u64, u64, u64, u64) -> u64;
        type Outer = extern "system" fn(u64, u64) -> u64;
        // SAFETY: each stub jumps on to the function it was made for.
        let (p, s, o): (Probe, Sum, Outer) = unsafe {
            (
                std::mem::transmute(wrap("t.dll", "probe", probe as *const () as usize)),
                std::mem::transmute(wrap("t.dll", "sum", sum as *const () as usize)),
                std::mem::transmute(wrap("t.dll", "outer", outer as *const () as usize)),
            )
        };
        assert_eq!(p(1, 0.5, 2, 0.25, 3, 0.125, 4, 5), 15.875);
        assert_eq!(s(1, 2, 3, 4, 5, 6), 654_321);
        assert_eq!(o(s as *const () as u64, 7), 654_328);
        note_failure_always("pipeline state", 0x58, 0x8000_4001_u32 as i32);
        let text = ring_lines().join("\n");
        assert!(
            text.contains("t.dll!sum(0x1, 0x2, 0x3, 0x4) -> 0x9fbf1"),
            "{text}"
        );
        assert!(
            text.contains("t.dll!sum(0x7, 0x2, 0x3, 0x4) -> 0x9fbf7"),
            "{text}"
        );
        assert!(text.contains("t.dll!outer("), "{text}");
        assert!(text.contains("-> 0x9fbf8"), "{text}");
        assert!(
            text.contains("pipeline state, slot 0x58, answered 0x80004001"),
            "{text}"
        );
        assert_eq!(
            with(|s| s.depth),
            0,
            "every return was caught and given back"
        );
    }
}
