//! Resizing scalable bricks (bricks with a `BrickSize`) by dragging the faces, edges and corners of
//! their box (or of their model), in the editor (not while moving bricks):
//!
//! - hold Alt ([`keybinds::RESIZE_BRICKS`], rebindable like the keys below): the box of the brick
//!   under the mouse shows, and the part of it the mouse is on lights up: a corner, an edge or a
//!   face (see [`parts`]), also just outside the box. Selected bricks come first, then the one in
//!   front (see [`scene::Scene::pick`]); bricks that can't be resized show a dark box;
//! - Alt + drag: that part follows the mouse, the opposite one stays in place, and the brick's size
//!   follows. A face changes one size, an edge two, a corner all three in proportion. The camera
//!   stays still meanwhile;
//! - hold Ctrl ([`keybinds::SNAP_RESIZE`]) while dragging: the part snaps to the point of another
//!   brick nearest the mouse among its corners, the middles of its edges and faces, where bricks'
//!   edges cross each other or faces, and halfway between those crossings; else along the edge
//!   the mouse is near, or onto the face under it (see [`snap`], where each kind can be turned
//!   off, and [`resize::Resize::snap`]). A mark shows the kind of point, a dashed line what the
//!   part lines up with. Each right click meanwhile sets aside one more of the bricks the mouse
//!   goes through, to snap to bricks behind them (back to the front one after the last). Let go of
//!   Ctrl and it keeps snapping until the mouse moves away (see [`SnapKey`]), so letting go of it
//!   a moment before the button doesn't unsnap;
//! - releasing the button makes it one step to undo. With mirror mode on, the brick's counterpart
//!   changes along;
//! - hold the model key too ([`keybinds::MODEL_EDITING`], the thumb mouse button by default): the
//!   bricks' models replace their boxes, for what the mouse points at, drags and snaps to: their
//!   faces, edges and vertices (see [`model`]). Dragging one resizes the box like the box's part it
//!   lies on: a face moves along its normal keeping its slope (its sizes in proportion), an edge
//!   across its length, a vertex away from the opposite corner; snapped, a vertex goes onto the
//!   target, an edge and a face through it. Bricks without a model (see `game::brick_model`) keep
//!   their box.
//!
//! While moving bricks with the mouse free (the move mode's gizmo showing: G pressed twice), the
//! same keys move them instead (a prototype, see [`motion`]):
//!
//! - hold Alt: the box of the moved brick under the mouse shows, and the part the mouse is on;
//! - Alt + drag: the grabbed point follows the mouse, in the plane across the vehicle's axis the
//!   camera looks along the most, by the game's grid steps while its snapping is on. With Ctrl held
//!   when pressing, the grabbed point is the brick's own snap point nearest the mouse (a corner, an
//!   edge's middle, a face's center...);
//! - hold Ctrl while dragging: the grabbed point snaps like a resize does, to bricks not moved;
//!   onto a face from a face, or onto an edge from an edge, the bricks also turn to match it, then
//!   square up with the target brick (see [`motion::MATCHING`]): the smallest such turn, and each
//!   press of R ([`keybinds::NEXT_ORIENTATION`]) the next one (facing the target or level with it,
//!   along an edge either way, squared up each way), kept while the snap stays on the same piece.
//!   Unsnapped again, they keep that turn and slide on from where the snap put them. With mirror mode on, snapping onto a moved
//!   brick's counterpart (it moves along, mirrored) meets it halfway across the mirror plane;
//! - releasing the button leaves the move going: a click ends it as usual (one undo step), the
//!   gizmo still works meanwhile. The game's own brick snapping (to connectors) stays off for
//!   these moves (`MoveMode::keep_snapping_off`), the gizmo's drags still have it.
//!
//! Every brick is resized through its box as it reports it, so a rounded brick through the box
//! around it, except a scalable brick whose box is its size around its origin (most): that box is
//! taken from its stored size, not from the box the game computes in `f32` from its mesh (a hair
//! off, like 4.9999976 for 5). The math runs in double precision in the vehicle's space, where
//! bricks keep their places (see [`geometry`]), and only the results are rounded, to the `f32`
//! the game stores: snapping a face to one at 110 cm gives 110 cm, not 110.000545.
//!
//! How: the editor's tick (`ABrickEditor::Tick`, hooked) finds what the mouse points at among the
//! bricks' boxes or models ([`scene`]) and draws lines through the engine's line batcher
//! (`game::line_batcher`). The editor's left click (`Action_Select`, its reflected handler hooked)
//! starts and ends a resize instead of selecting while the resize key is held over a scalable
//! brick; the right button's (`Action_MoveCamera`) counts right clicks instead of moving the camera
//! meanwhile.
//! While a button is held the engine reports the mouse's movement to the editor, which would turn
//! the camera with it: the resize takes it instead ([`super::mouse_moves`]), and follows it when
//! the engine holds the cursor still ([`Drag::point`]). Each change writes `BrickSize` and tells the
//! brick like the editor's panel does (`game::scalable_brick::BrickSizes`), then moves the brick
//! like a committed move (`game::editor_object::SpawnTransforms`); the last one runs as an editor
//! operation (`game::brick_editor`), which makes the undo step. Moving bricks instead gives the
//! move mode its own place (`super::precise_moves::place_move`): the game then moves the selection,
//! its mirror mode counterparts and the gizmo, and commits the move itself.

mod draw;
mod geometry;
mod model;
mod motion;
mod parts;
mod resize;
mod scene;
mod snap;

use core::ffi::c_void;
use std::collections::HashSet;
use std::sync::{Mutex, Once, OnceLock, PoisonError, TryLockError};
use std::time::Instant;

use brickrust::ue::coreuobject::UObjectBase;

use super::keybinds;
use super::mouse_moves::{self, Direction, Handling, Movement};
use super::precise_moves::{self, Place};
use crate::framework::hook;
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_model::BrickModels;
use crate::game::editor_input::{self, ButtonAction, Click};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{
    self, LocalBounds, Selection, SpawnTransforms, Vector, WorldTransforms,
};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::line_batcher::{self, LineBatcher};
use crate::game::mirror_mode::{self, MirrorMode};
use crate::game::move_mode::{self, Move, MoveMode};
use crate::game::reflection::NativeThunk;
use crate::game::scalable_brick::BrickSizes;
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::platform::keyboard;
use crate::prelude::*;

use draw::{Drawing, PixelSize};
use geometry::{Bounds, Frame, Ray, Vec3};
use model::{BrickModel, Element, Models};
use motion::{Aim, Grab, Motion, Orientation};
use resize::{Grip, Resize};
use scene::{Piece, Placed, Scene, View};
use snap::Snapper;

pub struct BrickResize;

static ORIGINAL_SELECT: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_MOVE_CAMERA: OnceLock<NativeThunk> = OnceLock::new();
/// Set once everything is found; until then (or if that fails) the hooks change nothing.
static GAME: OnceLock<Game> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

/// Logs things worth knowing once: the first gathering's time, the first resize, how the cursor
/// behaves while resizing, problems.
static LOG_GATHERED: Once = Once::new();
static LOG_RESIZED: Once = Once::new();
static LOG_MOVED: Once = Once::new();
static LOG_SNAPPING_ON: Once = Once::new();
static LOG_TURNED: Once = Once::new();
static LOG_CURSOR: Once = Once::new();
static LOG_NO_PLACES: Once = Once::new();
static LOG_NO_INTERFACE: Once = Once::new();
static LOG_NO_LINES: Once = Once::new();
static LOG_MODELS: Once = Once::new();
static LOG_NO_MODEL: Once = Once::new();
static LOG_NO_FIT: Once = Once::new();
static LOG_SNAPPED: Once = Once::new();
static LOG_SLOW_SNAP: Once = Once::new();

