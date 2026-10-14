//! The mod's keys, rebindable in the game's own input settings (category "Editor tweaks", by
//! [`Section`]): each [`Keybind`] is an input action of the game, so the settings list it,
//! rebind it (also to a key with modifier keys, like Ctrl + R) and reset it with the others, and
//! the game saves its keys with its own (`Input.ini`). Keys the mod can't read (gamepad buttons,
//! the mouse wheel) never count as held.
//!
//! How: after the game loads its input tables (`UBrickAssetManager::LoadAssetLibraries`, hooked),
//! a table of ours with one row per keybind joins them, in a top category of ours (listed right
//! after the editor's) with a subcategory per section (`game::input_table`). The player's input
//! then binds them like the game's actions, and [`Keybind::is_held`] reads its bindings
//! (`game::player_input`) and the keyboard. If any of that isn't found (e.g. after a game update),
//! the keybinds keep their default keys.

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

/// Shows the box of the brick under the mouse while held; clicking then resizes it, or moves it
/// while moving bricks with the gizmo showing (`brick_resize`).
pub static RESIZE_BRICKS: Keybind = Keybind::new(
    "EditorTweaks_ResizeBricks",
    "Fast editing",
    "Hold to show the box of a brick under the mouse. Use this box to move or resize it.",
    &["LeftAlt", "RightAlt"],
);

/// While held with [`RESIZE_BRICKS`], bricks' models replace their boxes: what the mouse points at,
/// drags and snaps to are the faces, edges and vertices of the models (`brick_resize`).
pub static MODEL_EDITING: Keybind = Keybind::new(
    "EditorTweaks_ModelEditing",
    "Use models",
    "Hold while fast editing to use the faces, edges and vertices of bricks' models instead of their boxes.",
    &["ThumbMouseButton"],
);

/// Snaps the dragged part to other bricks while held during a resize or move (`brick_resize`).
pub static SNAP_RESIZE: Keybind = Keybind::new(
    "EditorTweaks_SnapResize",
    "Snap",
    "Hold while fast editing to snap to other bricks: corners, middles, intersections, edges and faces. Moved bricks also turn to match faces and edges.",
    &["LeftControl", "RightControl"],
);

/// Each press while a moved brick snaps and turns to match the target turns it the next way that
/// matches (`brick_resize`).
pub static NEXT_ORIENTATION: Keybind = Keybind::new(
    "EditorTweaks_NextOrientation",
    "Next orientation",
    "Press while moving bricks with fast editing snapping to turn them the next way that matches the face or edge they snap to.",
    &["R"],
);

/// Clicked while fast editing moved bricks (the resize key held), places a point of the line they
/// turn about or slide along: two make it, one over no brick clears it. Dragged from one of them
/// with a line placed, slides them along it. On the right mouse button, the camera moves with it
/// once the resize key is let go (`brick_resize`).
pub static LINE_POINT: Keybind = Keybind::new(
    "EditorTweaks_LinePoint",
    "Line point",
    "Click while fast editing moved bricks to place a point of a line: two points make it. Dragging their box then turns them around the line, and dragging it with this key slides them along the line. Click over no brick to clear it. With the right mouse button, let go of the fast editing key to move the camera.",
    &["RightMouseButton"],
);

/// While held, dragging moved bricks with a line placed slides them along it instead of turning
/// them about it, like a drag of [`LINE_POINT`] (`brick_resize`). No key by default: Shift was,
/// but Alt + Shift switches Windows' keyboard layout.
pub static SLIDE_ALONG_LINE: Keybind = Keybind::new(
    "EditorTweaks_SlideAlongLine",
    "Slide along line",
    "Hold while fast editing moved bricks with a line placed to slide them along the line instead of turning them around it (like dragging with the line point key).",
    &[],
);

/// Pressed, clears the line placed for moving bricks about, and a first point (`brick_resize`).
/// No key by default.
pub static CLEAR_LINE: Keybind = Keybind::new(
    "EditorTweaks_ClearLine",
    "Clear line",
    "Press to clear the line placed for fast editing moved bricks.",
    &[],
);

/// Slows down mouse moves of bricks and of the camera while held (`move_slowdown`).
pub static SLOW_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowMoves",
    "Slow",
    "Hold to move and rotate bricks and turn the camera slower with the mouse",
    &["LeftControl", "RightControl"],
);

/// Slows down moves by a gizmo handle a lot while held (`move_slowdown`).
pub static SLOW_GIZMO_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowGizmoMoves",
    "Super slow",
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

