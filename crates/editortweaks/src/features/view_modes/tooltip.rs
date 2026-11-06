//! The tooltip of our view modes: the game's own tooltip (`game::tooltip_widget`) next to the
//! mouse, telling about what's under it, in the view modes that have something to tell
//! ([`super::describe`]): "Strength", "Connections", "Reference fit", and the game's "Physics" (its
//! centers, [`super::physics`]). Shown over the 3D view
//! (`game::view_canvas`), worked out again only when the mouse, the camera or the editor changed,
//! and hidden while the camera moves.
//! Off with the setting [`TOOLTIPS`] (shared with the shot preview's tooltips).
//!
//! What's under the mouse is traced like the editor's clicks (`game::line_trace`), hidden bricks
//! left out ([`Hover::point`]). [`Hover::line`] follows the line through every brick it crosses, as
//! a projectile flies: a projectile hitting a brick goes on with that brick left out, so it meets
//! each brick on its way, even bricks in the same place. So each brick whose box (`game::
//! component_bounds`, kept until the editor changes) the line meets is traced alone: where the line
//! enters it, and where it last leaves it (traced back from beyond it).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use super::Editor;
use crate::framework::settings::{Alternative, Choice};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::collision::Collision;
use crate::game::component_bounds::{self, ComponentBounds};
use crate::game::editor_object::Vector;
use crate::game::line_trace::{self, ComponentTrace, LineTrace};
use crate::game::view_tooltip::{self, ViewTooltip, ViewTooltipCode};
use crate::game::viewport::Ray;
use crate::math;
use crate::platform::keyboard;
use crate::prelude::*;

/// Whether the shot preview and our view modes show their tooltips, a setting.
pub static TOOLTIPS: Choice = Choice::new(
    "Tooltips",
    "Tooltips",
    "Tooltips next to the mouse. Shot preview: on a gun, a shot's path or what it hits, its damage, penetration, range and speed (Shift for more). View modes: about the brick or center under the mouse.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "ShotPreview",
            name: "Shot preview",
        },
        Alternative {
            key: "ViewModes",
            name: "View modes",
        },
        Alternative {
            key: "Both",
            name: "Both",
        },
    ],
    3,
);

/// Whether the shot preview's tooltips show now ([`TOOLTIPS`]).
pub fn shot_preview_shows() -> bool {
    matches!(TOOLTIPS.get(), 1 | 3)
}

/// Whether our view modes' tooltips show now ([`TOOLTIPS`]).
pub fn view_modes_show() -> bool {
    matches!(TOOLTIPS.get(), 2 | 3)
}

/// How far the line looks for bricks, in cm.
const REACH: f32 = 1.0e6;
/// How far past a brick's far side the next trace starts, in cm: past its surface.
const STEP: f32 = 0.01;
/// How far beyond where the line enters a brick the trace back to its far side starts, in cm:
/// beyond any brick.
const BEYOND: f32 = 1.0e5;
/// How much bigger than a brick's box the line must meet, in cm: the box is the engine's, rounded.
const BOX_MARGIN: f32 = 1.0;
/// How long after the camera last moved the tooltip stays away, so that it doesn't flash back
/// between two movements (slow ones can leave a frame or two with the camera still).
const AFTER_CAMERA: Duration = Duration::from_millis(150);
/// What the tooltip needs from the game (after engine init).
struct Code {
    editor: BrickEditor,
    trace: LineTrace,
    component_trace: ComponentTrace,
    bounds: ComponentBounds,
    collision: Collision,
    ui: ViewTooltipCode,
}

/// The tooltip between frames.
struct State {
    tooltip: ViewTooltip,
    /// What it was worked out for, and what it said then (no lines: hidden).
    key: Option<Key>,
    shown: Description,
    /// The ray through the middle of the view (the mouse's moves with the mouse), and when it last
    /// changed: the camera moving.
    view: Option<[Vector; 2]>,
    camera_moved: Option<Instant>,
}

