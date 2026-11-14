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
//! - a setting makes the resize key a toggle: a quick tap turns all this on or off, holding it
//!   still works meanwhile (see [`key_mode`], and [`is_fast_editing`] for the rest of the mod);
//! - a press of the resize key (held or tapped) gives the keyboard back to the 3D view
//!   (`game::game_focus`): a game button hovered on the way keeps it otherwise, and Enter would
//!   press that button instead of typing a length;
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
//! - a face dragged shows its size just past it, in light blue and the editor's unit (see
//!   [`line_label`]). Right after, [`keybinds::TYPE_LENGTH`] (Enter, Alt let go) opens a text box
//!   on it to type the size it should have, as in CAD (see [`typed_length`]): the same resize
//!   again, to that size (one more step to undo). With fast
//!   editing toggled on, the face under the mouse comes first. A face stays ready for
//!   [`FACE_LASTS`] after a drag or a typed size, unless its brick changes otherwise (an undo, a
//!   move), is deselected or the box is cancelled (see [`SizedFace`]); edges and corners have no
//!   size to type;
//! - hold the model key too ([`keybinds::MODEL_EDITING`], the middle or either thumb mouse button
//!   by default): the bricks' models replace their boxes, for what the mouse points at, drags and
//!   snaps to: their faces, edges and vertices (see [`model`]). Dragging one resizes the box like
//!   the box's part it lies on: a face moves along its normal keeping its slope (its sizes in proportion), an edge
//!   across its length, a vertex away from the opposite corner; snapped, a vertex goes onto the
//!   target, an edge and a face through it. Bricks without a model (see `game::brick_model`) keep
//!   their box.
//!
//! Spinners show their real shape instead of their box, models or not, and dragging it changes
//! their own values (see [`spinner`]): the inner or outer side the thickness, the top or bottom the
//! height (each a whole side, curved or not, split where it turns from facing up or down to facing
//! outward or inward), the rims between sides both, the end cap the angle (the start cap too on
//! round ones, turning the brick; with the snap key, also to angles in steps, a setting). Snapping
//! onto a spinner, and moving one, use what it's built of rather than its model's facets: its caps'
//! corners and edges, its rims and the crests of its curved sides (along the arc itself), its hub,
//! its sides' smooth normals (see [`snap`]); the model key shows the facets again. With the mouse
//! on a spinner in the default editing mode (no drag), a press of [`keybinds::NEXT_SPINNER_VARIANT`]
//! (R) switches its cross-section to the next variant of its shape (inward, outward, upward), of
//! [`keybinds::NEXT_SPINNER_SHAPE`] (Ctrl + R) to the next shape, one undo step, its mirror
//! counterpart too (see [`spinner_shape`]). Selected round spinners joined cap to cap make a curve:
//! its nodes and handles show, come before what's under them, and drag like Bezier curves' (see
//! [`spinner_curve`]), reshaping only the spinners next to them, never making or deleting one.
//!
//! While moving bricks with the mouse free (the move mode's gizmo showing: G pressed twice), the
//! same keys move them instead (a prototype, see [`motion`]):
//!
//! - hold Alt: the box of the moved brick under the mouse shows, and the part the mouse is on;
//! - Alt + drag: the grabbed point follows the mouse, in the plane across the vehicle's axis the
//!   camera looks along the most, by the game's grid steps while its snapping is on. A click with
//!   the mouse exactly where it was pressed moves nothing (see [`Moving::pressed_at`]). With Ctrl held
//!   when pressing, the grabbed point is the brick's own snap point nearest the mouse (a corner, an
//!   edge's middle, a face's center...), marked (blue) on its piece before the press;
//! - hold Ctrl while dragging: the grabbed point snaps like a resize does, to bricks not moved;
//!   onto a face from a face, or onto an edge from an edge, the bricks also turn to match it, then
//!   square up with the target brick (see [`motion::matching`]): the smallest such turn, and each
//!   press of R ([`keybinds::NEXT_ORIENTATION`]) the next one (facing the target or level with it,
//!   along an edge either way, squared up each way), kept while the snap stays on the same piece.
//!   Unsnapped again, they keep that turn and slide on from where the snap put them. With mirror
//!   mode on, snapping onto a moved brick's counterpart (it moves along, mirrored) meets it halfway
//!   across the mirror plane;
//! - with Alt held, a click of the line key ([`keybinds::LINE_POINT`], the right mouse button)
//!   places a point where snapping finds one, on any brick; two make a line, which shows while
//!   moving with the mouse free (see [`line_tool`]; the right button moves the camera again once
//!   Alt is let go), its length written beside it in the editor's unit (see [`line_label`]; so is
//!   the dashed line's from a first point to the mouse, to measure). With one of the line's points on
//!   the moved bricks, [`keybinds::TYPE_LENGTH`] (Enter) types the length it should have, and
//!   they slide along it to make it so (see [`typed_length`]). While there's a line, Alt + drag
//!   turns the moved bricks about it instead, the grabbed point going round it as near the mouse
//!   as it can, and Alt + a drag of the line key slides them along it (so does a drag with
//!   [`keybinds::SLIDE_ALONG_LINE`] held, no key by default: Alt + Shift switches Windows'
//!   keyboard layout); Ctrl turns them to where the grabbed face or edge matches the target best
//!   (R: the next way), or slides the grabbed point level with it (see [`hinge`]). A turn's angle
//!   shows past the grabbed point, in light purple; right after, [`keybinds::TYPE_LENGTH`] types
//!   the angle it should have instead of the line's length, for [`TURN_LASTS`] or until the move
//!   changes otherwise (see [`TurnedHinge`]): the same turn again from where the drag started, to
//!   that angle, the way it was dragged;
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
//! (`game::line_batcher`). The editor's left click (`Action_Select`, taken through
//! [`super::editor_buttons`]) starts and ends a resize instead of selecting while the resize key is
//! held over a scalable brick; the right button's (`Action_MoveCamera`) counts right clicks instead
//! of moving the camera meanwhile, and is the line key's (when bound to it) while a line point can
//! be placed.
//! While a button is held the engine reports the mouse's movement to the editor, which would turn
//! the camera with it: the resize takes it instead ([`super::mouse_moves`]), and follows it when
//! the engine holds the cursor still ([`Drag::point`]). The middle button's camera pivot
//! (`Action_PivotCamera`) is taken while fast editing is on (the resize key held, or toggled on) or
//! a drag goes on: it would hide the
//! cursor and hold it still, and the button may be one of the keys above (e.g. the model key, or
//! `copy_properties`' keys, which copy the brick [`nearest_hovered_brick`] tells). Each change writes
//! `BrickSize` and tells the brick like the editor's panel does (`game::scalable_brick::BrickSizes`), then moves the brick
//! like a committed move (`game::editor_object::SpawnTransforms`); the last one runs as an editor
//! operation (`game::brick_editor`), which makes the undo step. Moving bricks instead gives the
//! move mode its own place (`super::precise_moves::place_move`): the game then moves the selection,
//! its mirror mode counterparts and the gizmo, and commits the move itself.

mod draw;
mod geometry;
mod hinge;
mod key_mode;
mod line_label;
mod line_tool;
mod model;
mod motion;
mod parts;
mod resize;
mod scene;
mod snap;
mod spinner;
mod spinner_curve;
mod spinner_shape;
mod typed_length;

use std::collections::HashSet;
use std::sync::{Mutex, Once, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::editor_buttons::{self, Button, Click, Press};
use super::keybinds;
use super::mouse_moves::{self, Handling, Movement, Pointer};
use super::precise_moves::{self, Place};
use crate::framework::settings::{Section, Setting};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_model::BrickModels;
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{
    self, LocalBounds, Selection, SpawnTransforms, Vector, WorldTransforms,
};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::line_batcher::{self, LineBatcher};
use crate::game::mirror_mode::{self, MirrorMode};
use crate::game::mirrored_size::MirroredSizes;
use crate::game::move_mode::{self, Move, MoveMode};
use crate::game::scalable_brick::BrickSizes;
use crate::game::spinner::{Spinners, Values};
use crate::game::spinner_profiles::SpinnerProfiles;
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::platform::keyboard;
use crate::prelude::*;

use draw::{Drawing, PixelSize};
use geometry::{Bounds, Frame, Ray, Vec3};
use hinge::Hinge;
use line_label::{LineLabels, Segment};
use line_tool::LineTool;
use model::{BrickModel, Element, Models};
use motion::{Aim, Grab, Motion, Orientation};
use resize::{Grip, Resize};
use scene::{Piece, Placed, Scene, View};
use snap::Snapper;
use spinner::{SpinnerBrick, SpinnerModels};
use typed_length::Ready;

/// Whether fast editing is on: the resize key ([`keybinds::RESIZE_BRICKS`]) is held, or, with the
/// "Fast editing key" setting on "Tap to toggle", a tap of it has toggled it on (see [`key_mode`]).
pub use key_mode::is_on as is_fast_editing;

pub struct BrickResize;

/// Fast editing's settings, by section of the menu (see `features::SETTINGS`): how its key works,
/// what snapping snaps to, when it turns the moved bricks, and how long a line lasts.
pub static SETTINGS: [Section; 1] = [Section {
    title: "Fast editing",
    settings: &[
        Setting::Choice(&key_mode::KEY_MODE),
        Setting::Choice(&snap::POINTS),
        Setting::Choice(&snap::EDGE_FRACTIONS),
        Setting::Choice(&snap::CROSSINGS),
        Setting::Toggle(&snap::SEAMS),
        Setting::Choice(&snap::ANYWHERE),
        Setting::Choice(&spinner::ANGLE_STEPS),
        Setting::Choice(&motion::TURN_TO_MATCH),
        Setting::Toggle(&motion::SQUARE),
        Setting::Choice(&line_tool::LINE_LASTS),
    ],
    unlisted: &[],
}];

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
static LOG_HINGE: Once = Once::new();
static LOG_SLID: Once = Once::new();
static LOG_TYPED: Once = Once::new();
static LOG_REFUSED: Once = Once::new();

static LOG_FACE_TYPED: Once = Once::new();
static LOG_FACE_CHANGED: Once = Once::new();

/// What the label says when a length can't be typed (see [`typed_length`]): the line's, or over a
/// brick's edge or corner; and for how long.
const LINE_REFUSAL: &str = "Put one point on the moved bricks";
const FACE_REFUSAL: &str = "Sizes are typed on faces";
const REFUSAL_SHOWS: Duration = Duration::from_secs(3);
/// How long a face dragged (or given a typed size) stays ready for its size to be typed.
const FACE_LASTS: Duration = Duration::from_secs(10);
/// How long a turn about the line (or one given a typed angle) stays ready for its angle to be
/// typed; the line's length is typed again after.
const TURN_LASTS: Duration = Duration::from_secs(10);
static LOG_TURN_TYPED: Once = Once::new();
static LOG_TURN_CHANGED: Once = Once::new();
static LOG_CURSOR: Once = Once::new();
static LOG_NO_PLACES: Once = Once::new();
static LOG_NO_INTERFACE: Once = Once::new();
static LOG_NO_LINES: Once = Once::new();
static LOG_MODELS: Once = Once::new();
static LOG_NO_MODEL: Once = Once::new();
static LOG_NO_FIT: Once = Once::new();
static LOG_SNAPPED: Once = Once::new();
static LOG_SLOW_SNAP: Once = Once::new();
static LOG_SPINNER: Once = Once::new();
static LOG_SPINNER_SHAPE: Once = Once::new();
static LOG_SPINNER_BOX: Once = Once::new();
static LOG_CURVE: Once = Once::new();
static LOG_CURVE_DRAG: Once = Once::new();

/// What a finished resize or spinner change tells the editor changed: places (the brick's location
/// moves with its size) and a property, as the panel's final edits say. Without the property flag,
/// coop would send the new place only, not the new size or shape.
const CHANGED: u32 = brick_editor::MOVED | brick_editor::PROPERTY_CHANGED;

/// How long finding what to snap to may take before it's logged (once), in milliseconds.
const SLOW_SNAP_MS: f64 = 4.0;

/// How many segments the circle a grabbed point goes round a line on is drawn with.
const CIRCLE_SEGMENTS: usize = 64;

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
    /// To prefer selected bricks; without it none is.
    selection: Option<Selection>,
    /// For the counterparts of resized bricks; without it they stay as they are.
    mirror: Option<MirrorMode>,
    /// The order of a resized brick's size on its counterpart; without it the counterpart gets
    /// the same order (wrong for bricks mirrored by a quarter turn, like quarter pyramids).
    mirrored_sizes: Option<MirroredSizes>,
    /// Bricks' models; without them, bricks keep their boxes with the model key held.
    models: Option<BrickModels>,
    /// How the move mode moves bricks; without it (or without the selection, which tells the
    /// bricks moved), bricks can't be moved.
    moves: Option<&'static MoveMode>,
    /// Spinners' values and cross-sections; without both, spinners keep their box.
    spinners: Option<(Spinners, SpinnerProfiles)>,
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
                     (Ctrl) to snap, model key (middle or thumb mouse buttons) for models"
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
        selection,
        mirror: optional(mirror_mode::find(), "mirrored counterparts won't follow"),
        mirrored_sizes: optional(
            MirroredSizes::find(),
            "counterparts of bricks mirrored by a quarter turn get their size in the wrong order",
        ),
        models: optional(BrickModels::find(), "bricks keep their boxes on models"),
        moves,
        spinners: optional(
            Spinners::find().and_then(|spinners| Ok((spinners, SpinnerProfiles::find()?))),
            "spinners keep their box",
        ),
    };
    let _ = GAME.set(game);
    mouse_moves::listen(take_movement)?;
    editor_buttons::listen(on_button)
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

