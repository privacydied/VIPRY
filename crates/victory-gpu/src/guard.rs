//! Guard pages hit by our own copies of title memory.

use windows_sys::Win32::System::Memory::{
    MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualProtect, VirtualQuery,
};

const PAGE: usize = 4096;

/// Sets the guard flag again on the page holding `at`, with the protection it has
/// now: the fault that reported it cleared the flag.
pub(crate) fn rearm(at: usize) {
    let page = at / PAGE * PAGE;
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let mut old = 0;
    // SAFETY: a valid out pointer of the right size; a protection change of one committed page.
    unsafe {
        if VirtualQuery(page as *const _, &mut info, size_of_val(&info)) != 0 {
            VirtualProtect(page as *const _, PAGE, info.Protect | PAGE_GUARD, &mut old);
        }
    }
}
