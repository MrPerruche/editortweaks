//! How far a view target lets the player's view turn: `IViewTargetInterface::GetRotationLimits`
//! (slot 10 of the interface, `FVector2D& Min, FVector2D& Max`: pitch, then yaw). The base
//! `UpdateViewRotation` (game 0xEAB450, called first by `ABrickEditor::UpdateViewRotation`) asks
//! for them and clamps each axis of the rotation to its range after wrapping the angle to
//! (-180°, 180°] (0xE75F80); roll has no range of its own there. Found in game 1.11.7 (build
//! 25831956); see `docs/re/editor-camera.md`.
//!
//! The default one writes pitch ±90° and yaw ±180°, and the editor uses it as it is: it's why the
//! editor's camera can't go upside down. It's a leaf function (no unwind entry), shared by every
//! view target that keeps the default (the compiler folded identical ones), so it is found through
//! a virtual function table of the editor's interface instead: two slots after
//! [`UPDATE_VIEW_ROTATION`], whose function is in the same tables (slot 8).

use core::ffi::c_void;

use crate::framework::sig::{self, CodeShape};
use crate::game::editor_camera::UPDATE_VIEW_ROTATION;

/// The default `GetRotationLimits` (game RVA 0xE70E30): `Min = (-90, -180); Max = (90, 180)`.
pub const DEFAULT_LIMITS: CodeShape = CodeShape {
    name: "IViewTargetInterface::GetRotationLimits",
    shape: "c7 02 00 00 b4 c2 c7 42 04 00 00 34 c3 41 c7 00 00 00 b4 42 41 c7 40 04 00 00 34 43 c3",
};

/// [`DEFAULT_LIMITS`]'s signature; `interface` is the view target's `IViewTargetInterface`, `min` and
/// `max` pitch and yaw, in degrees.
pub type RotationLimitsFn =
    unsafe extern "C" fn(interface: *mut c_void, min: *mut [f32; 2], max: *mut [f32; 2]);

/// How far `GetRotationLimits` is from `UpdateViewRotation` in the interface's table.
const SLOTS_AFTER_UPDATE: usize = 2;

/// The editor's `GetRotationLimits`, the default one, found in the tables that list its
/// `UpdateViewRotation`. Before engine init too.
pub fn find() -> Result<*const u8, String> {
    let update = UPDATE_VIEW_ROTATION.resolve()?;
    let mut limits = sig::pointers_to(update).into_iter().map(|slot| {
        // SAFETY: a slot of a table in the exe's data, so the entries around it are mapped (a
        // table has more than a few entries after slot 8).
        unsafe { slot.add(SLOTS_AFTER_UPDATE).read() as *const u8 }
    });
    let first = limits
        .next()
        .ok_or("no table lists ABrickEditor::UpdateViewRotation")?;
    if !DEFAULT_LIMITS.matches(first) || limits.any(|other| other != first) {
        return Err(format!(
            "{}: not the default function (or the tables disagree)",
            DEFAULT_LIMITS.name
        ));
    }
    Ok(first)
}