/// The left button (`Select`) starts and ends resizing or moving; the right one (`MoveCamera`) looks
/// deeper for snap targets while dragging, or places line points (see [`State::right_click`]); the
/// middle one (`PivotCamera`) doesn't pivot the camera while fast editing ([`State::pivot_click`]).
/// Whether the game's handler is skipped.
fn on_button(press: &Press) -> bool {
    let Some(game) = GAME.get() else {
        return false;
    };
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: an input handler's call, on the game thread.
    unsafe {
        match (press.button, press.click) {
            (Button::Select, Click::Pressed) => state.press(game),
            (Button::Select, Click::Released) => state.release(game),
            (Button::MoveCamera, click) => state.right_click(click),
            (Button::PivotCamera, click) => state.pivot_click(click),
            // Shift + click and the keys.
            _ => false,
        }
    }
}

/// The brick nearest along the mouse's ray while fast editing points at one, selected or not (the
/// one it points at may be a selected brick behind it), unless something is dragged. Game thread
/// only.
pub fn nearest_hovered_brick() -> Option<*mut UObjectBase> {
    let state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if state.drag.is_some() {
        return None;
    }
    let (scene, hover) = (state.scene.as_ref()?, state.hover.as_ref()?);
    scene.get(hover.nearest).object.get_live()
}

/// A key the player's input got (`key`, `EInputEvent` `event`), before the game and the keybinds
/// (called by `click_swap`'s hook): whether typing a line's length takes it (see
/// [`typed_length`]), so that nothing else gets it.
///
/// # Safety
/// `key` must be a valid name (of the live name pool). Game thread only.
pub unsafe fn typed_key(key: &FName, event: i32) -> bool {
    if !typed_length::is_listening() {
        return false;
    }
    // SAFETY: per the contract.
    let name = unsafe { crate::game::reflection::name_text(key) };
    typed_length::key(&name, event)
}

/// Whether fast editing drags something now (a resize, a move, a spinner's handle). Game thread
/// only.
pub fn is_dragging() -> bool {
    STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drag
        .is_some()
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
    let moved = movement.pixels();
    // A press of the line key: moved too far, it's no click.
    state.line_tool.mouse_moved(moved);
    let Some(drag) = &mut state.drag else {
        return if state.line_tool.took_button() {
            Handling::Take
        } else {
            Handling::Pass
        };
    };
    drag.pointer.moved(moved);
    Handling::Take
}

/// A brick's piece under the mouse while the resize key is held.
#[derive(Clone, Copy)]
struct Hover {
    /// The brick (see [`Scene::get`]).
    index: usize,
    /// The brick nearest along the ray, selected or not: `index` puts selected bricks first.
    nearest: usize,
    piece: Piece,
    /// Whether dragging it does anything: resizes the brick (see [`Self::grip`]), or moves the
    /// bricks being moved.
    grabbable: bool,
    /// How dragging it resizes the brick; `None` if it can't (the brick isn't scalable, or the
    /// piece drags no side), or while moving bricks.
    grip: Option<Grip>,
    /// The spinner's piece it's on, and how dragging it changes the spinner (`None` if it can't);
    /// `None` if it isn't one, or while moving bricks.
    spinner: Option<(spinner::Part, Option<spinner::Grip>)>,
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
    /// What it goes on while held.
    held_by: HeldBy,
    action: Action,
}

/// What holds a drag: the left mouse button (the game's `Select`), or the line key (sliding moved
/// bricks along the line, see [`line_tool`]).
#[derive(Clone, Copy, PartialEq)]
enum HeldBy {
    Select,
    LineKey,
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

// Only one drag goes on at a time, so its size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Action {
    Resize(Resizing),
    Move(Moving),
    Spinner(SpinnerEditing),
    Curve(spinner_curve::Editing),
}

/// A spinner being changed.
struct SpinnerEditing {
    brick: Edited,
    /// Its mirror mode counterpart, and the mirror axis.
    counterpart: Option<(Edited, u8)>,
    dragging: spinner::Dragging,
    /// The spinner as grabbed.
    grabbed: SpinnerBrick,
    /// The change asked for last (none yet: a click).
    edit: Option<spinner::Edit>,
    /// Where the mouse was pressed (screen pixels), until it moves away: a click changes nothing
    /// (not even a stepped angle, or a thickness the radius doesn't allow).
    pressed_at: Option<[f32; 2]>,
}

/// How far the mouse must move (pixels) for a press on a spinner to drag it.
const SPINNER_DRAG_PIXELS: f32 = 1.0;

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
    /// The grabbed brick as grabbed (where the move showed it), its box, its model (a spinner's
    /// shape) and what of it.
    frame: Frame,
    bounds: Bounds,
    model: Option<BrickModel>,
    spinner: Option<SpinnerBrick>,
    piece: Piece,
    grab: Grab,
    /// Where the move put its reference brick when grabbed.
    start: Place,
    /// The game's grid and angle steps for moves (0 while its snapping is off).
    grid: f64,
    angle: f64,
    /// While a line is placed: turning the bricks about it, or sliding them along it.
    hinge: Option<Hinge>,
    /// Whether dragged by the line key: then they slide along the line.
    by_line_key: bool,
    /// The motion given last.
    motion: Motion,
    /// The motion the last snap gave (none before one): unsnapped, the bricks go on from there.
    kept: Motion,
    /// Which way snapping turns the bricks, of those matching the target.
    turn: TurnChoice,
    /// The mirror plane's axis (0 to 2) while mirror mode is on: the moved bricks' counterparts
    /// move along, mirrored.
    mirror: Option<usize>,
    /// Where the mouse was pressed (screen pixels), until it points anywhere else, by however
    /// little: a click moves nothing (turning about a line, the grabbed point's nearest place to
    /// the mouse is a hair off where it is).
    pressed_at: Option<[f32; 2]>,
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

/// A brick changed by a resize, as it was grabbed.
struct Edited {
    object: WeakObject,
    /// Where the editor places it: location and rotation (pitch, yaw, roll).
    location: Vector,
    rotation: Vector,
}

/// A face whose size can be typed (see [`typed_length`]): the one just dragged, or one hovered while
/// fast editing when the box opened. A typed size resizes its brick again, as grabbed then (its
/// mirror counterpart too).
struct SizedFace {
    editor: WeakObject,
    resizing: Resizing,
    /// Where the vehicle is: fast editing may be off meanwhile, nothing gathered.
    vehicle: Frame,
    /// The brick as it was left: if anything else changes it (an undo, a move), the face is
    /// forgotten, as resizing it from how it was grabbed would undo that.
    left: Left,
    /// Since when it's ready (see [`FACE_LASTS`]).
    since: Instant,
    /// Whether its brick has been selected since: deselecting it then forgets the face.
    selected: bool,
}

/// Where a brick is (location, rotation) and its size.
type Left = (Vector, Vector, Vector);

/// A turn about the line whose angle can be typed (see [`typed_length`]): the last drag's, while
/// moving with the mouse free. A typed angle turns the moved bricks again from where that drag
/// started, the way it was dragged (the move goes on: the game ends it).
struct TurnedHinge {
    /// The move mode it turned in.
    mode: WeakObject,
    /// The turn (its line, the grabbed point, how far turned and slid), and where the move put its
    /// reference brick when the drag started.
    hinge: Hinge,
    start: Place,
    /// Which way it was dragged (see [`Hinge::way`]): angles show and are typed that way.
    way: f64,
    /// Where the vehicle is: fast editing may be off meanwhile, nothing gathered.
    vehicle: Frame,
    /// The move as it was left (see [`MoveKey`]), taken the first frame after: if anything else
    /// changes it (the gizmo, another drag), the turn is forgotten, as turning it from where the
    /// drag started would undo that.
    left: Option<MoveKey>,
    /// Since when it's ready (see [`TURN_LASTS`]).
    since: Instant,
}

/// What tells that a move changed: its own place and rotation, and the mouse's drag and turn on
/// top (see [`Move`]).
type MoveKey = (Vector, Vector, Vector, Vector);

fn move_key(now: &Move) -> MoveKey {
    (now.location, now.rotation, now.drag, now.turn)
}

/// A press of the type key that couldn't type a length, told for [`REFUSAL_SHOWS`].
#[derive(Clone, Copy)]
struct Refusal {
    since: Instant,
    /// Where: on the line's label, or at a point of a brick (in the vehicle).
    at: Option<Vec3>,
    text: &'static str,
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
    /// Bricks' models made so far, kept across scenes, and spinners'.
    models: Option<Models>,
    spinner_models: Option<SpinnerModels>,
    /// The line moved bricks turn about or slide along, and placing it.
    line_tool: LineTool,
    /// The lengths of the line and of the dashed one to a second point (see [`line_label`]).
    line_labels: LineLabels,
    /// The last press of the type key that couldn't type a length (see [`typed_length`]).
    refused: Option<Refusal>,
    /// The face whose size can be typed, and whether it was chosen for the box open (once open,
    /// the face hovered doesn't take its place).
    face: Option<SizedFace>,
    face_open: bool,
    /// Whether the last frame typed in a move (the line's length) or not (a face's size): when
    /// that changes, the box closes rather than type for something else.
    typing_moves: Option<bool>,
    /// The last turn about the line, whose angle can be typed, and whether the type key is for it
    /// (else for the line's length): when that changes, the box closes too.
    turn: Option<TurnedHinge>,
    typing_turn: bool,
    /// Whether the editor is in a move with the mouse free, where the line shows.
    line_shows: bool,
    /// The moved brick hovered when the line key went down with a line placed: a drag of the key
    /// from there slides the bricks along the line.
    slide_from: Option<Hover>,
    /// Whether the camera pivot's press was taken (see [`Self::pivot_click`]): then so is its
    /// release.
    pivot_taken: bool,
    /// Which spinner cross-section keys were held last frame (see [`spinner_shape`]).
    cross_section_keys: spinner_shape::Keys,
    /// The curve the selected spinners make (see [`spinner_curve`]), in the default mode, and the
    /// selected spinners it was looked for with (by index in the scene): looked for again when
    /// they change.
    chain: Option<spinner_curve::Chain>,
    chain_of: Option<Vec<usize>>,
    /// The curve's control under the mouse, and the mouse then: it comes before the bricks.
    curve_hover: Option<(spinner_curve::Control, View)>,
    /// While moving with the snap key held, the hovered brick's own snap point a press would grab
    /// (see [`grabbed`]). Made when first needed, like the line tool's.
    grab_snap: Option<Snapper>,
}