/// What the tooltip's text depends on.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    mouse: [f32; 2],
    ray: [Vector; 2],
    /// [`super::GENERATION`]: changes with the editor.
    generation: u64,
    view: u8,
}

/// What a tooltip says: a title over lines.
#[derive(Clone, Default, PartialEq)]
pub struct Description {
    pub title: String,
    pub lines: Vec<String>,
}

/// The box around each brick's component, by brick address, as of the editor's
/// [`super::GENERATION`].
struct Boxes {
    generation: u64,
    boxes: Vec<(usize, [Vector; 2])>,
}

static CODE: OnceLock<Option<Code>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State {
    tooltip: ViewTooltip::new(),
    key: None,
    shown: Description {
        title: String::new(),
        lines: Vec::new(),
    },
    view: None,
    camera_moved: None,
});
static BOXES: Mutex<Option<Boxes>> = Mutex::new(None);

/// What's under the mouse, for a view mode's tooltip.
pub struct Hover<'a> {
    /// The ray from the camera through the mouse (in the world; a unit direction).
    pub ray: Ray,
    code: &'a Code,
    generation: u64,
}

/// A brick the line meets: how far along the ray it enters it, and last leaves it (in cm).
#[derive(Clone, Copy)]
pub struct Crossing {
    pub brick: *mut UObjectBase,
    pub entry: f32,
    pub exit: f32,
}