/// Every keybind, by section, in the order the settings list them.
static SECTIONS: [Section; 4] = [
    Section {
        name: "IC_EditorTweaks_FastEditing",
        title: "Fast editing",
        keybinds: &[
            &RESIZE_BRICKS,
            &MODEL_EDITING,
            &SNAP_RESIZE,
            &NEXT_ORIENTATION,
        ],
    },
    Section {
        name: "IC_EditorTweaks_Lines",
        title: "Fast editing: lines",
        keybinds: &[&LINE_POINT, &SLIDE_ALONG_LINE, &CLEAR_LINE],
    },
    Section {
        name: "IC_EditorTweaks_SlowMovement",
        title: "Slow movement",
        keybinds: &[&SLOW_MOVES, &SLOW_GIZMO_MOVES],
    },
    Section {
        name: "IC_EditorTweaks_Sliders",
        title: "Sliders",
        keybinds: &[&KEEP_DRAGGING_SLIDERS],
    },
];

/// Keybinds shown together in the input settings, under `title`: a subcategory of ours.
struct Section {
    /// Its object's name.
    name: &'static str,
    title: &'static str,
    keybinds: &'static [&'static Keybind],
}

/// Every keybind.
fn all() -> impl Iterator<Item = &'static Keybind> {
    SECTIONS
        .iter()
        .flat_map(|section| section.keybinds.iter().copied())
}

/// The game's category ours is listed after (the editor's bindings).
const EDITOR_CATEGORY: &str = "IC_BrickEditor";
/// Our category's name in the settings, and its object's.
const CATEGORY_LABEL: &str = "Editor tweaks (mod)";
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

/// Our table and categories, made once (the game keeps loaded tables).
#[derive(Clone, Copy)]
enum Ours {
    NotYet,
    Made {
        table: *mut UObjectBase,
        /// Our top category, which the game's list of them needs (subcategories aren't listed).
        category: *mut UObjectBase,
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
        let listed = all().all(|keybind| keybind.action.get().is_some());
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

/// Adds our table and category to the lists the game just loaded, making them the first time.
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
        if let Ours::Made { table, category } = *ours {
            input_table::add_once(tables.tables(manager), table, None);
            let categories = tables.categories(manager);
            let editor = input_table::find_named(categories, EDITOR_CATEGORY);
            input_table::add_once(categories, category, editor);
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

/// Our table, our categories and the rows.
///
/// # Safety
/// As for [`add_ours`].
unsafe fn make(tables: &InputTables, manager: *mut UObjectBase) -> Result<Ours, MakeError> {
    // Everything that can fail without leaving anything behind first.
    let prepared = prepare().map_err(MakeError::Retry)?;
    // SAFETY: per the contract; the names are new (made once), each subcategory's parent is our
    // live top category.
    unsafe {
        let category = tables
            .new_category(
                manager,
                prepared.category.name,
                &prepared.category.label,
                core::ptr::null_mut(),
                0,
            )
            .map_err(MakeError::Retry)?;
        let table = tables
            .new_table(manager, prepared.table_name)
            .map_err(MakeError::Final)?;
        for (place, (section, prepared)) in SECTIONS.iter().zip(&prepared.sections).enumerate() {
            let subcategory = tables
                .new_category(
                    manager,
                    prepared.category.name,
                    &prepared.category.label,
                    category,
                    place as i32,
                )
                .map_err(|reason| MakeError::Final(format!("{}: {reason}", section.name)))?;
            for (order, (keybind, action)) in
                section.keybinds.iter().zip(&prepared.actions).enumerate()
            {
                let row = Action {
                    name: action.name,
                    label: &action.label,
                    description: &action.description,
                    keys: &action.keys,
                    order: order as i32,
                };
                tables
                    .add_row(table, &row, subcategory)
                    .map_err(|reason| MakeError::Final(format!("{}: {reason}", keybind.name)))?;
                let _ = keybind.action.set(action.name);
            }
        }
        log!(
            "{} keybinds added to the input settings (category {CATEGORY_LABEL}, {} sections)",
            all().count(),
            SECTIONS.len()
        );
        Ok(Ours::Made { table, category })
    }
}

/// The names and texts our objects and rows need.
struct Prepared {
    table_name: FName,
    category: PreparedCategory,
    sections: Vec<PreparedSection>,
}

struct PreparedCategory {
    name: FName,
    label: Text,
}

struct PreparedSection {
    category: PreparedCategory,
    actions: Vec<PreparedAction>,
}

struct PreparedAction {
    name: FName,
    label: Text,
    description: Text,
    keys: Vec<FName>,
}

fn prepare() -> Result<Prepared, String> {
    let sections = SECTIONS
        .iter()
        .map(|section| {
            Ok(PreparedSection {
                category: prepare_category(section.name, section.title)?,
                actions: section
                    .keybinds
                    .iter()
                    .map(|keybind| prepare_action(keybind))
                    .collect::<Result<_, String>>()?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Prepared {
        table_name: kismet::string_to_name(TABLE_NAME)?,
        category: prepare_category(CATEGORY_NAME, CATEGORY_LABEL)?,
        sections,
    })
}

fn prepare_category(name: &str, label: &str) -> Result<PreparedCategory, String> {
    Ok(PreparedCategory {
        name: kismet::string_to_name(name)?,
        label: kismet::string_to_text(label)?,
    })
}

fn prepare_action(keybind: &Keybind) -> Result<PreparedAction, String> {
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
}
