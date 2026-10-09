//! Rewriting a few bytes of the game's code, for changes too small for a hook (e.g. skipping one
//! comparison inside a function).

use core::ffi::c_void;

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Replaces the code at `address` with `replacement`, if it's currently `expected` (so a game
/// update that changed it is noticed instead of corrupted). Swapping the arguments reverts it.
///
/// # Safety
/// `address` must point into the game's code, `expected` must be whole instructions that nothing
/// jumps into the middle of, and `replacement` must be valid code there. No other thread may be
/// running these bytes meanwhile (e.g. call it during start-up, or on the thread that runs them).
pub unsafe fn write_code(
    address: *mut u8,
    expected: &[u8],
    replacement: &[u8],
) -> Result<(), String> {
    assert_eq!(
        expected.len(),
        replacement.len(),
        "a patch keeps the code's size"
    );
    // SAFETY: per the contract, `address` points to at least `expected.len()` bytes of code,
    // which is mapped and readable.
    let current = unsafe { core::slice::from_raw_parts(address, expected.len()) };
    if current != expected {
        return Err(format!(
            "unexpected code at {address:p}: {current:02X?}, expected {expected:02X?}"
        ));
    }

    let size = replacement.len();
    let mut protection = 0;
    // SAFETY: changes the protection of the pages holding these bytes, then restores it; the copy
    // writes exactly the checked range. Per the contract, nothing runs it meanwhile.
    unsafe {
        if VirtualProtect(
            address.cast(),
            size,
            PAGE_EXECUTE_READWRITE,
            &mut protection,
        ) == 0
        {
            return Err(format!("can't make the code at {address:p} writable"));
        }
        core::ptr::copy_nonoverlapping(replacement.as_ptr(), address, size);
        VirtualProtect(address.cast(), size, protection, &mut protection);
        FlushInstructionCache(GetCurrentProcess(), address.cast::<c_void>(), size);
    }
    Ok(())
}
