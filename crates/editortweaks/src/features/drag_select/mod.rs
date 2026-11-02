//! Selecting bricks with a box, in the editor's default mode: hold the drag select key
//! ([`keybinds::DRAG_SELECT`], Ctrl + right mouse button by default) and drag over the 3D view. A
//! box shows from where the key went down to the mouse, and the bricks show as letting go would
//! leave them, with the game's own selection outline. Letting go selects them, replacing the
//! selection, or adds them to it with [`keybinds::ADD_TO_DRAG_SELECTION`] (Shift) held then.
//! Which bricks the box picks is a setting ([`PICKS`]): those it touches, those entirely inside
//! it, or those whose center is, by their shape (what a click hits). Bricks behind others count
//! (the box goes through the vehicle); hidden ones don't. A press without a drag (a click) changes
//! nothing.
//!
//! How: on a mouse button the game uses (`Select`, `MoveCamera`), the key's press and release are
//! taken from the game's handlers ([`super::editor_buttons`]), so the camera doesn't start moving
//! (they only get presses over the 3D view, not over the editor's panels); another key starts the
//! box at the editor's next tick. The mouse's movement meanwhile is taken
//! too ([`super::mouse_moves`]), so the camera doesn't turn, and followed where the engine holds the
//! cursor still. When the box starts, the editor's bricks are gathered with their shapes
//! ([`shape`]); every tick, the screen box becomes the view through it (the rays through its
//! corners, `Viewport::ray`), which picks bricks ([`frustum`]). The box is drawn with the game's UI
//! ([`marquee`]); the bricks' outlines are the game's (`game::outline`), changed on their
//! components while the box shows and given back before letting go selects through the editor
//! mode's own `SelectObjects`, as a click does (`game::editor_mode::Selector`).

mod frustum;
mod marquee;
mod shape;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once, OnceLock, PoisonError, TryLockError};

use brickrust::ue::coreuobject::UObjectBase;

use super::editor_buttons::{self, Button, Click, Press};
use super::keybinds;
use super::mouse_moves::{self, Handling, Movement, Pointer};
use crate::framework::settings::{Alternative, Choice};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_model::BrickModels;
use crate::game::collision::Collision;
use crate::game::editor_mode::{self, EditorModes, Selector};
use crate::game::editor_object::{self, LocalBounds, Selection, WorldTransforms};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::line_batcher::{self, Line, LineBatcher};
use crate::game::outline::{self, Look, OutlineState, Outlines};
use crate::game::ui_style::{self, ThemeColors};
use crate::game::view_canvas::{self, ViewCanvas};
use crate::game::viewport::{self, Ray, Viewport};
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat};
use crate::platform::keyboard;
use crate::prelude::*;

use frustum::Frustum;
use marquee::Marquee;
use shape::{Fit, Mesh, Meshes, Place, Shape};

/// Which bricks a box picks, a setting: the options of [`Picking`], in order.
pub static PICKS: Choice = Choice::new(
    "DragSelectPicks",
    "Drag select picks",
    "Which bricks a drag select box selects. Bricks behind others count too, hidden ones don't.",
    &[
        Alternative {
            key: "Touching",
            name: "Touched bricks",
        },
        Alternative {
            key: "Centers",
            name: "Center inside",
        },
        Alternative {
            key: "Inside",
            name: "Fully inside",
        },
    ],
    0,
);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Picking {
    Touching,
    Centers,
    Inside,
}

/// Which bricks a box picks now.
fn picking() -> Picking {
    match PICKS.get() {
        1 => Picking::Centers,
        2 => Picking::Inside,
        _ => Picking::Touching,
    }
}

/// How far the mouse must move (screen pixels, across or down) for a box: less is a click.
const MIN_DRAG: f32 = 4.0;
/// How far past the camera's near plane the box is drawn when it's drawn as lines (cm). Drawn over
/// everything anyway.
const BOX_DEPTH: f64 = 10.0;
/// This feature's lines, among others' (see `LineBatcher::replace`).
const LINES: &str = "drag_select";

pub struct DragSelect;

/// Set once everything is found; until then (or if that fails) nothing happens.
static GAME: OnceLock<Game> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

