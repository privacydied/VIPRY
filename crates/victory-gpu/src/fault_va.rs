//! `VICTORY_FAULT_VA=<address>[,<address>...]` (diagnostics builds): which of
//! our buffers and textures held a card address the kernel names in a fault.
//!
//! On this host the device-removed data has no faulting address (vkd3d gives
//! none), but the kernel's line has it (`NVRM: Xid ... 31 ... faulted @
//! 0x5_8d249000`, `journalctl -k`). Card addresses are given out in the same
//! order run after run, so the address of one run's fault names the same
//! thing in the next. With the switch, the book ([`crate::book`]) is kept,
//! and:
//!
//! - each time something of ours is made or goes in the 2 MiB of card
//!   memory around an address (a fault of type FAULT_PDE has no page table
//!   there at all), a line says what, where and when;
//! - when the host device goes, the book says who holds each address now and
//!   who held it before.

use std::sync::OnceLock;

/// The log target: under `victory_gpu::host`, which the usual log settings
/// let through.
const TARGET: &str = "victory_gpu::host::fault_va";

/// The card memory one entry of the top page table stands for.
const REGION: u64 = 2 << 20;

/// The addresses asked about (none: the switch is off).
fn addresses() -> &'static [u64] {
    static AT: OnceLock<Vec<u64>> = OnceLock::new();
    AT.get_or_init(|| {
        let Ok(text) = std::env::var("VICTORY_FAULT_VA") else {
            return Vec::new();
        };
        text.split(',')
            .filter_map(|a| {
                let a = a.trim().replace('_', "");
                let a = a.strip_prefix("0x").unwrap_or(&a);
                u64::from_str_radix(a, 16).ok()
            })
            .filter(|&a| a != 0)
            .collect()
    })
}

/// Whether any address is asked about.
pub(crate) fn on() -> bool {
    !addresses().is_empty()
}

/// Something of ours, `what`, at card address `start` and `size` bytes long,
/// was made (`made`) or went, at `at_ms` (milliseconds of the run).
pub(crate) fn told(made: bool, what: &str, start: u64, size: u64, at_ms: u64) {
    for &va in addresses() {
        let region = va & !(REGION - 1);
        if start >= region + REGION || start.saturating_add(size) <= region {
            continue;
        }
        let holds = va >= start && va - start < size;
        tracing::info!(
            target: TARGET,
            "fault address {va:#x}: {} at {:.3} s: {what} {start:#x}+{size:#x}, {}",
            if made { "made" } else { "gone" },
            at_ms as f64 / 1e3,
            if holds {
                format!("holds it at offset {:#x}", va - start)
            } else {
                "in its 2 MiB".to_owned()
            }
        );
    }
}

/// When the host device goes: who holds each address and who held it.
pub(crate) fn log_at_loss() {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !on() || SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    for &va in addresses() {
        tracing::error!(target: TARGET, "fault address {va:#x} when the host device went{}", crate::book::at_loss(va));
    }
}
