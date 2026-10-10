//! The editor's mouse movement, shared by the features that change what it does. Not a feature
//! itself: the loader hooks a function only once, so the features can't each hook the handlers.
//!
//! While a mouse button is held over the 3D view, the engine reports the mouse's movement to the
//! editor's reflected handlers `Axis_MouseMoveRight` and `Axis_MouseMoveUp`
//! (`game::editor_input::MouseMoveAxes`): in a move mode it moves the bricks, otherwise it turns the
//! camera. Their exec thunks are hooked here on the first [`listen`]; every listener sees each
//! movement and can scale it or take it (then the game's handler doesn't run).

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::game::editor_input::{self, Axis, MouseMoveAxes};
use crate::game::reflection::NativeThunk;

pub use crate::game::editor_input::AXIS_PER_PIXEL;

/// Which way a movement goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Right,
    Up,
}

/// One mouse movement, as the editor gets it.
#[derive(Clone, Copy)]
pub struct Movement {
    pub direction: Direction,
    /// How far (see [`AXIS_PER_PIXEL`]; left and down are negative).
    pub value: f32,
    /// The editor input component handling it (its outer is the editor).
    pub input: *const UObjectBase,
}

/// What a listener does with a movement.
pub enum Handling {
    /// Leaves it.
    Pass,
    /// Scales it (several listeners scaling it: their product).
    Scale(f32),
    /// Takes it: the game's handler doesn't run, so it neither moves bricks nor turns the camera.
    Take,
}

pub type Listener = fn(&Movement) -> Handling;

static AXES: OnceLock<MouseMoveAxes> = OnceLock::new();
static MOUSE_MOVE_RIGHT: OnceLock<NativeThunk> = OnceLock::new();
static MOUSE_MOVE_UP: OnceLock<NativeThunk> = OnceLock::new();
/// Whether the hooks are in place, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// Calls `listener` with every mouse movement in the editor from now on (after engine init: the
/// handlers are found through reflection).
pub fn listen(listener: Listener) -> Result<(), String> {
    HOOKED.get_or_init(hook_handlers).clone()?;
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(listener);
    Ok(())
}

fn hook_handlers() -> Result<(), String> {
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
        handle(Direction::Right, context, stack, result, || {
            original(context, stack, result)
        })
    }
}

unsafe extern "C" fn mouse_move_up(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *MOUSE_MOVE_UP.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe {
        handle(Direction::Up, context, stack, result, || {
            original(context, stack, result)
        })
    }
}

/// Runs `call`, the mouse axis handler for `direction` of `context` called with `stack` and
/// `result`, as the listeners say: with its movement scaled, or not at all.
///
/// # Safety
/// `context`, `stack` and `result` must be the arguments the handler's thunk was called with.
unsafe fn handle(
    direction: Direction,
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
    call: impl FnOnce(),
) {
    let Some(axes) = AXES.get() else {
        return call();
    };
    let axis: &Axis = match direction {
        Direction::Right => &axes.right,
        Direction::Up => &axes.up,
    };
    // SAFETY: per the contract.
    let value = unsafe {
        if axes.is_editor_input(context) {
            axes.axis_value(axis, stack)
        } else {
            None
        }
    };
    let Some(value) = value else {
        return call();
    };
    // SAFETY: the caller's parameters, which only this thread uses during the call.
    let moved = unsafe { value.read() };
    if moved == 0.0 {
        return call();
    }
    let movement = Movement {
        direction,
        value: moved,
        input: context.cast_const().cast(),
    };
    let (mut factor, mut taken) = (1.0, false);
    // Listeners don't listen from inside a listener, so holding the lock while they run is fine.
    for listener in LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
    {
        match listener(&movement) {
            Handling::Pass => {}
            Handling::Scale(scale) => factor *= scale,
            Handling::Take => taken = true,
        }
    }
    // SAFETY: per the contract; the parameter is restored after the call.
    unsafe {
        if taken {
            axes.handled(result);
        } else if factor != 1.0 {
            value.write(moved * factor);
            call();
            value.write(moved);
        } else {
            call();
        }
    }
}
