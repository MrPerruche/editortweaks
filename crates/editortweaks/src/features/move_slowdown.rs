//! Slower mouse moves in the editor:
//!
//! - while Control is held ([`keybinds::SLOW_MOVES`], rebindable like the other), moving or
//!   rotating bricks with the mouse in move mode (G, dragging a brick, or the gizmo) goes
//!   [`CONTROL_FACTOR`] as fast. Not the camera: Control is also the camera speed key (with the
//!   wheel), and turning the camera with it held shouldn't crawl;
//! - while Shift is held ([`keybinds::SLOW_GIZMO_MOVES`]), moving or rotating bricks by a gizmo
//!   handle goes [`SHIFT_FACTOR`] as fast (with Control too: their product). Not the camera, and
//!   not bricks following the mouse (G, or dragging a brick), where Shift stays the game's "move
//!   perpendicular".
//!
//! With snapping on, bricks still snap to the grid; they just need more mouse movement per grid
//! step. (Alt used to slow down too; it's the resize tool's key now.)
//!
//! How: the editor gets the mouse's movement through its reflected handlers `Axis_MouseMoveRight`
//! and `Axis_MouseMoveUp`, which this scales while the keys are held (through
//! [`super::mouse_moves`], which the resize tool shares). How bricks move comes from the editor's
//! move mode (`game::editor_mode`), whether the camera turns instead from the editor's input
//! (`game::editor_input::CameraFlags`).

use std::sync::{Once, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;

use super::keybinds;
use super::mouse_moves::{self, Handling, Movement};
use crate::game::editor_input::{self, CameraFlags};
use crate::game::editor_mode::{self, EditorModes, Moving};
use crate::prelude::*;

/// How fast the mouse moves things while [`keybinds::SLOW_MOVES`] is held.
const CONTROL_FACTOR: f32 = 1.0 / 25.0;
/// How fast the mouse moves bricks by a gizmo handle while [`keybinds::SLOW_GIZMO_MOVES`] is held.
const SHIFT_FACTOR: f32 = 1.0 / 1000.0;

pub struct MoveSlowdown;

/// What Shift needs (see [`moves_by_gizmo`]); without them Shift slows nothing down.
static MODES: OnceLock<EditorModes> = OnceLock::new();
static CAMERA: OnceLock<CameraFlags> = OnceLock::new();
/// Logs the first slowed down movement, to tell in the log whether slowing down works.
static FIRST_SLOWDOWN: Once = Once::new();

impl Feature for MoveSlowdown {
    fn id(&self) -> &'static str {
        "move_slowdown"
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!("active"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// The handlers can only be found through reflection, so they're hooked here.
fn engine_init() -> Result<(), String> {
    match editor_input::camera_flags() {
        Ok(camera) => {
            let _ = CAMERA.set(camera);
        }
        Err(reason) => log!("Control slows the camera too, and Shift won't slow down: {reason}"),
    }
    match editor_mode::find() {
        Ok(modes) if modes.knows_gizmo_axis() => {
            let _ = MODES.set(modes);
        }
        Ok(_) => log!(
            "Shift won't slow down: {} not found among the move mode's functions",
            editor_mode::SHOULD_CAPTURE_MOUSE.name
        ),
        Err(reason) => log!("Shift won't slow down: {reason}"),
    }
    mouse_moves::listen(slow_down)
}

fn slow_down(movement: &Movement) -> Handling {
    let mut factor = 1.0;
    // SAFETY: the movement's input component is live during its handler's call.
    if keybinds::SLOW_MOVES.is_held() && !unsafe { turns_camera(movement.input) } {
        factor *= CONTROL_FACTOR;
    }
    // SAFETY: the movement's input component is live during its handler's call.
    if keybinds::SLOW_GIZMO_MOVES.is_held() && unsafe { moves_by_gizmo(movement.input) } {
        factor *= SHIFT_FACTOR;
    }
    if factor == 1.0 {
        return Handling::Pass;
    }
    FIRST_SLOWDOWN.call_once(|| {
        log!(
            "slowing down (first: {} -> {})",
            movement.value,
            movement.value * factor
        )
    });
    Handling::Scale(factor)
}

/// Whether the mouse turns the camera of `input` (rather than moving bricks). Unknown (the camera
/// flags weren't found): no, so Control still slows bricks down.
///
/// # Safety
/// `input` must be a live editor input component.
unsafe fn turns_camera(input: *const UObjectBase) -> bool {
    // SAFETY: per the contract.
    CAMERA
        .get()
        .is_some_and(|camera| unsafe { camera.moving(input) })
}

/// Whether the mouse moves bricks by a gizmo handle in the editor of `input` (rather than turning
/// the camera, or moving them freely).
///
/// # Safety
/// `input` must be a live editor input component.
unsafe fn moves_by_gizmo(input: *const UObjectBase) -> bool {
    let (Some(modes), Some(camera)) = (MODES.get(), CAMERA.get()) else {
        return false;
    };
    // SAFETY: per the contract; an input component's editor is live while it is.
    unsafe {
        !camera.moving(input)
            && modes.editor_of(input).is_some_and(|editor| {
                matches!(
                    modes.moving(editor),
                    Some(Moving::ByGizmo | Moving::Precisely)
                )
            })
    }
}
