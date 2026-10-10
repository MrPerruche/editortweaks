//! Resizing scalable bricks (bricks with a `BrickSize`) by dragging the faces, edges and corners of
//! their box, in the editor (not while moving bricks):
//!
//! - hold Alt ([`keybinds::RESIZE_BRICKS`], rebindable like the keys below): the box of the brick
//!   under the mouse shows, and the part of it the mouse is on lights up: a corner, an edge or a face (see [`parts`]), also just outside the box. Selected
//!   bricks come first, then the one in front (see [`scene::Scene::pick`]); bricks that can't be
//!   resized show a dark box;
//! - Alt + drag: that part follows the mouse, the opposite one stays in place, and the brick's size
//!   follows. A face changes one size, an edge two, a corner all three in proportion. The camera
//!   stays still meanwhile;
//! - hold Ctrl ([`keybinds::SNAP_RESIZE`]) while dragging: the part snaps to the corner, edge or
//!   face of the brick under the mouse (see [`resize::Resize::snap`]); each right click meanwhile
//!   looks one brick deeper along the mouse, to snap to bricks behind it (back to the front one
//!   after the last);
//! - releasing the button makes it one step to undo. With mirror mode on, the brick's counterpart
//!   changes along.
//!
//! Every brick is resized through its box as it reports it, so a rounded brick through the box
//! around it, except a scalable brick whose box is its size around its origin (most): that box is
//! taken from its stored size, not from the box the game computes in `f32` from its mesh (a hair
//! off, like 4.9999976 for 5). The math runs in double precision in the vehicle's space, where
//! bricks keep their places (see [`geometry`]), and only the results are rounded, to the `f32`
//! the game stores: snapping a face to one at 110 cm gives 110 cm, not 110.000545.
//!
//! How: the editor's tick (`ABrickEditor::Tick`, hooked) finds what the mouse points at among the
//! bricks' boxes ([`scene`]) and draws lines through the engine's line batcher
//! (`game::line_batcher`). The editor's left click (`Action_Select`, its reflected handler hooked)
//! starts and ends a resize instead of selecting while the resize key is held over a scalable
//! brick; the right
//! button's (`Action_MoveCamera`) counts right clicks instead of moving the camera meanwhile.
//! While a button is held the engine reports the mouse's movement to the editor, which would turn
//! the camera with it: the resize takes it instead ([`super::mouse_moves`]), and follows it when
//! the engine holds the cursor still ([`Drag::point`]). Each change writes `BrickSize` and tells the
//! brick like the editor's panel does (`game::scalable_brick::BrickSizes`), then moves the brick
//! like a committed move (`game::editor_object::SpawnTransforms`); the last one runs as an editor
//! operation (`game::brick_editor`), which makes the undo step.

mod draw;
mod geometry;
mod parts;
mod resize;
mod scene;

use core::ffi::c_void;
use std::sync::{Mutex, Once, OnceLock, PoisonError, TryLockError};
use std::time::Instant;

use brickrust::ue::coreuobject::UObjectBase;

use super::keybinds;
use super::mouse_moves::{self, Direction, Handling, Movement};
use crate::framework::hook;
use crate::game::brick_editor::{self, BrickEditor, TickFn};
use crate::game::editor_input::{self, ButtonAction, Click};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{
    self, LocalBounds, Selection, SpawnTransforms, Vector, WorldTransforms,
};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::line_batcher::{self, LineBatcher};
use crate::game::mirror_mode::{self, MirrorMode};
use crate::game::reflection::NativeThunk;
use crate::game::scalable_brick::BrickSizes;
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::platform::keyboard;
use crate::prelude::*;

use draw::{Drawing, PixelSize};
use geometry::{Bounds, Frame, Ray, Vec3};
use parts::Part;
use resize::{Resize, Target};
use scene::{Placed, Scene, View};

pub struct BrickResize;

static ORIGINAL_TICK: OnceLock<TickFn> = OnceLock::new();
static ORIGINAL_SELECT: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_MOVE_CAMERA: OnceLock<NativeThunk> = OnceLock::new();
/// Set once everything is found; until then (or if that fails) the hooks change nothing.
static GAME: OnceLock<Game> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

/// Logs things worth knowing once: the first gathering's time, the first resize, how the cursor
/// behaves while resizing, problems.
static LOG_GATHERED: Once = Once::new();
static LOG_RESIZED: Once = Once::new();
static LOG_CURSOR: Once = Once::new();
static LOG_NO_PLACES: Once = Once::new();
static LOG_NO_INTERFACE: Once = Once::new();
static LOG_NO_LINES: Once = Once::new();

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
}

