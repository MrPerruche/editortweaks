//! Function hooks (detours) through the brickworks loader, which uses MinHook.

use std::sync::OnceLock;

/// Redirects every call of `target` to `detour` and returns a pointer that calls the original
/// function (MinHook's trampoline). `name` is only used in error messages.
///
/// Limitation of the 0.1.0 loader: if another mod already hooked `target`, this returns that
/// hook's trampoline without installing `detour`.
///
/// # Safety
/// `target` must be the start of a function, and `detour` a function with the exact same
/// signature and calling convention.
pub unsafe fn install(
    name: &str,
    target: *const u8,
    detour: *const (),
) -> Result<*const (), String> {
    // SAFETY: guaranteed by the caller; the loader patches `target` with a jump to `detour`.
    let original = unsafe { brickworks::iface::brickworks_create_hook(target.cast(), detour) };
    if original.is_null() {
        return Err(format!("{name}: failed to install hook at {target:p}"));
    }
    Ok(original)
}

/// [`install`] for a typed function pointer `F`; the trampoline is stored in `original`.
///
/// # Safety
/// As for [`install`]; `F` must be a function pointer type matching the target's signature.
pub unsafe fn install_typed<F: Copy>(
    name: &str,
    target: *const u8,
    detour: F,
    original: &OnceLock<F>,
) -> Result<(), String> {
    const {
        assert!(
            size_of::<F>() == size_of::<*const ()>(),
            "F must be a function pointer"
        )
    };
    // SAFETY: F is a function pointer (size checked above), so it converts to and from a
    // plain code pointer; the rest is the caller's contract.
    unsafe {
        let trampoline = install(name, target, std::mem::transmute_copy(&detour))?;
        let _ = original.set(std::mem::transmute_copy(&trampoline));
    }
    Ok(())
}