/// How long finding what to snap to may take before it's logged (once), in milliseconds.
const SLOW_SNAP_MS: f64 = 4.0;

/// How far the mouse may move after the snap key goes up before a drag stops snapping (screen
/// pixels): the hand shakes a little when releasing the button.
const SNAP_LINGER_PIXELS: f32 = 8.0;

/// The game code and reflection this feature uses.
struct Game {
    editor: BrickEditor,
    modes: EditorModes,
    code: EditorCode,
    bricks: Bricks,
    sizes: BrickSizes,
    spawn: SpawnTransforms,
    world: WorldTransforms,
    bounds: LocalBounds,
    viewport: Viewport,
    lines: LineBatcher,
    /// The left button: starts and ends resizing.
    select: ButtonAction,
    /// The right button: looks deeper for snap targets while resizing.
    move_camera: ButtonAction,
    /// To prefer selected bricks; without it none is.
    selection: Option<Selection>,
    /// For the counterparts of resized bricks; without it they stay as they are.
    mirror: Option<MirrorMode>,
    /// Bricks' models; without them, bricks keep their boxes with the model key held.
    models: Option<BrickModels>,
    /// How the move mode moves bricks; without it (or without the selection, which tells the
    /// bricks moved), bricks can't be moved.
    moves: Option<&'static MoveMode>,
}

impl Feature for BrickResize {
    fn id(&self) -> &'static str {
        "brick_resize"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => {
                log!(
                    "active: resize key (Alt by default) + drag a scalable brick's face, edge or \
                     corner (or a moved brick's, while moving with the gizmo showing), snap key \
                     (Ctrl) to snap, model key (thumb mouse button) for models"
                )
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let selection = optional(
        editor_object::selection(),
        "selected bricks won't come first, and bricks can't be moved",
    );
    let moves =
        optional(move_mode::shared(), "bricks can't be moved").filter(|_| selection.is_some());
    let game = Game {
        editor: brick_editor::find()?,
        modes: editor_mode::find()?,
        code: editor_view::editor_code()?,
        bricks: editor_view::bricks()?,
        sizes: BrickSizes::find()?,
        spawn: SpawnTransforms::find()?,
        world: WorldTransforms::find()?,
        bounds: LocalBounds::find()?,
        viewport: viewport::find()?,
        lines: line_batcher::find()?,
        select: editor_input::button_action(editor_input::SELECT)?,
        move_camera: editor_input::button_action(editor_input::MOVE_CAMERA)?,
        selection,
        mirror: optional(mirror_mode::find(), "mirrored counterparts won't follow"),
        models: optional(BrickModels::find(), "bricks keep their boxes on models"),
        moves,
    };
    let (select, move_camera) = (game.select.thunk, game.move_camera.thunk);
    let _ = GAME.set(game);
    mouse_moves::listen(take_movement)?;
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

/// What a part the feature can do without gives, or `None` after logging what it does without.
fn optional<T>(found: Result<T, String>, without: &str) -> Option<T> {
    match found {
        Ok(found) => Some(found),
        Err(reason) => {
            log!("{without}: {reason}");
            None
        }
    }
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(game) = GAME.get() else {
        return;
    };
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: `editor` is the live editor ticking, on the game thread.
    unsafe { state.tick(game, editor) };
}

unsafe extern "C" fn select_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let original = *ORIGINAL_SELECT.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments.
    unsafe {
        on_button(
            |game| &game.select,
            |state, game, click| match click {
                Click::Pressed => state.press(game),
                Click::Released => state.release(game),
            },
            context,
            stack,
            result,
            original,
        )
    }
}

unsafe extern "C" fn move_camera_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let original = *ORIGINAL_MOVE_CAMERA
        .get()
        .expect("hook recorded before use");
    // SAFETY: the thunk's own arguments.
    unsafe {
        on_button(
            |game| &game.move_camera,
            |state, _, click| state.right_click(click),
            context,
            stack,
            result,
            original,
        )
    }
}

/// Runs a button action's thunk `original` with its arguments, unless `handle` (given the game,
/// the state and the button's change) says it handled the call: then the game's handler is skipped.
///
/// # Safety
/// `context`, `stack` and `result` must be the arguments `original` (`action`'s thunk) was called
/// with, on the game thread.
unsafe fn on_button(
    action: impl FnOnce(&Game) -> &ButtonAction,
    handle: impl FnOnce(&mut State, &Game, Click) -> bool,
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
    original: NativeThunk,
) {
    // SAFETY: per the contract.
    unsafe {
        if let Some(game) = GAME.get() {
            let action = action(game);
            if action.is_editor_input(context)
                && let Some(click) = action.click(stack)
            {
                let handled = {
                    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
                    handle(&mut state, game, click)
                };
                if handled {
                    action.handled(result);
                    return;
                }
            }
        }
        original(context, stack, result);
    }
}

/// Takes the mouse's movement while resizing: the camera stays still, and the movement moves the
/// point the resize follows while the engine holds the cursor still (see [`Drag::point`]).
fn take_movement(movement: &Movement) -> Handling {
    let mut state = match STATE.try_lock() {
        Ok(state) => state,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        // Only the game thread uses the state, and not around input handlers.
        Err(TryLockError::WouldBlock) => return Handling::Pass,
    };
    let Some(drag) = &mut state.drag else {
        return Handling::Pass;
    };
    let pixels = movement.value / mouse_moves::AXIS_PER_PIXEL;
    match movement.direction {
        Direction::Right => drag.pointer.moved[0] += pixels,
        // Up is positive, and the screen's y goes down.
        Direction::Up => drag.pointer.moved[1] -= pixels,
    }
    Handling::Take
}

/// A brick's piece under the mouse while the resize key is held.
struct Hover {
    /// The brick (see [`Scene::get`]).
    index: usize,
    piece: Piece,
    /// Whether dragging it does anything: resizes the brick (see [`Self::grip`]), or moves the
    /// bricks being moved.
    grabbable: bool,
    /// How dragging it resizes the brick; `None` if it can't (the brick isn't scalable, or the
    /// piece drags no side), or while moving bricks.
    grip: Option<Grip>,
    /// The point of the brick under the mouse (in the vehicle).
    point: Vec3,
    /// The mouse then.
    view: View,
}

/// A drag with the resize key: resizing a brick, or moving the bricks being moved.
struct Drag {
    editor: WeakObject,
    /// Where it points.
    pointer: Pointer,
    /// What it snaps to (while snapping), to show.
    snapper: Snapper,
    /// Whether it snaps.
    snap_key: SnapKey,
    action: Action,
}

/// Whether a drag snaps: while the snap key is held, and after it goes up until the mouse moves
/// [`SNAP_LINGER_PIXELS`] away from where it was then. Letting go of the key a split second before
/// the button then doesn't unsnap what's dropped.
#[derive(Default)]
struct SnapKey {
    /// Where the mouse was when snapping with the key held last (screen pixels); `None` once
    /// it stopped snapping.
    held_at: Option<[f32; 2]>,
}

impl SnapKey {
    /// Whether to snap with the mouse `at` (screen pixels), the key `held` or not.
    fn snapping(&mut self, held: bool, at: [f32; 2]) -> bool {
        match self.held_at {
            _ if held => self.held_at = Some(at),
            Some(from) if (at[0] - from[0]).hypot(at[1] - from[1]) <= SNAP_LINGER_PIXELS => {}
            _ => self.held_at = None,
        }
        self.held_at.is_some()
    }
}