impl Feature for BrickResize {
    fn id(&self) -> &'static str {
        "brick_resize"
    }

    fn on_load(&self) -> LoadResult {
        let tick = brick_editor::TICK.resolve()?;
        // SAFETY: `tick` is the start of ABrickEditor::Tick, and the detour has its signature.
        unsafe {
            hook::install_typed(
                brick_editor::TICK.name,
                tick,
                tick_detour as TickFn,
                &ORIGINAL_TICK,
            )
        }
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => {
                log!(
                    "active: resize key (Alt by default) + drag a scalable brick's face, edge or                      corner, snap key (Ctrl) to snap"
                )
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
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
        selection: optional(
            editor_object::selection(),
            "selected bricks won't come first",
        ),
        mirror: optional(mirror_mode::find(), "mirrored counterparts won't follow"),
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

unsafe extern "C" fn tick_detour(editor: *mut UObjectBase, delta: f32) {
    let original = *ORIGINAL_TICK.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    unsafe { original(editor, delta) };
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
        Direction::Right => drag.moved[0] += pixels,
        // Up is positive, and the screen's y goes down.
        Direction::Up => drag.moved[1] -= pixels,
    }
    Handling::Take
}

/// A brick's part under the mouse while the resize key is held.
struct Hover {
    /// The brick (see [`Scene::get`]).
    index: usize,
    /// The part under the mouse, if the brick is scalable.
    part: Option<Part>,
    /// The point of the brick under the mouse (in the vehicle).
    point: Vec3,
    /// Where the mouse was (screen pixels).
    mouse: [f32; 2],
}

/// A brick being resized.
struct Drag {
    editor: WeakObject,
    brick: Edited,
    /// Its mirror mode counterpart, and the mirror axis.
    counterpart: Option<(Edited, u8)>,
    resize: Resize,
    /// The size asked for last, as written (the brick may have kept another).
    asked: Vector,
    /// Its box now, in its frame as grabbed.
    bounds: Bounds,
    /// What it snapped to last, to show.
    snapped: Option<Snap>,
    /// Where the resize points (screen pixels), see [`Self::point`].
    pointer: [f32; 2],
    /// Where the cursor was when the button went down.
    pressed_at: [f32; 2],
    /// Whether the cursor has moved since (else the engine holds it still).
    cursor_moves: bool,
    /// The mouse's movement not yet added to the pointer (pixels), from [`take_movement`].
    moved: [f32; 2],
    /// How many bricks deeper than the first under the mouse to look for snap targets (right
    /// clicks so far).
    depth: usize,
}

/// A brick changed by a resize, as it was grabbed.
struct Edited {
    object: WeakObject,
    /// Where the editor places it: location and rotation (pitch, yaw, roll).
    location: Vector,
    rotation: Vector,
}

/// A part of another brick snapped to.
struct Snap {
    index: usize,
    part: Part,
    target: Target,
}

struct State {
    /// The bricks' boxes while the resize key is held or a brick is resized.
    scene: Option<Scene>,
    hover: Option<Hover>,
    drag: Option<Drag>,
    /// Whether our lines are showing.
    drawn: bool,
}

// SAFETY: object pointers are only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

impl Drag {
    /// Where the resize points now (screen pixels), given where the cursor is (if known). While a
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
        self.pointer = match cursor {
            Some(cursor) if self.cursor_moves => cursor,
            _ => [self.pointer[0] + moved[0], self.pointer[1] + moved[1]],
        };
        self.pointer
    }
}

impl State {
    const fn new() -> Self {
        Self {
            scene: None,
            hover: None,
            drag: None,
            drawn: false,
        }
    }