/// Whether a box is being dragged: it restyles the bricks' outlines meanwhile, so other features
/// leave them alone (`game::outline_cases`). Busy (asked from inside this feature's tick) counts
/// as dragging.
pub fn dragging() -> bool {
    match STATE.try_lock() {
        Ok(state) => state.drag.is_some(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().drag.is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
    }
}

static LOG_STARTED: Once = Once::new();
static LOG_SELECTED: Once = Once::new();
static LOG_NO_LINES: Once = Once::new();
static LOG_NO_MODEL: Once = Once::new();
static LOG_NO_PIECES: Once = Once::new();
static LOG_NO_CANVAS: Once = Once::new();

/// The game code and reflection this feature uses.
struct Game {
    editor: BrickEditor,
    modes: EditorModes,
    bricks: Bricks,
    code: EditorCode,
    bounds: LocalBounds,
    world: WorldTransforms,
    viewport: Viewport,
    lines: LineBatcher,
    selector: Selector,
    /// To leave selected bricks out when adding (else the game may unselect them all instead), and
    /// to show which a box unselects.
    selection: Option<Selection>,
    /// Bricks' meshes, to pick bricks by their shape (else by their box).
    models: Option<BrickModels>,
    /// Bricks' collision, to pick spinners by their shape (else by their box).
    collision: Option<Collision>,
    /// The selection outline, to show bricks as letting go would leave them.
    outlines: Option<Outlines>,
    /// The game's UI over the 3D view, for the box (else it's lines).
    canvas: Option<ViewCanvas>,
    /// The box's color: the UI style's colors, and which is the highlight.
    theme: Option<(ThemeColors, u8)>,
}

impl Feature for DragSelect {
    fn id(&self) -> &'static str {
        "drag_select"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!("active: drag select key (Ctrl + right mouse button by default) + drag"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let theme = ui_style::theme_colors()
        .and_then(|colors| Ok((colors, ui_style::color_style_values()?.highlight)));
    let game = Game {
        editor: brick_editor::find()?,
        modes: editor_mode::find()?,
        bricks: editor_view::bricks()?,
        code: editor_view::editor_code()?,
        bounds: LocalBounds::find()?,
        world: WorldTransforms::find()?,
        viewport: viewport::find()?,
        lines: line_batcher::find()?,
        selector: editor_mode::selector()?,
        selection: optional(
            editor_object::selection(),
            "adding to the selection may unselect bricks instead",
        ),
        models: optional(BrickModels::find(), "bricks are picked by their boxes"),
        collision: optional(Collision::find(), "spinners are picked by their boxes"),
        outlines: optional(outline::find(), "bricks don't show what a box selects"),
        canvas: optional(view_canvas::find(), "the box is drawn as lines"),
        theme: optional(theme, "the box takes the default style's color"),
    };
    let _ = GAME.set(game);
    mouse_moves::listen(take_movement)?;
    editor_buttons::listen(on_button)
}

/// `found`, or `None` after logging what goes `without` it.
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

/// While a box is drawn, takes the buttons' presses and releases (the key's release ends it);
/// else a press with the key held starts one. Whether the game's handler is skipped.
fn on_button(press: &Press) -> bool {
    let Some(game) = GAME.get() else {
        return false;
    };
    if press.button == Button::AddToSelection || press.button.is_key() {
        return false;
    }
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: an input handler's call, on the game thread; its input component is live.
    unsafe {
        if state.drag.is_some() {
            if press.click == Click::Released && !keybinds::DRAG_SELECT.is_held() {
                state.finish(game);
            }
            return true;
        }
        if press.click != Click::Pressed || !keybinds::DRAG_SELECT.is_held() {
            return false;
        }
        let Some(editor) = game.modes.editor_of(press.input) else {
            return false;
        };
        state.start(game, editor)
    }
}

/// Takes the mouse's movement while a box is drawn: the camera stays still, and the box follows it
/// while the engine holds the cursor still.
fn take_movement(movement: &Movement) -> Handling {
    let mut state = match STATE.try_lock() {
        Ok(state) => state,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        // Only the game thread uses the state, and not around input handlers.
        Err(TryLockError::WouldBlock) => return Handling::Pass,
    };
    match &mut state.drag {
        Some(drag) => {
            drag.pointer.moved(movement.pixels());
            Handling::Take
        }
        None => Handling::Pass,
    }
}

/// A box being drawn.
struct Drag {
    editor: WeakObject,
    pointer: Pointer,
    /// The editor's bricks as the box started.
    bricks: Vec<Brick>,
    /// Those the box picks now (indices in `bricks`), nearest first.
    picked: Vec<usize>,
    /// The bricks whose outline the box changed (by index in `bricks`).
    restyled: HashMap<usize, Restyled>,
    /// The box's color (linear).
    color: [f32; 3],
    /// Whether the box shows with the game's UI (else as lines).
    on_canvas: bool,
}

/// A brick of the editor.
struct Brick {
    object: WeakObject,
    /// Its root component: what the outline shows of it.
    component: Option<WeakObject>,
    shape: Shape,
}

/// A brick whose outline a box changed: how the game had it, and how the box shows it.
struct Restyled {
    game: Look,
    shown: Look,
}

struct State {
    drag: Option<Drag>,
    /// Whether the key was held at the last tick (one hold starts one box).
    held: bool,
    /// Whether our lines are showing.
    drawn: bool,
    /// Bricks' meshes read so far, kept between boxes.
    meshes: Option<Meshes>,
    /// The box's widgets, kept between boxes.
    marquee: Marquee,
}

// SAFETY: object pointers are only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

impl State {
    const fn new() -> Self {
        Self {
            drag: None,
            held: false,
            drawn: false,
            meshes: None,
            marquee: Marquee::new(),
        }
    }

    /// Every frame of `editor`: starts a box when the key goes down (if it isn't on the left or
    /// right button, whose handlers start it), follows the mouse, and ends the box when the key
    /// goes up.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn tick(&mut self, game: &Game, editor: *mut UObjectBase) {
        let held = keybinds::DRAG_SELECT.is_held();
        let pressed = held && !self.held;
        self.held = held;
        // SAFETY: per the contract.
        unsafe {
            // With a button down, the key is on it: pressed over a panel, if no handler started.
            if self.drag.is_none() && pressed && !keyboard::is_mouse_button_down() {
                self.start(game, editor);
            }
            let Some(drag) = &self.drag else {
                if self.drawn {
                    self.draw(game, Vec::new());
                }
                return;
            };
            if !drag.editor.is(editor) || game.modes.default_mode(editor).is_none() {
                // Another editor (that one is gone or not ticking), or the editor went into another
                // mode (moving bricks, say): no selecting.
                self.cancel(game);
            } else if !held {
                // Released where no handler saw it (another window, or not a mouse button).
                self.finish(game);
            } else {
                let lines = self.follow(game, editor);
                self.draw(game, lines);
            }
        }
    }

    /// Starts a box in `editor` at the mouse, if it's in its default mode; whether it did.
    ///
    /// # Safety
    /// `editor` must be a live editor. Game thread only.
    unsafe fn start(&mut self, game: &Game, editor: *mut UObjectBase) -> bool {
        // SAFETY: per the contract; the controller is live if not null.
        unsafe {
            if game.modes.default_mode(editor).is_none() {
                return false;
            }
            let controller = game.editor.player_controller(editor);
            if controller.is_null() {
                return false;
            }
            let Some(mouse) = game.viewport.mouse(controller) else {
                return false;
            };
            let meshes = self.meshes.get_or_insert_with(Meshes::default);
            let bricks = gather(game, meshes, editor);
            let color = box_color(game);
            let on_canvas = match &game.canvas {
                Some(canvas) => self.marquee.begin(canvas, controller, color),
                None => false,
            };
            if !on_canvas && game.canvas.is_some() {
                LOG_NO_CANVAS.call_once(|| {
                    log!("the box is drawn as lines: the editor's view panel wasn't found")
                });
            }
            LOG_STARTED.call_once(|| {
                let fitted = |fit| {
                    bricks
                        .iter()
                        .filter(|brick| brick.shape.fit() == Some(fit))
                        .count()
                };
                log!(
                    "drawing a box over {} bricks, {} by their mesh and {} spinners by their \
                     pieces (picks: {:?}, {})",
                    bricks.len(),
                    fitted(Fit::Scaled),
                    fitted(Fit::AsIs),
                    picking(),
                    if on_canvas { "UI box" } else { "lines" },
                )
            });
            self.drag = Some(Drag {
                editor: WeakObject::new(editor),
                pointer: Pointer::new(mouse),
                bricks,
                picked: Vec::new(),
                restyled: HashMap::new(),
                color,
                on_canvas,
            });
        }
        true
    }

    /// Follows the mouse: finds the bricks the box picks, shows them, and places the box. The lines
    /// to draw, if it's drawn as lines.
    ///
    /// # Safety
    /// `editor` must be the live editor ticking. Game thread only.
    unsafe fn follow(&mut self, game: &Game, editor: *mut UObjectBase) -> Vec<Line> {
        let Some(drag) = &mut self.drag else {
            return Vec::new();
        };
        // SAFETY: per the contract; the controller is live if not null.
        let controller = unsafe { game.editor.player_controller(editor) };
        if controller.is_null() {
            return Vec::new();
        }
        // SAFETY: as above.
        let cursor = unsafe { game.viewport.mouse(controller) };
        let (start, end) = (drag.pointer.start(), drag.pointer.point(cursor));
        let mut rays = Vec::with_capacity(4);
        for corner in screen_box(start, end) {
            // SAFETY: as above.
            match unsafe { game.viewport.ray(controller, corner) } {
                Some(ray) => rays.push(ray),
                None => return Vec::new(),
            }
        }
        let rays: [Ray; 4] = [rays[0], rays[1], rays[2], rays[3]];
        drag.picked.clear();
        let view = is_drag(start, end).then(|| Frustum::new(&rays)).flatten();
        if let Some(view) = &view {
            let picking = picking();
            let mut picked: Vec<(f64, usize)> = drag
                .bricks
                .iter()
                .enumerate()
                .filter(|(_, brick)| match picking {
                    Picking::Touching => brick.shape.touched_by(view),
                    Picking::Inside => brick.shape.inside(view),
                    Picking::Centers => view.contains(brick.shape.center()),
                })
                .map(|(index, brick)| (view.distance(brick.shape.center()), index))
                .collect();
            picked.sort_by(|a, b| a.0.total_cmp(&b.0));
            drag.picked = picked.into_iter().map(|(_, index)| index).collect();
        }
        // SAFETY: as above.
        unsafe {
            // A click shows nothing on the bricks: it changes nothing.
            let add = keybinds::ADD_TO_DRAG_SELECTION.is_held();
            preview(game, drag, view.is_some(), add);
            if drag.on_canvas
                && let Some(canvas) = &game.canvas
                && self.marquee.show(canvas, controller, start, end)
            {
                return Vec::new();
            }
        }
        draw_box(&rays, drag.color)
    }

    /// Ends the box: selects the bricks it picks (unless it was a click).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn finish(&mut self, game: &Game) {
        // SAFETY: per the contract.
        let Some(mut drag) = (unsafe { self.end(game) }) else {
            return;
        };
        // Where the mouse went since the last tick doesn't count: the box showed what it picks.
        if !is_drag(drag.pointer.start(), drag.pointer.at()) {
            return;
        }
        let Some(editor) = drag.editor.get() else {
            return;
        };
        // SAFETY: the editor is live; the objects are its live bricks (`WeakObject::get`).
        unsafe {
            let Some(mode) = game.modes.default_mode(editor) else {
                return;
            };
            let add = keybinds::ADD_TO_DRAG_SELECTION.is_held();
            let objects: Vec<*mut UObjectBase> = drag
                .picked
                .iter()
                .filter_map(|&index| drag.bricks[index].object.get())
                .filter(|&object| {
                    // Adding a selected brick could unselect the others (see `Selector::select`).
                    !add || !game
                        .selection
                        .as_ref()
                        .is_some_and(|selection| selection.is_selected(object))
                })
                .collect();
            if add && objects.is_empty() {
                return;
            }
            game.selector.select(mode, &objects, add);
            LOG_SELECTED.call_once(|| {
                log!(
                    "{} {} bricks with a box",
                    if add { "added" } else { "selected" },
                    objects.len()
                )
            });
        }
        drag.picked.clear();
    }

    /// Ends the box without selecting.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn cancel(&mut self, game: &Game) {
        // SAFETY: per the contract.
        unsafe { self.end(game) };
    }

    /// Takes the box away: hides it and gives the bricks their outlines back. The box, if there
    /// was one.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn end(&mut self, game: &Game) -> Option<Drag> {
        let mut drag = self.drag.take()?;
        // SAFETY: per the contract.
        unsafe {
            self.draw(game, Vec::new());
            if let Some(canvas) = &game.canvas {
                self.marquee.hide(canvas);
            }
            preview(game, &mut drag, false, false);
        }
        Some(drag)
    }

    /// Shows `lines` (none to clear ours).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn draw(&mut self, game: &Game, lines: Vec<Line>) {
        if lines.is_empty() && !self.drawn {
            return;
        }
        // SAFETY: per the contract; the batcher is live if not null.
        unsafe {
            let batcher = game.lines.batcher();
            if batcher.is_null() {
                LOG_NO_LINES.call_once(|| log!("no box shown: no line batcher in this world"));
                return;
            }
            game.lines.replace(batcher, LINES, &lines);
        }
        self.drawn = !lines.is_empty();
    }
}