impl Hover<'_> {
    /// The point `along` cm along the ray.
    pub fn at(&self, along: f32) -> Vector {
        math::narrow(math::add(
            math::widen(self.ray.origin),
            math::scale(math::widen(self.ray.direction), f64::from(along)),
        ))
    }

    /// How far along the ray it meets the ball around `component` (its box's center, as wide as the
    /// box's widest side), if it does: for round markers, which are drawn over everything.
    ///
    /// # Safety
    /// `component` must be a live scene component. Game thread only.
    pub unsafe fn sphere_hit(&self, component: *mut UObjectBase) -> Option<f32> {
        // SAFETY: per the contract.
        let [min, max] = unsafe { self.code.bounds.of(component) };
        let center = math::scale(math::add(math::widen(min), math::widen(max)), 0.5);
        let radius = (0..3)
            .map(|axis| f64::from(max[axis] - min[axis]) / 2.0)
            .fold(0.0, f64::max);
        let along = f64::from(self.along(math::narrow(center)));
        let closest = math::add(
            math::widen(self.ray.origin),
            math::scale(math::widen(self.ray.direction), along),
        );
        let miss = math::length(math::sub(center, closest));
        (along > 0.0 && miss <= radius)
            .then(|| (along - (radius * radius - miss * miss).sqrt()) as f32)
    }

    /// How far along the ray `point` is, in cm.
    fn along(&self, point: Vector) -> f32 {
        let offset = math::sub(math::widen(point), math::widen(self.ray.origin));
        math::dot(offset, math::widen(self.ray.direction)) as f32
    }

    /// The brick of `editor` under the mouse, as the editor's clicks find it (hidden ones left
    /// out), and how far along the ray the line meets it (`exit` is `entry`).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn point(&self, editor: &Editor) -> Option<Crossing> {
        // SAFETY: per the contract; the editor is live, and so are the objects it lists.
        let bricks = unsafe { editor.brick_list() };
        let mut along = 0.0;
        while along < REACH {
            // SAFETY: per the contract; the editor is a live actor in the world to trace.
            let hit = unsafe {
                self.code
                    .trace
                    .first_hit(editor.editor, self.at(along), self.at(REACH))
            }?;
            let component = hit.component.filter(|_| hit.is_brick())?;
            let entry = along + hit.distance;
            let brick = hit.editor_object().filter(|object| bricks.contains(object));
            // SAFETY: one of the editor's bricks, live.
            if let Some(brick) = brick.filter(|&brick| !unsafe { editor.is_hidden(brick) }) {
                return Some(Crossing {
                    brick,
                    entry,
                    exit: entry,
                });
            }
            // Hidden, or not a brick: on past its far side.
            // SAFETY: a live component (just resolved).
            along = unsafe { self.far_side(component, entry) } + STEP;
        }
        None
    }

    /// Every brick of `editor` the line meets (hidden ones left out), nearest first, each traced
    /// alone: bricks in the same place each count.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn line(&self, editor: &Editor) -> Vec<Crossing> {
        // SAFETY: per the contract; the editor is live, and so are the objects it lists.
        let bricks = unsafe { editor.brick_list() };
        // SAFETY: as above.
        let met = unsafe { self.boxes_met(&bricks) };
        let bricks: HashSet<usize> = bricks.into_iter().map(|brick| brick as usize).collect();
        let mut crossed: Vec<Crossing> = met
            .into_iter()
            .filter(|brick| bricks.contains(brick))
            .filter_map(|brick| {
                let brick = brick as *mut UObjectBase;
                // SAFETY: one of the editor's bricks, live, and its live component.
                unsafe {
                    if editor.is_hidden(brick) {
                        return None;
                    }
                    let component = self.code.collision.component_of(brick)?;
                    let entry = self.code.component_trace.first_hit(
                        component,
                        self.ray.origin,
                        self.at(REACH),
                    )?;
                    let entry = self.along(entry);
                    let exit = self.far_side(component, entry);
                    Some(Crossing { brick, entry, exit })
                }
            })
            .collect();
        crossed.sort_by(|a, b| a.entry.total_cmp(&b.entry));
        crossed
    }

    /// Where the line last leaves `component` after entering it `entry` cm along: traced back from
    /// beyond it, against it alone.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    unsafe fn far_side(&self, component: *mut UObjectBase, entry: f32) -> f32 {
        // SAFETY: per the contract.
        unsafe {
            self.code
                .component_trace
                .first_hit(component, self.at(entry + BEYOND), self.at(entry))
        }
        .map_or(entry, |point| self.along(point).max(entry))
    }

    /// The bricks (by address) whose box the line meets, the boxes measured again if the editor
    /// changed.
    ///
    /// # Safety
    /// `bricks` must be the editor's bricks, live. Game thread only.
    unsafe fn boxes_met(&self, bricks: &[*mut UObjectBase]) -> Vec<usize> {
        let mut boxes = BOXES.lock().unwrap_or_else(PoisonError::into_inner);
        if boxes
            .as_ref()
            .is_none_or(|boxes| boxes.generation != self.generation)
        {
            let measured = bricks
                .iter()
                .filter_map(|&brick| {
                    // SAFETY: per the contract; the component is live.
                    unsafe {
                        let component = self.code.collision.component_of(brick)?;
                        Some((brick as usize, self.code.bounds.of(component)))
                    }
                })
                .collect();
            *boxes = Some(Boxes {
                generation: self.generation,
                boxes: measured,
            });
        }
        let Some(boxes) = boxes.as_ref() else {
            return Vec::new();
        };
        boxes
            .boxes
            .iter()
            .filter(|(_, bounds)| meets(self.ray, *bounds))
            .map(|&(brick, _)| brick)
            .collect()
    }
}

/// Whether `ray` meets the box `[min, max]` (grown by [`BOX_MARGIN`]) within [`REACH`].
fn meets(ray: Ray, [min, max]: [Vector; 2]) -> bool {
    let (mut near, mut far) = (0.0_f32, REACH);
    for axis in 0..3 {
        let (low, high) = (min[axis] - BOX_MARGIN, max[axis] + BOX_MARGIN);
        let (origin, direction) = (ray.origin[axis], ray.direction[axis]);
        if direction.abs() < 1e-9 {
            if origin < low || origin > high {
                return false;
            }
            continue;
        }
        let (a, b) = ((low - origin) / direction, (high - origin) / direction);
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return false;
        }
    }
    true
}

