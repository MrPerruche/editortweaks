//! The editor's mouse movement, shared by the features that change what it does. Not a feature
//! itself: the loader hooks a function only once, so the features can't each hook the handlers.
//!
//! While a mouse button is held over the 3D view, the engine reports the mouse's movement to the
//! editor's reflected handlers `Axis_MouseMoveRight` and `Axis_MouseMoveUp`
//! (`game::editor_input::MouseMoveAxes`): in a move mode it moves the bricks, otherwise it turns the
//! camera. Their exec thunks are hooked here on the first [`listen`]; every listener sees each
//! movement and can scale it or take it (then the game's handler doesn't run). A listener taking
//! the movement for a drag follows the mouse with a [`Pointer`].

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

impl Movement {
    /// How far it moves on the screen, in pixels: right, down.
    pub fn pixels(&self) -> [f32; 2] {
        let pixels = self.value / AXIS_PER_PIXEL;
        match self.direction {
            Direction::Right => [pixels, 0.0],
            // Up is positive, and the screen's y goes down.
            Direction::Up => [0.0, -pixels],
        }
    }
}

/// Where a drag points (screen pixels), see [`Self::point`].
pub struct Pointer {
    at: [f32; 2],
    /// Where the cursor was when the button went down.
    pressed_at: [f32; 2],
    /// Whether the cursor has moved since (else the engine holds it still).
    cursor_moves: bool,
    /// The mouse's movement not yet added (pixels), see [`Self::moved`].
    moved: [f32; 2],
}

impl Pointer {
    pub fn new(mouse: [f32; 2]) -> Self {
        Self {
            at: mouse,
            pressed_at: mouse,
            cursor_moves: false,
            moved: [0.0; 2],
        }
    }

    /// Adds the mouse's movement (pixels, see [`Movement::pixels`]), for the next [`Self::point`].
    pub fn moved(&mut self, pixels: [f32; 2]) {
        self.moved[0] += pixels[0];
        self.moved[1] += pixels[1];
    }

    /// Where the drag points now (screen pixels), given where the cursor is (if known). While a
    /// button is held over the 3D view, the engine may hide the cursor and hold it where it was
    /// pressed: then the mouse's movement moves the pointer from there. Once the cursor moves, the
    /// pointer is the cursor.
    pub fn point(&mut self, cursor: Option<[f32; 2]>) -> [f32; 2] {
        let moved = core::mem::take(&mut self.moved);
        if let Some(cursor) = cursor
            && (cursor[0] - self.pressed_at[0]).abs() + (cursor[1] - self.pressed_at[1]).abs() > 0.5
        {
            self.cursor_moves = true;
        }
        self.at = match cursor {
            Some(cursor) if self.cursor_moves => cursor,
            _ => [self.at[0] + moved[0], self.at[1] + moved[1]],
        };
        self.at
    }

    /// Where the drag started.
    pub fn start(&self) -> [f32; 2] {
        self.pressed_at
    }

    /// Where the drag pointed at the last [`Self::point`].
    pub fn at(&self) -> [f32; 2] {
        self.at
    }

    /// Whether the cursor has moved since the drag started (else the engine holds it still).
    pub fn cursor_moves(&self) -> bool {
        self.cursor_moves
    }

    /// Whether the mouse moved while the engine held the cursor still: then the pointer shows.
    pub fn held_still(&self) -> bool {
        !self.cursor_moves && self.at != self.pressed_at
    }
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
