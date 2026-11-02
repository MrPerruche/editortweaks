//! A camera that can go upside down in the editor, a setting ([`UPSIDE_DOWN`]): the mouse turns the
//! view past straight up and straight down, all the way round. The game stops it at ±90°.
//!
//! The limit is the view target's: `ABrickEditor::UpdateViewRotation` clamps the rotation to the
//! ranges its interface's `GetRotationLimits` gives (`game::rotation_limits`), and the editor's
//! says pitch ±90°. That function is hooked: for the editor, with the setting on, the pitch's range
//! becomes ±180°, which the clamp (after wrapping the angle to ±180°) lets everything through.
//! Nothing else changes, and other view targets keep their ranges.
//!
//! The second option also turns the horizontal mouse movement round while the camera is upside
//! down and turns, since there the view's yaw runs against the mouse (it turns about the world's
//! up axis, which now points down the screen): the mouse right turns the view right. Through
//! `mouse_moves`, like the other features changing what the mouse does.

use core::ffi::c_void;
use std::sync::OnceLock;

use super::editor_frame;
use super::mouse_moves::{self, Direction, Handling, Movement};
use crate::framework::hook;
use crate::framework::settings::{Alternative, Choice};
use crate::framework::timings;
use crate::game::editor_input::{self, CameraFlags};
use crate::game::reflection;
use crate::game::rotation_limits::{self, RotationLimitsFn};
use crate::game::view_target;
use crate::prelude::*;

/// Whether the editor's camera may turn past straight up and down, a setting: the options of
/// [`Upside`], in order.
pub static UPSIDE_DOWN: Choice = Choice::new(
    "CameraUpsideDown",
    "Camera upside down",
    "In the vehicle editor, the camera can turn past straight up and down, all the way round. Yaw \
     inverted: the mouse's left and right also swap while the view is upside down.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "On",
            name: "On",
        },
        Alternative {
            key: "OnYawInverted",
            name: "Yaw inverted",
        },
    ],
    0,
);

/// The options of [`UPSIDE_DOWN`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Upside {
    Off,
    On,
    YawInverted,
}

impl Upside {
    fn chosen() -> Self {
        match UPSIDE_DOWN.get() {
            1 => Self::On,
            2 => Self::YawInverted,
            _ => Self::Off,
        }
    }
}

/// The widest pitch range: every angle, after the game wrapped it to ±180°.
const ANY_PITCH: f32 = 180.0;

pub struct FreePitch;

static ORIGINAL: OnceLock<RotationLimitsFn> = OnceLock::new();
/// `BrickEditor`'s address, set once reflection is available; until then (or if it fails) the
/// limits stay the game's.
static EDITOR_CLASS: OnceLock<usize> = OnceLock::new();
/// Whether the editor's camera is being turned; without it, the mouse isn't turned round.
static CAMERA_FLAGS: OnceLock<CameraFlags> = OnceLock::new();

impl Feature for FreePitch {
    fn id(&self) -> &'static str {
        "free_pitch"
    }

    fn on_load(&self) -> LoadResult {
        let target = rotation_limits::find()?;
        // SAFETY: `target` is the start of the default GetRotationLimits (its shape was checked),
        // and the detour has its signature.
        unsafe {
            hook::install_typed(
                rotation_limits::DEFAULT_LIMITS.name,
                target,
                rotation_limits_detour as RotationLimitsFn,
                &ORIGINAL,
            )
        }
    }

    fn on_engine_init(&self) {
        let Some(class) = reflection::find_class("BrickEditor") else {
            log!("inactive: class BrickEditor not found");
            return;
        };
        let _ = EDITOR_CLASS.set(class as usize);
        match editor_input::camera_flags() {
            Ok(flags) => {
                let _ = CAMERA_FLAGS.set(flags);
                if let Err(reason) = mouse_moves::listen(invert_yaw) {
                    log!("the yaw isn't inverted upside down: {reason}");
                }
            }
            Err(reason) => log!("the yaw isn't inverted upside down: {reason}"),
        }
        log!("active");
    }
}

unsafe extern "C" fn rotation_limits_detour(
    interface: *mut c_void,
    min: *mut [f32; 2],
    max: *mut [f32; 2],
) {
    let _span = timings::span("free_pitch::rotation_limits_detour");
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the game's own arguments, passed on: the view target's interface and two vectors.
    unsafe {
        timings::game(rotation_limits::DEFAULT_LIMITS.name, || {
            original(interface, min, max)
        });
        if Upside::chosen() != Upside::Off && is_editor(interface) {
            (*min)[0] = -ANY_PITCH;
            (*max)[0] = ANY_PITCH;
        }
    }
}

/// Whether `interface` is the editor's `IViewTargetInterface`.
///
/// # Safety
/// `interface` must point to a live object's `IViewTargetInterface`. Game thread only.
unsafe fn is_editor(interface: *mut c_void) -> bool {
    let Some(&class) = EDITOR_CLASS.get() else {
        return false;
    };
    // SAFETY: per the contract; the class is the live `BrickEditor`.
    unsafe {
        view_target::object_of(interface)
            .is_some_and(|object| reflection::is_a(object, class as *const _))
    }
}

/// Turns the mouse's horizontal movement round while it turns an upside down camera.
fn invert_yaw(movement: &Movement) -> Handling {
    if Upside::chosen() != Upside::YawInverted || movement.direction != Direction::Right {
        return Handling::Pass;
    }
    let Some(flags) = CAMERA_FLAGS.get() else {
        return Handling::Pass;
    };
    // SAFETY: the movement's input is the live editor input component handling it, and its
    // outer is the editor.
    let upside_down = unsafe {
        flags.moving(movement.input)
            && camera_is_upside_down((*movement.input).outer_private.cast())
    };
    if upside_down {
        Handling::Scale(-1.0)
    } else {
        Handling::Pass
    }
}

/// Whether the camera of `editor` has its up axis pointing down.
///
/// # Safety
/// `editor` must be a live editor. Game thread only.
unsafe fn camera_is_upside_down(editor: *const u8) -> bool {
    let Some(offset) = editor_frame::camera_rotation_offset() else {
        return false;
    };
    // SAFETY: per the contract; the rotation (pitch, yaw, roll) is at `offset` in the editor, as
    // the game's own code reads it (`editor_frame`).
    let rotation = unsafe { editor.add(offset).cast::<[f32; 3]>().read_unaligned() };
    is_upside_down(rotation)
}

/// Whether the camera with this rotation (pitch, yaw, roll, in degrees) has its up axis pointing
/// down: the up axis' height is `cos(pitch) * cos(roll)`.
fn is_upside_down([pitch, _, roll]: [f32; 3]) -> bool {
    pitch.to_radians().cos() * roll.to_radians().cos() < 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upside_down_is_up_pointing_down() {
        assert!(!is_upside_down([0.0, 40.0, 0.0]));
        assert!(!is_upside_down([-89.0, 0.0, 0.0]));
        assert!(!is_upside_down([89.9, 170.0, 0.0]));
        assert!(is_upside_down([100.0, 0.0, 0.0]));
        assert!(is_upside_down([-135.0, 10.0, 0.0]));
        // The same rotations as rotators with pitch within ±90°.
        assert!(is_upside_down([80.0, 180.0, 180.0]));
        assert!(!is_upside_down([80.0, 0.0, 0.0]));
    }
}
