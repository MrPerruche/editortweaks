//! The editor's mouse button actions, shared by the features that take them over. Not a feature
//! itself: the loader hooks a function only once, so the features can't each hook the handlers.
//!
//! The left button is the game's `Select` action and the right one its `MoveCamera` action (as
//! bound; swapped by `click_swap`), handled by the editor input's reflected `Action_Select` and
//! `Action_MoveCamera` (`game::editor_input::ButtonAction`), called on press and on release. Their
//! exec thunks are hooked here on the first [`listen`]; each press or release goes to the listeners
//! in the order they listened, until one takes it: then the game's handler doesn't run.

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::game::editor_input::{self, ButtonAction};
use crate::game::reflection::NativeThunk;

pub use crate::game::editor_input::Click;

/// Which button's action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// `Select`: the left button.
    Select,
    /// `MoveCamera`: the right button.
    MoveCamera,
}

/// A press or release of a button, as the editor gets it.
#[derive(Clone, Copy)]
pub struct Press {
    pub button: Button,
    pub click: Click,
    /// The editor input component handling it (its outer is the editor).
    pub input: *const UObjectBase,
}

/// Whether the listener takes the press or release: then the game's handler, and the listeners
/// after it, don't get it.
pub type Listener = fn(&Press) -> bool;

struct Actions {
    select: ButtonAction,
    move_camera: ButtonAction,
}

static ACTIONS: OnceLock<Actions> = OnceLock::new();
static ORIGINAL_SELECT: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_MOVE_CAMERA: OnceLock<NativeThunk> = OnceLock::new();
/// Whether the hooks are in place, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// Calls `listener` with every press and release of the editor's buttons from now on, after the
/// listeners before it (after engine init: the handlers are found through reflection).
pub fn listen(listener: Listener) -> Result<(), String> {
    HOOKED.get_or_init(hook_handlers).clone()?;
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(listener);
    Ok(())
}

fn hook_handlers() -> Result<(), String> {
    let actions = Actions {
        select: editor_input::button_action(editor_input::SELECT)?,
        move_camera: editor_input::button_action(editor_input::MOVE_CAMERA)?,
    };
    let (select, move_camera) = (actions.select.thunk, actions.move_camera.thunk);
    let _ = ACTIONS.set(actions);
    // SAFETY: both targets are exec thunks, and the detours are NativeThunks.
    unsafe {
        hook::install_typed(
            "UBaseEditorInputComponent::execAction_Select",
            select,
            select_detour as NativeThunk,
            &ORIGINAL_SELECT,
        )?;
        hook::install_typed(
            "UEditorInputComponent::execAction_MoveCamera",
            move_camera,
            move_camera_detour as NativeThunk,
            &ORIGINAL_MOVE_CAMERA,
        )
    }
}

unsafe extern "C" fn select_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *ORIGINAL_SELECT.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::Select, context, stack, result, original) }
}

unsafe extern "C" fn move_camera_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_MOVE_CAMERA
        .get()
        .expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::MoveCamera, context, stack, result, original) }
}

/// Runs `original`, the thunk of `button`'s action called with `context`, `stack` and `result`,
/// unless a listener takes the call.
///
/// # Safety
/// `context`, `stack` and `result` must be the arguments `original` was called with, on the game
/// thread.
unsafe fn handle(
    button: Button,
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
    original: NativeThunk,
) {
    // SAFETY: per the contract.
    unsafe {
        if let Some(actions) = ACTIONS.get() {
            let action = match button {
                Button::Select => &actions.select,
                Button::MoveCamera => &actions.move_camera,
            };
            if action.is_editor_input(context)
                && let Some(click) = action.click(stack)
            {
                let press = Press {
                    button,
                    click,
                    input: context.cast_const().cast(),
                };
                // Copied out, so that a listener may listen too without a deadlock.
                let listeners = LISTENERS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if listeners.iter().any(|listener| listener(&press)) {
                    action.handled(result);
                    return;
                }
            }
        }
        original(context, stack, result);
    }
}