enum Action {
    Resize(Resizing),
    Move(Moving),
}

/// A brick being resized.
struct Resizing {
    brick: Edited,
    /// Its mirror mode counterpart, and the mirror axis.
    counterpart: Option<(Edited, u8)>,
    resize: Resize,
    /// What was grabbed, and the brick's model as grabbed (to show a piece of it).
    piece: Piece,
    model: Option<BrickModel>,
    /// The size asked for last, as written (the brick may have kept another).
    asked: Vector,
    /// The size it has now.
    kept: Vec3,
    /// Its box now, in its frame as grabbed.
    bounds: Bounds,
}

/// The bricks of a move being moved by a grabbed point (see [`motion`]).
struct Moving {
    /// The move mode.
    mode: WeakObject,
    /// The grabbed brick as grabbed (where the move showed it), its box, its model and what of it.
    frame: Frame,
    bounds: Bounds,
    model: Option<BrickModel>,
    piece: Piece,
    grab: Grab,
    /// Where the move put its reference brick when grabbed.
    start: Place,
    /// The game's grid step for moves (0 while its snapping is off).
    grid: f64,
    /// The motion given last.
    motion: Motion,
    /// The motion the last snap gave (none before one): unsnapped, the bricks go on from there.
    kept: Motion,
    /// Which way snapping turns the bricks, of those matching the target.
    turn: TurnChoice,
    /// The mirror plane's axis (0 to 2) while mirror mode is on: the moved bricks' counterparts
    /// move along, mirrored.
    mirror: Option<usize>,
}

/// Which of the turns matching a snap's target the bricks take (see
/// [`motion::Grab::matching_turns`]): the smallest, then the next for each press of the orientation
/// key ([`keybinds::NEXT_ORIENTATION`]) while the snap stays on the same pieces.
#[derive(Default)]
struct TurnChoice {
    /// The pieces snapped onto last.
    on: Vec<(usize, Piece)>,
    /// 0 for the smallest turn.
    choice: usize,
    /// Whether the key was held last frame: holding it counts once.
    held: bool,
}

impl TurnChoice {
    /// Whether the key went down since last frame, given whether it's `held` now.
    fn pressed(&mut self, held: bool) -> bool {
        let pressed = held && !self.held;
        self.held = held;
        pressed
    }

    /// The choice for a snap onto the pieces `on`: the smallest turn on other pieces than last
    /// time, then one further if the key was `pressed`.
    fn choose(&mut self, on: &[(usize, Piece)], pressed: bool) -> usize {
        if self.on != on {
            self.on = on.to_vec();
            self.choice = 0;
        }
        if pressed {
            self.choice += 1;
        }
        self.choice
    }
}

/// Where a drag points (screen pixels), see [`Self::point`].
struct Pointer {
    at: [f32; 2],
    /// Where the cursor was when the button went down.
    pressed_at: [f32; 2],
    /// Whether the cursor has moved since (else the engine holds it still).
    cursor_moves: bool,
    /// The mouse's movement not yet added (pixels), from [`take_movement`].
    moved: [f32; 2],
}

/// A brick changed by a resize, as it was grabbed.
struct Edited {
    object: WeakObject,
    /// Where the editor places it: location and rotation (pitch, yaw, roll).
    location: Vector,
    rotation: Vector,
}

/// What the scene was gathered for: the editor in its default mode (for resizing), or a move with
/// the mouse free (for moving), as it was then.
#[derive(PartialEq)]
enum Gathered {
    Editing,
    Moving { mode: *mut UObjectBase, now: Move },
}

struct State {
    /// The bricks' boxes while the resize key is held or a brick is resized or moved.
    scene: Option<Scene>,
    /// What for.
    gathered: Option<Gathered>,
    hover: Option<Hover>,
    drag: Option<Drag>,
    /// Whether our lines are showing.
    drawn: bool,
    /// Whether the model key is held (this frame).
    on_models: bool,
    /// Bricks' models made so far, kept across scenes.
    models: Option<Models>,
}

// SAFETY: object pointers are only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

impl Pointer {
    fn new(mouse: [f32; 2]) -> Self {
        Self {
            at: mouse,
            pressed_at: mouse,
            cursor_moves: false,
            moved: [0.0; 2],
        }
    }

    /// Where the drag points now (screen pixels), given where the cursor is (if known). While a
    /// button is held over the 3D view, the engine may hide the cursor and hold it where it was
    /// pressed: then the mouse's movement moves the pointer from there. Once the cursor moves, the
    /// pointer is the cursor.
    fn point(&mut self, cursor: Option<[f32; 2]>) -> [f32; 2] {
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

    /// Whether the mouse moved while the engine held the cursor still: then the pointer shows.
    fn held_still(&self) -> bool {
        !self.cursor_moves && self.at != self.pressed_at
    }
}

impl Drag {
    /// The grabbed point now (in the vehicle), as deep as the pointer shows.
    fn grabbed_point(&self) -> Vec3 {
        match &self.action {
            Action::Resize(resizing) => resizing.resize.frame.point(resizing.resize.grab),
            Action::Move(moving) => moving.motion.point(moving.grab.point),
        }
    }
}

impl State {
    const fn new() -> Self {
        Self {
            scene: None,
            gathered: None,
            hover: None,
            drag: None,
            drawn: false,
            on_models: false,
            models: None,
        }
    }

    /// Every frame of `editor`.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn tick(&mut self, game: &Game, editor: *mut UObjectBase) {
        let held = keybinds::RESIZE_BRICKS.is_held();
        if !held && self.drag.is_none() && self.scene.is_none() && !self.drawn {
            return;
        }
        self.on_models = keybinds::MODEL_EDITING.is_held();
        if let Some(drag) = &self.drag
            && !drag.editor.is(editor)
        {
            // Another editor: that one is gone or not ticking; drop the drag.
            self.drag = None;
        }
        // SAFETY: per the contract.
        unsafe {
            if self.drag.is_none() {
                // Gathered again when the editor's mode changes, or the move does (moved bricks
                // show elsewhere).
                let wanted = if held { wanted(game, editor) } else { None };
                if wanted != self.gathered {
                    self.hover = None;
                    self.scene = None;
                }
                if let Some(wanted) = &wanted
                    && self.scene.is_none()
                {
                    self.scene = Some(gather(game, editor, wanted));
                }
                self.gathered = wanted;
            }
            if self.on_models
                && let (Some(scene), Some(reader)) = (&mut self.scene, &game.models)
                && !scene.has_models()
            {
                let models = self.models.get_or_insert_with(Models::default);
                add_models(reader, models, scene);
            }
            let controller = game.editor.player_controller(editor);
            let view = match &self.scene {
                Some(scene) if !controller.is_null() => {
                    let cursor = game.viewport.mouse(controller);
                    let mouse = match &mut self.drag {
                        Some(drag) => Some(drag.pointer.point(cursor)),
                        None => cursor,
                    };
                    mouse.and_then(|mouse| view(game, controller, mouse, &scene.vehicle))
                }
                _ => None,
            };
            if self.drag.is_some() {
                if !keyboard::is_mouse_button_down() {
                    // The release went elsewhere (another window): finish here.
                    self.finish(game);
                } else if let Some(view) = &view {
                    self.follow(game, editor, view);
                }
            } else if let Some(view) = &view {
                self.point(game, editor, view);
            } else {
                // The mouse is off the view.
                self.hover = None;
            }
            self.draw(game, view.as_ref());
        }
    }

