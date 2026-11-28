//! `VICTORY_HANG_LOOK=1` (diagnostics builds, off): watches the title's own
//! hang check, to find what keeps its frames from starting.
//!
//! The title runs a thread that, once a second, compares a time its frame
//! start writes (`BEAT`) with the one it saw before. After 30 checks in a
//! row with the same time, and no long operation marked (`LONG_OPS`), it
//! calls an optional handler (`HANDLER`) and then writes to address 0 on
//! purpose. This look reads the same words every 250 ms. When the time has
//! stood still for [`DUMP_AT`] it prints every thread's stack, once per
//! stall and again just before the title's check would fire, so the
//! thread its frame start waits on shows. When a stall over [`NOTE_OVER`]
//! ends, it says how long it was and how many frames we began meanwhile.
//!
//! Only offsets into the title's image here; nothing changes but the log.

use std::time::{Duration, Instant};

/// The time the title's frame start last wrote (`u64`, performance counter).
const BEAT: usize = 0x71b_6338;
/// The title's count of long operations under way (`u32`).
const LONG_OPS: usize = 0x71b_6334;
/// The handler the title calls before it ends itself (`u64`, 0 when none).
const HANDLER: usize = 0x71b_6348;
/// Two bytes the title sets while its frames are paused on purpose.
const PAUSED: usize = 0x71b_6330;

/// How often the words are read.
const EVERY: Duration = Duration::from_millis(250);
/// A stall this long prints every thread's stack.
const DUMP_AT: Duration = Duration::from_secs(20);
/// And again this long: the title's check fires at 30 to 32 s.
const DUMP_AGAIN_AT: Duration = Duration::from_secs(27);
/// Stalls longer than this are said when they end.
const NOTE_OVER: Duration = Duration::from_secs(5);

/// Starts the look if `VICTORY_HANG_LOOK=1`. `base` and `size` are the
/// title's mapped image.
pub(crate) fn start(base: usize, size: usize) {
    if std::env::var("VICTORY_HANG_LOOK").ok().as_deref() != Some("1") {
        return;
    }
    if [BEAT, LONG_OPS, HANDLER, PAUSED]
        .iter()
        .any(|&rva| rva + 8 > size)
    {
        eprintln!("VIctory: hang look: the image is too small, not started");
        return;
    }
    let read = move |rva: usize| {
        // SAFETY: eight bytes inside the mapped image (checked above), title
        // data, read whole.
        unsafe { ((base + rva) as *const u64).read_volatile() }
    };
    std::thread::Builder::new()
        .name("victory-hang-look".into())
        .spawn(move || look(read))
        .ok();
}

fn look(read: impl Fn(usize) -> u64) {
    let start = Instant::now();
    let mut beat = read(BEAT);
    let mut since = Instant::now();
    let mut frames_then = victory_hle::frames_begun();
    let mut dumped = 0;
    let mut longest = Duration::ZERO;
    loop {
        std::thread::sleep(EVERY);
        let now = read(BEAT);
        let stood = since.elapsed();
        if now != beat {
            if stood > NOTE_OVER {
                longest = longest.max(stood);
                eprintln!(
                    "VIctory: hang look: the title's frame start came back after {:.1} s, at {:.0} s; we began {} frames meanwhile; longest so far {:.1} s",
                    stood.as_secs_f64(),
                    start.elapsed().as_secs_f64(),
                    victory_hle::frames_begun().saturating_sub(frames_then),
                    longest.as_secs_f64(),
                );
            }
            beat = now;
            since = Instant::now();
            frames_then = victory_hle::frames_begun();
            dumped = 0;
            continue;
        }
        let due = [DUMP_AT, DUMP_AGAIN_AT]
            .get(dumped)
            .is_some_and(|&at| stood >= at);
        if due {
            dumped += 1;
            let paused = read(PAUSED) & 0xffff;
            eprintln!(
                "VIctory: hang look: the title's frame start has not run for {:.1} s, at {:.0} s; long operations {}, paused {paused:#x}, handler {}, we began {} frames meanwhile",
                stood.as_secs_f64(),
                start.elapsed().as_secs_f64(),
                read(LONG_OPS) as u32,
                victory_loader::locate(read(HANDLER) as usize),
                victory_hle::frames_begun().saturating_sub(frames_then),
            );
            victory_loader::dump_stacks();
        }
    }
}
