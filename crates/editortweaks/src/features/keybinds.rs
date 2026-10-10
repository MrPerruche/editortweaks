//! The mod's keys, rebindable in the game's own input settings (the editor's category, section
//! "Editor tweaks"): each [`Keybind`] is an input action of the game, so the settings list it,
//! rebind it (also to a key with modifier keys, like Ctrl + R) and reset it with the others, and
//! the game saves its keys with its own (`Input.ini`). Keys the mod can't read (gamepad buttons,
//! the mouse wheel) never count as held.
//!
//! How: after the game loads its input tables (`UBrickAssetManager::LoadAssetLibraries`, hooked),
//! a table of ours with one row per keybind joins them, in a category of ours inside the editor's
//! (`game::input_table`). The player's input then binds them like the game's actions, and
//! [`Keybind::is_held`] reads its bindings (`game::player_input`) and the keyboard. If any of that
//! isn't found (e.g. after a game update), the keybinds keep their default keys.

use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use crate::framework::hook;
use crate::game::input_table::{self, Action, InputTables, LoadAssetLibrariesFn};
use crate::game::kismet;
use crate::game::player_input::{self, Chord, PlayerInputs};
use crate::game::text::Text;
use crate::platform::keyboard;
use crate::prelude::*;

/// Shows the box of the brick under the mouse while held; clicking then resizes it
/// (`brick_resize`).
pub static RESIZE_BRICKS: Keybind = Keybind::new(
    "EditorTweaks_ResizeBricks",
    "Fast editing",
    "Hold to show the box of a brick under the mouse. Use this box to move or resize it.",
    &["LeftAlt", "RightAlt"],
);

/// Snaps the dragged part to other bricks while held during a resize (`brick_resize`).
pub static SNAP_RESIZE: Keybind = Keybind::new(
    "EditorTweaks_SnapResize",
    "Snap resizing",
    "Hold while fast editing to snap a brick to faces, edges and corners.",
    &["LeftControl", "RightControl"],
);

/// Slows down mouse moves of bricks and of the camera while held (`move_slowdown`).
pub static SLOW_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowMoves",
    "Slow movement",
    "Hold to move and rotate bricks and turn the camera slower with the mouse",
    &["LeftControl", "RightControl"],
);

/// Slows down moves by a gizmo handle a lot while held (`move_slowdown`).
pub static SLOW_GIZMO_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowGizmoMoves",
    "Super slow movement",
    "Hold to move and rotate bricks by a gizmo handle much slower. Tip: combine slow & super slow for ultra slow!",
    &["LeftShift", "RightShift"],
);

/// Keeps a dragged slider's value changing while held (`slider_drag`).
pub static KEEP_DRAGGING_SLIDERS: Keybind = Keybind::new(
    "EditorTweaks_KeepDraggingSliders",
    "Value scrubbing",
    "When held, slider values change continuously instead of going to mouse position.",
    &["LeftAlt", "RightAlt"],
);

/// Every keybind, in the order the settings list them.
static ALL: [&Keybind; 5] = [
    &RESIZE_BRICKS,
    &SNAP_RESIZE,
    &SLOW_MOVES,
    &SLOW_GIZMO_MOVES,
    &KEEP_DRAGGING_SLIDERS,
];

/// The game's top category our section goes in (the editor's bindings).
const EDITOR_CATEGORY: &str = "IC_BrickEditor";
/// Our section's name in the settings, and its object's.
const CATEGORY_LABEL: &str = "Editor tweaks";
const CATEGORY_NAME: &str = "IC_EditorTweaks";
const TABLE_NAME: &str = "EditorTweaksInputTable";

/// A key of the mod, which players can rebind in the game's input settings.
pub struct Keybind {
    /// The action's name: its row's in the input tables, which the game saves bindings under
    /// (`Input.ini`). Never rename: rebound keys would be lost.
    name: &'static str,
    /// What the input settings call it, and its description there.
    label: &'static str,
    description: &'static str,
    /// Its default keys (`FKey` names), each alone.
    keys: &'static [&'static str],
    /// The action's name, once in the game's input tables.
    action: OnceLock<FName>,
}