/// Outlines the box's bricks as letting go would leave them, while `active` (a drag, not a
/// click): those it picks selected, and unless `add`ing, the selected ones it misses unselected.
/// The others get the game's outline back.
///
/// # Safety
/// Game thread only.
unsafe fn preview(game: &Game, drag: &mut Drag, active: bool, add: bool) {
    let Some(outlines) = &game.outlines else {
        return;
    };
    let mut picked = vec![false; drag.bricks.len()];
    if active {
        for &index in &drag.picked {
            picked[index] = true;
        }
    }
    for (index, brick) in drag.bricks.iter().enumerate() {
        let Some(component) = brick.component.as_ref().and_then(WeakObject::get) else {
            drag.restyled.remove(&index);
            continue;
        };
        // SAFETY: per the contract; the brick and its component are live (`WeakObject::get`).
        unsafe {
            let selected = brick.object.get().is_some_and(|object| {
                game.selection
                    .as_ref()
                    .is_some_and(|selection| selection.is_selected(object))
            });
            let state = match (active, picked[index], selected) {
                (true, true, false) => Some(OutlineState::Selected),
                (true, false, true) if !add => Some(OutlineState::Unselected),
                _ => None,
            };
            let now = outlines.look(component);
            match state {
                Some(state) => {
                    let restyled = drag.restyled.entry(index).or_insert(Restyled {
                        game: now,
                        shown: now,
                    });
                    if now != restyled.shown {
                        // The game changed it meanwhile (hovered, say): that's its look now.
                        restyled.game = now;
                    }
                    restyled.shown = restyled.game.in_state(state);
                    outlines.set_look(component, restyled.shown);
                }
                None => {
                    if let Some(restyled) = drag.restyled.remove(&index)
                        && now == restyled.shown
                    {
                        outlines.set_look(component, restyled.game);
                    }
                }
            }
        }
    }
}