    /// Finds the part under the mouse among the gathered bricks: while moving, among the moved
    /// ones only.
    ///
    /// # Safety
    /// `editor` must be a live editor. Game thread only.
    unsafe fn point(&mut self, game: &Game, editor: *mut UObjectBase, view: &View) {
        let Some(scene) = &self.scene else {
            return;
        };
        let moving = matches!(self.gathered, Some(Gathered::Moving { .. }));
        let vehicle = scene.vehicle;
        // SAFETY: per the contract.
        let pointed = scene.pick(
            view,
            self.on_models,
            |placed| moving && !placed.moving,
            |placed| unsafe { is_selected(game, placed) },
            0,
            |point| unsafe { project(game, editor, &vehicle, point) },
        );
        self.hover = pointed.map(|pointed| {
            let placed = scene.get(pointed.index);
            let grip = (placed.scalable && !moving)
                .then(|| Grip::of(pointed.piece, &placed.bounds, placed.model.as_ref()))
                .flatten();
            Hover {
                index: pointed.index,
                piece: pointed.piece,
                grabbable: moving || grip.is_some(),
                grip,
                point: pointed.point,
                view: *view,
            }
        });
    }

    /// Starts a drag on a click with the resize key held over a brick's piece that can be dragged:
    /// resizing a scalable brick, or moving the bricks being moved; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press(&mut self, game: &Game) -> bool {
        if !keybinds::RESIZE_BRICKS.is_held() {
            return false;
        }
        // SAFETY: per the contract.
        unsafe {
            match self.gathered {
                Some(Gathered::Editing) => self.press_resize(game),
                Some(Gathered::Moving { .. }) => self.press_move(game),
                None => false,
            }
        }
    }

    /// Starts resizing the hovered brick, if it can be; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_resize(&mut self, game: &Game) -> bool {
        let (Some(scene), Some(hover)) = (&self.scene, &self.hover) else {
            return false;
        };
        let Some(grip) = hover.grip else {
            return false;
        };
        let placed = scene.get(hover.index);
        let Some(object) = placed.object.get() else {
            return false;
        };
        if !game.spawn.known() {
            LOG_NO_PLACES
                .call_once(|| log!("can't resize: where bricks keep their place wasn't found"));
            return false;
        }
        // SAFETY: per the contract; `object` is a live scalable brick in the editor.
        unsafe {
            let editor = game.bricks.editor_of(object);
            let Some(brick) = edited(game, object) else {
                return false;
            };
            if editor.is_null() {
                return false;
            }
            let counterpart = game.mirror.as_ref().and_then(|mirror| {
                let axis = mirror.axis()?;
                let other = game.code.mirrored_object(object)?;
                (game.sizes.is_scalable(other)).then_some(())?;
                Some((edited(game, other)?, axis))
            });
            let size = game.sizes.size(object);
            LOG_RESIZED.call_once(|| {
                log!(
                    "resizing a brick by {} (size {size:?}, box {:?})",
                    describe(hover.piece),
                    placed.bounds
                )
            });
            self.drag = Some(Drag {
                editor: WeakObject::new(editor),
                pointer: Pointer::new(hover.view.mouse),
                snapper: Snapper::default(),
                snap_key: SnapKey::default(),
                action: Action::Resize(Resizing {
                    brick,
                    counterpart,
                    resize: Resize {
                        grip,
                        frame: placed.frame,
                        bounds: placed.bounds,
                        size: math::widen(size),
                        grab: placed.frame.local(hover.point),
                    },
                    piece: hover.piece,
                    model: placed.model.clone(),
                    asked: size,
                    kept: math::widen(size),
                    bounds: placed.bounds,
                }),
            });
        }
        true
    }

    /// Starts moving the bricks of the move by the hovered one; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_move(&mut self, game: &Game) -> bool {
        let (Some(scene), Some(hover), Some(Gathered::Moving { mode, now })) =
            (&self.scene, &self.hover, &self.gathered)
        else {
            return false;
        };
        let placed = scene.get(hover.index);
        let Some(object) = placed.object.get().filter(|_| placed.moving) else {
            return false;
        };
        // SAFETY: per the contract; `object` is a live brick in the editor.
        let editor = unsafe { game.bricks.editor_of(object) };
        if editor.is_null() {
            return false;
        }
        let Some(moves) = game.moves else {
            return false;
        };
        // SAFETY: per the contract; the editor is live.
        if let Err(reason) = unsafe { moves.keep_snapping_off(editor) } {
            LOG_SNAPPING_ON.call_once(|| {
                log!(
                    "moved bricks may shift to connectors while the game's snapping is on: {reason}"
                )
            });
        }
        let (piece, point) = grabbed(scene, hover, self.on_models);
        LOG_MOVED.call_once(|| {
            log!(
                "moving bricks by {} of one (the move at {:?}, {:?})",
                describe(piece),
                now.location,
                now.rotation
            )
        });
        let moving = Moving {
            // SAFETY: the move mode is live: it's the editor's current one.
            mode: unsafe { WeakObject::new(*mode) },
            frame: placed.frame,
            bounds: placed.bounds,
            model: placed.model.clone(),
            piece,
            grab: Grab {
                point,
                orientation: Orientation::of(placed, piece),
                axes: placed.frame.axes,
                across: Grab::across_for(hover.view.ray.direction),
            },
            start: precise_moves::effective(*mode, now),
            grid: f64::from(now.grid),
            motion: Motion::NONE,
            kept: Motion::NONE,
            turn: TurnChoice::default(),
            // SAFETY: per the contract.
            mirror: unsafe { mirror_axis(game) },
        };
        self.drag = Some(Drag {
            // SAFETY: the editor is live.
            editor: unsafe { WeakObject::new(editor) },
            pointer: Pointer::new(hover.view.mouse),
            snapper: Snapper::default(),
            snap_key: SnapKey::default(),
            action: Action::Move(moving),
        });
        true
    }

    /// While dragging, takes the right button (the camera stays still): each press sets aside one
    /// more of the bricks the mouse goes through for snapping. Whether it took it.
    fn right_click(&mut self, click: Click) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if click == Click::Pressed {
            drag.snapper.deeper();
        }
        true
    }

    /// Ends dragging when the button is released; whether one was.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn release(&mut self, game: &Game) -> bool {
        if self.drag.is_none() {
            return false;
        }
        // SAFETY: per the contract.
        unsafe { self.finish(game) };
        true
    }

    /// Follows the mouse: resizes the brick, or moves the bricks.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn follow(&mut self, game: &Game, editor: *mut UObjectBase, view: &View) {
        let (Some(scene), Some(drag)) = (&self.scene, &mut self.drag) else {
            return;
        };
        let models = self.on_models;
        let snapping = drag
            .snap_key
            .snapping(keybinds::SNAP_RESIZE.is_held(), view.mouse);
        if !snapping {
            drag.snapper.stop();
        }
        let snapper = snapping.then_some(&mut drag.snapper);
        // SAFETY: per the contract.
        let going = unsafe {
            match &mut drag.action {
                Action::Resize(resizing) => {
                    follow_resize(game, scene, resizing, snapper, view, models)
                }
                Action::Move(moving) => {
                    follow_move(game, editor, scene, moving, snapper, view, models)
                }
            }
        };
        if !going {
            self.drag = None;
        }
    }

    /// Finishes dragging: a resize's last change, as one editor operation (one undo step); a move
    /// goes on (the game ends it).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish(&mut self, game: &Game) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let changed = match &drag.action {
            Action::Resize(resizing) => resizing.asked != math::narrow(resizing.resize.size),
            Action::Move(moving) => moving.motion != Motion::NONE,
        };
        if !changed {
            // A click without a drag.
            return;
        }
        LOG_CURSOR.call_once(|| {
            log!(
                "while dragging, the cursor {}",
                if drag.pointer.cursor_moves {
                    "moves"
                } else {
                    "stays still: following the mouse's movement"
                }
            )
        });
        if let Action::Resize(resizing) = drag.action {
            // SAFETY: per the contract.
            unsafe { self.finish_resize(game, &drag.editor, resizing) };
        }
    }

    /// Finishes resizing: the last change, as one editor operation (one undo step).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish_resize(&mut self, game: &Game, editor: &WeakObject, mut resizing: Resizing) {
        let (Some(editor), Some(object)) = (editor.get(), resizing.brick.object.get()) else {
            return;
        };
        // SAFETY: per the contract; the editor and the brick are live.
        unsafe {
            let size = game.sizes.size(object);
            game.editor.operation(editor, brick_editor::MOVED, || {
                apply(game, &mut resizing, size, false);
            });
            if let Some(scene) = &mut self.scene {
                let counterpart = resizing
                    .counterpart
                    .as_ref()
                    .map(|(other, _)| &other.object);
                for edited in [Some(&resizing.brick.object), counterpart]
                    .into_iter()
                    .flatten()
                {
                    if let Some(object) = edited.get()
                        && let Some(mut placed) = placed(game, object)
                    {
                        if scene.has_models()
                            && let (Some(reader), Some(models)) = (&game.models, &mut self.models)
                        {
                            placed.model = model_of(reader, models, object, &placed);
                        }
                        scene.update(placed);
                    }
                }
            }
        }
    }

    /// Shows what's going on, or nothing.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn draw(&mut self, game: &Game, view: Option<&View>) {
        let lines = match (view, &self.scene) {
            (Some(view), Some(scene)) => self.lines(scene, view),
            _ => Vec::new(),
        };
        if lines.is_empty() && !self.drawn {
            return;
        }
        // SAFETY: per the contract; the batcher is live if not null.
        unsafe {
            let batcher = game.lines.batcher();
            if batcher.is_null() {
                LOG_NO_LINES
                    .call_once(|| log!("nothing can be shown: no line batcher in this world"));
                return;
            }
            game.lines.replace(batcher, &lines);
        }
        self.drawn = !lines.is_empty();
    }

    fn lines(&self, scene: &Scene, view: &View) -> Vec<line_batcher::Line> {
        let mut drawing = Drawing::new(view.pixels, scene.vehicle);
        if let Some(drag) = &self.drag {
            match &drag.action {
                Action::Resize(resizing) => draw_resizing(&mut drawing, resizing),
                Action::Move(moving) => draw_moving(&mut drawing, moving),
            }
            if let Some(snapped) = drag.snapper.snapped() {
                // What lines up with the snap point, and how counterparts of moved bricks moved.
                let (lined, mirrored) = match &drag.action {
                    Action::Resize(resizing) => {
                        let resize = &resizing.resize;
                        let lined = resize
                            .frame
                            .point(resize.pulled(resizing.kept, snapped.target.point));
                        (lined, None)
                    }
                    Action::Move(moving) => (
                        moving.motion.point(moving.grab.point),
                        moving.mirror.map(|axis| moving.motion.mirrored(axis)),
                    ),
                };
                let snap = Snapped {
                    snapper: &drag.snapper,
                    snapped,
                    lined,
                    mirrored,
                };
                draw_snap(&mut drawing, scene, &snap);
            }
            if drag.pointer.held_still() {
                // The mouse moved but the cursor didn't (the engine holds it still): show where the
                // drag points, as deep as the grab.
                let grab = drag.grabbed_point();
                let depth = geometry::length(geometry::sub(grab, view.ray.origin));
                drawing.pointer(geometry::along(&view.ray, depth), draw::POINTER);
            }
        } else if let Some(hover) = &self.hover {
            let placed = scene.get(hover.index);
            let outline = if placed.scalable || hover.grabbable {
                draw::BOX
            } else {
                draw::FIXED_BOX
            };
            drawing.outline(&placed.frame, &placed.bounds, outline);
            let frame = &placed.frame;
            match (hover.grabbable, hover.piece) {
                (true, piece) => draw_piece(&mut drawing, frame, placed, piece, draw::PART),
                // A model's piece that can't drag still shows what the mouse is on.
                (false, piece @ Piece::Model(_)) => {
                    draw_piece(&mut drawing, frame, placed, piece, draw::FIXED_PART)
                }
                (false, Piece::Box(_)) => {}
            }
        }
        drawing.into_lines()
    }
}

