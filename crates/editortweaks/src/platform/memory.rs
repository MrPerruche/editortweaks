//! Whether memory can be read, for data whose place in the game's memory is inferred rather than
//! reflected: checked before it's read, so a wrong guess is an error, not a crash.

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualQuery,
};

/// Whether the `len` bytes at `address` are committed, readable memory of this process. Only says
/// so for the moment it's asked: the memory must not be freed meanwhile (e.g. read on the thread
/// that owns it).
pub fn is_readable(address: *const u8, len: usize) -> bool {
    const READABLE: u32 = PAGE_READONLY
        | PAGE_READWRITE
        | PAGE_WRITECOPY
        | PAGE_EXECUTE_READ
        | PAGE_EXECUTE_READWRITE
        | PAGE_EXECUTE_WRITECOPY;
    if address.is_null() {
        return false;
    }
    let Some(end) = (address as usize).checked_add(len) else {
        return false;
    };
    let mut at = address as usize;
    while at < end {
        // SAFETY: an all-zero MEMORY_BASIC_INFORMATION is valid (plain integers and pointers).
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        // SAFETY: VirtualQuery only reads the address space's bookkeeping; `info` is ours.
        let written = unsafe {
            VirtualQuery(
                at as *const _,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0
            || info.State != MEM_COMMIT
            || info.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0
            || info.Protect & READABLE == 0
        {
            return false;
        }
        let region_end = info.BaseAddress as usize + info.RegionSize;
        if region_end <= at {
            return false;
        }
        at = region_end;
    }
    true
}
