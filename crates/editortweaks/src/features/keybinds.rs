//! The mod's keys, rebindable in the game's own input settings (category "Editor tweaks", by
//! [`Section`]): each [`Keybind`] is an input action of the game, so the settings list it,
//! rebind it (also to a key with modifier keys, like Ctrl + R) and reset it with the others, and
//! the game saves its keys with its own (`Input.ini`). Default keys may need modifier keys too
//! (`Ctrl+RightMouseButton`). Keys the mod can't read (gamepad buttons, the mouse wheel) never count
//! as held. With the mouse buttons swapped in the editor (`click_swap`), a key bound to one is held
//! with the other.
//!
//! How: after the game loads its input tables (`UBrickAssetManager::LoadAssetLibraries`, hooked),
//! a table of ours with one row per keybind joins them, in a top category of ours (listed right
//! after the editor's) with a subcategory per section (`game::input_table`). The player's input
//! then binds them like the game's actions, and [`Keybind::is_held`] reads its bindings
//! (`game::player_input`) and the keyboard (`platform::keyboard`: only while the game's window is
//! focused, and keys that type text only if the viewport got them, so not while typing in a text
//! box; see [`key_delivered`]). If any of that isn't found (e.g. after a game update), the keybinds
//! keep their default keys.
//!
//! Some keybinds are also entries of the mod's menu in the editor's top bar ("Editor tweaks", see
//! `editor_menu`): [`Keybind::menu`]. Their rows are clickable, and [`menu_category`] is the menu.
//!
//! Default keys are `FKey` names, which Unreal derives from Windows' virtual keys: the top row's
//! digits (`One`..) are the same physical keys on every layout (on AZERTY, the one typing `&`),
//! letters follow the layout like the game's own (a letter's key is where the layout puts it).
//!
//! The mod is released: to change a keybind's default key, give it a new action name too (e.g.
//! `EditorTweaks_DragSelect2`), so the binding players have saved under the old name doesn't keep
//! the old key. Otherwise never rename an action: players would lose their binding.

use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::click_swap;
use crate::framework::{hook, timings};
use crate::game::input_table::{self, Action, InputTables, LoadAssetLibrariesFn};
use crate::game::kismet;
use crate::game::player_input::{self, Chord, PlayerInputs};
use crate::game::reflection;
use crate::game::text::Text;
use crate::platform::keyboard;
use crate::prelude::*;

/// Held while dragging over the 3D view in the editor's default mode, draws a box: the bricks in it
/// get selected when it's let go (`drag_select`). On the left or right mouse button, the game's own
/// action on it (selecting, moving the camera) doesn't run while it's held.
pub static DRAG_SELECT: Keybind = Keybind::new(
    "EditorTweaks_DragSelect",
    "Drag select",
    "Hold and drag in the editor to draw a box: the bricks in it get selected when you let go. Which bricks count is a setting (Editor tweaks options, Selection).",
    &["Ctrl+RightMouseButton"],
);

/// Held when a [`DRAG_SELECT`] box is let go, adds its bricks to the selection instead of replacing
/// it (`drag_select`).
pub static ADD_TO_DRAG_SELECTION: Keybind = Keybind::new(
    "EditorTweaks_DragSelectAdd",
    "Add to drag selection",
    "Hold when letting go of a drag select box to add its bricks to the selection instead of replacing it.",
    &["LeftShift", "RightShift"],
);

/// Shows the box of the brick under the mouse while held, or while toggled on by a tap of it (see
/// `brick_resize::is_fast_editing`); clicking then resizes it, or moves it while moving bricks with
/// the gizmo showing (`brick_resize`).
pub static RESIZE_BRICKS: Keybind = Keybind::new(
    "EditorTweaks_ResizeBricks",
    "Fast editing",
    "Hold to show the box of a brick under the mouse. Use this box to move or resize it. Can be a key to tap instead (a setting: Editor tweaks options, Fast editing).",
    &["LeftAlt", "RightAlt"],
);

