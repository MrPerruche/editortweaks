//! Copying a brick's properties while fast editing: with [`keybinds::RESIZE_BRICKS`] held, a click
//! on a brick (the nearest under the mouse, `brick_resize::nearest_hovered_brick`: fast editing
//! itself points at selected bricks behind others first) gives the selected bricks its properties,
//! as one step to undo:
//!
//! - [`keybinds::COPY_LOOKS`] (the middle mouse button by default): its looks, the color, material,
//!   pattern and the pattern's scale, rotation and offset ([`LOOKS`]).
//! - [`keybinds::COPY_PROPERTIES`] (Ctrl + the middle mouse button by default): every brick
//!   property it can (see [`State::brick_properties`]), each to the selected bricks having it: a
//!   wheel's size goes to wheels only, its color to every brick.
//!
//! New bricks get the properties `UBrick` declares (the looks, and whether it generates lift), as
//! after editing each of them on that brick in the panel (`game::new_brick_properties`). With
//! mirror mode on, counterparts change only if selected too.
//!
//! A click: the key goes down with the fast editing key held, in the editor's default mode (not
//! while moving or placing bricks), over a brick, and comes up again with neither the left nor the
//! right mouse button pressed meanwhile (fast editing's drags, which the key may take part in: it
//! may be the model key too), and the mouse moved at most [`CLICK_PIXELS`] from where it went down.
//! While fast editing, `brick_resize` keeps the middle button from pivoting the camera, so the
//! cursor stays free.
//!
//! How: the editor's tick (hooked) watches the keys, after `brick_resize`'s. The selected bricks
//! are changed like the panel does (`game::brick_property`): in one editor operation, each value
//! that differs written and the brick told, then the operation marked
//! [`brick_editor::PROPERTY_CHANGED`].

use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use super::{brick_resize, keybinds};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_property::{self, ChangedEvent};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{self, Selection};
use crate::game::editor_view::{self, Bricks};
use crate::game::new_brick_properties::{self, NewBrickProperties};
use crate::game::property_copy::CopiedProperty;
use crate::game::reflection;
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;
use crate::platform::keyboard;
use crate::prelude::*;

/// The brick properties [`keybinds::COPY_LOOKS`] copies, all declared by `UBrick`.
const LOOKS: [&str; 6] = [
    "BrickColor",
    "BrickMaterial",
    "BrickPattern",
    "PatternScale",
    "PatternRotation",
    "PatternOffset",
];

/// How far the mouse may move from where the key went down for it to be a click (pixels).
const CLICK_PIXELS: f32 = 4.0;

pub struct CopyProperties;

/// The game code this feature uses (after engine init).
struct Game {
    editor: BrickEditor,
    modes: EditorModes,
    viewport: Viewport,
    properties: NewBrickProperties,
    bricks: Bricks,
    selection: Selection,
    /// `Brick`.
    brick: *const UStruct,
    /// The [`LOOKS`] bricks have.
    looks: Vec<CopiedProperty>,
    /// Where bricks keep their `IBrickPropertyInterface`, once found on one.
    interface: OnceLock<usize>,
}

// SAFETY: game code and reflection records living as long as the game; only used on the game
// thread.
unsafe impl Send for Game {}
unsafe impl Sync for Game {}

static GAME: OnceLock<Game> = OnceLock::new();
static KEY: Mutex<Key> = Mutex::new(Key::Up);
static LOG_NO_INTERFACE: Once = Once::new();

/// What a click copies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Copied {
    /// The [`LOOKS`] ([`keybinds::COPY_LOOKS`]).
    Looks,
    /// Every brick property it can ([`keybinds::COPY_PROPERTIES`]).
    Everything,
}

/// The copy keys between ticks.
#[derive(Clone, Copy)]
enum Key {
    Up,
    /// Down for a click on `brick` copying `copied`, the mouse at `at` (screen pixels) when it went
    /// down.
    Down {
        brick: WeakObject,
        copied: Copied,
        at: [f32; 2],
    },
    /// Down, but no click (see [`State::tick`]): nothing until they're up again.
    Ignored,
}

// SAFETY: the brick is only used on the game thread; the mutex just satisfies statics.
unsafe impl Send for Key {}

