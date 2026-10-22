//! The shot preview's tooltip: the game's own tooltip over the 3D view (`game::view_tooltip`), next
//! to the mouse while it's on something the preview shows ([`super::hover`]), telling about it
//! ([`super::describe`]): the essentials, or everything while the extended tooltip key is held
//! (`keybinds::EXTENDED_TOOLTIP`). Nothing near the mouse: no tooltip.
//!
//! What the mouse is on is found again only when the mouse, the camera or the shots changed. A
//! frame where the tooltip can't be placed doesn't hide it (only [`HIDE_AFTER`] of them in a row
//! does), and why is logged once each, so a hiccup doesn't start its fade-in over.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use super::describe::{self, Context, Units};
use super::hover::{self, Screen, Target};
use super::projection::Projection;
use super::{Preview, Vehicle};
use crate::features::keybinds::EXTENDED_TOOLTIP;
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::display_name::{self, DisplayNames};
use crate::game::editor_object::Vector;
use crate::game::line_trace::{self, LineTrace};
use crate::game::reflection;
use crate::game::ui_library::{self, UnitConversion};
use crate::game::view_tooltip::{self, ViewTooltip, ViewTooltipCode};
use crate::math::{self, Vec3};
use crate::prelude::*;

/// How long the tooltip stays while it can't be placed.
const HIDE_AFTER: Duration = Duration::from_millis(250);
/// How far the line under the mouse looks for bricks, in cm.
const REACH: f32 = 1.0e6;
/// How far past a hidden brick's surface the line under the mouse goes on from, in cm.
const PAST_HIDDEN: f32 = 0.5;
/// How many hidden bricks the line under the mouse looks past at most.
const MAX_HIDDEN: usize = 16;

struct Code {
    editor: BrickEditor,
    ui: ViewTooltipCode,
    trace: LineTrace,
    names: DisplayNames,
    /// The factor from cm to the distances the player sees (`ENumericValueType::DistanceAuto`):
    /// metric or imperial.
    units: UnitConversion,
    distance: u8,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

static CODE: OnceLock<Option<Code>> = OnceLock::new();
/// Whether the engine is up (the tooltip's code is looked for on the first tick after it).
static ENGINE_UP: OnceLock<()> = OnceLock::new();
/// Whether the tooltip shows.
static SHOWS: AtomicBool = AtomicBool::new(false);

/// Marks the engine as up: the code is looked for on the next tick (the game's UI is loaded by
/// then).
pub fn find() {
    let _ = ENGINE_UP.set(());
}

/// Whether the tooltip shows.
pub fn shows() -> bool {
    SHOWS.load(Ordering::Relaxed)
}

fn code() -> Option<&'static Code> {
    ENGINE_UP.get()?;
    CODE.get_or_init(|| {
        let found = (|| {
            let distance =
                reflection::enum_value("ENumericValueType", "ENumericValueType::DistanceAuto")?;
            Ok::<_, String>(Code {
                editor: brick_editor::find()?,
                ui: view_tooltip::find()?,
                trace: line_trace::find()?,
                names: display_name::find()?,
                units: ui_library::unit_conversion()?,
                distance: u8::try_from(distance)
                    .map_err(|_| format!("ENumericValueType::DistanceAuto is {distance}"))?,
            })
        })();
        found
            .map_err(|reason| log!("the shot preview shows no tooltip: {reason}"))
            .ok()
    })
    .as_ref()
}

/// Where the editor's camera is, if it can be told.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
pub unsafe fn camera(editor: *mut UObjectBase) -> Option<Vec3> {
    let code = code()?;
    // SAFETY: per the contract; the controller is live if not null.
    unsafe {
        let controller = code.editor.player_controller(editor);
        if controller.is_null() {
            return None;
        }
        let [width, height] = code.ui.viewport.size(controller)?;
        let middle = [width as f32 * 0.5, height as f32 * 0.5];
        Some(math::widen(
            code.ui.viewport.ray(controller, middle)?.origin,
        ))
    }
}

/// The camera and shots the shots on the screen were placed for: the shots' key, the rays of the
/// [`Projection`] (origin and direction), and the view's size.
#[derive(Clone, Copy, PartialEq)]
struct ScreenKey {
    shots: u64,
    rays: [[Vector; 2]; 3],
    size: [i32; 2],
}