/// While held with [`RESIZE_BRICKS`], bricks' models replace their boxes: what the mouse points at,
/// drags and snaps to are the faces, edges and vertices of the models (`brick_resize`).
pub static MODEL_EDITING: Keybind = Keybind::new(
    "EditorTweaks_ModelEditing",
    "Use models",
    "Hold while fast editing to use the faces, edges and vertices of bricks' models instead of their boxes (for spinners, their model's facets instead of their own corners, lines and sides). On the middle mouse button, a quick click of it copies looks instead.",
    &["MiddleMouseButton", "ThumbMouseButton", "ThumbMouseButton2"],
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

/// Pressed while fast editing (the resize key held) over a spinner, switches its cross-section to
/// the next variant of its shape: inward, outward, upward (`brick_resize`). R also turns snapped
/// moves ([`NEXT_ORIENTATION`]), which only exist while moving bricks.
pub static NEXT_SPINNER_VARIANT: Keybind = Keybind::new(
    "EditorTweaks_NextSpinnerVariant",
    "Next spinner variant",
    "Press while fast editing over a spinner to switch its cross-section to the next variant of its shape (inward, outward, upward). One step to undo; its mirror mode counterpart changes too.",
    &["R"],
);

/// Like [`NEXT_SPINNER_VARIANT`], but switches to the next shape: rectangle, triangle, isosceles
/// triangle, circle, half circle, quarter circle, diamond (`brick_resize`). Takes the press when
/// both keys are held, its key including [`NEXT_SPINNER_VARIANT`]'s.
pub static NEXT_SPINNER_SHAPE: Keybind = Keybind::new(
    "EditorTweaks_NextSpinnerShape",
    "Next spinner shape",
    "Press while fast editing over a spinner to switch its cross-section to the next shape (rectangle, triangle, isosceles triangle, circle, half circle, quarter circle, diamond), keeping its variant where the shape has it. One step to undo; its mirror mode counterpart changes too.",
    &["Ctrl+R"],
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

/// Pressed, opens a box to type an exact length (`brick_resize`): the size of a brick's face hovered
/// while fast editing or just dragged, or while moving bricks the line's length (the moved bricks
/// slide along it). Enter commits, Escape cancels.
pub static TYPE_LENGTH: Keybind = Keybind::new(
    "EditorTweaks_TypeLength",
    "Type length",
    "Press to type an exact length, then Enter (Escape cancels; math like the sliders', x is the length now; start with + or - to add or take away): the size of the brick's face under the mouse while fast editing, or of the face just resized (for a few seconds); while fast editing moved bricks, the length of the line placed when one of its points is on the moved bricks (they slide along it), or right after turning them about the line, the angle they turned (in degrees).",
    &["Enter"],
);

/// Clicked while fast editing (the resize key held) on a brick, gives its looks (color, material,
/// pattern and the pattern's scale, rotation and offset) to the selected bricks and the bricks made
/// next (`copy_properties`).
pub static COPY_LOOKS: Keybind = Keybind::new(
    "EditorTweaks_CopyLooks",
    "Copy brick looks",
    "Click while fast editing to copy the color, material, pattern and pattern scale, rotation and offset of the brick under the mouse onto the selected bricks (one step to undo); new bricks get them too. Not when the left or right mouse button is pressed before letting go of it (a drag), the mouse moves, or it is held longer than a moment.",
    &["MiddleMouseButton"],
);

/// Like [`COPY_LOOKS`], but gives every brick property it can, each to the selected bricks having
/// it (`copy_properties`). Takes the click when both keys are held.
pub static COPY_PROPERTIES: Keybind = Keybind::new(
    "EditorTweaks_CopyProperties",
    "Copy brick properties",
    "Click while fast editing to copy every property of the brick under the mouse that the selected bricks have (looks, size, wheel and axle settings, input channels...) onto them (one step to undo); not names. New bricks get its looks too. Not when the left or right mouse button is pressed before letting go of it (a drag), the mouse moves, or it is held longer than a moment.",
    &["Ctrl+MiddleMouseButton"],
);

/// Slows down mouse moves of bricks while held, not the camera's (`move_slowdown`).
pub static SLOW_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowMoves",
    "Slow",
    "Hold to move and rotate bricks slower with the mouse",
    &["LeftControl", "RightControl"],
);

/// Slows down moves by a gizmo handle a lot while held (`move_slowdown`).
pub static SLOW_GIZMO_MOVES: Keybind = Keybind::new(
    "EditorTweaks_SlowGizmoMoves",
    "Super slow",
    "Hold to move and rotate bricks by a gizmo handle much slower. Tip: combine slow & super slow for ultra slow!",
    &["LeftShift", "RightShift"],
);

/// Switches the editor's view between perspective and orthographic, sized by the zoom
/// (`ortho_view`).
pub static ORTHO_VIEW: Keybind = Keybind::new(
    "EditorTweaks_OrthoView",
    "Orthographic view",
    "Press in the editor to switch between the normal view and an orthographic one (no perspective, like a field of view of 0). The zoom sets how wide it is.",
    &["One"],
);

/// Held while the editor's camera is moved (its right or middle mouse button held), the mouse pans
/// the view rather than turning it, in the orthographic view only (`ortho_view`).
pub static PAN_VIEW: Keybind = Keybind::new(
    "EditorTweaks_PanView",
    "Pan view",
    "Hold while moving the camera in the orthographic view (right mouse button) to pan the view: the mouse moves the camera sideways and up or down instead of turning it.",
    &["Shift+RightMouseButton"],
);

/// Shows or hides the map around the vehicle in the editor: sets the "Show map" row of the editor's
/// panel (`map_in_editor`) to None and back.
pub static SHOW_MAP: Keybind = Keybind::new(
    "EditorTweaks_ShowMap",
    "Show map",
    "Press in the editor to show the map around the vehicle, placed as it will be when the vehicle is spawned at the garage last chosen in the editor's panel (Show map, with nothing selected); if none was, the garage the editor was opened from, else the first. Press again to hide it.",
    &["Zero"],
);

/// Shows or hides every reference image and model in the editor, like the "Show references" row of
/// the editor's panel (`references`).
pub static SHOW_REFERENCES: Keybind = Keybind::new(
    "EditorTweaks_ShowReferences",
    "Show references",
    "Press in the editor to hide every reference image and model, and again to show them. Hidden this way, they come back as they were (unlike opacity 0).",
    &["Two"],
);

/// Flips the "Disable lighting in the editor" setting (`editor_lighting`).
pub static LIGHTING: Keybind = Keybind::new(
    "EditorTweaks_Lighting",
    "Toggle lighting",
    "Press in the editor to turn the editor's lighting off (everything fully bright, no shadows), and again to turn it back on (the same as the Disable lighting in the editor setting).",
    &["Three"],
);

/// Keeps a dragged slider's value changing while held (`slider_drag`).
pub static KEEP_DRAGGING_SLIDERS: Keybind = Keybind::new(
    "EditorTweaks_KeepDraggingSliders",
    "Value scrubbing",
    "When held, slider values change continuously instead of going to mouse position.",
    &["LeftAlt", "RightAlt"],
);

/// Shows more in the shot preview's tooltips while held (`gun_preview`).
pub static EXTENDED_TOOLTIP: Keybind = Keybind::new(
    "EditorTweaks_ExtendedTooltip",
    "Extended tooltip information",
    "Hold to show everything the shot preview's tooltips can tell, instead of the essentials.",
    &["LeftShift", "RightShift"],
);

/// Opens the dialog writing the vehicle open in the editor as a 3D model (`model_export`). An entry
/// of the editor's "Editor tweaks" menu; no key by default.
pub static EXPORT_MODEL: Keybind = Keybind::menu(
    "EditorTweaks_ExportModel",
    "Export 3D model",
    "Press in the editor (or pick it in the editor's Editor tweaks menu) to save the vehicle as a 3D model (.glb, .obj or .stl) with its colors and materials, in the Exports folder next to the game's saved vehicles. Opens a dialog to choose the bricks, the detail, collision instead and the file format.",
    &[],
    "/Game/BrickRigs/UI/Textures/Icons/T_SaveFileAs.T_SaveFileAs",
);

/// Opens the coop page (`coop::ui`).
pub static COOP_MENU: Keybind = Keybind::menu(
    "EditorTweaks_Coop",
    "Coop...",
    "Press in the editor (or pick it in the editor's Editor tweaks menu) to open the coop page: host this vehicle, join a friend hosting, answer who asks to join, leave.",
    &[],
    "/Game/BrickRigs/UI/Textures/Icons/T_Dummy.T_Dummy",
);

/// Puts the editor's camera where the selected camera brick sees from (`camera_view`). An entry of
/// the editor's "Editor tweaks" menu, and of its bottom bar while a camera brick is selected; no key
/// by default.
pub static CAMERA_VIEW: Keybind = Keybind::menu(
    "EditorTweaks_CameraView",
    "Go to camera view",
    "Press in the editor with a camera brick selected (or click it in the editor's bottom bar) to see from that camera: its place, angle and zoom. Only your view changes.",
    &[],
    "/Game/BrickRigs/UI/Textures/Icons/T_Camera.T_Camera",
);

/// The Tools section's keybinds (with coop's).
const TOOLS: &[&Keybind] = &[&EXPORT_MODEL, &CAMERA_VIEW, &COOP_MENU];

/// Every keybind, by section, in the order the settings list them. The sections are the
/// changelog's headers (those having keys).
static SECTIONS: [Section; 6] = [
    Section {
        name: "IC_EditorTweaks_FastEditing",
        title: "Fast editing",
        keybinds: &[
            &RESIZE_BRICKS,
            &SNAP_RESIZE,
            &MODEL_EDITING,
            &LINE_POINT,
            &NEXT_ORIENTATION,
            &NEXT_SPINNER_VARIANT,
            &NEXT_SPINNER_SHAPE,
            &SLIDE_ALONG_LINE,
            &CLEAR_LINE,
            &TYPE_LENGTH,
            &COPY_LOOKS,
            &COPY_PROPERTIES,
        ],
    },
    Section {
        name: "IC_EditorTweaks_UserInterface",
        title: "User interface",
        keybinds: &[&KEEP_DRAGGING_SLIDERS, &EXTENDED_TOOLTIP],
    },
    Section {
        name: "IC_EditorTweaks_Building",
        title: "Classic building & movement",
        keybinds: &[&SLOW_MOVES, &SLOW_GIZMO_MOVES, &ORTHO_VIEW, &PAN_VIEW],
    },
    Section {
        name: "IC_EditorTweaks_Selection",
        title: "Selection",
        keybinds: &[&DRAG_SELECT, &ADD_TO_DRAG_SELECTION],
    },
    Section {
        name: "IC_EditorTweaks_References",
        title: "References",
        keybinds: &[&SHOW_MAP, &SHOW_REFERENCES, &LIGHTING],
    },
    Section {
        name: "IC_EditorTweaks_Tools",
        title: "Tools",
        keybinds: TOOLS,
    },
];

/// Keybinds shown together in the input settings, under `title`: a subcategory of ours.
struct Section {
    /// Its object's name.
    name: &'static str,
    title: &'static str,
    keybinds: &'static [&'static Keybind],
}