impl Keybind {
    const fn new(
        name: &'static str,
        label: &'static str,
        description: &'static str,
        keys: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            label,
            description,
            keys,
            action: OnceLock::new(),
        }
    }

    /// Whether it's held right now: one of the keys it's bound to in the game's input settings,
    /// with the modifier keys that binding needs (others may be held too). Its default keys if the
    /// bindings can't be read. Game thread only (ticks, input handlers, timers).
    pub fn is_held(&self) -> bool {
        match self.bound_held() {
            Some(held) => held,
            None => self
                .keys
                .iter()
                .any(|key| keyboard::is_key_down(key) == Some(true)),
        }
    }

    /// Whether a key it's bound to is held, if the bindings can be read.
    fn bound_held(&self) -> Option<bool> {
        let action = *self.action.get()?;
        let players = PLAYERS.get()?;
        // SAFETY: on the game thread, per `is_held`'s contract; the input is the local player's.
        unsafe {
            let input = players.local()?;
            let mut held = false;
            players.chords(input, action, |chord| held |= is_chord_held(&chord));
            Some(held)
        }
    }
}

/// Whether `chord`'s key and the modifier keys it needs are held.
fn is_chord_held(chord: &Chord) -> bool {
    let modifiers = (!chord.shift || keyboard::is_shift_down())
        && (!chord.ctrl || keyboard::is_control_down())
        && (!chord.alt || keyboard::is_alt_down())
        && (!chord.cmd || keyboard::is_command_down());
    if !modifiers {
        return false;
    }
    let key = chord.key.to_string();
    keyboard::is_key_down(&key).unwrap_or_else(|| {
        LOG_UNREADABLE_KEY.call_once(|| {
            log!("can't tell whether {key} is held: not a key of the keyboard or mouse")
        });
        false
    })
}

pub struct Keybinds;

static ORIGINAL_LOAD: OnceLock<LoadAssetLibrariesFn> = OnceLock::new();
/// Found at the first table load (reflection works from there on).
static TABLES: OnceLock<InputTables> = OnceLock::new();
/// Where the bindings are read from (after engine init).
static PLAYERS: OnceLock<PlayerInputs> = OnceLock::new();
static OURS: Mutex<Ours> = Mutex::new(Ours::NotYet);
static LOG_UNREADABLE_KEY: Once = Once::new();

/// Our table and category, made once (the game keeps loaded tables).
#[derive(Clone, Copy)]
enum Ours {
    NotYet,
    Made {
        table: *mut UObjectBase,
        /// Ours, a top category (to list) if the editor's wasn't found.
        top_category: Option<*mut UObjectBase>,
    },
    /// Making them failed halfway: not tried again, the objects' names are taken.
    Failed,
}

// SAFETY: objects kept for the whole game (rooted); only used on the game thread.
unsafe impl Send for Ours {}

impl Feature for Keybinds {
    fn id(&self) -> &'static str {
        "keybinds"
    }

    fn on_load(&self) -> LoadResult {
        let target = input_table::LOAD_ASSET_LIBRARIES.resolve()?;
        // SAFETY: `target` is the start of LoadAssetLibraries, which `load_asset_libraries`
        // matches.
        unsafe {
            hook::install_typed(
                input_table::LOAD_ASSET_LIBRARIES.name,
                target,
                load_asset_libraries as LoadAssetLibrariesFn,
                &ORIGINAL_LOAD,
            )
        }
    }

    fn on_engine_init(&self) {
        match player_input::find() {
            Ok(players) => {
                let _ = PLAYERS.set(players);
            }
            Err(reason) => log!("rebound keys can't be read, the default keys apply: {reason}"),
        }
        let listed = ALL.iter().all(|keybind| keybind.action.get().is_some());
        log!(
            "active{}",
            if listed {
                ""
            } else {
                " (not in the input settings yet)"
            }
        );
    }
}

unsafe extern "C" fn load_asset_libraries(manager: *mut UObjectBase, mode: u8) {
    let Some(original) = ORIGINAL_LOAD.get() else {
        return;
    };
    // SAFETY: forwards the caller's arguments.
    unsafe { original(manager, mode) };
    // SAFETY: the asset manager the game just filled, on the game thread.
    if let Err(reason) = unsafe { add_ours(manager) } {
        log!("keybinds not in the input settings: {reason}");
    }
}