/// What the scene is to be gathered for in `editor`'s mode now, if anything: its default mode
/// (resizing), or a move with the mouse free (moving).
///
/// # Safety
/// `editor` must be a live editor. Game thread only.
unsafe fn wanted(game: &Game, editor: *mut UObjectBase) -> Option<Gathered> {
    // SAFETY: per the contract; the editor's current mode is live.
    unsafe {
        if game.modes.is_editing(editor) {
            return Some(Gathered::Editing);
        }
        let moves = game.moves?;
        if game.modes.moving(editor) != Some(editor_mode::Moving::Precisely) {
            return None;
        }
        let mode = game.modes.move_mode(editor)?;
        let now = moves.current(mode)?;
        Some(Gathered::Moving { mode, now })
    }
}

/// Resizes the brick for the mouse, snapped with a `snapper` (while snapping, see [`SnapKey`]);
/// false once it can't be.
///
/// # Safety
/// Game thread only.
unsafe fn follow_resize(
    game: &Game,
    scene: &Scene,
    resizing: &mut Resizing,
    snapper: Option<&mut Snapper>,
    view: &View,
    models: bool,
) -> bool {
    let sides = if let Some(snapper) = snapper {
        // The resized bricks are left out: they move.
        let counterpart = resizing
            .counterpart
            .as_ref()
            .map(|(other, _)| &other.object);
        let resized: Vec<*mut UObjectBase> = [Some(&resizing.brick.object), counterpart]
            .into_iter()
            .flatten()
            .filter_map(WeakObject::get)
            .collect();
        let is_resized = |placed: &Placed| resized.iter().any(|&object| placed.object.is(object));
        let started = Instant::now();
        let snapped = snapper.snap(&snap::SNAPPING, scene, view, models, is_resized);
        log_snap(snapped.map(|snapped| snapped.kind), started);
        match snapped {
            Some(snapped) => Some(resizing.resize.snap(&snapped.target)),
            None => resizing.resize.follow(&view.ray),
        }
    } else {
        resizing.resize.follow(&view.ray)
    };
    let Some(sides) = sides else {
        return true;
    };
    let size = math::narrow(resizing.resize.size_for(&resizing.resize.resized(sides)));
    if size == resizing.asked {
        return true;
    }
    resizing.asked = size;
    // SAFETY: per the contract.
    unsafe { apply(game, resizing, size, true) }
}

