//! editortweaks: a native Brick Rigs mod that extends the vehicle editor.
//!
//! The brickworks loader loads this DLL from `<game>/brickworks/editortweaks.dll` very early in
//! process start-up (before the engine exists) and calls [`mod_info`] then [`mod_init`].
//! From there [`framework::lifecycle`] drives every feature listed in [`features::ALL`].

mod features;
mod framework;
mod platform;
mod prelude;

use core::ffi::CStr;

use brickworks::modinfo::ModInfo;

/// Game version this build was developed against. Informational: the loader logs it and the
/// `hello` feature warns when the running game reports a different version.
pub const TARGET_GAME_VERSION: &CStr = c"1.11.7";

/// Describes the mod to the loader (printed in `brickworks.txt`).
#[unsafe(no_mangle)]
pub extern "C" fn mod_info() -> ModInfo {
    ModInfo {
        name: c"editortweaks".as_ptr().cast(),
        description: c"Vehicle editor improvements".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr(),
        game_version: TARGET_GAME_VERSION.as_ptr().cast(),
        authors: c"Perru".as_ptr().cast(),
    }
}

/// Called once by the loader, before the engine initializes.
#[unsafe(no_mangle)]
pub extern "C" fn mod_init() {
    framework::lifecycle::start(features::ALL);
}