/// Finds the game code the tooltip needs, on the first tick: `None` if something's missing
/// (logged once).
fn code() -> Option<&'static Code> {
    CODE.get_or_init(|| {
        let found = (|| {
            Ok::<_, String>(Code {
                editor: brick_editor::find()?,
                trace: line_trace::find()?,
                component_trace: line_trace::component_trace()?,
                bounds: component_bounds::find()?,
                collision: Collision::find()?,
                ui: view_tooltip::find()?,
            })
        })();
        found
            .map_err(|reason| log!("the view modes show no tooltip: {reason}"))
            .ok()
    })
    .as_ref()
}

/// Every frame: shows the tooltip of the view mode shown, if it has one, or hides it.
pub fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(code) = code() else { return };
    // SAFETY: on the game thread (the editor's tick), `editor` the live editor ticking.
    unsafe {
        let shown = (view_modes_show() && !crate::features::gun_preview::tooltip_shows())
            .then(|| state.update(code, editor))
            .flatten();
        if shown.is_none() {
            state.hide(code);
        }
    }
}

impl State {
    /// Works out and shows the tooltip; `None` if there's none to show.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn update(&mut self, code: &Code, editor: *mut UObjectBase) -> Option<()> {
        // SAFETY: per the contract; the controller is live if not null.
        unsafe {
            let (view, generation) = super::tooltip_view(editor)?;
            let controller = code.editor.player_controller(editor);
            if controller.is_null() {
                return None;
            }
            if self.camera_moving(code, controller)? {
                return None;
            }
            let mouse = code.ui.viewport.mouse(controller)?;
            let ray = code.ui.viewport.ray(controller, mouse)?;
            let key = Key {
                mouse,
                ray: [ray.origin, ray.direction],
                generation,
                view,
            };
            if self.key != Some(key) {
                self.key = Some(key);
                let hover = Hover {
                    ray,
                    code,
                    generation,
                };
                let description = super::describe(editor, view, &hover);
                if description != self.shown {
                    self.shown = description;
                    let Description { title, lines } = self.shown.clone();
                    self.tooltip.set(&code.ui, title, lines);
                }
            }
            if self.shown.lines.is_empty() {
                return None;
            }
            self.tooltip.show(&code.ui, controller, mouse).ok()
        }
    }

    /// Whether the camera is moving, or about to (a button that turns it is down: the mouse stays
    /// where it was meanwhile, so nothing else tells). The tooltip is about what's under the mouse,
    /// which doesn't hold still then.
    ///
    /// # Safety
    /// `controller` must be the live editor's. Game thread only.
    unsafe fn camera_moving(&mut self, code: &Code, controller: *mut UObjectBase) -> Option<bool> {
        // SAFETY: per the contract.
        let view = unsafe {
            let [width, height] = code.ui.viewport.size(controller)?;
            let middle = [width as f32 * 0.5, height as f32 * 0.5];
            code.ui.viewport.ray(controller, middle)?
        };
        let view = [view.origin, view.direction];
        if self.view.replace(view).is_some_and(|last| last != view) {
            self.camera_moved = Some(Instant::now());
        }
        let moved_lately = self
            .camera_moved
            .is_some_and(|at| at.elapsed() < AFTER_CAMERA);
        Some(moved_lately || keyboard::is_mouse_button_down() || keyboard::is_middle_button_down())
    }

    /// Hides the tooltip.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn hide(&mut self, code: &Code) {
        self.key = None;
        // SAFETY: per the contract.
        unsafe { self.tooltip.hide(&code.ui) };
    }
}

/// `value` for people, its thousands grouped: two decimals, or three significant digits below 1
/// (up to six decimals).
pub fn number(value: f64) -> String {
    let size = value.abs();
    let decimals = if size >= 1.0 || size == 0.0 || !size.is_finite() {
        2
    } else {
        (2 - size.log10().floor() as i32).clamp(2, 6) as usize
    };
    let text = format!("{size:.decimals$}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}