impl Feature for CopyProperties {
    fn id(&self) -> &'static str {
        "copy_properties"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!(
                "active: click a brick with a copy key (middle mouse button, Ctrl + it for every property) while fast editing"
            ),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let brick = reflection::find_class("Brick").ok_or("class Brick not found")?;
    let looks: Vec<CopiedProperty> = LOOKS
        .iter()
        .filter_map(|name| {
            // SAFETY: a live, linked class.
            unsafe { CopiedProperty::find(brick.cast(), name) }
                .map_err(|reason| log!("{name} isn't copied: {reason}"))
                .ok()
        })
        .collect();
    if looks.is_empty() {
        return Err("bricks have none of the looks copied".into());
    }
    let game = Game {
        editor: brick_editor::find()?,
        modes: editor_mode::find()?,
        viewport: viewport::find()?,
        properties: new_brick_properties::find()?,
        bricks: editor_view::bricks()?,
        selection: editor_object::selection()?,
        brick: brick.cast(),
        looks,
        interface: OnceLock::new(),
    };
    let _ = GAME.set(game);
    Ok(())
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(game) = GAME.get() else {
        return;
    };
    let mut key = KEY.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: `editor` is the live editor ticking, on the game thread.
    unsafe { State { game, editor }.tick(&mut key) };
}

/// One tick of the editor.
struct State<'a> {
    game: &'a Game,
    /// The live editor ticking.
    editor: *mut UObjectBase,
}