    /// Every frame of `editor`.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn tick(&mut self, game: &Game, editor: *mut UObjectBase) {
        let alt = keybinds::RESIZE_BRICKS.is_held();
        if !alt && self.drag.is_none() && self.scene.is_none() && !self.drawn {
            return;
        }
        if let Some(drag) = &self.drag
            && !drag.editor.is(editor)
        {
            // Another editor: that one is gone or not ticking; drop the resize.
            self.drag = None;
        }
        // SAFETY: per the contract.
        unsafe {
            if self.drag.is_none() {
                if alt && game.modes.is_editing(editor) {
                    self.scene.get_or_insert_with(|| gather(game, editor));
                } else {
                    self.hover = None;
                    self.scene = None;
                }
            }
            let controller = game.editor.player_controller(editor);
            let view = match &self.scene {
                Some(scene) if !controller.is_null() => {
                    let cursor = game.viewport.mouse(controller);
                    let mouse = match &mut self.drag {
                        Some(drag) => Some(drag.point(cursor)),
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
                    self.follow(game, view);
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

    /// Finds the part under the mouse among the gathered bricks.
    ///
    /// # Safety
    /// `editor` must be a live editor. Game thread only.
    unsafe fn point(&mut self, game: &Game, editor: *mut UObjectBase, view: &View) {
        let Some(scene) = &self.scene else {
            return;
        };
        let vehicle = scene.vehicle;
        // SAFETY: per the contract.
        let pointed = scene.pick(
            view,
            |_| false,
            |placed| unsafe { is_selected(game, placed) },
            0,
            |point| unsafe { project(game, editor, &vehicle, point) },
        );
        self.hover = pointed.map(|pointed| Hover {
            index: pointed.index,
            part: scene.get(pointed.index).scalable.then_some(pointed.part),
            point: pointed.point,
            mouse: view.mouse,
        });
    }

    /// Starts resizing on a click with the resize key held over a scalable brick's part; whether
    /// it did.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn press(&mut self, game: &Game) -> bool {
        if !keybinds::RESIZE_BRICKS.is_held() {
            return false;
        }
        let (Some(scene), Some(hover)) = (&self.scene, &self.hover) else {
            return false;
        };
        let Some(part) = hover.part else {
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
                    "resizing a brick by a {} (size {size:?}, box {:?})",
                    ["face", "edge", "corner"][part.dimensions_fixed() - 1],
                    placed.bounds
                )
            });
            self.drag = Some(Drag {
                editor: WeakObject::new(editor),
                brick,
                counterpart,
                resize: Resize {
                    part,
                    frame: placed.frame,
                    bounds: placed.bounds,
                    size: math::widen(size),
                    grab: placed.frame.local(hover.point),
                },
                asked: size,
                bounds: placed.bounds,
                snapped: None,
                pointer: hover.mouse,
                pressed_at: hover.mouse,
                cursor_moves: false,
                moved: [0.0; 2],
                depth: 0,
            });
        }
        true
    }