/// The box's color: the UI style's highlight, as the selection outline shows it (linear).
///
/// # Safety
/// Game thread only.
unsafe fn box_color(game: &Game) -> [f32; 3] {
    let [red, green, blue, _] = game
        .theme
        .as_ref()
        // SAFETY: per the contract.
        .and_then(|(colors, highlight)| unsafe { colors.color(*highlight) })
        .unwrap_or(ui_style::DEFAULT_HIGHLIGHT);
    [red, green, blue]
}

/// The corners of the box from `start` to `end` (screen pixels), in order around it, at least a
/// pixel apart.
fn screen_box(start: [f32; 2], end: [f32; 2]) -> [[f32; 2]; 4] {
    let left = start[0].min(end[0]);
    let top = start[1].min(end[1]);
    let right = start[0].max(end[0]).max(left + 1.0);
    let bottom = start[1].max(end[1]).max(top + 1.0);
    [[left, top], [right, top], [right, bottom], [left, bottom]]
}

/// Whether the mouse went far enough from `start` to `end` for a box.
fn is_drag(start: [f32; 2], end: [f32; 2]) -> bool {
    (end[0] - start[0]).abs() >= MIN_DRAG || (end[1] - start[1]).abs() >= MIN_DRAG
}

/// The box's edges, a little past the near plane, over everything, in `color`.
fn draw_box(rays: &[Ray; 4], color: [f32; 3]) -> Vec<Line> {
    let corners = rays.map(|ray| {
        let origin = math::widen(ray.origin);
        let direction = math::widen(ray.direction);
        math::narrow(math::add(origin, math::scale(direction, BOX_DEPTH)))
    });
    (0..4)
        .map(|index| Line {
            start: corners[index],
            end: corners[(index + 1) % 4],
            color,
            thickness: 0.0,
            on_top: true,
        })
        .collect()
}

