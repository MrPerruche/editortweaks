//! The editor's mouse button actions, and its keys acting on the selection (Delete, Duplicate, Hide,
//! Unhide all), shared by the features that take them over. Not a feature itself: the loader hooks
//! a function only once, so the features can't each hook the handlers.
//!
//! The left button is the game's `Select` action and the right one its `MoveCamera` action (as
//! bound; swapped by `click_swap`), handled by the editor input's reflected `Action_Select` and
//! `Action_MoveCamera` (`game::editor_input::ButtonAction`), called on press and on release; Shift
//! + the left button is `AddToSelection` instead of `Select`, the middle button `PivotCamera`. The
//! keys are `Action_DeleteSelection` (Delete), `Action_DuplicateSelection` (Ctrl + C),
//! `Action_HideSelection` (H) and `Action_UnhideAll` (Alt + H), handlers of the same kind. Their
//! exec thunks are hooked here on the first [`listen`] (`AddToSelection`, `PivotCamera` and the
//! keys only if found: the left and right buttons work without them); each press or release goes
//! to the listeners in the order they listened, until one takes it: then the game's handler doesn't
//! run.

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::game::editor_input::{self, ButtonAction};
use crate::game::reflection::NativeThunk;
use crate::prelude::log;

pub use crate::game::editor_input::Click;

/// Which button's action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// `Select`: the left button.
    Select,
    /// `AddToSelection`: Shift + the left button.
    AddToSelection,
    /// `MoveCamera`: the right button.
    MoveCamera,
    /// `PivotCamera`: the middle button (and Space).
    PivotCamera,
    /// `DeleteSelection`: Delete.
    Delete,
    /// `DuplicateSelection`: Ctrl + C.
    Duplicate,
    /// `HideSelection`: H.
    Hide,
    /// `UnhideAll`: Alt + H.
    UnhideAll,
}

impl Button {
    /// Whether it's one of the keys acting on the selection, not a mouse button.
    pub fn is_key(self) -> bool {
        matches!(
            self,
            Button::Delete | Button::Duplicate | Button::Hide | Button::UnhideAll
        )
    }
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
    /// `None` if not found (logged).
    add_to_selection: Option<ButtonAction>,
    pivot_camera: Option<ButtonAction>,
    delete: Option<ButtonAction>,
    duplicate: Option<ButtonAction>,
    hide: Option<ButtonAction>,
    unhide_all: Option<ButtonAction>,
}

static ACTIONS: OnceLock<Actions> = OnceLock::new();
static ORIGINAL_SELECT: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_ADD_TO_SELECTION: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_MOVE_CAMERA: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_PIVOT_CAMERA: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_DELETE: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_DUPLICATE: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_HIDE: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_UNHIDE_ALL: OnceLock<NativeThunk> = OnceLock::new();
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
    let optional = |name| {
        editor_input::button_action(name)
            .map_err(|reason| log!("{name} not taken over: {reason}"))
            .ok()
    };
    let actions = Actions {
        select: editor_input::button_action(editor_input::SELECT)?,
        move_camera: editor_input::button_action(editor_input::MOVE_CAMERA)?,
        add_to_selection: optional(editor_input::ADD_TO_SELECTION),
        pivot_camera: optional(editor_input::PIVOT_CAMERA),
        delete: optional(editor_input::DELETE_SELECTION),
        duplicate: optional(editor_input::DUPLICATE_SELECTION),
        hide: optional(editor_input::HIDE_SELECTION),
        unhide_all: optional(editor_input::UNHIDE_ALL),
    };
    let (select, move_camera) = (actions.select.thunk, actions.move_camera.thunk);
    let add_to_selection = actions.add_to_selection.as_ref().map(|action| action.thunk);
    let pivot_camera = actions.pivot_camera.as_ref().map(|action| action.thunk);
    let delete = actions.delete.as_ref().map(|action| action.thunk);
    let duplicate = actions.duplicate.as_ref().map(|action| action.thunk);
    let hide = actions.hide.as_ref().map(|action| action.thunk);
    let unhide_all = actions.unhide_all.as_ref().map(|action| action.thunk);
    let _ = ACTIONS.set(actions);
    // SAFETY: every target is an exec thunk, and the detours are NativeThunks.
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
        )?;
        if let Some(add_to_selection) = add_to_selection
            && let Err(reason) = hook::install_typed(
                "execAction_AddToSelection",
                add_to_selection,
                add_to_selection_detour as NativeThunk,
                &ORIGINAL_ADD_TO_SELECTION,
            )
        {
            log!("AddToSelection not taken over: {reason}");
        }
        if let Some(pivot_camera) = pivot_camera
            && let Err(reason) = hook::install_typed(
                "UEditorInputComponent::execAction_PivotCamera",
                pivot_camera,
                pivot_camera_detour as NativeThunk,
                &ORIGINAL_PIVOT_CAMERA,
            )
        {
            log!("PivotCamera not taken over: {reason}");
        }
        if let Some(delete) = delete
            && let Err(reason) = hook::install_typed(
                "execAction_DeleteSelection",
                delete,
                delete_detour as NativeThunk,
                &ORIGINAL_DELETE,
            )
        {
            log!("Delete not taken over: {reason}");
        }
        if let Some(duplicate) = duplicate
            && let Err(reason) = hook::install_typed(
                "execAction_DuplicateSelection",
                duplicate,
                duplicate_detour as NativeThunk,
                &ORIGINAL_DUPLICATE,
            )
        {
            log!("Duplicate not taken over: {reason}");
        }
        if let Some(hide) = hide
            && let Err(reason) = hook::install_typed(
                "execAction_HideSelection",
                hide,
                hide_detour as NativeThunk,
                &ORIGINAL_HIDE,
            )
        {
            log!("Hide not taken over: {reason}");
        }
        if let Some(unhide_all) = unhide_all
            && let Err(reason) = hook::install_typed(
                "execAction_UnhideAll",
                unhide_all,
                unhide_all_detour as NativeThunk,
                &ORIGINAL_UNHIDE_ALL,
            )
        {
            log!("Unhide all not taken over: {reason}");
        }
    }
    Ok(())
}

unsafe extern "C" fn select_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *ORIGINAL_SELECT.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::Select, context, stack, result, original) }
}

unsafe extern "C" fn add_to_selection_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_ADD_TO_SELECTION
        .get()
        .expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::AddToSelection, context, stack, result, original) }
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

unsafe extern "C" fn pivot_camera_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_PIVOT_CAMERA
        .get()
        .expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::PivotCamera, context, stack, result, original) }
}

unsafe extern "C" fn delete_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *ORIGINAL_DELETE.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::Delete, context, stack, result, original) }
}

unsafe extern "C" fn duplicate_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_DUPLICATE.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::Duplicate, context, stack, result, original) }
}

unsafe extern "C" fn hide_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *ORIGINAL_HIDE.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::Hide, context, stack, result, original) }
}

unsafe extern "C" fn unhide_all_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_UNHIDE_ALL.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { handle(Button::UnhideAll, context, stack, result, original) }
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
                Button::Select => Some(&actions.select),
                Button::AddToSelection => actions.add_to_selection.as_ref(),
                Button::MoveCamera => Some(&actions.move_camera),
                Button::PivotCamera => actions.pivot_camera.as_ref(),
                Button::Delete => actions.delete.as_ref(),
                Button::Duplicate => actions.duplicate.as_ref(),
                Button::Hide => actions.hide.as_ref(),
                Button::UnhideAll => actions.unhide_all.as_ref(),
            };
            if let Some(action) = action
                && action.is_editor_input(context)
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