/// Adds our table (and category, if a top one) to the lists the game just loaded, making them
/// the first time.
///
/// # Safety
/// `manager` must be the live asset manager, its lists just loaded. Game thread only.
unsafe fn add_ours(manager: *mut UObjectBase) -> Result<(), String> {
    let tables = match TABLES.get() {
        Some(tables) => tables,
        None => {
            let found = InputTables::find()?;
            TABLES.get_or_init(|| found)
        }
    };
    // SAFETY: per the contract.
    unsafe {
        if tables.tables(manager).num <= 0 {
            // Unloaded (at shutdown).
            return Ok(());
        }
        let mut ours = OURS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ours::NotYet = *ours {
            *ours = match make(tables, manager) {
                Ok(made) => made,
                Err(MakeError::Retry(reason)) => return Err(reason),
                Err(MakeError::Final(reason)) => {
                    *ours = Ours::Failed;
                    return Err(reason);
                }
            };
        }
        if let Ours::Made {
            table,
            top_category,
        } = *ours
        {
            input_table::add_once(tables.tables(manager), table);
            if let Some(category) = top_category {
                input_table::add_once(tables.categories(manager), category);
            }
        }
    }
    Ok(())
}

enum MakeError {
    /// Nothing made yet: try again at the next load.
    Retry(String),
    /// Objects made, but not finished.
    Final(String),
}

/// Our table, its rows and their category.
///
/// # Safety
/// As for [`add_ours`].
unsafe fn make(tables: &InputTables, manager: *mut UObjectBase) -> Result<Ours, MakeError> {
    // Everything that can fail without leaving anything behind first.
    let prepared = prepare().map_err(MakeError::Retry)?;
    // SAFETY: per the contract; the names are new (made once), the category's parent is null or a
    // live category from the game's list.
    unsafe {
        let parent = input_table::find_named(tables.categories(manager), EDITOR_CATEGORY)
            .unwrap_or(core::ptr::null_mut());
        let category = tables
            .new_category(
                manager,
                prepared.category_name,
                &prepared.category_label,
                parent,
            )
            .map_err(MakeError::Retry)?;
        let table = tables
            .new_table(manager, prepared.table_name)
            .map_err(MakeError::Final)?;
        for (order, (keybind, action)) in ALL.iter().zip(&prepared.actions).enumerate() {
            let row = Action {
                name: action.name,
                label: &action.label,
                description: &action.description,
                keys: &action.keys,
                order: order as i32,
            };
            tables
                .add_row(table, &row, category)
                .map_err(|reason| MakeError::Final(format!("{}: {reason}", keybind.name)))?;
            let _ = keybind.action.set(action.name);
        }
        log!(
            "{} keybinds added to the input settings ({})",
            ALL.len(),
            if parent.is_null() {
                "own category: the editor's wasn't found"
            } else {
                "in the editor's category"
            }
        );
        Ok(Ours::Made {
            table,
            top_category: parent.is_null().then_some(category),
        })
    }
}

/// The names and texts our objects and rows need.
struct Prepared {
    table_name: FName,
    category_name: FName,
    category_label: Text,
    actions: Vec<PreparedAction>,
}

struct PreparedAction {
    name: FName,
    label: Text,
    description: Text,
    keys: Vec<FName>,
}

fn prepare() -> Result<Prepared, String> {
    let actions = ALL
        .iter()
        .map(|keybind| {
            Ok(PreparedAction {
                name: kismet::string_to_name(keybind.name)?,
                label: kismet::string_to_text(keybind.label)?,
                description: kismet::string_to_text(keybind.description)?,
                keys: keybind
                    .keys
                    .iter()
                    .map(|key| kismet::string_to_name(key))
                    .collect::<Result<_, String>>()?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Prepared {
        table_name: kismet::string_to_name(TABLE_NAME)?,
        category_name: kismet::string_to_name(CATEGORY_NAME)?,
        category_label: kismet::string_to_text(CATEGORY_LABEL)?,
        actions,
    })
}