/// The keybinds of the editor menu, in its order.
pub fn menu_keybinds() -> impl Iterator<Item = &'static Keybind> {
    all().filter(|keybind| keybind.menu_icon.is_some())
}

/// The editor menu's category, once made (see [`menu_keybinds`]).
pub fn menu_category() -> Option<*mut UObjectBase> {
    match *OURS.lock().unwrap_or_else(PoisonError::into_inner) {
        Ours::Made { menu, .. } => Some(menu),
        _ => None,
    }
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
/// The editor menu's category: its name in the top bar, its object's and its icon's path. Not one of
/// the settings' categories.
const MENU_LABEL: &str = "Editor tweaks";
const MENU_NAME: &str = "IC_EditorTweaks_Menu";
const MENU_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_VehicleEditor.T_VehicleEditor";

/// A key of the mod, which players can rebind in the game's input settings.
pub struct Keybind {
    /// The action's name: its row's in the input tables, which the game saves bindings under
    /// (`Input.ini`). Never rename: rebound keys would be lost.
    name: &'static str,
    /// What the input settings call it, and its description there.
    label: &'static str,
    description: &'static str,
    /// Its default keys: `FKey` names, after the modifier keys each needs (see [`DefaultKey`]).
    keys: &'static [&'static str],
    /// For an entry of the editor menu: its icon's path there.
    menu_icon: Option<&'static str>,
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
            menu_icon: None,
            action: OnceLock::new(),
        }
    }

    /// A keybind that's also an entry of the editor menu, shown there with the texture at the path
    /// `icon`.
    const fn menu(
        name: &'static str,
        label: &'static str,
        description: &'static str,
        keys: &'static [&'static str],
        icon: &'static str,
    ) -> Self {
        Self {
            name,
            label,
            description,
            keys,
            menu_icon: Some(icon),
            action: OnceLock::new(),
        }
    }

    /// Its action's name in the game's input tables, once it's there.
    pub fn action(&self) -> Option<FName> {
        self.action.get().copied()
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
                .map(|key| DefaultKey::parse(key))
                .any(|key| modifiers_held(&key.modifiers) && key_held(key.key)),
        }
    }

    /// Whether a press of the key Unreal calls `name`, with the modifier keys held now, is a press
    /// of it (others may be held too). By its default keys if the bindings can't be read. Game
    /// thread only.
    pub fn is_pressed_by(&self, name: &str) -> bool {
        let pressed_by = |key: &str, modifiers: &Modifiers| {
            click_swap::pressed_for(key) == name && modifiers_held(modifiers)
        };
        let bound = (|| {
            let action = *self.action.get()?;
            let players = PLAYERS.get()?;
            // SAFETY: on the game thread, per the contract; the input is the local player's.
            unsafe {
                let input = players.local()?;
                let mut pressed = false;
                players.chords(input, action, |chord| {
                    let modifiers = Modifiers {
                        shift: chord.shift,
                        ctrl: chord.ctrl,
                        alt: chord.alt,
                        cmd: chord.cmd,
                    };
                    pressed |= pressed_by(&chord.key.to_string(), &modifiers);
                });
                Some(pressed)
            }
        })();
        bound.unwrap_or_else(|| {
            self.keys
                .iter()
                .map(|key| DefaultKey::parse(key))
                .any(|key| pressed_by(key.key, &key.modifiers))
        })
    }

    /// The key it's bound to, for people: its first binding, e.g. "Shift" or "Ctrl+Q" (its first
    /// default key if the bindings can't be read). Game thread only.
    pub fn key_label(&self) -> String {
        self.first_bound().unwrap_or_else(|| {
            let key = DefaultKey::parse(self.keys.first().copied().unwrap_or_default());
            chord_label(key.key, &key.modifiers)
        })
    }

    /// [`Self::key_label`] of its first binding, if the bindings can be read.
    fn first_bound(&self) -> Option<String> {
        let action = *self.action.get()?;
        let players = PLAYERS.get()?;
        // SAFETY: on the game thread, per `key_label`'s contract; the input is the local player's.
        unsafe {
            let input = players.local()?;
            let mut first = None;
            players.chords(input, action, |chord| {
                if first.is_none() {
                    let modifiers = Modifiers {
                        shift: chord.shift,
                        ctrl: chord.ctrl,
                        alt: chord.alt,
                        cmd: chord.cmd,
                    };
                    first = Some(chord_label(&chord.key.to_string(), &modifiers));
                }
            });
            first
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

/// Tells the keyboard state that the game's viewport got `event` for `key`, a key of the player's
/// input (called by the hooked `UPlayerInput::InputKey`): keys a focused text box takes never get
/// here, so they don't count as held (see `platform::keyboard`).
///
/// # Safety
/// `key` must be a valid name (of the live name pool).
pub unsafe fn key_delivered(key: &FName, event: i32) {
    let down = match event {
        player_input::PRESSED | player_input::REPEAT | player_input::DOUBLE_CLICK => true,
        player_input::RELEASED => false,
        // Axes (the mouse's movement, ...): no key.
        _ => return,
    };
    // SAFETY: per the contract.
    let name = unsafe { reflection::name_text(key) };
    if let Some(key) = keyboard::virtual_key(&name) {
        keyboard::key_delivered(key, down);
    }
}

/// Whether `chord`'s key and the modifier keys it needs are held.
fn is_chord_held(chord: &Chord) -> bool {
    let modifiers = Modifiers {
        shift: chord.shift,
        ctrl: chord.ctrl,
        alt: chord.alt,
        cmd: chord.cmd,
    };
    modifiers_held(&modifiers) && key_held(&chord.key.to_string())
}

/// A key Unreal calls `key` with the modifier keys `modifiers`, for people: "Ctrl+Left Mouse
/// Button", "Shift" (either side's).
fn chord_label(key: &str, modifiers: &Modifiers) -> String {
    let mut label = String::new();
    for (needed, name) in [
        (modifiers.ctrl, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        (modifiers.cmd, "Win"),
    ] {
        if needed {
            label += name;
            label.push('+');
        }
    }
    let side = key
        .strip_prefix("Left")
        .or_else(|| key.strip_prefix("Right"));
    let name = match side.unwrap_or(key) {
        "Shift" => "Shift",
        "Control" => "Ctrl",
        "Alt" => "Alt",
        "Command" => "Win",
        "SpaceBar" => "Space",
        _ => "",
    };
    if name.is_empty() {
        // Words apart: "RightMouseButton" is "Right Mouse Button".
        for (index, letter) in key.chars().enumerate() {
            if index > 0 && letter.is_uppercase() && !label.ends_with(' ') {
                label.push(' ');
            }
            label.push(letter);
        }
    } else {
        label += name;
    }
    label
}

/// Whether the modifier keys `needed` are held (others may be too).
fn modifiers_held(needed: &Modifiers) -> bool {
    (!needed.shift || keyboard::is_shift_down())
        && (!needed.ctrl || keyboard::is_control_down())
        && (!needed.alt || keyboard::is_alt_down())
        && (!needed.cmd || keyboard::is_command_down())
}

/// Whether the key Unreal calls `name` is held: with the mouse buttons swapped, a mouse button is the
/// other one.
fn key_held(name: &str) -> bool {
    let key = click_swap::pressed_for(name);
    keyboard::is_key_down(key).unwrap_or_else(|| {
        LOG_UNREADABLE_KEY.call_once(|| {
            log!("can't tell whether {key} is held: not a key of the keyboard or mouse")
        });
        false
    })
}

/// Modifier keys a key needs.
#[derive(Default)]
struct Modifiers {
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// The Windows key.
    cmd: bool,
}

/// A default key as [`Keybind::new`] takes it: an `FKey` name after the modifier keys it needs,
/// e.g. `Ctrl+RightMouseButton` (`Shift+`, `Ctrl+`, `Alt+`, `Cmd+`).
struct DefaultKey<'a> {
    key: &'a str,
    modifiers: Modifiers,
}

impl<'a> DefaultKey<'a> {
    fn parse(text: &'a str) -> Self {
        let mut parsed = Self {
            key: text,
            modifiers: Modifiers::default(),
        };
        while let Some((modifier, rest)) = parsed.key.split_once('+') {
            let flag = match modifier {
                "Shift" => &mut parsed.modifiers.shift,
                "Ctrl" => &mut parsed.modifiers.ctrl,
                "Alt" => &mut parsed.modifiers.alt,
                "Cmd" => &mut parsed.modifiers.cmd,
                _ => break,
            };
            *flag = true;
            parsed.key = rest;
        }
        parsed
    }

    /// As the input tables take it (after engine init: names are made).
    fn chord(&self) -> Result<Chord, String> {
        Ok(Chord {
            key: kismet::string_to_name(self.key)?,
            shift: self.modifiers.shift,
            ctrl: self.modifiers.ctrl,
            alt: self.modifiers.alt,
            cmd: self.modifiers.cmd,
        })
    }
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
        /// The editor menu's category.
        menu: *mut UObjectBase,
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
    let _span = timings::span("keybinds::load_asset_libraries");
    let Some(original) = ORIGINAL_LOAD.get() else {
        return;
    };
    // SAFETY: forwards the caller's arguments.
    unsafe {
        timings::game(input_table::LOAD_ASSET_LIBRARIES.name, || {
            original(manager, mode)
        })
    };
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
        if let Ours::Made {
            table, category, ..
        } = *ours
        {
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
                None,
                core::ptr::null_mut(),
                0,
            )
            .map_err(MakeError::Retry)?;
        let menu = tables
            .new_category(
                manager,
                prepared.menu.name,
                &prepared.menu.label,
                Some(prepared.menu_icon),
                core::ptr::null_mut(),
                0,
            )
            .map_err(MakeError::Final)?;
        let table = tables
            .new_table(manager, prepared.table_name)
            .map_err(MakeError::Final)?;
        for (place, (section, prepared)) in SECTIONS.iter().zip(&prepared.sections).enumerate() {
            let subcategory = tables
                .new_category(
                    manager,
                    prepared.category.name,
                    &prepared.category.label,
                    None,
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
                    menu_icon: action.menu_icon,
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
        Ok(Ours::Made {
            table,
            category,
            menu,
        })
    }
}

/// The names and texts our objects and rows need.
struct Prepared {
    table_name: FName,
    category: PreparedCategory,
    menu: PreparedCategory,
    menu_icon: FName,
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
    keys: Vec<Chord>,
    menu_icon: Option<FName>,
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
        menu: prepare_category(MENU_NAME, MENU_LABEL)?,
        menu_icon: kismet::string_to_name(MENU_ICON)?,
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
            .map(|key| DefaultKey::parse(key).chord())
            .collect::<Result<_, String>>()?,
        menu_icon: keybind.menu_icon.map(kismet::string_to_name).transpose()?,
    })
}
