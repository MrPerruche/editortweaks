//! Game assets by path (`/Game/...`, `/Engine/...`), loaded from the game's files if no map loaded
//! them yet: `UObject* StaticLoadObject(UClass*, UObject* InOuter, const TCHAR* Name, ...)`, which
//! brickrust finds (and hooks) itself, so its signature is checked at start-up.

use brickrust::ue::coreuobject::{StaticLoadObject, UObjectBase};
use brickrust::ue::uclass::UClass;

/// `ELoadFlags`: `LOAD_NoWarn | LOAD_Quiet`, a missing asset is no error worth logging.
const QUIET: u32 = 0x0000_0002 | 0x0000_2000;

/// The asset at `path` (`/Package/Path/Name.Name`), of `class` or a subclass; loaded now if it
/// wasn't. Null if the game has no such asset.
///
/// Loading reads the game's files on the game thread: keep it for small assets, once.
///
/// # Safety
/// `class` must be a live class. Game thread only, after engine init.
pub unsafe fn load(class: *const UClass, path: &str) -> *mut UObjectBase {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    // SAFETY: per the contract; the name is a null-terminated wide string living through the
    // call, no outer and no file name (both optional), and the same arguments brickrust's own
    // hook passes for the game's loads.
    unsafe {
        StaticLoadObject(
            class.cast_mut(),
            core::ptr::null_mut(),
            wide.as_ptr(),
            core::ptr::null(),
            QUIET,
            false,
        )
    }
}
