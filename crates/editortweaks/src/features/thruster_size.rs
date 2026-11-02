//! Thrusters' size as three values in the property panel, like every other scalable brick's
//! ([`THREE_VALUES`]). The game locks a thruster's X and Y together (a square nozzle): its row
//! shows two sliders (X and Y as one, Z) for a size of three values, which the value order button
//! and slider drags can't follow. Only the panel knows of that lock (see
//! `game::thruster::size_axis_lock`): the game itself takes any X and Y, also from files.
//!
//! How: the thruster's `GetBrickSizeAxisLock` is too small to hook, so its entry in the thrusters'
//! virtual function table is replaced with [`size_axis_lock`]: no lock with the setting on, the
//! game's (XY) with it off. Panels refreshed after a change of the setting follow it.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::patch;
use crate::framework::settings::Toggle;
use crate::game::thruster::{self, SizeAxisLockFn};
use crate::prelude::*;

pub struct ThrusterSize;

/// Whether thrusters' X and Y sizes are separate values in the property panel.
pub static THREE_VALUES: Toggle = Toggle::new(
    "ThrusterSizeThreeValues",
    "Thruster size in three values",
    "A thruster's size shows its X, Y and Z, like other bricks' sizes. The game keeps X and Y \
     together (a square nozzle) in the editor only: vehicles with other nozzles load the same \
     without the mod.",
    true,
);

/// `EFluAxisLock::None`.
const NO_LOCK: u8 = 0;

/// The thrusters' own `GetBrickSizeAxisLock`.
static ORIGINAL: OnceLock<SizeAxisLockFn> = OnceLock::new();

impl Feature for ThrusterSize {
    fn id(&self) -> &'static str {
        "thruster_size"
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => log!("active: thrusters' X and Y sizes are separate in the panel"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Puts [`size_axis_lock`] in the thrusters' table. After engine init (default objects).
fn start() -> Result<(), String> {
    let slot = thruster::size_axis_lock()?;
    // SAFETY: an entry of the thrusters' virtual function table, checked to lead to their
    // `GetBrickSizeAxisLock`, which has `SizeAxisLockFn`'s signature (`this` in, a byte out).
    unsafe {
        let original = slot.read();
        let _ = ORIGINAL.set(core::mem::transmute::<usize, SizeAxisLockFn>(original));
        patch::write_pointer(slot, original, size_axis_lock as SizeAxisLockFn as usize)
    }
}

/// A thruster's size's axis lock: none with [`THREE_VALUES`] on, else the game's.
unsafe extern "C" fn size_axis_lock(brick: *const UObjectBase) -> u8 {
    if THREE_VALUES.get() {
        return NO_LOCK;
    }
    let original = *ORIGINAL
        .get()
        .expect("recorded before the table was changed");
    // SAFETY: forwards the caller's argument to the function this one replaced.
    unsafe { original(brick) }
}
