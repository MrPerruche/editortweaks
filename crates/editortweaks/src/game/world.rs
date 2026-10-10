//! The current world (`GWorld`).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::gameplay::world::GWORLD;

/// The current world (a `UWorld`), or null: before the first map, or if the loader didn't find
/// `GWorld`.
///
/// # Safety
/// Game thread only.
pub unsafe fn current() -> *mut UObjectBase {
    // SAFETY: per the contract; the lookup is read by value (set once at start) and points to
    // `GWorld`, whose first field is the world pointer.
    unsafe {
        let global = GWORLD.generic.cast::<*mut UObjectBase>();
        if global.is_null() {
            return core::ptr::null_mut();
        }
        global.read()
    }
}