/// The editor's visible objects with a box, and their shapes in the world.
///
/// # Safety
/// `editor` must be a live editor. Game thread only.
unsafe fn gather(game: &Game, meshes: &mut Meshes, editor: *mut UObjectBase) -> Vec<Brick> {
    // SAFETY: per the contract; the editor's objects are live (entries may be null), and so are
    // their components and meshes if not null.
    unsafe {
        game.bricks
            .editor_objects(editor)
            .iter()
            .copied()
            .filter(|&object| !object.is_null() && !game.code.is_hidden(object))
            .filter_map(|object| {
                let bounds = game.bounds.of(object)?.map(math::widen);
                let transform = game.world.of(object);
                let [x, y, z, _] = transform.translation;
                let place = Place {
                    location: math::widen([x, y, z]),
                    rotation: Quat::widen(transform.rotation),
                };
                let component = game
                    .outlines
                    .as_ref()
                    .and_then(|outlines| outlines.component_of(object));
                let mesh = click_shape(game, meshes, object, component);
                let shape = Shape::new(place, bounds, mesh.clone());
                if let Some((mesh, Fit::AsIs)) = &mesh
                    && shape.fit().is_none()
                {
                    LOG_NO_PIECES.call_once(|| {
                        log!(
                            "a spinner is picked by its box: its pieces ({:?}) stick out of its \
                             box ({bounds:?})",
                            mesh.bounds()
                        )
                    });
                }
                Some(Brick {
                    object: WeakObject::new(object),
                    component: component.map(|component| WeakObject::new(component)),
                    shape,
                })
            })
            .collect()
    }
}