/// Logged once if our projection disagrees with the game's.
const SLOW_PROJECTION: &str =
    "the shot tooltip places paths with the game's projection (ours disagrees): slower";

/// Whether `projection` places the first point of `shots` (the muzzle) within a pixel of where
/// `game` places it (or neither can).
fn agrees(
    projection: &Projection,
    shots: &[Preview],
    game: impl Fn(Vec3) -> Option<[f32; 2]>,
) -> bool {
    let Some(point) = shots.first().and_then(|shot| shot.flight.points.first()) else {
        return true;
    };
    match (projection.project(point.at), game(point.at)) {
        (Some(ours), Some(theirs)) => (ours[0] - theirs[0]).hypot(ours[1] - theirs[1]) <= 1.0,
        (None, None) => true,
        _ => false,
    }
}

/// The tooltip between frames.
pub struct State {
    tooltip: ViewTooltip,
    /// The shots on the screen, and what they were placed for.
    screen: Option<(ScreenKey, Screen)>,
    /// What the mouse was on, and where it and the camera were then.
    hovered: Option<([f32; 2], ScreenKey, Option<Target>)>,
    /// What the text was worked out for: the shots, the target, and whether extended.
    shown_for: Option<(u64, Target, bool)>,
    /// What the editor calls each brick named so far (by address), for the shots of
    /// [`Self::names_for`].
    names: RefCell<Vec<(usize, Option<String>)>>,
    names_for: u64,
    /// Since when it couldn't be placed, and why that's been logged for.
    failing: Option<Instant>,
    logged: Vec<&'static str>,
}

impl State {
    pub const fn new() -> Self {
        Self {
            tooltip: ViewTooltip::new(),
            screen: None,
            hovered: None,
            shown_for: None,
            names: RefCell::new(Vec::new()),
            names_for: 0,
            failing: None,
            logged: Vec::new(),
        }
    }

    /// Shows the tooltip of what the mouse is on among `shots` (worked out for `shots_key`), or
    /// hides it. `hidden` tells the editor's hidden objects.
    ///
    /// # Safety
    /// `editor` must be the live editor, `vehicle` measured for it. Game thread only.
    pub unsafe fn update(
        &mut self,
        editor: *mut UObjectBase,
        shots: &[Preview],
        shots_key: u64,
        vehicle: Option<&Vehicle>,
        hidden: impl Fn(*mut UObjectBase) -> bool,
    ) {
        let Some(code) = code() else { return };
        // SAFETY: per the contract.
        let shown = unsafe { self.show(code, editor, shots, shots_key, vehicle, hidden) };
        match shown {
            Ok(true) => self.failing = None,
            Ok(false) => {
                // SAFETY: per the contract.
                unsafe { self.hide() };
            }
            Err(reason) => {
                if !self.logged.contains(&reason) {
                    self.logged.push(reason);
                    log!("the shot tooltip can't be placed this frame: {reason}");
                }
                let since = *self.failing.get_or_insert_with(Instant::now);
                if since.elapsed() >= HIDE_AFTER {
                    // SAFETY: per the contract.
                    unsafe { self.hide() };
                }
            }
        }
        SHOWS.store(self.tooltip.is_visible(), Ordering::Relaxed);
    }

