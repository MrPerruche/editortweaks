//! Logging into the loader's `brickworks.txt` (in the game's root folder). Use the [`log!`] macro.

use std::ffi::CString;

/// `log!("format {}", args)`: like `println!`, but writes a line to `brickworks.txt`, tagged
/// with the calling module (e.g. `[editortweaks/hello]`).
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::framework::log::write(module_path!(), &::std::format!($($arg)*))
    };
}
pub(crate) use log;

/// Writes one tagged line to `brickworks.txt`. Prefer the [`log!`] macro.
pub fn write(module_path: &str, message: &str) {
    let tag = CString::new(tag(module_path)).unwrap_or_default();
    let message = CString::new(message.replace('\0', "\\0")).unwrap_or_default();
    // SAFETY: both pointers are valid NUL-terminated strings for the duration of the call, and
    // the loader formats them into its log without keeping them.
    unsafe { brickworks::iface::brickworks_puts(tag.as_ptr().cast(), message.as_ptr().cast()) };
}

/// `editortweaks::features::hello` -> `editortweaks/hello`,
/// `editortweaks::framework::lifecycle` -> `editortweaks/framework::lifecycle`.
fn tag(module_path: &str) -> String {
    let rest = module_path
        .strip_prefix("editortweaks")
        .unwrap_or(module_path)
        .trim_start_matches("::");
    let rest = rest.strip_prefix("features::").unwrap_or(rest);
    if rest.is_empty() {
        "editortweaks".to_owned()
    } else {
        format!("editortweaks/{rest}")
    }
}