    /// While resizing, takes the right button (the camera stays still): each press looks one brick
    /// deeper for snap targets. Whether it took it.
    fn right_click(&mut self, click: Click) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if click == Click::Pressed {
            drag.depth += 1;
        }
        true
    }

    /// Ends resizing when the button is released; whether one was.
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

    /// Resizes the brick for the mouse (snapped with the snap key held).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn follow(&mut self, game: &Game, view: &View) {
        let (Some(scene), Some(drag)) = (&self.scene, &mut self.drag) else {
            return;
        };
        drag.snapped = None;
        let sides = if keybinds::SNAP_RESIZE.is_held() {
            // SAFETY: per the contract.
            drag.snapped = unsafe { snap(game, scene, drag, view) };
            match &drag.snapped {
                Some(snapped) => Some(drag.resize.snap(&snapped.target)),
                None => drag.resize.follow(&view.ray),
            }
        } else {
            drag.resize.follow(&view.ray)
        };
        let Some(sides) = sides else {
            return;
        };
        let size = math::narrow(drag.resize.size_for(&drag.resize.resized(sides)));
        if size == drag.asked {
            return;
        }
        drag.asked = size;
        // SAFETY: per the contract.
        if !unsafe { apply(game, drag, size, true) } {
            self.drag = None;
        }
    }

    /// Finishes resizing: the last change, as one editor operation (one undo step).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish(&mut self, game: &Game) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        if drag.asked == math::narrow(drag.resize.size) {
            // A click without a drag: nothing changed.
            return;
        }
        LOG_CURSOR.call_once(|| {
            log!(
                "while resizing, the cursor {}",
                if drag.cursor_moves {
                    "moves"
                } else {
                    "stays still: following the mouse's movement"
                }
            )
        });
        let (Some(editor), Some(object)) = (drag.editor.get(), drag.brick.object.get()) else {
            return;
        };
        // SAFETY: per the contract; the editor and the brick are live.
        unsafe {
            let size = game.sizes.size(object);
            game.editor.operation(editor, brick_editor::MOVED, || {
                apply(game, &mut drag, size, false);
            });
            if let Some(scene) = &mut self.scene {
                let counterpart = drag.counterpart.as_ref().map(|(other, _)| &other.object);
                for edited in [Some(&drag.brick.object), counterpart]
                    .into_iter()
                    .flatten()
                {
                    if let Some(object) = edited.get()
                        && let Some(placed) = placed(game, object)
                    {
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
            let frame = &drag.resize.frame;
            drawing.outline(frame, &drag.bounds, draw::BOX);
            drawing.part(frame, &drag.bounds, drag.resize.part, draw::PART);
            if let Some(snapped) = &drag.snapped {
                let placed = scene.get(snapped.index);
                drawing.outline(&placed.frame, &placed.bounds, draw::TARGET_BOX);
                drawing.part(&placed.frame, &placed.bounds, snapped.part, draw::TARGET);
                drawing.cross(snapped.target.point, draw::SNAP_POINT);
            }
            if !drag.cursor_moves && drag.pointer != drag.pressed_at {
                // The mouse moved but the cursor didn't (the engine holds it still): show where the
                // resize points, as deep as the grab.
                let grab = frame.point(drag.resize.grab);
                let depth = geometry::length(geometry::sub(grab, view.ray.origin));
                drawing.pointer(geometry::along(&view.ray, depth), draw::POINTER);
            }
        } else if let Some(hover) = &self.hover {
            let placed = scene.get(hover.index);
            match hover.part {
                Some(part) => {
                    drawing.outline(&placed.frame, &placed.bounds, draw::BOX);
                    drawing.part(&placed.frame, &placed.bounds, part, draw::PART);
                }
                None => drawing.outline(&placed.frame, &placed.bounds, draw::FIXED_BOX),
            }
        }
        drawing.lines
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

/// The editor's visible bricks with their boxes, and where the vehicle is.
///
/// # Safety
/// `editor` must be a live editor. Game thread only.
unsafe fn gather(game: &Game, editor: *mut UObjectBase) -> Scene {
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
        // The vehicle's place in the world, from a brick's places in both.
        let vehicle = bricks
            .first()
            .and_then(|&object| {
                let (location, rotation) = game.spawn.read(object)?;
                let world = Frame::of_transform(&game.world.of(object));
                Some(Frame::of_place(location, rotation).parent(&world))
            })
            .unwrap_or(Frame::new([0.0; 3], math::Quat::IDENTITY));
        let bricks: Vec<Placed> = bricks
            .into_iter()
            .filter_map(|object| placed(game, object))
            .collect();
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
        })
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

/// What the dragged part snaps to: the part of another brick under the mouse.
///
/// # Safety
/// Game thread only.
unsafe fn snap(game: &Game, scene: &Scene, drag: &Drag, view: &View) -> Option<Snap> {
    let editor = drag.editor.get()?;
    let counterpart = drag.counterpart.as_ref().map(|(other, _)| &other.object);
    let resized = |placed: &Placed| {
        drag.brick
            .object
            .get()
            .is_some_and(|object| placed.object.is(object))
            || counterpart
                .and_then(WeakObject::get)
                .is_some_and(|object| placed.object.is(object))
    };
    // SAFETY: per the contract; the editor is live.
    let pointed = scene.pick(
        view,
        resized,
        |_| false,
        drag.depth,
        |point| unsafe { project(game, editor, &scene.vehicle, point) },
    )?;
    let placed = scene.get(pointed.index);
    let part = pointed.part;
    let corners: Vec<Vec3> = part
        .corners(&placed.bounds)
        .into_iter()
        .map(|corner| placed.frame.point(corner))
        .collect();
    let target = match part.dimensions_fixed() {
        3 => Target {
            point: corners[0],
            normal: None,
        },
        2 => Target {
            point: geometry::closest_on_segment(&view.ray, corners[0], corners[1]),
            normal: None,
        },
        _ => {
            let axis = part.axes().next()?;
            let outward = f64::from(part.sides[axis]);
            Target {
                point: pointed.point,
                normal: Some(geometry::scale(placed.frame.axes[axis], outward)),
            }
        }
    };
    Some(Snap {
        index: pointed.index,
        part,
        target,
    })
}

/// Asks the dragged brick for `size`, then places it (and its counterpart) to keep the parts not
/// dragged in place. False if the brick is gone or can't be resized.
///
/// # Safety
/// Game thread only.
unsafe fn apply(game: &Game, drag: &mut Drag, size: Vector, interactive: bool) -> bool {
    let Some(object) = drag.brick.object.get() else {
        return false;
    };
    // SAFETY: per the contract; the brick and its counterpart are live scalable bricks.
    unsafe {
        let Some(kept) = game.sizes.resize(object, size, interactive) else {
            LOG_NO_INTERFACE
                .call_once(|| log!("can't resize: bricks' property interface not found"));
            return false;
        };
        let (bounds, shift) = drag.resize.placed(math::widen(kept));
        let spawn = Frame::of_place(drag.brick.location, drag.brick.rotation);
        let location = math::narrow(spawn.point(shift));
        game.spawn.set(object, location, drag.brick.rotation);
        drag.bounds = bounds;
        if let (Some((other, axis)), Some(mirror)) = (&drag.counterpart, &game.mirror)
            && let Some(counterpart) = other.object.get()
        {
            game.sizes.resize(counterpart, kept, interactive);
            let mirrored = mirror.mirrored_location(object, location, drag.brick.rotation, *axis);
            game.spawn.set(counterpart, mirrored, other.rotation);
        }
    }
    true
}