/// Moves the bricks of the move for the mouse, snapped with a `snapper` (while snapping, see
/// [`SnapKey`]): gives the move its place. False once the move is over, or doesn't leave the mouse
/// free anymore.
///
/// # Safety
/// `editor` must be the live editor ticking. Game thread only.
unsafe fn follow_move(
    game: &Game,
    editor: *mut UObjectBase,
    scene: &Scene,
    moving: &mut Moving,
    snapper: Option<&mut Snapper>,
    view: &View,
    models: bool,
) -> bool {
    let (Some(moves), Some(mode)) = (game.moves, moving.mode.get()) else {
        return false;
    };
    // SAFETY: per the contract.
    let still = unsafe {
        game.modes.move_mode(editor) == Some(mode)
            && game.modes.moving(editor) == Some(editor_mode::Moving::Precisely)
    };
    if !still {
        return false;
    }
    let free = moving.grab.follow(&view.ray, moving.grid, &moving.kept);
    let pressed = moving.turn.pressed(keybinds::NEXT_ORIENTATION.is_held());
    let motion = if let Some(snapper) = snapper {
        let started = Instant::now();
        // The moved bricks are left out.
        let snapped = snapper.snap(&snap::SNAPPING, scene, view, models, |placed| placed.moving);
        log_snap(snapped.map(|snapped| snapped.kind), started);
        match snapped {
            Some(snapped) => {
                let choice = moving.turn.choose(&snapped.on, pressed);
                if pressed {
                    LOG_TURNED.call_once(|| log!("snapping turns the next way that matches"));
                }
                let snapped = snap_move(scene, moving, snapped, free, choice);
                if let Some(snapped) = snapped {
                    moving.kept = snapped;
                }
                snapped
            }
            None => free,
        }
    } else {
        free
    };
    if let Some(motion) = motion
        && motion != moving.motion
    {
        moving.motion = motion;
        let place = Place {
            location: motion.point(moving.start.location),
            rotation: motion.rotation * moving.start.rotation,
        };
        // SAFETY: per the contract; `mode` is the editor's current move mode.
        unsafe { precise_moves::place_move(moves, mode, place) };
    }
    true
}

/// The motion snapping a move to `snapped`: onto it, turned the `choice`th way matching it where it
/// lines up (see [`motion::MATCHING`]); on a counterpart of the moved bricks, meeting it halfway
/// across the mirror plane, the rest following the mouse (`free`, if it gives a point).
fn snap_move(
    scene: &Scene,
    moving: &Moving,
    snapped: &snap::Snap,
    free: Option<Motion>,
    choice: usize,
) -> Option<Motion> {
    let on_counterpart = snapped
        .on
        .iter()
        .any(|&(index, _)| scene.get(index).mirroring);
    match moving.mirror {
        Some(axis) if on_counterpart => {
            free.map(|free| moving.grab.meet_mirrored(snapped.target.point, axis, free))
        }
        _ => Some(moving.grab.snap(
            &aim(scene, snapped),
            &motion::MATCHING,
            &moving.kept,
            choice,
        )),
    }
}

/// The mirror plane's axis (0 to 2) while mirror mode is on.
///
/// # Safety
/// Game thread only.
unsafe fn mirror_axis(game: &Game) -> Option<usize> {
    // SAFETY: per the contract.
    let axis = unsafe { game.mirror.as_ref()?.axis()? };
    Some(usize::from(axis - 1))
}

/// What's grabbed of the hovered brick, and where: with the snap key held, the brick's own snap
/// point nearest the mouse (a corner, an edge's middle, a face's center...) and its piece there;
/// else the piece under the mouse, at its point there (see [`motion::grabbed_point`]).
fn grabbed(scene: &Scene, hover: &Hover, models: bool) -> (Piece, Vec3) {
    let placed = scene.get(hover.index);
    if keybinds::SNAP_RESIZE.is_held() {
        let mut own = Snapper::default();
        let others = |other: &Placed| !core::ptr::eq(other, placed);
        if let Some(snapped) = own.snap(&snap::SNAPPING, scene, &hover.view, models, others)
            && let Some(&(_, piece)) = snapped.on.iter().find(|(index, _)| *index == hover.index)
        {
            return (piece, snapped.target.point);
        }
    }
    let point = motion::grabbed_point(placed, hover.piece, &hover.view.ray, hover.point);
    (hover.piece, point)
}

/// Where a snapped grab goes: the snap point, how the piece it's on lies (if it's on one), and
/// that brick's axes.
fn aim(scene: &Scene, snapped: &snap::Snap) -> Aim {
    let orientation = match snapped.on.as_slice() {
        &[(index, piece)] => Orientation::of(scene.get(index), piece),
        _ => None,
    };
    let axes = snapped
        .on
        .first()
        .map_or(scene.vehicle.axes, |&(index, _)| {
            scene.get(index).frame.axes
        });
    Aim {
        point: snapped.target.point,
        orientation,
        axes,
    }
}

/// A brick being resized: its box, and the piece dragged.
fn draw_resizing(drawing: &mut Drawing, resizing: &Resizing) {
    let frame = &resizing.resize.frame;
    drawing.outline(frame, &resizing.bounds, draw::BOX);
    match resizing.piece {
        Piece::Box(part) => drawing.part(frame, &resizing.bounds, part, draw::PART),
        Piece::Model(element) => {
            if let Some(model) = &resizing.model {
                let moved = |point| resizing.resize.moved(resizing.kept, point);
                drawing.element(frame, model, element, draw::PART, moved);
            }
        }
    }
}

/// The grabbed brick of a move where it's going: its box, and the piece grabbed.
fn draw_moving(drawing: &mut Drawing, moving: &Moving) {
    let frame = moving.motion.frame(&moving.frame);
    drawing.outline(&frame, &moving.bounds, draw::BOX);
    match moving.piece {
        Piece::Box(part) => drawing.part(&frame, &moving.bounds, part, draw::PART),
        Piece::Model(element) => {
            if let Some(model) = &moving.model {
                drawing.element(&frame, model, element, draw::PART, |point| point);
            }
        }
    }
}

/// The ray and pixel size at screen position `mouse` this frame, in the vehicle (at `vehicle` in
/// the world).
///
/// # Safety
/// `controller` must be a live player controller. Game thread only.
unsafe fn view(
    game: &Game,
    controller: *mut UObjectBase,
    mouse: [f32; 2],
    vehicle: &Frame,
) -> Option<View> {
    // The ray through a screen position, in the vehicle.
    let ray_at = |screen: [f32; 2]| {
        // SAFETY: per the contract.
        let ray = unsafe { game.viewport.ray(controller, screen) }?;
        Some(vehicle.local_ray(&Ray {
            origin: math::widen(ray.origin),
            direction: math::widen(ray.direction),
        }))
    };
    let ray = ray_at(mouse)?;
    let right = ray_at([mouse[0] + 1.0, mouse[1]])?;
    let down = ray_at([mouse[0], mouse[1] + 1.0])?;
    let step_right = geometry::sub(right.direction, ray.direction);
    let step_down = geometry::sub(down.direction, ray.direction);
    Some(View {
        mouse,
        ray,
        pixels: PixelSize {
            camera: ray.origin,
            per_distance: geometry::length(step_right),
            right: geometry::normalized(step_right)?,
            down: geometry::normalized(step_down)?,
        },
    })
}

/// Where `point` of the vehicle (at `vehicle` in the world) shows on the screen of `editor`'s
/// player.
///
/// # Safety
/// `editor` must be a live editor. Game thread only.
unsafe fn project(
    game: &Game,
    editor: *mut UObjectBase,
    vehicle: &Frame,
    point: Vec3,
) -> Option<[f32; 2]> {
    // SAFETY: per the contract; the controller is live if not null.
    unsafe {
        let controller = game.editor.player_controller(editor);
        if controller.is_null() {
            return None;
        }
        game.viewport
            .project(controller, math::narrow(vehicle.point(point)))
    }
}

