//! Slower brick moves: while Control or Alt is held, moving or rotating bricks with the mouse in
//! move mode (G or the gizmo) goes [`CONTROL_FACTOR`] or [`ALT_FACTOR`] as fast (both: their
//! product). Shift stays the game's "move perpendicular". With snapping on, bricks still snap to
//! the grid; they just need more mouse movement per grid step.
//!
//! How: the editor's mouse movement reaches move mode only through its reflected handlers
//! `Axis_MouseMoveRight` and `Axis_MouseMoveUp` (`game::editor_input`). Their exec thunks are
//! hooked, and scale the mouse movement they pass on while the keys are held.

use core::ffi::c_void;
use std::sync::{Once, OnceLock};

use crate::framework::hook;
use crate::game::editor_input::{self, Axis, MouseMoveAxes};
use crate::game::reflection::NativeThunk;
use crate::platform::keyboard;
use crate::prelude::*;

/// How fast bricks move while Control is held.
const CONTROL_FACTOR: f32 = 1.0 / 25.0;
/// How fast bricks move while Alt is held.
const ALT_FACTOR: f32 = 1.0 / 1000.0;

pub struct MoveSlowdown;

static AXES: OnceLock<MouseMoveAxes> = OnceLock::new();
static MOUSE_MOVE_RIGHT: OnceLock<NativeThunk> = OnceLock::new();
static MOUSE_MOVE_UP: OnceLock<NativeThunk> = OnceLock::new();
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
    let axes = editor_input::mouse_move_axes()?;
    let (right, up) = (axes.right.thunk, axes.up.thunk);
    let _ = AXES.set(axes);
    // SAFETY: both targets are exec thunks, and the detours are NativeThunks.
    unsafe {
        hook::install_typed(
            "UBaseInputComponent::execAxis_MouseMoveRight",
            right,
            mouse_move_right as NativeThunk,
            &MOUSE_MOVE_RIGHT,
        )?;
        hook::install_typed(
            "UBaseInputComponent::execAxis_MouseMoveUp",
            up,
            mouse_move_up as NativeThunk,
            &MOUSE_MOVE_UP,
        )
    }
}

unsafe extern "C" fn mouse_move_right(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *MOUSE_MOVE_RIGHT.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe {
        slowed_down(
            context,
            stack,
            |axes| &axes.right,
            || original(context, stack, result),
        )
    }
}

unsafe extern "C" fn mouse_move_up(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *MOUSE_MOVE_UP.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe {
        slowed_down(
            context,
            stack,
            |axes| &axes.up,
            || original(context, stack, result),
        )
    }
}

/// Runs `call`, the mouse axis handler `axis` of `context` called with `stack`, with its mouse
/// movement scaled by the modifier keys held.
///
/// # Safety
/// `context` and `stack` must be the arguments the handler's thunk was called with.
unsafe fn slowed_down(
    context: *mut c_void,
    stack: *mut c_void,
    axis: impl FnOnce(&MouseMoveAxes) -> &Axis,
    call: impl FnOnce(),
) {
    let factor = speed_factor();
    let value = AXES.get().filter(|_| factor != 1.0).and_then(|axes| {
        // SAFETY: per the contract.
        unsafe {
            if axes.is_editor_input(context) {
                axes.axis_value(axis(axes), stack)
            } else {
                None
            }
        }
    });
    let Some(value) = value else {
        return call();
    };
    // SAFETY: the caller's parameters, which only this thread uses during the call; restored
    // after it.
    unsafe {
        let moved = value.read();
        value.write(moved * factor);
        FIRST_SLOWDOWN.call_once(|| log!("slowing down (first: {moved} -> {})", moved * factor));
        call();
        value.write(moved);
    }
}

/// The speed factor of the modifier keys held right now.
fn speed_factor() -> f32 {
    let mut factor = 1.0;
    if keyboard::is_control_down() {
        factor *= CONTROL_FACTOR;
    }
    if keyboard::is_alt_down() {
        factor *= ALT_FACTOR;
    }
    factor
}