impl State<'_> {
    /// Follows the keys, and copies the properties of the brick clicked when they come up after a
    /// click. Both keys count as one: Ctrl + the middle mouse button also holds the middle mouse
    /// button, and letting go of Ctrl first still ends the click as one copying everything.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn tick(&self, key: &mut Key) {
        // SAFETY: per the contract, and `editor` is live.
        unsafe {
            if !self.game.properties.is_vehicle_editor(self.editor) {
                return;
            }
            let held = keybinds::COPY_LOOKS.is_held() || keybinds::COPY_PROPERTIES.is_held();
            let mouse = self.mouse();
            *key = match (*key, held) {
                (Key::Up, false) => Key::Up,
                (Key::Up, true) => self.press(mouse),
                (Key::Down { brick, copied, at }, held) => {
                    if !self.clicking(at, mouse) {
                        Key::Ignored
                    } else if held {
                        Key::Down { brick, copied, at }
                    } else {
                        if let Some(brick) = brick.get_live() {
                            self.copy(brick, copied);
                        }
                        Key::Up
                    }
                }
                (Key::Ignored, true) => Key::Ignored,
                (Key::Ignored, false) => Key::Up,
            };
        }
    }

    /// A key went down with the mouse at `mouse`: a click starts if fast editing points at a brick
    /// in the editor's default mode, with no other mouse button held. It copies everything if
    /// [`keybinds::COPY_PROPERTIES`] is held, its keys including [`keybinds::COPY_LOOKS`]'.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press(&self, mouse: Option<[f32; 2]>) -> Key {
        // SAFETY: per the contract; `nearest_hovered_brick` returns a live object.
        unsafe {
            if !keybinds::RESIZE_BRICKS.is_held() || !self.can_click() {
                return Key::Ignored;
            }
            let copied = if keybinds::COPY_PROPERTIES.is_held() {
                Copied::Everything
            } else {
                Copied::Looks
            };
            match (mouse, brick_resize::nearest_hovered_brick()) {
                (Some(at), Some(brick)) if self.game.properties.is_brick(brick) => Key::Down {
                    brick: WeakObject::new(brick),
                    copied,
                    at,
                },
                _ => Key::Ignored,
            }
        }
    }

    /// Whether a click that started with the mouse at `at` still is one, the mouse at `mouse` now.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn clicking(&self, at: [f32; 2], mouse: Option<[f32; 2]>) -> bool {
        let still = mouse.is_some_and(|[x, y]| (x - at[0]).hypot(y - at[1]) <= CLICK_PIXELS);
        // SAFETY: per the contract.
        still && unsafe { self.can_click() }
    }

    /// Whether a key may click now: the editor in its default mode, no other mouse button held,
    /// nothing dragged by fast editing.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn can_click(&self) -> bool {
        // SAFETY: per the contract, and `editor` is live.
        let editing = unsafe { self.game.modes.is_editing(self.editor) };
        editing && !keyboard::is_mouse_button_down() && !brick_resize::is_dragging()
    }

    /// Where the mouse is on the screen (pixels), if over the view.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn mouse(&self) -> Option<[f32; 2]> {
        // SAFETY: per the contract, and `editor` is live; so is its controller if not null.
        unsafe {
            let controller = self.game.editor.player_controller(self.editor);
            if controller.is_null() {
                return None;
            }
            self.game.viewport.mouse(controller)
        }
    }

    /// Gives the selected bricks `brick`'s `copied` properties, as one step to undo, and makes the
    /// editor give those `UBrick` declares to new bricks.
    ///
    /// # Safety
    /// `brick` must be a live brick of the editor. Game thread only.
    unsafe fn copy(&self, brick: *mut UObjectBase, copied: Copied) {
        let game = self.game;
        // SAFETY: per the contract; the editor's objects are live, and don't change while bricks'
        // properties do.
        unsafe {
            let properties = match copied {
                Copied::Looks => game.looks.clone(),
                Copied::Everything => self.brick_properties(brick),
            };
            let selected: Vec<*mut UObjectBase> = game
                .bricks
                .editor_objects(self.editor)
                .iter()
                .copied()
                .filter(|&object| {
                    !object.is_null()
                        && object != brick
                        && game.bricks.is_brick(object)
                        && game.selection.is_selected(object)
                })
                .collect();
            // Only bricks that change, so that copying the same again adds no step to undo.
            let changing: Vec<*mut UObjectBase> = selected
                .iter()
                .copied()
                .filter(|&target| {
                    properties.iter().any(|property| {
                        property.applies_to(target) && property.differs(brick, target)
                    })
                })
                .collect();
            if !changing.is_empty() {
                let mut changed = 0;
                game.editor
                    .operation(self.editor, brick_editor::PROPERTY_CHANGED, || {
                        changed = changing
                            .iter()
                            .filter(|&&target| self.give(brick, target, &properties))
                            .count();
                    });
                log!(
                    "gave {changed} of {} selected bricks a brick's properties",
                    selected.len()
                );
            } else if !selected.is_empty() {
                log!(
                    "the {} selected bricks already have the brick's properties",
                    selected.len()
                );
            }
            let kept = properties
                .iter()
                .filter(|property| property.owner() == game.brick)
                .filter(|property| game.properties.keep(self.editor, brick, property.name))
                .count();
            if kept == 0 {
                log!("a brick's properties couldn't be kept for new bricks: it has no weak name");
            } else {
                log!("kept a brick's properties for new bricks ({kept} properties)");
            }
        }
    }

    /// The brick properties of `brick` that can be copied (`brick_property::is_brick_property`),
    /// of any type (texts like seat names included). Logs what it leaves out.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    unsafe fn brick_properties(&self, brick: *const UObjectBase) -> Vec<CopiedProperty> {
        let mut copied = Vec::new();
        let mut left_out = Vec::new();
        // SAFETY: per the contract; a live object's class and its property records live as long
        // as it does.
        unsafe {
            for property in reflection::properties(reflection::class_of(brick.cast())) {
                if !brick_property::is_brick_property(property, self.game.brick) {
                    continue;
                }
                match CopiedProperty::of(property) {
                    Ok(property) => copied.push(property),
                    Err(reason) => left_out.push(reason),
                }
            }
        }
        if !left_out.is_empty() {
            log!(
                "{} brick properties copied; not: {}",
                copied.len(),
                left_out.join(", ")
            );
        }
        copied
    }

    /// Gives `target` the `properties` of `brick` it has whose values differ, telling it of each
    /// like the panel does. Whether it could be told (its property interface was found).
    ///
    /// # Safety
    /// `brick` and `target` must be live bricks of the editor, and `properties` some of `brick`'s.
    /// Game thread only.
    unsafe fn give(
        &self,
        brick: *mut UObjectBase,
        target: *mut UObjectBase,
        properties: &[CopiedProperty],
    ) -> bool {
        let game = self.game;
        // SAFETY: per the contract; the interface offset is checked on a brick first (any brick has
        // it at the same place: it comes from `UBrickEditorObject`).
        unsafe {
            let offset = match game.interface.get() {
                Some(&offset) => offset,
                None => {
                    let Some(offset) = brick_property::interface_offset(game.brick, target) else {
                        LOG_NO_INTERFACE.call_once(|| {
                            log!(
                                "bricks can't be given properties: their property interface wasn't found"
                            )
                        });
                        return false;
                    };
                    *game.interface.get_or_init(|| offset)
                }
            };
            let interface = target.cast::<u8>().add(offset).cast();
            for property in properties {
                if !property.applies_to(target) || !property.differs(brick, target) {
                    continue;
                }
                property.copy(brick, target);
                ChangedEvent::with_property(property.name, false, |event| {
                    brick_property::post_modify(interface, event);
                });
            }
        }
        true
    }
}
