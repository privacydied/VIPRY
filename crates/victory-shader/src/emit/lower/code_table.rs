//! Where a program's constant table starts in its code (`VICTORY_CODE_TABLE_TAIL=1`, off by
//! default).
//!
//! [`Lower::code_dword`] reads the dwords the compiler appends after a program's code from an
//! internal constant array. That array began at the lowest offset any load or descriptor names,
//! and a load at a computed offset names offset 0: the array was then the whole program, up to
//! 64 KiB. A private constant array that large can make a Vulkan driver reserve per-thread local
//! memory for the whole card (gigabytes on NVIDIA under vkd3d-proton, as measured upstream).
//!
//! With the switch the table starts where the instructions end, unless a load at a fixed offset
//! reads below that. A computed-offset load below the new start reads the table's last dword,
//! as an index past the table always did.
//!
//! After Marvisak/VIctory e04c0781 (desp, pull request #2143), reimplemented here behind a switch.

use crate::{ir, resource};

/// Whether the constant table starts after the instructions (`VICTORY_CODE_TABLE_TAIL=1`). It
/// changes the translation, so the shader cache keys on it (`shader_cache::build`).
pub fn code_table_tail() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CODE_TABLE_TAIL").is_some_and(|v| v == "1"))
}

/// The byte where a program's constant table starts: where its instructions end, unless a load
/// at a fixed offset reads below that (then at `lowest`, the lowest offset any load or
/// descriptor names).
pub(super) fn code_table_start(p: &ir::Program, addresses: &[resource::Address], lowest: u32) -> u32 {
    let code_end = p
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .map(|i| i.machine.offset + 4 * i.machine.words.len())
        .max()
        .unwrap_or(0) as u32;
    let fixed_below = addresses.iter().any(|a| a.dynamic.is_none() && a.offset < code_end);
    if fixed_below { lowest } else { lowest.max(code_end & !3) }
}

#[cfg(test)]
mod tests {
    use super::code_table_start;
    use crate::{ir, resource};

    const S_NOP: u32 = 0xBF80_0000;
    const S_ENDPGM: u32 = 0xBF81_0000;

    /// The table starts after the instructions, unless a load at a fixed offset reads below.
    #[test]
    fn the_code_table_starts_after_the_instructions() {
        // s_nop 0, s_endpgm, then 1,000 dwords of constants.
        let mut words = vec![S_NOP, S_ENDPGM];
        words.extend(0..1000u32);
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = ir::build(&code, false, 256).unwrap();
        let at = |offset: u32, dynamic: bool| resource::Address {
            param: resource::CODE,
            offset,
            dynamic: dynamic.then_some(ir::Value(0)),
        };
        let (computed, fixed_after, fixed_inside) = (at(0, true), at(12, false), at(4, false));
        // A load at a computed offset from the code's start: the constants only.
        assert_eq!(code_table_start(&p, &[computed], 0), 8);
        assert_eq!(code_table_start(&p, &[computed, fixed_after], 0), 8);
        // A load at a fixed offset inside the instructions keeps the start.
        assert_eq!(code_table_start(&p, &[computed, fixed_inside], 0), 0);
        // Loads past the instructions only: from the lowest of them.
        assert_eq!(code_table_start(&p, &[fixed_after], 12), 12);
    }
}
