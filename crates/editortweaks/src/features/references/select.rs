//! Selecting references with the mouse, like bricks, and the editor's keys acting on the selected
//! ones: Delete removes them, Duplicate (Ctrl + C) makes copies beside them, Hide (H) hides them
//! (they look like hidden bricks and can't be clicked) until Unhide all (Alt + H), which the game
//! then gets too, for its bricks.
//!
//! References and bricks are never selected together. A click (the `Select` button, through
//! `features/editor_buttons`) whose ray meets a reference before any brick selects that reference
//! alone: the press is taken from the game, and the release unselects the bricks the way a click
//! on nothing does. With Shift (`AddToSelection`) it adds the reference to the selected ones, or
//! takes it out if it's selected already. Any other click, or bricks getting selected some other
//! way (seen at the next tick), unselects the references (a Shift click on nothing leaves them).
//! Clicks only pick references while the setting [`SELECTABLE`] is on, and only in the editor's
//! default mode (not while moving bricks); leaving that mode unselects them, as does the editor's
//! Cancel key, which then does nothing else.

use brickrust::ue::coreuobject::UObjectBase;
use model_io::pick::Ray;

use super::{CODE, State, edit, state};
use crate::features::editor_buttons::{self, Button, Click, Press};
use crate::framework::settings::Toggle;
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::editor_mode::{self, EditorModes, Selector};
use crate::game::editor_object::{self, Selection, Transform};
use crate::game::editor_view::{self, Bricks};
use crate::game::line_trace::{self, LineTrace};
use crate::game::reflection::{self, Getter};
use crate::game::viewport::{self, Viewport};
use crate::math::{self, Quat};
use crate::prelude::*;

/// Whether clicks select references.
pub static SELECTABLE: Toggle = Toggle::new(
    "ReferencesSelectable",
    "Select references",
    "Clicking a reference image or model selects it, to change it in the side panel, delete it (Delete), duplicate it (Ctrl + C) or hide it (H, until Alt + H). Shift + click adds one to the selection or takes it out. Off: clicks go through references to the bricks behind them.",
    true,
);

/// How far a click looks for bricks in front of a reference, in cm.
const REACH: f32 = 1.0e6;

/// How far a duplicate goes from its original, beyond the original's width, in cm.
const DUPLICATE_GAP: f64 = 30.0;

/// What clicking needs from the game (after engine init).
pub(super) struct Input {
    modes: EditorModes,
    pub(super) editor: BrickEditor,
    viewport: Viewport,
    selector: Selector,
    trace: LineTrace,
    bricks: Bricks,
    /// `None` if not found (logged): bricks selected otherwise than by a click keep the reference
    /// selected too.
    selection: Option<Selection>,
    /// `FTransform USceneComponent::K2_GetComponentToWorld() const`.
    to_world: Getter<Transform>,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Input {}
unsafe impl Sync for Input {}

pub(super) fn find() -> Result<Input, String> {
    let scene = reflection::find_class("SceneComponent").ok_or("class SceneComponent not found")?;
    Ok(Input {
        modes: editor_mode::find()?,
        editor: brick_editor::find()?,
        viewport: viewport::find()?,
        selector: editor_mode::selector()?,
        trace: line_trace::find()?,
        bricks: editor_view::bricks()?,
        selection: editor_object::selection()
            .map_err(|reason| {
                log!("selecting bricks by a box or a key keeps the reference selected: {reason}")
            })
            .ok(),
        to_world: Getter::find(scene, "K2_GetComponentToWorld", "StructProperty")?,
    })
}

/// Takes the editor's clicks and keys from now on.
pub(super) fn listen() -> Result<(), String> {
    editor_buttons::listen(on_button)
}

/// A click or key of the editor: whether it's taken from the game.
fn on_button(press: &Press) -> bool {
    let Some(code) = CODE.get() else {
        return false;
    };
    let Some(input) = code.input.as_ref() else {
        return false;
    };
    let Some(mut state) = state() else {
        return false;
    };
    // SAFETY: an input handler's call, on the game thread; its input component is live.
    unsafe {
        let Some(editor) = input.modes.editor_of(press.input) else {
            return false;
        };
        match (press.button, press.click) {
            (Button::Select, Click::Pressed) => press_select(input, &mut state, editor, false),
            (Button::AddToSelection, Click::Pressed) => {
                press_select(input, &mut state, editor, true)
            }
            (Button::Select | Button::AddToSelection, Click::Released) => {
                release_select(input, &mut state, editor)
            }
            (Button::Delete, Click::Pressed) => delete_selected(&mut state),
            (Button::Duplicate, Click::Pressed) => duplicate_selected(&mut state),
            (Button::Hide, Click::Pressed) => state.hide_selected(),
            (Button::UnhideAll, Click::Pressed) => {
                state.unhide_all();
                false
            }
            (Button::Cancel, Click::Pressed) => {
                state.pressed = None;
                let any = !state.selected.is_empty();
                state.select(Vec::new());
                any
            }
            _ => false,
        }
    }
}

/// A press of `Select` (or `AddToSelection`, `add`): taken if it's on a reference (selected at the
/// release); else the game handles it, and without `add` the references are unselected.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn press_select(
    input: &Input,
    state: &mut State,
    editor: *mut UObjectBase,
    add: bool,
) -> bool {
    state.pressed = None;
    // SAFETY: per the contract.
    let picked = unsafe { picked(input, state, editor) };
    match picked {
        Some(id) => {
            state.pressed = Some((id, add));
            true
        }
        None => {
            if !add {
                state.select(Vec::new());
            }
            false
        }
    }
}