/// Whether the brick is selected in the editor (false if that can't be told).
///
/// # Safety
/// Game thread only.
unsafe fn is_selected(game: &Game, placed: &Placed) -> bool {
    let (Some(selection), Some(object)) = (&game.selection, placed.object.get()) else {
        return false;
    };
    // SAFETY: per the contract; the weak pointer resolved to a live editor object.
    unsafe { selection.is_selected(object) }
}

/// The editor's visible bricks with their boxes, and where the vehicle is. For a move: the moved
/// bricks (the selected ones) and their mirror mode counterparts where the move shows them.
///
/// # Safety
/// `editor` must be a live editor, and a move's mode live. Game thread only.
unsafe fn gather(game: &Game, editor: *mut UObjectBase, gathered: &Gathered) -> Scene {
    let started = Instant::now();
    // SAFETY: per the contract; the editor's objects are live (entries may be null).
    let (vehicle, bricks) = unsafe {
        let objects = game.bricks.editor_objects(editor);
        let bricks: Vec<*mut UObjectBase> = objects
            .iter()
            .copied()
            .filter(|&object| {
                !object.is_null() && game.bricks.is_brick(object) && !game.code.is_hidden(object)
            })
            .collect();
        game.spawn.learn(bricks.iter().copied());
        let carry = match gathered {
            Gathered::Editing => None,
            Gathered::Moving { mode, now } => carry(game, *mode, now, &bricks),
        };
        let bricks: Vec<(*mut UObjectBase, Placed)> = bricks
            .into_iter()
            .filter_map(|object| {
                let mut placed = placed(game, object)?;
                if let Some(carry) = &carry {
                    if is_selected(game, &placed) {
                        placed.frame = carry.frame(&placed.frame);
                        placed.moving = true;
                    } else if let Some(axis) = carry.mirror
                        && carry.counterparts.contains(&(object as usize))
                    {
                        placed.frame = carry.motion().mirrored(axis).frame(&placed.frame);
                        placed.mirroring = true;
                    }
                }
                Some((object, placed))
            })
            .collect();
        // The vehicle's place in the world, from a brick's places in both (one not moved, if any:
        // the world shows moved ones a little off, in `f32`).
        let vehicle = bricks
            .iter()
            .find(|(_, placed)| !placed.moving && !placed.mirroring)
            .or(bricks.first())
            .map(|(object, placed)| {
                let world = Frame::of_transform(&game.world.of(*object));
                placed.frame.parent(&world)
            })
            .unwrap_or(Frame::new([0.0; 3], math::Quat::IDENTITY));
        let bricks = bricks.into_iter().map(|(_, placed)| placed).collect();
        (vehicle, bricks)
    };
    let scene = Scene::new(vehicle, bricks);
    LOG_GATHERED.call_once(|| {
        log!(
            "gathered {} bricks in {:.1} ms (places {})",
            scene.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            if game.spawn.known() {
                "found"
            } else {
                "not found"
            }
        )
    });
    scene
}

/// Where a move shows the bricks it moves (see [`carry`]).
struct Carry {
    /// Where it puts its reference brick, and where that brick is stored.
    moved: Place,
    reference: Place,
    /// The mirror plane's axis (0 to 2) while mirror mode is on, and the counterparts of the moved
    /// bricks then (by address; not moved ones themselves).
    mirror: Option<usize>,
    counterparts: HashSet<usize>,
}

impl Carry {
    /// Where the move shows a moved brick stored at `frame`.
    fn frame(&self, frame: &Frame) -> Frame {
        let stored = Place {
            location: frame.origin,
            rotation: frame.rotation,
        };
        let place = precise_moves::carried(&self.moved, &self.reference, &stored);
        Frame::new(place.location, place.rotation)
    }

    /// The motion taking bricks from where they're stored to where the move shows them.
    fn motion(&self) -> Motion {
        Motion {
            rotation: self.moved.rotation * self.reference.rotation.inverse(),
            pivot: self.reference.location,
            to: self.moved.location,
        }
    }
}

/// How the move `now` of `mode` shows the bricks it moves, the selected ones among `bricks`.
///
/// # Safety
/// `mode` must be a live move mode, `bricks` live bricks. Game thread only.
unsafe fn carry(
    game: &Game,
    mode: *mut UObjectBase,
    now: &Move,
    bricks: &[*mut UObjectBase],
) -> Option<Carry> {
    // SAFETY: per the contract; the reference brick is live while the move is.
    unsafe {
        let (location, rotation) = game.spawn.read(now.reference)?;
        let selection = game.selection.as_ref()?;
        let mirror = mirror_axis(game);
        let counterparts = if mirror.is_some() {
            bricks
                .iter()
                .filter(|&&object| selection.is_selected(object))
                .filter_map(|&object| game.code.mirrored_object(object))
                .filter(|&object| !selection.is_selected(object))
                .map(|object| object as usize)
                .collect()
        } else {
            HashSet::new()
        };
        Some(Carry {
            moved: precise_moves::effective(mode, now),
            reference: Place::stored(location, rotation),
            mirror,
            counterparts,
        })
    }
}

/// A brick's box in the vehicle, if it has one.
///
/// # Safety
/// `object` must be a live brick. Game thread only.
unsafe fn placed(game: &Game, object: *mut UObjectBase) -> Option<Placed> {
    // SAFETY: per the contract.
    unsafe {
        let [a, b] = game.bounds.of(object)?.map(math::widen);
        let (location, rotation) = game.spawn.read(object)?;
        // A negative size (typed) mirrors the brick, which may turn its box inside out.
        let min = core::array::from_fn(|axis| a[axis].min(b[axis]));
        let max = core::array::from_fn(|axis| a[axis].max(b[axis]));
        let scalable = game.sizes.is_scalable(object);
        let mut bounds = Bounds { min, max };
        if scalable {
            bounds = sized(bounds, math::widen(game.sizes.size(object)));
        }
        Some(Placed {
            object: WeakObject::new(object),
            frame: Frame::of_place(location, rotation),
            bounds,
            scalable,
            mirrored: core::array::from_fn(|axis| a[axis] > b[axis]),
            model: None,
            moving: false,
            mirroring: false,
        })
    }
}

/// Gives the scene's bricks their models (the first time the model key is held), and logs how
/// that went the first time.
///
/// # Safety
/// The scene's bricks must be the editor's (alive or gone). Game thread only.
unsafe fn add_models(reader: &BrickModels, models: &mut Models, scene: &mut Scene) {
    let started = Instant::now();
    // SAFETY: per the contract; `get` only gives live bricks.
    let with_model = scene.add_models(|placed| unsafe {
        let object = placed.object.get()?;
        model_of(reader, models, object, placed)
    });
    LOG_MODELS.call_once(|| {
        let (made, meshes) = models.counts();
        let largest = match models.largest() {
            Some([triangles, faces, edges, vertices]) => format!(
                "; largest: {triangles} triangles, {faces} faces, {edges} edges, {vertices} vertices"
            ),
            None => String::new(),
        };
        log!(
            "models: {with_model} of {} bricks in {:.1} ms ({made} of {meshes} meshes{largest})",
            scene.len(),
            started.elapsed().as_secs_f64() * 1000.0,
        )
    });
}