/// What a click on `object` hits, and how the brick shows it: a spinner's own collision pieces,
/// else its mesh's complex collision (see [`shape`]). `None` (logged once) to pick it by its box.
///
/// # Safety
/// `object` must be a live editor object and `component` its root component, live if any. Game
/// thread only.
unsafe fn click_shape(
    game: &Game,
    meshes: &mut Meshes,
    object: *mut UObjectBase,
    component: Option<*mut UObjectBase>,
) -> Option<(Arc<Mesh>, Fit)> {
    // SAFETY: per the contract; body setups and meshes are live if not null.
    unsafe {
        if let Some(collision) = &game.collision
            && collision.has_own_body_setup(object)
        {
            let body_setup = collision
                .component_of(object)
                .and_then(|component| collision.body_setup_of(object, component).0);
            let pieces = match body_setup {
                Some(body_setup) => meshes.get(body_setup, || {
                    Mesh::from_pieces(&collision.shapes(body_setup)?)
                }),
                None => Err("no collision".into()),
            };
            return match pieces {
                Ok(pieces) => Some((pieces, Fit::AsIs)),
                Err(reason) => {
                    LOG_NO_PIECES.call_once(|| log!("a spinner is picked by its box: {reason}"));
                    None
                }
            };
        }
        let models = game.models.as_ref()?;
        let mesh = match component {
            Some(component) => models.mesh_of_component(component),
            None => models.mesh_of(object),
        }?;
        match meshes.get(mesh, || Mesh::from_triangles(&models.triangles(mesh)?)) {
            Ok(mesh) => Some((mesh, Fit::Scaled)),
            Err(reason) => {
                LOG_NO_MODEL.call_once(|| log!("a brick is picked by its box: {reason}"));
                None
            }
        }
    }
}