/// A release after a press on a reference: selects it, alone or added to (or taken out of) the
/// references selected, unselecting the bricks.
///
/// # Safety
/// As for [`press_select`].
unsafe fn release_select(input: &Input, state: &mut State, editor: *mut UObjectBase) -> bool {
    let Some((id, add)) = state.pressed.take() else {
        return false;
    };
    // SAFETY: per the contract.
    let bricks = unsafe { bricks_selected(input, editor) };
    // SAFETY: per the contract.
    unsafe {
        if let Some(mode) = input.modes.default_mode(editor) {
            input.selector.select(mode, &[], false);
        }
    }
    if add && !bricks {
        state.toggle(id);
    } else {
        state.select(vec![id]);
    }
    // Unselecting the bricks made the game show its panel for no selection, even when the same
    // references stay selected (a click on a selected one), which `select` doesn't refresh for.
    state.refresh_panel = true;
    true
}

/// The reference under the mouse, if clicks select references now and no brick is in front of it.
///
/// # Safety
/// As for [`press_select`].
unsafe fn picked(input: &Input, state: &State, editor: *mut UObjectBase) -> Option<String> {
    if !SELECTABLE.get() {
        return None;
    }
    // SAFETY: per the contract; the controller is live if not null.
    unsafe {
        input.modes.default_mode(editor)?;
        let controller = input.editor.player_controller(editor);
        if controller.is_null() {
            return None;
        }
        let mouse = input.viewport.mouse(controller)?;
        let ray = input.viewport.ray(controller, mouse)?;
        let local = root_ray(input, editor, ray.origin, ray.direction)?;
        let (id, along) = state.scene.pick(&local)?;
        let end = math::narrow(math::add(
            math::widen(ray.origin),
            math::scale(math::widen(ray.direction), f64::from(REACH)),
        ));
        let brick = input
            .trace
            .first_hit(editor, ray.origin, end)
            .filter(|hit| hit.is_brick());
        // The ray's direction is a unit vector, and the root isn't scaled: `along` is in cm.
        if brick.is_some_and(|hit| f64::from(hit.distance) < along) {
            return None;
        }
        Some(id.to_owned())
    }
}

/// The ray from `origin` along `direction` (in the world) in the space of `editor`'s root, where
/// references are placed.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
pub(super) unsafe fn root_ray(
    input: &Input,
    editor: *mut UObjectBase,
    origin: [f32; 3],
    direction: [f32; 3],
) -> Option<Ray> {
    let code = CODE.get()?;
    // SAFETY: per the contract; the root is live if not null.
    unsafe {
        let root = code.root.call(editor);
        if root.is_null() {
            return None;
        }
        to_local(&input.to_world.call(root), origin, direction)
    }
}

/// The ray from `origin` along `direction` (in the world) in the space of a component whose
/// transform to the world is `transform`.
fn to_local(transform: &Transform, origin: [f32; 3], direction: [f32; 3]) -> Option<Ray> {
    let unturn = Quat::widen(transform.rotation).inverse();
    let scale = math::widen([transform.scale[0], transform.scale[1], transform.scale[2]]);
    if scale.contains(&0.0) {
        return None;
    }
    let unscale = |vector: math::Vec3| -> math::Vec3 {
        core::array::from_fn(|axis| vector[axis] / scale[axis])
    };
    let translation = math::widen([
        transform.translation[0],
        transform.translation[1],
        transform.translation[2],
    ]);
    Some(Ray {
        origin: unscale(unturn.rotate(math::sub(math::widen(origin), translation))),
        direction: unscale(unturn.rotate(math::widen(direction))),
    })
}

/// Delete: removes the selected references, if there are any.
fn delete_selected(state: &mut State) -> bool {
    if state.selected.is_empty() {
        return false;
    }
    // The session's references are the host's: taken, doing nothing.
    if state.received.is_some() {
        return true;
    }
    let folder = state.folder.clone();
    for id in core::mem::take(&mut state.selected) {
        if let Err(reason) = edit::remove(&mut state.sidecar, folder.as_deref(), &id) {
            log!("removing {id}: {reason}");
        }
    }
    state.refresh_panel = true;
    state.save();
    true
}

/// Duplicate: copies of the selected references beside them (to their right, past their width),
/// then selected instead.
fn duplicate_selected(state: &mut State) -> bool {
    if state.selected.is_empty() {
        return false;
    }
    if state.received.is_some() {
        return true;
    }
    let offset = state.scene.width_of(&state.selected).unwrap_or(0.0) + DUPLICATE_GAP;
    let mut copies = Vec::new();
    for id in &state.selected {
        let Some(original) = state
            .sidecar
            .references
            .iter()
            .find(|reference| reference.id == *id)
        else {
            continue;
        };
        let mut copy = original.clone();
        copy.id = state.sidecar.new_id();
        // A copied file is copied again for the duplicate when saving.
        copy.source.copy = None;
        copy.position[1] += offset;
        log!("duplicated {} as {}", original.name, copy.id);
        copies.push(copy.id.clone());
        state.sidecar.references.push(copy);
    }
    state.select(copies);
    state.save();
    true
}

/// Whether references can stay selected in `editor`: it's in its default mode, no brick selected.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
pub(super) unsafe fn keeps_references(input: &Input, editor: *mut UObjectBase) -> bool {
    // SAFETY: per the contract.
    unsafe { input.modes.default_mode(editor).is_some() && !bricks_selected(input, editor) }
}

/// Whether any brick of `editor` is selected (`false` if that can't be told).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn bricks_selected(input: &Input, editor: *mut UObjectBase) -> bool {
    let Some(selection) = &input.selection else {
        return false;
    };
    // SAFETY: per the contract; the editor's objects are live.
    unsafe {
        input
            .bricks
            .editor_objects(editor)
            .iter()
            .any(|&object| selection.is_selected(object))
    }
}