/// The model `object` shows, scaled to its box (`placed`), if it has one that fits.
///
/// # Safety
/// `object` must be a live brick. Game thread only.
unsafe fn model_of(
    reader: &BrickModels,
    models: &mut Models,
    object: *mut UObjectBase,
    placed: &Placed,
) -> Option<BrickModel> {
    // SAFETY: per the contract; the mesh is a live static mesh.
    unsafe {
        let mesh = reader.mesh_of(object)?;
        let model = models.get(mesh, || reader.triangles(mesh));
        let model = match model {
            Ok(model) => model,
            Err(reason) => {
                LOG_NO_MODEL.call_once(|| log!("a brick keeps its box on models: {reason}"));
                return None;
            }
        };
        let model_bounds = *model.bounds();
        let fitted = BrickModel::fit(model, placed.mirrored, &placed.bounds);
        if fitted.is_none() {
            LOG_NO_FIT.call_once(|| {
                log!(
                    "a brick keeps its box on models: its model's box {model_bounds:?} doesn't \
                     scale to its box {:?}",
                    placed.bounds
                )
            });
        }
        fitted
    }
}

/// Draws a brick's `piece` with the brick at `frame`: a part of its box, or of its model.
fn draw_piece(drawing: &mut Drawing, frame: &Frame, placed: &Placed, piece: Piece, pen: draw::Pen) {
    match piece {
        Piece::Box(part) => drawing.part(frame, &placed.bounds, part, pen),
        Piece::Model(element) => {
            if let Some(model) = &placed.model {
                drawing.element(frame, model, element, pen, |point| point);
            }
        }
    }
}

/// A drag's snap, to show.
struct Snapped<'a> {
    snapper: &'a Snapper,
    snapped: &'a snap::Snap,
    /// What lines up with the snap point.
    lined: Vec3,
    /// How the counterparts of moved bricks move (while moving with mirror mode on): snapped onto
    /// one, the snap point moves along.
    mirrored: Option<Motion>,
}

/// What a drag snaps to: the pieces of bricks it's on with their boxes, the stretch of an edge
/// it's halfway along, its mark, and a dashed line from what lines up with it where they're apart.
fn draw_snap(drawing: &mut Drawing, scene: &Scene, snap: &Snapped) {
    let snapped = snap.snapped;
    // Counterparts of moved bricks show moved along, and so does a snap point on one.
    let mut carried = None;
    for &(index, piece) in &snapped.on {
        let placed = scene.get(index);
        let frame = match snap.mirrored {
            Some(mirrored) if placed.mirroring => {
                carried = Some(mirrored);
                mirrored.frame(&placed.frame)
            }
            _ => placed.frame,
        };
        drawing.outline(&frame, &placed.bounds, draw::TARGET_BOX);
        draw_piece(drawing, &frame, placed, piece, draw::TARGET);
    }
    let now = |point| carried.map_or(point, |motion: Motion| motion.point(point));
    if let Some([start, end]) = snapped.span.map(|span| span.map(now)) {
        drawing.segment(start, end, draw::SPAN);
        for end in [start, end] {
            drawing.mark(draw::Mark::Cross, end, draw::END_MARK_PIXELS, draw::SPAN);
        }
    }
    let point = now(snapped.target.point);
    if drawing.pixels_between(snap.lined, point) >= GUIDE_MIN_PIXELS {
        drawing.dashed(snap.lined, point, draw::GUIDE);
    }
    // A new point pops up white, then pulses.
    let seconds = snap.snapper.snapped_for().as_secs_f64();
    let pen = draw::pulsed(draw::SNAP_POINT, seconds);
    let size = draw::popped(draw::MARK_PIXELS, seconds);
    let normal = snapped
        .target
        .normal
        .map(|normal| carried.map_or(normal, |motion| motion.rotation.rotate(normal)));
    match (snapped.kind.mark(), normal) {
        (Some(mark), _) => drawing.mark(mark, point, size, pen),
        (None, Some(normal)) => drawing.flat_mark(point, normal, size, pen),
        (None, None) => drawing.mark(draw::Mark::Square, point, size, pen),
    }
}

/// How far apart (pixels) the dragged part and its snap point must look for a guide between them.
const GUIDE_MIN_PIXELS: f64 = 4.0;

/// Logs the first snap, and the first search that takes long (see [`SLOW_SNAP_MS`]).
fn log_snap(kind: Option<snap::Kind>, started: Instant) {
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(kind) = kind {
        LOG_SNAPPED.call_once(|| log!("snapped to {} ({elapsed:.2} ms)", kind.name()));
    }
    if elapsed > SLOW_SNAP_MS {
        LOG_SLOW_SNAP.call_once(|| log!("finding what to snap to took {elapsed:.1} ms"));
    }
}

/// What `piece` is, for the log.
fn describe(piece: Piece) -> &'static str {
    match piece {
        Piece::Box(part) => ["a face", "an edge", "a corner"][part.dimensions_fixed() - 1],
        Piece::Model(Element::Face(_)) => "a model's face",
        Piece::Model(Element::Edge(_)) => "a model's edge",
        Piece::Model(Element::Vertex(_)) => "a model's vertex",
    }
}

/// How far a scalable brick's box may be from its size around its origin to be taken as that
/// (per coordinate, as a share of the size): the game's `f32` noise stays far below (about 3e-7),
/// a box off its origin far above.
const SIZED_BOX_MATCH: f64 = 1e-4;

/// A scalable brick's box `reported` by the game, or exactly its `size` around its origin if
/// that's what it is (see [`SIZED_BOX_MATCH`]).
fn sized(reported: Bounds, size: Vec3) -> Bounds {
    let half = size.map(|side| side.abs() / 2.0);
    let close = |value: f64, expected: f64, side: f64| {
        (value - expected).abs() <= SIZED_BOX_MATCH * side.abs().max(1.0)
    };
    let matches = (0..3).all(|axis| {
        close(reported.min[axis], -half[axis], size[axis])
            && close(reported.max[axis], half[axis], size[axis])
    });
    if matches {
        Bounds {
            min: half.map(|side| -side),
            max: half,
        }
    } else {
        reported
    }
}

/// A brick about to be resized, as it is.
///
/// # Safety
/// `object` must be a live brick. Game thread only.
unsafe fn edited(game: &Game, object: *mut UObjectBase) -> Option<Edited> {
    // SAFETY: per the contract.
    let place = unsafe { game.spawn.of(object)? };
    Some(Edited {
        // SAFETY: per the contract.
        object: unsafe { WeakObject::new(object) },
        location: place.location,
        rotation: place.rotation,
    })
}

/// Asks the dragged brick for `size`, then places it (and its counterpart) to keep the parts not
/// dragged in place. False if the brick is gone or can't be resized.
///
/// # Safety
/// Game thread only.
unsafe fn apply(game: &Game, resizing: &mut Resizing, size: Vector, interactive: bool) -> bool {
    let Some(object) = resizing.brick.object.get() else {
        return false;
    };
    // SAFETY: per the contract; the brick and its counterpart are live scalable bricks.
    unsafe {
        let Some(kept) = game.sizes.resize(object, size, interactive) else {
            LOG_NO_INTERFACE
                .call_once(|| log!("can't resize: bricks' property interface not found"));
            return false;
        };
        resizing.kept = math::widen(kept);
        let (bounds, shift) = resizing.resize.placed(resizing.kept);
        let spawn = Frame::of_place(resizing.brick.location, resizing.brick.rotation);
        let location = math::narrow(spawn.point(shift));
        game.spawn.set(object, location, resizing.brick.rotation);
        resizing.bounds = bounds;
        if let (Some((other, axis)), Some(mirror)) = (&resizing.counterpart, &game.mirror)
            && let Some(counterpart) = other.object.get()
        {
            game.sizes.resize(counterpart, kept, interactive);
            let mirrored =
                mirror.mirrored_location(object, location, resizing.brick.rotation, *axis);
            game.spawn.set(counterpart, mirrored, other.rotation);
        }
    }
    true
}