    /// Shows the tooltip if the mouse is on something: whether it is.
    ///
    /// # Safety
    /// As for [`Self::update`].
    unsafe fn show(
        &mut self,
        code: &Code,
        editor: *mut UObjectBase,
        shots: &[Preview],
        shots_key: u64,
        vehicle: Option<&Vehicle>,
        hidden: impl Fn(*mut UObjectBase) -> bool,
    ) -> Result<bool, &'static str> {
        let viewport = &code.ui.viewport;
        // SAFETY: per the contract; the controller is live if not null.
        unsafe {
            let controller = code.editor.player_controller(editor);
            if controller.is_null() {
                return Err("no player controller");
            }
            let Some(mouse) = viewport.mouse(controller) else {
                return Ok(false);
            };
            let size = viewport.size(controller).ok_or("no viewport size")?;
            let [width, height] = size.map(|pixels| pixels as f32);
            let ray = |pixel: [f32; 2]| viewport.ray(controller, pixel);
            let (Some(middle), Some(right), Some(bottom)) = (
                ray([width * 0.5, height * 0.5]),
                ray([width, height * 0.5]),
                ray([width * 0.5, height]),
            ) else {
                return Err("no camera");
            };
            let key = ScreenKey {
                shots: shots_key,
                rays: [middle, right, bottom].map(|ray| [ray.origin, ray.direction]),
                size,
            };
            if self.screen.as_ref().is_none_or(|(known, _)| *known != key) {
                let game = |point: Vec3| viewport.project(controller, math::narrow(point));
                let ours = Projection::new(middle, right, bottom, size)
                    .filter(|projection| agrees(projection, shots, game));
                let screen = match ours {
                    Some(projection) => Screen::new(shots, |point| projection.project(point)),
                    None => {
                        if !self.logged.contains(&SLOW_PROJECTION) {
                            self.logged.push(SLOW_PROJECTION);
                            log!("{SLOW_PROJECTION}");
                        }
                        Screen::new(shots, game)
                    }
                };
                self.screen = Some((key, screen));
                self.hovered = None;
            }
            let known = self
                .hovered
                .filter(|(at, for_key, _)| *at == mouse && *for_key == key);
            let target = match known {
                Some((_, _, target)) => target,
                None => {
                    let on_screen = self
                        .screen
                        .as_ref()
                        .and_then(|(_, screen)| screen.target(shots, mouse));
                    let target = on_screen.or_else(|| {
                        let brick = under_mouse(code, editor, controller, mouse, &hidden)?;
                        hover::brick_target(shots, brick)
                    });
                    self.hovered = Some((mouse, key, target));
                    target
                }
            };
            let Some(target) = target else {
                return Ok(false);
            };
            let extended = EXTENDED_TOOLTIP.is_held();
            if self.shown_for != Some((shots_key, target, extended)) {
                self.shown_for = Some((shots_key, target, extended));
                if self.names_for != shots_key {
                    self.names_for = shots_key;
                    self.names.get_mut().clear();
                }
                let names = &self.names;
                // Bricks of the shots worked out this frame, live.
                let name = |brick: usize| {
                    let mut names = names.borrow_mut();
                    if let Some((_, name)) = names.iter().find(|(known, _)| *known == brick) {
                        return name.clone();
                    }
                    let name = code.names.of(&code.ui.texts, brick as *mut UObjectBase);
                    names.push((brick, name.clone()));
                    name
                };
                let more_key = EXTENDED_TOOLTIP.key_label();
                let context = Context {
                    units: Units {
                        // Shown distances: m (0.01 per cm) or ft (0.0328).
                        imperial: code.units.factor(code.distance) > 0.02,
                    },
                    extended,
                    vehicle,
                    name: &name,
                    more_key: &more_key,
                };
                let Some(text) = describe::describe(shots, target, &context) else {
                    return Ok(false);
                };
                self.tooltip.set(&code.ui, text.title, text.lines);
            }
            self.tooltip.show(&code.ui, controller, mouse)?;
        }
        Ok(true)
    }

    /// Hides the tooltip.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self) {
        self.failing = None;
        SHOWS.store(false, Ordering::Relaxed);
        if let Some(code) = code() {
            // SAFETY: per the contract.
            unsafe { self.tooltip.hide(&code.ui) };
        }
    }
}

/// The brick (address) under the mouse, as the editor's clicks find it, hidden ones left out.
///
/// # Safety
/// `editor` must be the live editor, `controller` its live player controller. Game thread only.
unsafe fn under_mouse(
    code: &Code,
    editor: *mut UObjectBase,
    controller: *mut UObjectBase,
    mouse: [f32; 2],
    hidden: &impl Fn(*mut UObjectBase) -> bool,
) -> Option<usize> {
    // SAFETY: per the contract; the editor is a live actor in the world to trace.
    unsafe {
        let ray = code.ui.viewport.ray(controller, mouse)?;
        let at = |along: f32| -> Vector {
            core::array::from_fn(|axis| ray.origin[axis] + ray.direction[axis] * along)
        };
        let mut along = 0.0;
        for _ in 0..MAX_HIDDEN {
            let hit = code.trace.first_hit(editor, at(along), at(REACH))?;
            let object = hit.editor_object().filter(|_| hit.is_brick())?;
            if !hidden(object) {
                return Some(object as usize);
            }
            // On past it (a trace starting inside it doesn't hit it again).
            along += hit.distance + PAST_HIDDEN;
        }
        None
    }
}