// SAFETY: object pointers are only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

impl Drag {
    /// Whether what holds it is still down.
    fn is_held(&self) -> bool {
        match self.held_by {
            HeldBy::Select => keyboard::is_mouse_button_down(),
            HeldBy::LineKey => keybinds::LINE_POINT.is_held(),
        }
    }

    /// The grabbed point now (in the vehicle), as deep as the pointer shows.
    fn grabbed_point(&self) -> Vec3 {
        match &self.action {
            Action::Resize(resizing) => resizing.resize.frame.point(resizing.resize.grab),
            Action::Move(moving) => moving.motion.point(moving.grab.point),
            Action::Spinner(editing) => editing.dragging.grabbed(),
            Action::Curve(editing) => editing.grabbed(),
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
            spinner_models: None,
            line_tool: LineTool::new(),
            line_labels: LineLabels::new(),
            refused: None,
            face: None,
            face_open: false,
            typing_moves: None,
            turn: None,
            typing_turn: false,
            line_shows: false,
            slide_from: None,
            pivot_taken: false,
            cross_section_keys: spinner_shape::Keys::new(),
            chain: None,
            chain_of: None,
            curve_hover: None,
            grab_snap: None,
        }
    }

    /// Every frame of `editor`.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn tick(&mut self, game: &Game, editor: *mut UObjectBase) {
        let key_pressed = key_mode::update();
        let held = is_fast_editing();
        if !held
            && self.drag.is_none()
            && self.scene.is_none()
            && !self.drawn
            && self.line_tool.is_idle()
            && self.face.is_none()
            && self.turn.is_none()
        {
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
                    self.curve_hover = None;
                    self.slide_from = None;
                    self.scene = None;
                    self.chain = None;
                    self.chain_of = None;
                    self.grab_snap = None;
                    self.line_tool.forget_bricks();
                }
                if let Some(wanted) = &wanted
                    && self.scene.is_none()
                {
                    let spinners = self.spinner_models.get_or_insert_with(Default::default);
                    self.scene = Some(gather(game, editor, wanted, spinners));
                }
                self.gathered = wanted;
                self.find_chain(game);
            }
            if self.on_models
                && let (Some(scene), Some(reader)) = (&mut self.scene, &game.models)
                && !scene.has_models()
            {
                let models = self.models.get_or_insert_with(Models::default);
                add_models(reader, models, scene);
            }
            self.line_tool
                .in_move(game.modes.move_mode(editor).is_some());
            // The line shows while moving with the mouse free.
            let show_line = game.modes.moving(editor) == Some(editor_mode::Moving::Precisely);
            self.line_shows = show_line;
            let controller = game.editor.player_controller(editor);
            if key_pressed {
                // A game button hovered before keeps the keyboard: Enter would press it.
                self.line_labels.give_keyboard_to_view(controller);
            }
            // Where the vehicle is: from its bricks, else from when the line was placed.
            let vehicle = match &self.scene {
                Some(scene) => Some(scene.vehicle),
                None => (self.line_tool.vehicle())
                    .or_else(|| self.face.as_ref().map(|face| face.vehicle))
                    .or_else(|| self.turn.as_ref().map(|turn| turn.vehicle)),
            };
            let view = match vehicle {
                Some(vehicle) if !controller.is_null() => {
                    let cursor = game.viewport.mouse(controller);
                    let mouse = match &mut self.drag {
                        Some(drag) => Some(drag.pointer.point(cursor)),
                        None => cursor,
                    };
                    mouse.and_then(|mouse| view(game, controller, mouse, &vehicle))
                }
                _ => None,
            };
            if let Some(drag) = &self.drag {
                if !drag.is_held() {
                    // The release went elsewhere (another window), or it's the line key's: finish
                    // here.
                    self.finish(game);
                } else if let Some(view) = &view {
                    self.follow(game, editor, view);
                }
            } else if let Some(view) = &view {
                self.point(game, editor, view);
            } else {
                // The mouse is off the view.
                self.hover = None;
                self.curve_hover = None;
            }
            // A press of a spinner cross-section key over a spinner (read every frame, so that
            // holding the key counts once).
            if let Some(step) = self.cross_section_keys.pressed()
                && held
                && self.drag.is_none()
                && matches!(self.gathered, Some(Gathered::Editing))
            {
                self.cycle_cross_section(game, editor, step);
            }
            self.place_line(game, view.as_ref(), held);
            self.type_value(game, editor);
            self.label(
                game,
                controller,
                vehicle.as_ref(),
                show_line && view.is_some(),
            );
            self.draw(game, view.as_ref(), show_line);
        }
    }

    /// Looks for the curve the selected spinners make (see [`spinner_curve`]) when they changed,
    /// in the editor's default mode only.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn find_chain(&mut self, game: &Game) {
        let Some(scene) = &self.scene else {
            self.chain = None;
            self.chain_of = None;
            return;
        };
        let selected: Vec<usize> = if matches!(self.gathered, Some(Gathered::Editing)) {
            scene
                .bricks()
                .iter()
                .enumerate()
                // SAFETY: per the contract.
                .filter(|(_, placed)| {
                    placed.spinner.is_some() && unsafe { is_selected(game, placed) }
                })
                .map(|(index, _)| index)
                .collect()
        } else {
            Vec::new()
        };
        if self.chain_of.as_ref() == Some(&selected) {
            return;
        }
        self.chain = if selected.is_empty() {
            None
        } else {
            spinner_curve::Chain::of(scene, &selected)
        };
        if let Some(chain) = &self.chain {
            let count = chain.len();
            LOG_CURVE.call_once(|| log!("the selected spinners make a curve ({count} spinners)"));
        }
        self.chain_of = Some(selected);
    }

    /// The line key (see [`line_tool`]): while a point can be placed (the resize key `held` in a
    /// move with the mouse free, nothing dragged), what a click would place shows, and a click
    /// places it; a drag from a moved brick with a line placed slides the bricks along it. The
    /// clear key clears it.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn place_line(&mut self, game: &Game, view: Option<&View>, held: bool) {
        self.line_tool.clear_key(keybinds::CLEAR_LINE.is_held());
        // Wherever the line shows, even in a move that lost its brick: with nothing gathered there's
        // nothing to place, and a click clears.
        let can_be_placed = held && self.drag.is_none() && self.line_shows;
        // The key first: a press places what showed before it.
        let vehicle = self.scene.as_ref().map(|scene| scene.vehicle);
        let pressed = self.line_tool.key(
            keybinds::LINE_POINT.is_held(),
            can_be_placed,
            view.map(|view| view.mouse),
            vehicle,
        );
        if pressed {
            let line = self.line_tool.line().is_some();
            self.slide_from = self.hover.filter(|hover| line && hover.grabbable);
        }
        if self.line_tool.is_dragging()
            && let Some(hover) = self.slide_from.take()
            && self.line_tool.line().is_some()
        {
            // The press is the drag's now: its release places nothing.
            self.line_tool.cancel_press();
            // SAFETY: per the contract.
            if unsafe { self.press_move(game, hover, HeldBy::LineKey) } {
                LOG_SLID.call_once(|| log!("sliding bricks along a line (a drag of the line key)"));
            }
        }
        match (&self.scene, view) {
            (Some(scene), Some(view)) if can_be_placed => {
                self.line_tool.preview(scene, view, self.on_models)
            }
            _ => self.line_tool.stop_preview(),
        }
    }

    /// Typing an exact length (see [`typed_length`]): what the type key does in the editor's mode
    /// now, and applying a length committed. From the editor rather than from the gathered bricks:
    /// the resize key is let go meanwhile (Alt + Enter is the engine's full screen key), and with
    /// it held only is anything gathered.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn type_value(&mut self, game: &Game, editor: *mut UObjectBase) {
        // SAFETY: per the contract.
        let wanted = unsafe { wanted(game, editor) };
        let moves = wanted
            .as_ref()
            .map(|wanted| matches!(wanted, Gathered::Moving { .. }));
        if moves != self.typing_moves {
            typed_length::set_ready(Ready::No);
            typed_length::take_committed();
            self.typing_moves = moves;
        }
        // SAFETY: per the contract.
        unsafe {
            match wanted {
                Some(Gathered::Moving { mode, now }) => {
                    self.face = None;
                    self.type_length(game, editor, mode, &now);
                }
                Some(Gathered::Editing) => {
                    self.forget_turn();
                    self.type_size(game, editor);
                }
                None => {
                    self.face = None;
                    self.forget_turn();
                    typed_length::set_ready(Ready::No);
                }
            }
        }
        if !matches!(self.refused, Some(refusal) if refusal.since.elapsed() < REFUSAL_SHOWS) {
            self.refused = None;
        }
    }

    /// Tells that a press of the type key couldn't type a length (see [`Refusal`]).
    fn refuse(&mut self, at: Option<Vec3>, text: &'static str) {
        self.refused = Some(Refusal {
            since: Instant::now(),
            at,
            text,
        });
        LOG_REFUSED.call_once(|| log!("a length can't be typed there: {text}"));
    }

    /// Typing the line's length while moving with the mouse free (the move mode `mode`, as it is
    /// `now`): only with one of its points on the moved bricks. A length committed slides the moved
    /// bricks along the line until the point on them is that far from the other.
    ///
    /// # Safety
    /// `editor` must be the live editor, `mode` its move mode. Game thread only.
    unsafe fn type_length(
        &mut self,
        game: &Game,
        editor: *mut UObjectBase,
        mode: *mut UObjectBase,
        now: &Move,
    ) {
        // SAFETY: per the contract.
        if unsafe { self.type_turn(game, editor, mode, now) } {
            return;
        }
        let line = self.line_shows && self.drag.is_none() && self.line_tool.line().is_some();
        // The moved bricks are the selected ones.
        let moved_end = line
            .then(|| {
                self.line_tool
                    // SAFETY: per the contract.
                    .moved_end(|object| unsafe { is_selected_object(game, object) })
            })
            .flatten();
        typed_length::set_ready(match (line, moved_end) {
            (false, _) => Ready::No,
            (true, None) => Ready::Refused,
            (true, Some(_)) => Ready::Yes,
        });
        if typed_length::take_refused() {
            self.refuse(None, LINE_REFUSAL);
        }
        typed_length::take_cancelled();
        let (Some(index), Some(text)) = (moved_end, typed_length::take_committed()) else {
            return;
        };
        let (Some(ends), Some(moves)) = (self.line_tool.ends(), game.moves) else {
            return;
        };
        let (moved, other) = (ends[index], ends[1 - index]);
        let away = geometry::sub(moved, other);
        let now_length = geometry::length(away);
        // SAFETY: per the contract.
        let Some(length) = (unsafe { line_label::typed_length(&text, now_length) }) else {
            log!("typed line length '{text}' isn't a length");
            return;
        };
        // The moved point goes that far from the other one, along the line.
        let direction = geometry::scale(away, 1.0 / now_length);
        let to = geometry::add(other, geometry::scale(direction, length));
        let shift = geometry::sub(to, moved);
        let start = precise_moves::effective(mode, now);
        let place = Place {
            location: geometry::add(start.location, shift),
            rotation: start.rotation,
        };
        // SAFETY: per the contract; the move mode is the editor's current one (just gathered).
        unsafe {
            if let Err(reason) = moves.keep_snapping_off(editor) {
                LOG_SNAPPING_ON.call_once(|| {
                    log!(
                        "moved bricks may shift to connectors while the game's snapping is on: {reason}"
                    )
                });
            }
            precise_moves::place_move(moves, mode, place);
        }
        self.line_tool.move_end(index, to);
        LOG_TYPED.call_once(|| log!("line length typed: {length:.3} cm, the moved bricks slid"));
    }

    /// Forgets the turn whose angle could be typed (out of the move it was in).
    fn forget_turn(&mut self) {
        self.turn = None;
        self.typing_turn = false;
    }

    /// Typing the angle of the last turn about the line (see [`TurnedHinge`]) while moving with the
    /// mouse free (the move mode `mode`, as it is `now`): while it's kept, it comes before the
    /// line's length. Whether the type key is for it this frame (else the line's length).
    ///
    /// # Safety
    /// `editor` must be the live editor, `mode` its move mode. Game thread only.
    unsafe fn type_turn(
        &mut self,
        game: &Game,
        editor: *mut UObjectBase,
        mode: *mut UObjectBase,
        now: &Move,
    ) -> bool {
        let open = typed_length::is_open();
        // An angle typed just before (a click elsewhere commits the box) still applies.
        let in_use = open || typed_length::has_committed();
        if let Some(turn) = &mut self.turn {
            let key = move_key(now);
            let left = *turn.left.get_or_insert(key);
            let there = turn.mode.is(mode) && left == key;
            if !there {
                LOG_TURN_CHANGED.call_once(|| {
                    log!("a turn's angle can't be typed any more: the move changed otherwise")
                });
            }
            if !there || !(in_use || turn.since.elapsed() < TURN_LASTS) {
                self.turn = None;
            }
        }
        let for_turn = self.turn.is_some() && self.drag.is_none() && self.line_shows;
        if for_turn != self.typing_turn {
            self.typing_turn = for_turn;
            if open {
                // The box was open for the line's length, or the turn is gone: it closes rather
                // than type for the other.
                typed_length::set_ready(Ready::No);
                typed_length::take_committed();
                return true;
            }
        }
        if !for_turn {
            return false;
        }
        typed_length::set_ready(Ready::Yes);
        typed_length::take_refused();
        if typed_length::take_cancelled() {
            self.turn = None;
            return true;
        }
        if let Some(text) = typed_length::take_committed() {
            // SAFETY: per the contract.
            unsafe { self.turn_to(game, editor, mode, &text) };
        }
        true
    }

    /// Turns the moved bricks about the kept turn's line to the angle `text` asks for (see
    /// [`TurnedHinge`]), from where its drag started: the move's new place.
    ///
    /// # Safety
    /// `editor` must be the live editor, `mode` its move mode. Game thread only.
    unsafe fn turn_to(
        &mut self,
        game: &Game,
        editor: *mut UObjectBase,
        mode: *mut UObjectBase,
        text: &str,
    ) {
        let (Some(turn), Some(moves)) = (&mut self.turn, game.moves) else {
            return;
        };
        let shown = turn.hinge.turned * turn.way;
        let Some(angle) = typed_formula::typed_angle(text, shown) else {
            log!("typed angle '{text}' isn't an angle");
            return;
        };
        turn.hinge.turned = angle * turn.way;
        let motion = turn.hinge.motion();
        let place = Place {
            location: motion.point(turn.start.location),
            rotation: motion.rotation * turn.start.rotation,
        };
        // SAFETY: per the contract; the move mode is the editor's current one (just gathered).
        unsafe {
            if let Err(reason) = moves.keep_snapping_off(editor) {
                LOG_SNAPPING_ON.call_once(|| {
                    log!(
                        "moved bricks may shift to connectors while the game's snapping is on: {reason}"
                    )
                });
            }
            precise_moves::place_move(moves, mode, place);
        }
        // The move changed: it's taken as left again next frame.
        turn.left = None;
        turn.since = Instant::now();
        LOG_TURN_TYPED
            .call_once(|| log!("turn angle typed: {angle:.3} degrees, the bricks turned"));
    }

    /// Typing a face's size in the default mode (see [`SizedFace`]): the face hovered while fast
    /// editing when the box opens, else the one kept (just dragged). A size committed resizes its
    /// brick again.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn type_size(&mut self, game: &Game, editor: *mut UObjectBase) {
        let open = typed_length::is_open();
        // Taken first: the face it's for stays while it's applied, however long typing took.
        let committed = typed_length::take_committed();
        if typed_length::take_cancelled() {
            self.face = None;
        }
        if let Some(face) = &self.face {
            // SAFETY: per the contract.
            let there = unsafe { face.is_as_left(game, editor) };
            if !there {
                LOG_FACE_CHANGED.call_once(|| {
                    log!("a face's size can't be typed any more: its brick changed (undone, moved)")
                });
            }
            // SAFETY: on the game thread.
            let selected = unsafe { is_selected_object(game, &face.resizing.brick.object) };
            // A size typed just before (a click elsewhere commits the box) still applies.
            let deselected = face.selected && !selected && committed.is_none();
            let in_use = open || committed.is_some();
            if !there || deselected || !(in_use || face.since.elapsed() < FACE_LASTS) {
                self.face = None;
            } else if let Some(face) = &mut self.face {
                face.selected |= selected;
            }
        }
        // What fast editing points at, unless something is dragged.
        let hovered = self
            .hover
            .filter(|hover| self.drag.is_none() && hover.grip.is_some());
        let hovered_face = hovered.filter(|hover| {
            hover
                .grip
                .is_some_and(|grip| grip.part.dimensions_fixed() == 1)
        });
        if open
            && !self.face_open
            && let (Some(hover), Some(scene)) = (hovered_face, &self.scene)
        {
            // The box just opened over a face.
            // SAFETY: per the contract.
            if let Some(face) = unsafe { SizedFace::of_hover(game, scene, &hover) } {
                self.face = Some(face);
            }
        }
        self.face_open = open;
        let ready = if self.drag.is_some() {
            Ready::No
        } else if self.face.is_some() || (!open && hovered_face.is_some()) {
            Ready::Yes
        } else if hovered.is_some() && !open {
            // An edge or a corner.
            Ready::Refused
        } else {
            Ready::No
        };
        typed_length::set_ready(ready);
        if typed_length::take_refused() {
            self.refuse(hovered.map(|hover| hover.point), FACE_REFUSAL);
        }
        if let Some(text) = committed {
            // SAFETY: per the contract.
            unsafe { self.size_face(game, &text) };
        }
    }

    /// Gives the face kept the size `text` asks for (see [`SizedFace`]), as one editor operation
    /// (one undo step).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn size_face(&mut self, game: &Game, text: &str) {
        let Some(face) = &mut self.face else {
            return;
        };
        let now = face.extent();
        // SAFETY: per the contract.
        let typed = unsafe { line_label::typed_length(text, now) };
        let Some(extent) = typed.filter(|&extent| extent > 0.0) else {
            log!("typed size '{text}' isn't a size");
            return;
        };
        let (Some(size), Some(editor)) = (
            face.resizing.resize.size_with_extent(extent),
            face.editor.get(),
        ) else {
            return;
        };
        let size = math::narrow(size);
        let resizing = &mut face.resizing;
        // SAFETY: per the contract; the editor is live, the brick checked this frame.
        unsafe {
            game.editor.operation(editor, CHANGED, || {
                apply(game, resizing, size, false);
            });
        }
        resizing.asked = size;
        let objects = resizing.objects();
        // SAFETY: per the contract; `objects` are live.
        if let Some(left) = objects
            .first()
            .and_then(|&object| unsafe { left(game, object) })
        {
            face.left = left;
        }
        face.since = Instant::now();
        let kept = face.extent();
        LOG_FACE_TYPED.call_once(|| {
            log!("face size typed: {extent:.3} cm asked, {kept:.3} cm kept by the brick")
        });
        // SAFETY: per the contract.
        unsafe { self.update_scene(game, &objects) };
    }

    /// Writes lengths over the view of `controller` (see [`line_label`]), with the vehicle at
    /// `vehicle`: the line's and the dashed one's to a second point if they show (`show_line`), and
    /// the size of a face dragged or kept (see [`SizedFace`]); hides them otherwise. While a length
    /// is typed, a text box shows instead of its label (and how it closes goes to
    /// [`typed_length`]); a moment after a refusal, why.
    ///
    /// # Safety
    /// `controller` must be a live player controller or null. Game thread only.
    unsafe fn label(
        &mut self,
        game: &Game,
        controller: *mut UObjectBase,
        vehicle: Option<&Frame>,
        show_line: bool,
    ) {
        let open = typed_length::is_open();
        // What the text box opens with, for a length typed.
        let edit = |ends: [Vec3; 2]| {
            let now = geometry::length(geometry::sub(ends[1], ends[0]));
            // SAFETY: on the game thread.
            open.then(|| unsafe { line_label::edit_text(now) })
        };
        let mut measured = Vec::new();
        if show_line {
            if let Some(ends) = self.line_tool.ends() {
                let refusal = self.refused.filter(|refusal| refusal.at.is_none());
                let text = refusal.map(|refusal| refusal.text.to_owned());
                let mut line = Segment::line(ends, text);
                if !self.typing_turn {
                    line.edit = edit(ends);
                }
                measured.push(line);
            }
            if let Some(ends) = self.line_tool.dashed() {
                measured.push(Segment::line(ends, None));
            }
            // The angle of a turn about the line: while dragging it, then while it can be typed.
            let turned = match &self.drag {
                Some(Drag {
                    action: Action::Move(moving),
                    ..
                }) if !moving.slides() => moving.hinge.map(|hinge| (hinge, hinge.way())),
                Some(_) => None,
                None => self.turn.as_ref().map(|turn| (turn.hinge, turn.way)),
            };
            if let Some((hinge, way)) = turned {
                let text = typed_formula::angle_text(hinge.turned * way);
                let typing = open && self.typing_turn && self.drag.is_none();
                let edit = typing.then(|| text.clone());
                measured.push(Segment::angle(hinge.angle_ends(), text, edit));
            }
        }
        let dragged = match &self.drag {
            Some(Drag {
                action: Action::Resize(resizing),
                ..
            }) => size_line(resizing),
            _ => None,
        };
        if let Some(ends) = dragged {
            measured.push(Segment::face(ends, None));
        } else if let Some(ends) = self
            .face
            .as_ref()
            .and_then(|face| size_line(&face.resizing))
        {
            measured.push(Segment::face(ends, edit(ends)));
        }
        if let Some(Refusal {
            at: Some(at), text, ..
        }) = self.refused
        {
            let mut refusal = Segment::face([at, at], None);
            refusal.text = Some(text.to_owned());
            refusal.spot = line_label::Spot::Beside;
            measured.push(refusal);
        }
        let escape = typed_length::take_escaped() || keyboard::is_escape_down();
        // SAFETY: per the contract.
        let closed = unsafe {
            self.line_labels
                .show(&game.viewport, controller, &measured, vehicle, escape)
        };
        if let Some(closed) = closed {
            typed_length::close(closed);
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
        let pick = |preferred: &dyn Fn(&Placed) -> bool| {
            // SAFETY: per the contract.
            scene.pick(
                view,
                self.on_models,
                |placed| moving && !placed.moving,
                preferred,
                0,
                |point| unsafe { project(game, editor, &vehicle, point) },
            )
        };
        // SAFETY: per the contract.
        let selected = |placed: &Placed| unsafe { is_selected(game, placed) };
        let pointed = pick(&selected);
        let on_models = self.on_models;
        let previous = self.hover.map(|hover| hover.index);
        self.hover = pointed.map(|mut pointed| {
            let placed = scene.get(pointed.index);
            // Moved by a spinner's own pieces (its caps' corners and edges, its rims...), not its
            // model's facets, unless the model key is held.
            if moving
                && !on_models
                && let (Some(brick), Some(model), Piece::Model(element)) =
                    (&placed.spinner, &placed.model, pointed.piece)
            {
                let local = placed.frame.local(pointed.point);
                let part = spinner::Part::moved_of(brick, model, element, local);
                pointed.piece = Piece::Spinner(part);
            }
            // Picked again without preferring selected bricks only if a selected one came first.
            let nearest = if selected(placed) {
                pick(&|_| false).map_or(pointed.index, |nearest| nearest.index)
            } else {
                pointed.index
            };
            let grip = (placed.scalable && !moving)
                .then(|| Grip::of(pointed.piece, &placed.bounds, placed.model.as_ref()))
                .flatten();
            let spinner = (!moving)
                .then(|| spinner_grip(placed, pointed.piece, pointed.point, view))
                .flatten();
            Hover {
                index: pointed.index,
                nearest,
                piece: pointed.piece,
                grabbable: moving
                    || grip.is_some()
                    || spinner.is_some_and(|(_, grip)| grip.is_some()),
                grip,
                spinner,
                point: pointed.point,
                view: *view,
            }
        });
        self.curve_hover = (!moving)
            .then(|| {
                let control = self.chain.as_ref()?.control_under(view)?;
                Some((control, *view))
            })
            .flatten();
        // A spinner's hub (it shows while the spinner is hovered) comes before what's under it:
        // it's often in the empty middle of the ring. The spinner hovered now, or just before.
        if !moving
            && let Some(index) = [self.hover.map(|hover| hover.index), previous]
                .into_iter()
                .flatten()
                .find(|&index| {
                    let placed = scene.get(index);
                    placed.spinner.is_some() && spinner::Part::hub_under(&placed.frame, view)
                })
        {
            let placed = scene.get(index);
            let part = spinner::Part::Hub;
            let grip = placed.spinner.as_ref().and_then(|brick| part.grip(brick));
            self.hover = Some(Hover {
                index,
                nearest: index,
                piece: Piece::Spinner(part),
                grabbable: grip.is_some(),
                grip: None,
                spinner: Some((part, grip)),
                point: placed.frame.point([0.0; 3]),
                view: *view,
            });
        }
        // With the snap key held over a moved brick: the snap point of its own a press would grab.
        let grab_snap = self.grab_snap.get_or_insert_with(Snapper::default);
        match self
            .hover
            .filter(|_| moving && keybinds::SNAP_RESIZE.is_held())
        {
            Some(hover) => {
                let hovered = scene.get(hover.index);
                let others = |other: &Placed| !core::ptr::eq(other, hovered);
                grab_snap.snap(&snap::snapping(), scene, view, on_models, others);
            }
            None => grab_snap.stop(),
        }
    }

    /// The hovered brick's own snap point a press would grab now, and its piece there (see
    /// [`grabbed`]).
    fn grab_preview(&self) -> Option<(Piece, &snap::Snap)> {
        let hover = self.hover?;
        let snapped = self.grab_snap.as_ref()?.snapped()?;
        own_snap(snapped, hover.index).map(|piece| (piece, snapped))
    }

    /// Starts a drag on a click with the resize key held over a brick's piece that can be dragged:
    /// resizing a scalable brick, or moving the bricks being moved; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press(&mut self, game: &Game) -> bool {
        if self.drag.is_some() {
            // A drag of the line key is going on: the click would end the move under it.
            return true;
        }
        if !is_fast_editing() {
            return false;
        }
        // SAFETY: per the contract.
        let pressed = unsafe {
            match (&self.gathered, self.hover) {
                (Some(Gathered::Editing), _) => self.press_resize(game),
                (Some(Gathered::Moving { .. }), Some(hover)) => {
                    self.press_move(game, hover, HeldBy::Select)
                }
                _ => false,
            }
        };
        if pressed {
            self.line_tool.cancel_press();
            self.face = None;
        }
        pressed
    }

    /// Starts resizing the hovered brick, if it can be; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_resize(&mut self, game: &Game) -> bool {
        if let Some((control, view)) = self.curve_hover {
            // SAFETY: per the contract.
            return unsafe { self.press_curve(game, control, &view) };
        }
        let (Some(scene), Some(hover)) = (&self.scene, &self.hover) else {
            return false;
        };
        if let Some((_, grip)) = hover.spinner {
            // SAFETY: per the contract.
            return grip.is_some_and(|grip| unsafe { self.press_spinner(game, grip) });
        }
        // SAFETY: per the contract.
        let Some((editor, resizing)) = (unsafe { resizing_of(game, scene, hover) }) else {
            return false;
        };
        LOG_RESIZED.call_once(|| {
            log!(
                "resizing a brick by {} (size {:?}, box {:?})",
                describe(hover.piece),
                resizing.asked,
                resizing.bounds
            )
        });
        self.drag = Some(Drag {
            // SAFETY: a live editor (just found).
            editor: unsafe { WeakObject::new(editor) },
            pointer: Pointer::new(hover.view.mouse),
            snapper: Snapper::default(),
            snap_key: SnapKey::default(),
            held_by: HeldBy::Select,
            action: Action::Resize(resizing),
        });
        true
    }

    /// Starts dragging `control` of the selected spinners' curve, the mouse at `view`; whether it
    /// did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_curve(
        &mut self,
        game: &Game,
        control: spinner_curve::Control,
        view: &View,
    ) -> bool {
        let Some(chain) = &self.chain else {
            return false;
        };
        if !game.spawn.known() {
            LOG_NO_PLACES.call_once(|| {
                log!("can't change spinners: where bricks keep their place wasn't found")
            });
            return false;
        }
        // SAFETY: per the contract; the chain's spinners are the editor's.
        unsafe {
            let Some(editing) = spinner_curve::Editing::new(game, chain, control, view) else {
                return false;
            };
            let Some(&object) = editing.objects().first() else {
                return false;
            };
            let editor = game.bricks.editor_of(object);
            if editor.is_null() {
                return false;
            }
            let count = chain.len();
            LOG_CURVE_DRAG
                .call_once(|| log!("shaping a curve of {count} spinners by its {control:?}"));
            self.drag = Some(Drag {
                editor: WeakObject::new(editor),
                pointer: Pointer::new(view.mouse),
                snapper: Snapper::default(),
                snap_key: SnapKey::default(),
                held_by: HeldBy::Select,
                action: Action::Curve(editing),
            });
        }
        true
    }

    /// Starts changing the hovered spinner by `grip`; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_spinner(&mut self, game: &Game, grip: spinner::Grip) -> bool {
        let (Some(scene), Some(hover), Some((spinners, _))) =
            (&self.scene, &self.hover, &game.spinners)
        else {
            return false;
        };
        let placed = scene.get(hover.index);
        let (Some(object), Some(brick)) = (placed.object.get(), &placed.spinner) else {
            return false;
        };
        if !game.spawn.known() {
            LOG_NO_PLACES.call_once(|| {
                log!("can't change spinners: where bricks keep their place wasn't found")
            });
            return false;
        }
        // SAFETY: per the contract; `object` is a live spinner in the editor.
        unsafe {
            let editor = game.bricks.editor_of(object);
            let Some(edited_brick) = edited(game, object) else {
                return false;
            };
            if editor.is_null() {
                return false;
            }
            let counterpart = game.mirror.as_ref().and_then(|mirror| {
                let axis = mirror.axis()?;
                let other = game.code.mirrored_object(object)?;
                spinners.is_spinner(other).then_some(())?;
                Some((edited(game, other)?, axis))
            });
            let Some(dragging) =
                spinner::Dragging::new(grip, placed.frame, brick.spinner, hover.point)
            else {
                return false;
            };
            LOG_SPINNER.call_once(|| {
                log!(
                    "changing a spinner by {} ({:?})",
                    grip.describe(),
                    brick.stored
                )
            });
            self.drag = Some(Drag {
                editor: WeakObject::new(editor),
                pointer: Pointer::new(hover.view.mouse),
                snapper: Snapper::default(),
                snap_key: SnapKey::default(),
                held_by: HeldBy::Select,
                action: Action::Spinner(SpinnerEditing {
                    brick: edited_brick,
                    counterpart,
                    dragging,
                    grabbed: brick.clone(),
                    edit: None,
                    pressed_at: Some(hover.view.mouse),
                }),
            });
        }
        true
    }

    /// Starts moving the bricks of the move by the `hover`ed one, a drag `held_by` the left button
    /// or the line key; whether it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press_move(&mut self, game: &Game, hover: Hover, held_by: HeldBy) -> bool {
        let (Some(scene), Some(Gathered::Moving { mode, now })) = (&self.scene, &self.gathered)
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
        let own = self.grab_snap.as_ref().and_then(Snapper::snapped);
        let (piece, point) = grabbed(scene, &hover, own);
        let hinge = self.line_tool.line().map(|line| Hinge::new(line, point));
        if hinge.is_some() {
            LOG_HINGE.call_once(|| log!("moving bricks about a line"));
        }
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
            spinner: placed.spinner.clone(),
            piece,
            grab: Grab {
                point,
                orientation: Orientation::of(placed, piece, point),
                axes: placed.frame.axes,
                across: Grab::across_for(hover.view.ray.direction),
            },
            start: precise_moves::effective(*mode, now),
            grid: f64::from(now.grid),
            angle: f64::from(now.angle),
            hinge,
            by_line_key: held_by == HeldBy::LineKey,
            motion: Motion::NONE,
            kept: Motion::NONE,
            turn: TurnChoice::default(),
            // SAFETY: per the contract.
            mirror: unsafe { mirror_axis(game) },
            // Where the drag's pointer starts.
            pressed_at: Some(hover.view.mouse),
        };
        self.drag = Some(Drag {
            // SAFETY: the editor is live.
            editor: unsafe { WeakObject::new(editor) },
            pointer: Pointer::new(hover.view.mouse),
            snapper: Snapper::default(),
            snap_key: SnapKey::default(),
            held_by,
            action: Action::Move(moving),
        });
        true
    }

    /// While dragging, takes the right button (the camera stays still): each press sets aside one
    /// more of the bricks the mouse goes through for snapping. Whether it took it.
    ///
    /// Else, with the line key bound to it (down with it) while a point can be placed, its press
    /// and release are the line key's (see [`line_tool`]): the camera would capture the mouse.
    fn right_click(&mut self, click: Click) -> bool {
        let pressed = click == Click::Pressed;
        if let Some(drag) = &mut self.drag {
            if pressed {
                drag.snapper.deeper();
            } else {
                self.line_tool.right_button(false, false);
            }
            return true;
        }
        let line_key =
            pressed && is_fast_editing() && self.line_shows && keybinds::LINE_POINT.is_held();
        self.line_tool.right_button(pressed, line_key)
    }

    /// Takes the camera pivot's press by the middle button (not Space) while the resize key is held
    /// or a drag goes on, and then its release: the camera would hide the cursor and hold it still,
    /// while the button may be one of fast editing's keys. Whether it took it.
    fn pivot_click(&mut self, click: Click) -> bool {
        match click {
            Click::Pressed => {
                let dragging = self.drag.is_some();
                let fast_editing = is_fast_editing();
                self.pivot_taken = keyboard::is_middle_button_down() && (dragging || fast_editing);
                if self.pivot_taken {
                    let why = if dragging { "dragging" } else { "fast editing" };
                    log!("the middle button doesn't pivot the camera: {why}");
                }
                self.pivot_taken
            }
            Click::Released => core::mem::take(&mut self.pivot_taken),
        }
    }

    /// Ends dragging when the button is released; whether one was (or a drag of the line key, which
    /// took its press: see [`Self::press`]).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn release(&mut self, game: &Game) -> bool {
        match &self.drag {
            None => false,
            Some(drag) if drag.held_by == HeldBy::LineKey => true,
            Some(_) => {
                // SAFETY: per the contract.
                unsafe { self.finish(game) };
                true
            }
        }
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
        let still = is_still(drag.pointer.start(), view.mouse);
        // SAFETY: per the contract.
        let going = unsafe {
            match &mut drag.action {
                Action::Resize(resizing) => {
                    follow_resize(game, scene, resizing, snapper, view, models, still)
                }
                Action::Move(moving) => {
                    follow_move(game, editor, scene, moving, snapper, view, models)
                }
                Action::Spinner(editing) => {
                    follow_spinner(game, scene, editing, snapper, view, models)
                }
                Action::Curve(editing) => follow_curve(game, scene, editing, snapper, view, models),
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
            Action::Spinner(editing) => editing
                .edit
                .is_some_and(|edit| edit != editing.dragging.unchanged()),
            Action::Curve(editing) => editing.changed(),
        };
        if !changed {
            // A click without a drag: a face clicked can have its size typed next.
            if let (Action::Resize(resizing), Some(editor), Some(scene)) =
                (drag.action, drag.editor.get(), &self.scene)
            {
                let vehicle = scene.vehicle;
                // SAFETY: per the contract; the editor is live.
                self.face = unsafe { SizedFace::new(game, editor, resizing, vehicle) };
            }
            return;
        }
        if let Action::Move(moving) = &drag.action
            && let Some(hinge) = moving.hinge
        {
            self.line_tool.drag_done();
            // A turn's angle can be typed next (not a slide's).
            let vehicle = self.scene.as_ref().map(|scene| scene.vehicle);
            self.turn = match vehicle {
                Some(vehicle) if hinge.turned != 0.0 => Some(TurnedHinge {
                    mode: moving.mode,
                    hinge,
                    start: moving.start,
                    way: hinge.way(),
                    vehicle,
                    left: None,
                    since: Instant::now(),
                }),
                _ => None,
            };
        }
        LOG_CURSOR.call_once(|| {
            log!(
                "while dragging, the cursor {}",
                if drag.pointer.cursor_moves() {
                    "moves"
                } else {
                    "stays still: following the mouse's movement"
                }
            )
        });
        match drag.action {
            // SAFETY: per the contract.
            Action::Resize(resizing) => unsafe { self.finish_resize(game, &drag.editor, resizing) },
            // SAFETY: per the contract.
            Action::Spinner(editing) => unsafe { self.finish_spinner(game, &drag.editor, editing) },
            // SAFETY: per the contract.
            Action::Curve(editing) => unsafe { self.finish_curve(game, &drag.editor, editing) },
            Action::Move(_) => {}
        }
    }

    /// Switches the hovered spinner (if the mouse is on one) to the cross-section `step` asks for,
    /// and finds again what's known of it in the scene, so that its new outline shows at once.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn cycle_cross_section(
        &mut self,
        game: &Game,
        editor: *mut UObjectBase,
        step: spinner_shape::Step,
    ) {
        let (Some(scene), Some(hover)) = (&self.scene, &self.hover) else {
            return;
        };
        let placed = scene.get(hover.index);
        let (Some(object), true) = (placed.object.get(), placed.spinner.is_some()) else {
            return;
        };
        // SAFETY: per the contract; `object` is a live spinner in the editor.
        unsafe {
            let changed = spinner_shape::cycle(game, editor, object, step);
            self.update_scene(game, &changed);
        }
    }

    /// Finishes changing a spinner: the last change, as one editor operation (one undo step).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish_spinner(
        &mut self,
        game: &Game,
        editor: &WeakObject,
        mut editing: SpinnerEditing,
    ) {
        let (Some(editor), Some(object)) = (editor.get(), editing.brick.object.get()) else {
            return;
        };
        // SAFETY: per the contract; the editor and the spinner are live.
        unsafe {
            game.editor.operation(editor, CHANGED, || {
                apply_spinner(game, &mut editing, false);
            });
            let counterpart = editing.counterpart.as_ref().map(|(other, _)| &other.object);
            let objects: Vec<*mut UObjectBase> =
                [Some(object), counterpart.and_then(WeakObject::get)]
                    .into_iter()
                    .flatten()
                    .collect();
            self.update_scene(game, &objects);
        }
    }

    /// Finishes shaping a curve: the last change, as one editor operation (one undo step).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish_curve(
        &mut self,
        game: &Game,
        editor: &WeakObject,
        mut editing: spinner_curve::Editing,
    ) {
        let Some(editor) = editor.get() else {
            return;
        };
        // SAFETY: per the contract; the editor is live, the chain's spinners its own.
        unsafe {
            game.editor.operation(editor, CHANGED, || {
                editing.write(game, false);
            });
            self.update_scene(game, &editing.objects());
        }
    }

    /// Finds again what's known of `objects` (changed bricks) in the scene, and the selected
    /// spinners' curve (they may have changed).
    ///
    /// # Safety
    /// `objects` must be live bricks. Game thread only.
    unsafe fn update_scene(&mut self, game: &Game, objects: &[*mut UObjectBase]) {
        self.chain_of = None;
        let Some(scene) = &mut self.scene else {
            return;
        };
        let spinners = self.spinner_models.get_or_insert_with(Default::default);
        for &object in objects {
            // SAFETY: per the contract.
            unsafe {
                if let Some(mut placed) = placed(game, object, spinners) {
                    if scene.has_models()
                        && placed.spinner.is_none()
                        && let (Some(reader), Some(models)) = (&game.models, &mut self.models)
                    {
                        placed.model = model_of(reader, models, object, &placed);
                    }
                    scene.update(placed);
                }
            }
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
            game.editor.operation(editor, CHANGED, || {
                apply(game, &mut resizing, size, false);
            });
            self.update_scene(game, &resizing.objects());
            // Its size can be typed next, if it's a face's.
            let vehicle = self.scene.as_ref().map(|scene| scene.vehicle);
            self.face = vehicle.and_then(|vehicle| SizedFace::new(game, editor, resizing, vehicle));
        }
    }

    /// Shows what's going on, or nothing.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn draw(&mut self, game: &Game, view: Option<&View>, show_line: bool) {
        let lines = match view {
            Some(view) => self.lines(view, show_line),
            None => Vec::new(),
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
            game.lines.replace(batcher, "brick_resize", &lines);
        }
        self.drawn = !lines.is_empty();
    }

    /// What shows with the mouse at `view`: the line (if `show_line`), and what's pointed at or
    /// dragged.
    fn lines(&self, view: &View, show_line: bool) -> Vec<line_batcher::Line> {
        let face_vehicle = self.face.as_ref().map(|face| face.vehicle);
        let vehicle = match (&self.scene, self.line_tool.vehicle().or(face_vehicle)) {
            (Some(scene), _) => scene.vehicle,
            (None, Some(vehicle)) => vehicle,
            (None, None) => return Vec::new(),
        };
        let mut drawing = Drawing::new(view.pixels, vehicle);
        if show_line {
            self.line_tool.draw(&mut drawing);
        }
        // The face whose size is typed.
        if let Some(face) = &self.face
            && typed_length::is_open()
        {
            draw_resizing(&mut drawing, &face.resizing);
        }
        let Some(scene) = &self.scene else {
            return drawing.into_lines();
        };
        // A press with the snap key would grab the hovered brick's piece its snap point is on.
        let grab_preview = self.grab_preview();
        let hovered_piece = |hover: &Hover| grab_preview.map_or(hover.piece, |(piece, _)| piece);
        if let Some(drag) = &self.drag {
            match &drag.action {
                Action::Resize(resizing) => draw_resizing(&mut drawing, resizing),
                Action::Move(moving) => draw_moving(&mut drawing, moving),
                Action::Spinner(editing) => draw_spinner_editing(&mut drawing, editing),
                Action::Curve(editing) => editing.draw(&mut drawing),
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
                    // What's dragged goes through the point itself.
                    Action::Spinner(_) | Action::Curve(_) => (snapped.target.point, None),
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
        } else if let (Some((control, _)), Some(chain)) = (self.curve_hover, &self.chain) {
            chain.draw(&mut drawing, Some(control));
        } else if let Some(hover) = &self.hover
            && let Some(brick) = &scene.get(hover.index).spinner
        {
            let placed = scene.get(hover.index);
            let frame = &placed.frame;
            let editing = matches!(self.gathered, Some(Gathered::Editing));
            spinner::draw_outline(
                &mut drawing,
                frame,
                &brick.spinner,
                brick,
                draw::BOX,
                editing,
            );
            let pen = if hover.grabbable {
                draw::PART
            } else {
                draw::FIXED_PART
            };
            match hover.spinner {
                Some((part, _)) => {
                    spinner::draw_part(&mut drawing, frame, &brick.spinner, brick, part, pen)
                }
                None => draw_piece(&mut drawing, frame, placed, hovered_piece(hover), pen),
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
            match (hover.grabbable, hovered_piece(hover)) {
                (true, piece) => draw_piece(&mut drawing, frame, placed, piece, draw::PART),
                // A model's piece that can't drag still shows what the mouse is on.
                (false, piece @ (Piece::Model(_) | Piece::Spinner(_))) => {
                    draw_piece(&mut drawing, frame, placed, piece, draw::FIXED_PART)
                }
                (false, Piece::Box(_)) => {}
            }
        }
        // The selected spinners' curve shows while nothing else is dragged.
        if self.drag.is_none()
            && self.curve_hover.is_none()
            && let Some(chain) = &self.chain
        {
            chain.draw(&mut drawing, None);
        }
        if self.drag.is_none()
            && let (Some((_, snapped)), Some(snapper)) = (grab_preview, &self.grab_snap)
        {
            // Pops up and pulses like a drag's snap point, in the grabbed piece's color.
            let seconds = snapper.snapped_for().as_secs_f64();
            let pen = draw::pulsed(draw::PART, seconds);
            let size = draw::popped(draw::MARK_PIXELS, seconds);
            let point = snapped.target.point;
            draw_snap_mark(
                &mut drawing,
                snapped,
                point,
                snapped.target.normal,
                size,
                pen,
            );
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

impl Resizing {
    /// The live bricks resized: the brick, and its counterpart.
    fn objects(&self) -> Vec<*mut UObjectBase> {
        let counterpart = self.counterpart.as_ref().map(|(other, _)| &other.object);
        [Some(&self.brick.object), counterpart]
            .into_iter()
            .flatten()
            .filter_map(WeakObject::get)
            .collect()
    }
}

impl SizedFace {
    /// The face `hover` is on (in the scene), its brick as it is now; `None` if it can't be resized,
    /// or it's not a face.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn of_hover(game: &Game, scene: &Scene, hover: &Hover) -> Option<Self> {
        // SAFETY: per the contract.
        unsafe {
            let (editor, resizing) = resizing_of(game, scene, hover)?;
            Self::new(game, editor, resizing, scene.vehicle)
        }
    }

    /// The face `resizing` grabbed, in `editor`, the vehicle at `vehicle`; `None` if it's not a
    /// face, or its brick is gone.
    ///
    /// # Safety
    /// `editor` must be a live editor. Game thread only.
    unsafe fn new(
        game: &Game,
        editor: *mut UObjectBase,
        resizing: Resizing,
        vehicle: Frame,
    ) -> Option<Self> {
        resizing.resize.axis()?;
        let object = resizing.brick.object.get()?;
        // SAFETY: per the contract; the brick is live.
        unsafe {
            Some(Self {
                editor: WeakObject::new(editor),
                left: left(game, object)?,
                selected: is_selected_object(game, &resizing.brick.object),
                resizing,
                vehicle,
                since: Instant::now(),
            })
        }
    }

    /// Whether its brick is still in `editor`, as it was left.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn is_as_left(&self, game: &Game, editor: *mut UObjectBase) -> bool {
        let Some(object) = self.resizing.brick.object.get() else {
            return false;
        };
        // SAFETY: per the contract; the brick is live.
        self.editor.is(editor) && unsafe { left(game, object) } == Some(self.left)
    }

    /// Its size now (cm): the box's along its axis.
    fn extent(&self) -> f64 {
        let axis = self.resizing.resize.axis();
        axis.map_or(0.0, |axis| self.resizing.bounds.extent(axis))
    }
}

/// Where `object` is and its size (see [`SizedFace::left`]).
///
/// # Safety
/// `object` must be a live scalable brick. Game thread only.
unsafe fn left(game: &Game, object: *mut UObjectBase) -> Option<Left> {
    // SAFETY: per the contract.
    unsafe {
        let place = game.spawn.of(object)?;
        Some((place.location, place.rotation, game.sizes.size(object)))
    }
}

/// The line showing a resized face's size (in the vehicle): through the box's middle, from the
/// opposite side to the face's. `None` if it's not a face (see [`Resize::axis`]).
fn size_line(resizing: &Resizing) -> Option<[Vec3; 2]> {
    let resize = &resizing.resize;
    let axis = resize.axis()?;
    let side = resize.grip.part.sides[axis];
    let ends = [-side, side].map(|side| {
        let mut sides = [0; 3];
        sides[axis] = side;
        resize.frame.point(resizing.bounds.point(sides))
    });
    Some(ends)
}

/// Resizing the brick `hover` is on (in the scene) by its grip, from how it is now, and the editor
/// it's in: what a press starts, or a typed size changes. `None` if it can't be resized.
///
/// # Safety
/// Game thread only.
unsafe fn resizing_of(
    game: &Game,
    scene: &Scene,
    hover: &Hover,
) -> Option<(*mut UObjectBase, Resizing)> {
    let grip = hover.grip?;
    let placed = scene.get(hover.index);
    let object = placed.object.get()?;
    if !game.spawn.known() {
        LOG_NO_PLACES
            .call_once(|| log!("can't resize: where bricks keep their place wasn't found"));
        return None;
    }
    // SAFETY: per the contract; `object` is a live scalable brick in the editor.
    unsafe {
        let editor = game.bricks.editor_of(object);
        let brick = edited(game, object)?;
        if editor.is_null() {
            return None;
        }
        let counterpart = game.mirror.as_ref().and_then(|mirror| {
            let axis = mirror.axis()?;
            let other = game.code.mirrored_object(object)?;
            (game.sizes.is_scalable(other)).then_some(())?;
            Some((edited(game, other)?, axis))
        });
        let size = game.sizes.size(object);
        let resizing = Resizing {
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
        };
        Some((editor, resizing))
    }
}

/// Resizes the brick for the mouse, snapped with a `snapper` (while snapping, see [`SnapKey`]);
/// false once it can't be. With the mouse `still` where it was pressed (see [`is_still`]), the
/// brick has its size as grabbed, exactly: the ray found again through the grabbed point lands a
/// float's rounding away from it, which would change the size by as much.
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
    still: bool,
) -> bool {
    if still {
        let grabbed = math::narrow(resizing.resize.size);
        if grabbed == resizing.asked {
            return true;
        }
        resizing.asked = grabbed;
        // SAFETY: per the contract.
        return unsafe { apply(game, resizing, grabbed, true) };
    }
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
        let snapped = snapper.snap(&snap::snapping(), scene, view, models, is_resized);
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

/// Whether a drag pressed at `start` points at `now` (screen pixels) still: within the pixel it was
/// pressed in, not moved by a whole one either way. A mouse's smallest step can be less than a
/// pixel.
fn is_still(start: [f32; 2], now: [f32; 2]) -> bool {
    (now[0] - start[0]).abs() < 0.5 && (now[1] - start[1]).abs() < 0.5
}

/// Changes the spinner for the mouse, snapped with a `snapper` (while snapping, see [`SnapKey`]);
/// false once it can't be.
///
/// # Safety
/// Game thread only.
unsafe fn follow_spinner(
    game: &Game,
    scene: &Scene,
    editing: &mut SpinnerEditing,
    snapper: Option<&mut Snapper>,
    view: &View,
    models: bool,
) -> bool {
    if let Some([x, y]) = editing.pressed_at {
        if (view.mouse[0] - x).hypot(view.mouse[1] - y) < SPINNER_DRAG_PIXELS {
            return true;
        }
        editing.pressed_at = None;
    }
    let edit = if let Some(snapper) = snapper {
        // The changed spinners are left out: they change.
        let counterpart = editing.counterpart.as_ref().map(|(other, _)| &other.object);
        let changed: Vec<*mut UObjectBase> = [Some(&editing.brick.object), counterpart]
            .into_iter()
            .flatten()
            .filter_map(WeakObject::get)
            .collect();
        let is_changed = |placed: &Placed| changed.iter().any(|&object| placed.object.is(object));
        let started = Instant::now();
        let snapped = snapper.snap(&snap::snapping(), scene, view, models, is_changed);
        log_snap(snapped.map(|snapped| snapped.kind), started);
        // A cap near an angle in steps (a setting) goes there, unless a point of another brick is
        // nearer the mouse.
        let stepped = spinner::angle_step()
            .and_then(|step| editing.dragging.stepped(view, step))
            .filter(|&(_, pixels)| pixels <= f64::from(parts::CORNER_PIXELS));
        let point_nearer = |pixels: f64| {
            snapped.is_some_and(|snapped| {
                snapped.kind.is_point()
                    && snap::from_mouse(view, snapped.target.point)
                        .is_some_and(|nearer| f64::from(nearer) < pixels)
            })
        };
        match stepped {
            Some((turned, pixels)) if !point_nearer(pixels) => {
                snapper.stop();
                editing.dragging.turn_to(turned)
            }
            // A point it can't go through (the hub, for a side): it follows the mouse.
            _ => snapped
                .and_then(|snapped| editing.dragging.snap(&snapped.target))
                .or_else(|| editing.dragging.follow(view)),
        }
    } else {
        editing.dragging.follow(view)
    };
    let Some(edit) = edit else {
        return true;
    };
    if editing.edit == Some(edit) {
        return true;
    }
    editing.edit = Some(edit);
    // SAFETY: per the contract.
    unsafe { apply_spinner(game, editing, true) }
}

/// Takes the dragged control of a curve where the mouse is, snapped with a `snapper` (while
/// snapping, see [`SnapKey`]) to a point of the other bricks, else in the plane it was grabbed
/// in, and gives the spinners the curve. False if they're gone.
///
/// # Safety
/// Game thread only.
unsafe fn follow_curve(
    game: &Game,
    scene: &Scene,
    editing: &mut spinner_curve::Editing,
    snapper: Option<&mut Snapper>,
    view: &View,
    models: bool,
) -> bool {
    if let Some([x, y]) = editing.pressed_at {
        if (view.mouse[0] - x).hypot(view.mouse[1] - y) < SPINNER_DRAG_PIXELS {
            return true;
        }
        editing.pressed_at = None;
    }
    let snapped = snapper.and_then(|snapper| {
        let started = Instant::now();
        // The curve's spinners are left out: they change.
        let skip = |placed: &Placed| editing.has(&placed.object);
        let snapped = snapper.snap(&snap::snapping(), scene, view, models, skip);
        log_snap(snapped.map(|snapped| snapped.kind), started);
        // On a face, which way it faces: a node there lies along it.
        snapped.map(|snapped| match aim(scene, snapped).orientation {
            Some(Orientation::Face(normal)) => (snapped.target.point, Some(normal)),
            _ => (snapped.target.point, None),
        })
    });
    let Some((point, on)) = snapped.or_else(|| Some((editing.followed(view)?, None))) else {
        return true;
    };
    if !editing.aim(point, on) {
        return true;
    }
    // SAFETY: per the contract.
    unsafe { editing.write(game, true) }
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
    if let Some(pressed_at) = moving.pressed_at {
        if view.mouse == pressed_at {
            return true;
        }
        moving.pressed_at = None;
    }
    let pressed = moving.turn.pressed(keybinds::NEXT_ORIENTATION.is_held());
    let motion = if moving.hinge.is_some() {
        follow_hinge(scene, moving, snapper, view, models, pressed)
    } else if let Some(snapper) = snapper {
        let free = moving.grab.follow(&view.ray, moving.grid, &moving.kept);
        let started = Instant::now();
        // The moved bricks are left out.
        let snapped = snapper.snap(&snap::snapping(), scene, view, models, |placed| {
            placed.moving
        });
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
        moving.grab.follow(&view.ray, moving.grid, &moving.kept)
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

/// The motion of a move about the line (see [`hinge`]): turned, the grabbed point going round the
/// line as near the mouse as it can, or slid along it (see [`Moving::slides`]), by the game's
/// steps. Snapped with a `snapper`: turned to where what's grabbed matches
/// the target best (the next way each time the orientation key is `pressed`), or slid level with
/// it. `None` while the mouse gives no steady answer.
fn follow_hinge(
    scene: &Scene,
    moving: &mut Moving,
    snapper: Option<&mut Snapper>,
    view: &View,
    models: bool,
    pressed: bool,
) -> Option<Motion> {
    let slides = moving.slides();
    let hinge = moving.hinge.as_mut()?;
    let snapped = snapper.and_then(|snapper| {
        let started = Instant::now();
        // The moved bricks and their counterparts are left out: they move.
        let snapped = snapper.snap(&snap::snapping(), scene, view, models, |placed| {
            placed.moving || placed.mirroring
        });
        log_snap(snapped.map(|snapped| snapped.kind), started);
        snapped
    });
    if slides {
        hinge.slid = match snapped {
            Some(snapped) => hinge.slide_to(snapped.target.point),
            None => hinge::stepped(hinge.slide_for(&view.ray)?, moving.grid),
        };
        return Some(hinge.motion());
    }
    // Where the mouse turns the bricks: matching turns nearest it come first.
    let free = hinge.angle_for(&view.ray, view.pixels.per_distance);
    let matching = snapped.map(|snapped| {
        let aim = aim(scene, snapped);
        let near = free.unwrap_or(hinge.turned);
        let angles = hinge.matching_angles(&moving.grab, &aim, &motion::matching(), near);
        (angles, &snapped.on)
    });
    hinge.turned = match matching {
        Some((angles, on)) if !angles.is_empty() => {
            let choice = moving.turn.choose(on, pressed);
            angles[choice % angles.len()]
        }
        _ => hinge::stepped(free?, moving.angle),
    };
    Some(hinge.motion())
}

/// The motion snapping a move to `snapped`: onto it, turned the `choice`th way matching it where it
/// lines up (see [`motion::matching`]); on a counterpart of the moved bricks, meeting it halfway
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
            &motion::matching(),
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

impl Moving {
    /// Whether the bricks slide along the line rather than turn about it: dragged by the line key,
    /// with the slide key held, or grabbed by a point on the line (it can't go round it).
    fn slides(&self) -> bool {
        self.hinge.as_ref().is_some_and(|hinge| {
            self.by_line_key || keybinds::SLIDE_ALONG_LINE.is_held() || hinge.on_line()
        })
    }
}

/// What's grabbed of the hovered brick, and where: with the snap key held, its `own` snap point
/// nearest the mouse that showed (a corner, an edge's middle, a face's center..., see
/// [`State::grab_preview`]) and its piece there; else the piece under the mouse, at its point there
/// (see [`motion::grabbed_point`]).
fn grabbed(scene: &Scene, hover: &Hover, own: Option<&snap::Snap>) -> (Piece, Vec3) {
    if keybinds::SNAP_RESIZE.is_held()
        && let Some(own) = own
        && let Some(piece) = own_snap(own, hover.index)
    {
        return (piece, own.target.point);
    }
    let placed = scene.get(hover.index);
    let point = motion::grabbed_point(placed, hover.piece, &hover.view.ray, hover.point);
    (hover.piece, point)
}

/// The piece of brick `index` that `snapped` is on, if it's on that brick.
fn own_snap(snapped: &snap::Snap, index: usize) -> Option<Piece> {
    snapped
        .on
        .iter()
        .find(|(on, _)| *on == index)
        .map(|&(_, piece)| piece)
}

/// Where a snapped grab goes: the snap point, how the piece it's on lies (if it's on one), and
/// that brick's axes.
fn aim(scene: &Scene, snapped: &snap::Snap) -> Aim {
    let orientation = match snapped.on.as_slice() {
        &[(index, piece)] => Orientation::of(scene.get(index), piece, snapped.target.point),
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
        // Spinners change by their own values instead (`draw_spinner_editing`).
        Piece::Spinner(_) => {}
    }
}

/// A spinner being changed: its outline as asked last, and what's dragged.
fn draw_spinner_editing(drawing: &mut Drawing, editing: &SpinnerEditing) {
    let dragging = &editing.dragging;
    let edit = editing.edit.unwrap_or_else(|| dragging.unchanged());
    let frame = spinner::moved_frame(&dragging.frame, &edit);
    let brick = &editing.grabbed;
    spinner::draw_outline(drawing, &frame, &edit.spinner, brick, draw::BOX, false);
    dragging.draw(drawing, &edit, brick, draw::PART);
}

/// The piece of `placed` that `piece` (pointed at `point`, the mouse at `view`) is on if it's a
/// spinner, and how dragging it changes it: a separator on the side under the mouse first (see
/// [`spinner::Part::separator_under`]), else the piece.
fn spinner_grip(
    placed: &Placed,
    piece: Piece,
    point: Vec3,
    view: &View,
) -> Option<(spinner::Part, Option<spinner::Grip>)> {
    let (Some(brick), Some(model), Piece::Model(element)) = (&placed.spinner, &placed.model, piece)
    else {
        return None;
    };
    let mut part = spinner::Part::of(brick, model, element, placed.frame.local(point));
    if let spinner::Part::Side(_) = part
        && let Some(separator) = spinner::Part::separator_under(brick, &placed.frame, view, point)
    {
        part = separator;
    }
    Some((part, part.grip(brick)))
}

/// The grabbed brick of a move where it's going: its box, and the piece grabbed; moving about a
/// line, the circle the grabbed point goes round it on.
fn draw_moving(drawing: &mut Drawing, moving: &Moving) {
    if let Some(hinge) = &moving.hinge
        && !moving.slides()
    {
        let step = 360.0 / CIRCLE_SEGMENTS as f64;
        let points: Vec<Vec3> = (0..=CIRCLE_SEGMENTS)
            .map(|index| hinge.around(index as f64 * step))
            .collect();
        for pair in points.windows(2) {
            drawing.segment(pair[0], pair[1], draw::CIRCLE);
        }
    }
    let frame = moving.motion.frame(&moving.frame);
    match &moving.spinner {
        Some(brick) => {
            spinner::draw_outline(drawing, &frame, &brick.spinner, brick, draw::BOX, false);
        }
        None => drawing.outline(&frame, &moving.bounds, draw::BOX),
    }
    match moving.piece {
        Piece::Box(part) => drawing.part(&frame, &moving.bounds, part, draw::PART),
        Piece::Model(element) => {
            if let Some(model) = &moving.model {
                drawing.element(&frame, model, element, draw::PART, |point| point);
            }
        }
        Piece::Spinner(part) => {
            if let Some(brick) = &moving.spinner {
                spinner::draw_part(drawing, &frame, &brick.spinner, brick, part, draw::PART);
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
    // SAFETY: per the contract.
    unsafe { is_selected_object(game, &placed.object) }
}

/// Whether `object` is still there and selected in the editor (false if that can't be told).
///
/// # Safety
/// Game thread only.
unsafe fn is_selected_object(game: &Game, object: &WeakObject) -> bool {
    let (Some(selection), Some(object)) = (&game.selection, object.get()) else {
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
unsafe fn gather(
    game: &Game,
    editor: *mut UObjectBase,
    gathered: &Gathered,
    spinners: &mut SpinnerModels,
) -> Scene {
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
                let mut placed = placed(game, object, spinners)?;
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

/// A brick's box in the vehicle, if it has one; a spinner's shape too (made with `spinners`).
///
/// # Safety
/// `object` must be a live brick. Game thread only.
unsafe fn placed(
    game: &Game,
    object: *mut UObjectBase,
    spinners: &mut SpinnerModels,
) -> Option<Placed> {
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
        let (spinner, model) = match spinner_brick(game, object, spinners, &bounds) {
            Some((brick, model)) => (Some(brick), Some(model)),
            None => (None, None),
        };
        Some(Placed {
            object: WeakObject::new(object),
            frame: Frame::of_place(location, rotation),
            bounds,
            scalable,
            mirrored: core::array::from_fn(|axis| a[axis] > b[axis]),
            model,
            spinner,
            moving: false,
            mirroring: false,
        })
    }
}

/// How far a spinner's box worked out from its values may be from the one the game reports, per
/// coordinate (cm, and as a share of the box's size): the game's is in `f32`.
const SPINNER_BOX_MATCH: f64 = 0.01;
const SPINNER_BOX_SHARE: f64 = 1e-4;

/// `object`'s values and shape if it's a spinner, with its model (made with `spinners`), if its
/// box `bounds` is the one its values give: else the game shapes spinners otherwise now, and it's
/// handled as a box (logged once).
///
/// # Safety
/// `object` must be a live brick. Game thread only.
unsafe fn spinner_brick(
    game: &Game,
    object: *mut UObjectBase,
    spinners: &mut SpinnerModels,
    bounds: &Bounds,
) -> Option<(SpinnerBrick, BrickModel)> {
    let (reader, profiles) = game.spinners.as_ref()?;
    // SAFETY: per the contract.
    unsafe {
        if !reader.is_spinner(object) {
            return None;
        }
        let values = reader.read(object);
        let Some((brick, model)) = spinners.brick(values, || profiles.of(object)) else {
            LOG_SPINNER_SHAPE.call_once(|| {
                log!("a spinner keeps its box: its cross-section wasn't read ({values:?})")
            });
            return None;
        };
        let [min, max] = brick.spinner.bounds();
        let size = geometry::length(geometry::sub(max, min));
        let tolerance = SPINNER_BOX_MATCH + SPINNER_BOX_SHARE * size;
        let matches = (0..3).all(|axis| {
            (min[axis] - bounds.min[axis]).abs() <= tolerance
                && (max[axis] - bounds.max[axis]).abs() <= tolerance
        });
        if !matches {
            LOG_SPINNER_BOX.call_once(|| {
                log!(
                    "a spinner keeps its box: the game's {bounds:?} isn't the one its values give, \
                     {min:?}..{max:?} ({values:?})"
                )
            });
            return None;
        }
        Some((brick, model))
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
        Piece::Spinner(part) => {
            if let Some(brick) = &placed.spinner {
                spinner::draw_part(drawing, frame, &brick.spinner, brick, part, pen);
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
    draw_snap_mark(drawing, snapped, point, normal, size, pen);
}

/// The mark of `snapped`'s kind at `point`, `size` pixels wide: lying on the face across `normal`
/// for a point on a face.
fn draw_snap_mark(
    drawing: &mut Drawing,
    snapped: &snap::Snap,
    point: Vec3,
    normal: Option<Vec3>,
    size: f32,
    pen: draw::Pen,
) {
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
        Piece::Spinner(part) => part.describe(),
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

/// Gives the spinner the values the last change asked for, telling it about those its grip changes
/// (`interactive`ly while dragging), then places it (and its counterpart, with the opposite angle)
/// to keep what's not dragged in place. False if it's gone or can't be told.
///
/// # Safety
/// Game thread only.
unsafe fn apply_spinner(game: &Game, editing: &mut SpinnerEditing, interactive: bool) -> bool {
    let (Some((spinners, _)), Some(edit), Some(object)) =
        (&game.spinners, editing.edit, editing.brick.object.get())
    else {
        return false;
    };
    let values = spinner::values_of(&editing.grabbed.stored, &edit);
    let changes = editing.dragging.grip.changes();
    // SAFETY: per the contract; the spinner and its counterpart are live spinners.
    unsafe {
        let Some(kept) = spinners.write(object, &values, changes, interactive) else {
            LOG_NO_INTERFACE
                .call_once(|| log!("can't change spinners: bricks' property interface not found"));
            return false;
        };
        let spawn = Frame::of_place(editing.brick.location, editing.brick.rotation);
        let moved = spinner::moved_frame(&spawn, &edit);
        let location = math::narrow(moved.origin);
        let rotation = if edit.turn != 0.0 {
            math::narrow(moved.rotation.to_rotator())
        } else {
            editing.brick.rotation
        };
        game.spawn.set(object, location, rotation);
        if let (Some((other, axis)), Some(mirror)) = (&editing.counterpart, &game.mirror)
            && let Some(counterpart) = other.object.get()
        {
            let mirrored = Values {
                angle: -kept.angle,
                ..kept
            };
            spinners.write(counterpart, &mirrored, changes, interactive);
            let (location, rotation) = if edit.turn != 0.0 {
                let (location, turned) = mirror.mirrored_place(object, location, rotation, *axis);
                let rotation = math::Quat::widen(turned).to_rotator();
                (location, math::narrow(rotation))
            } else {
                let location = mirror.mirrored_location(object, location, rotation, *axis);
                (location, other.rotation)
            };
            game.spawn.set(counterpart, location, rotation);
        }
    }
    true
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
            // A brick mirrored by a quarter turn (a quarter pyramid) has its counterpart's X and
            // Y sizes swapped.
            let size = game
                .mirrored_sizes
                .as_ref()
                .and_then(|sizes| sizes.of_counterpart(counterpart, kept))
                .unwrap_or(kept);
            game.sizes.resize(counterpart, size, interactive);
            let mirrored =
                mirror.mirrored_location(object, location, resizing.brick.rotation, *axis);
            game.spawn.set(counterpart, mirrored, other.rotation);
        }
    }
    true
}
