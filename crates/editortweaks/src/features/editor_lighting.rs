//! Lighting off in the editor, a setting ([`NO_LIGHTING`]) that the key [`keybinds::LIGHTING`]
//! flips. Shadows and unlit sides can hide the shape of what you build, and cost frames.
//!
//! How: while the editor is open, the game viewport draws in the engine's unlit view mode
//! (`game::view_mode`): every surface shows its plain colour, with no light, shading, shadow or
//! tint. When the editor closes ([`restore`]: when the game leaves the editor or loads a map) the
//! lit view comes back. Purely visual: the vehicle and how the game treats it don't change.
//!
//! The panel only reads its rows when it lists them, so the key has it list them again
//! (`game::editor_panel`): the setting's row then shows what the key did.

use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use super::keybinds;
use super::settings_list;
use crate::framework::settings::{Setting, Toggle};
use crate::game::brick_editor;
use crate::game::editor_panel::{self, EditorPanel};
use crate::game::view_mode::{self, ViewMode, ViewModes};
use crate::game::weak_object::WeakObject;
use crate::game::world;
use crate::prelude::*;

/// Whether the editor draws no lighting, a row of the editor's side panel (at the end of its
/// "Editor settings") rather than of the options menu: it is flipped often while building.
pub static NO_LIGHTING: Toggle = Toggle::new("NoEditorLighting", "Disable lighting", "", false);

/// What the side panel asks the settings list for, with nothing selected.
const EDITOR_SETTINGS: &str = "Properties.Editor";

pub struct EditorLighting;

static VIEW_MODES: OnceLock<ViewModes> = OnceLock::new();
/// The editor's side panel, to show what the key changed; without it, the row catches up when the
/// panel next lists it.
static PANELS: OnceLock<EditorPanel> = OnceLock::new();

struct State {
    /// Whether the editor switched to the unlit view.
    unlit: bool,
    /// Whether the key was held at the last tick.
    key_held: bool,
    /// The side panel last found, and the editor's address it's for: finding it looks through
    /// every object.
    panel: Option<(usize, WeakObject)>,
}

// SAFETY: a weak object reference, only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State {
    unlit: false,
    key_held: false,
    panel: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Feature for EditorLighting {
    fn id(&self) -> &'static str {
        "editor_lighting"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        let started = view_mode::find().and_then(|view_modes| {
            let row = settings_list::setting_row(Setting::Toggle(&NO_LIGHTING))?;
            let _ = VIEW_MODES.set(view_modes);
            match editor_panel::find() {
                Ok(panels) => drop(PANELS.set(panels)),
                Err(reason) => log!("the key's change shows in the panel only later: {reason}"),
            }
            // Untitled: the row ends the game's "Editor settings".
            settings_list::add(EDITOR_SETTINGS, vec![("", vec![row])])
        });
        match started {
            Ok(()) => log!("active: lighting can be turned off in the editor"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }

    // A new map: the view may still be unlit if the editor closed without notice.
    fn on_map_load(&self) {
        // SAFETY: on the game thread, after a map load.
        unsafe { restore(world::current()) };
    }
}

/// Flips the setting on a press of the key, and keeps the view as the setting says.
fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(view_modes) = VIEW_MODES.get() else {
        return;
    };
    let held = keybinds::LIGHTING.is_held();
    let pressed = {
        let mut state = state();
        let pressed = held && !state.key_held;
        state.key_held = held;
        pressed
    };
    if pressed {
        NO_LIGHTING.set(!NO_LIGHTING.get());
        settings_list::save();
    }
    // SAFETY: `editor` is the live editor ticking, on the game thread.
    unsafe { follow_setting(view_modes, editor) };
    if pressed {
        // After the setting took effect: a refusal puts it back off.
        // SAFETY: as above, holding no lock (the panel reads the setting's row).
        unsafe { show_in_panel(editor) };
    }
}

/// Keeps the view as the setting says.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn follow_setting(view_modes: &ViewModes, editor: *mut UObjectBase) {
    let unlit = state().unlit;
    if NO_LIGHTING.get() == unlit {
        return;
    }
    if unlit {
        // SAFETY: per the contract.
        unsafe { restore(editor) };
        return;
    }
    // SAFETY: per the contract.
    match unsafe { view_modes.set(editor, ViewMode::Unlit) } {
        Ok(()) => state().unlit = true,
        Err(reason) => {
            log!("lighting not turned off: {reason}");
            // Not retried every frame: the setting goes back off.
            NO_LIGHTING.set(false);
        }
    }
}

/// Has the side panel list its rows again, so the setting's row shows what the key did.
///
/// # Safety
/// `editor` must be the live editor. Game thread only, holding no lock of this feature.
unsafe fn show_in_panel(editor: *mut UObjectBase) {
    let Some(panels) = PANELS.get() else {
        return;
    };
    let known = state()
        .panel
        .filter(|(of, _)| *of == editor as usize)
        .and_then(|(_, panel)| panel.get());
    // SAFETY: per the contract; the panel found is live.
    unsafe {
        let panel = known.or_else(|| panels.of(editor));
        state().panel = panel.map(|panel| (editor as usize, WeakObject::new(panel)));
        if let Some(panel) = panel {
            panels.refresh(panel);
        }
    }
}

/// Puts the lit view back, if the editor switched to the unlit one. `context` is any live object
/// of the world (null: nothing happens, and the view stays unlit until the next call).
///
/// # Safety
/// `context` must be null or a live object. Game thread only.
pub unsafe fn restore(context: *mut UObjectBase) {
    let Some(view_modes) = VIEW_MODES.get() else {
        return;
    };
    if context.is_null() || !state().unlit {
        return;
    }
    // SAFETY: per the contract.
    match unsafe { view_modes.set(context, ViewMode::Lit) } {
        Ok(()) => state().unlit = false,
        Err(reason) => log!("lighting not put back: {reason}"),
    }
}
