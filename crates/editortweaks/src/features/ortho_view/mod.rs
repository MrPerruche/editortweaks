//! Orthographic view in the editor: [`keybinds::ORTHO_VIEW`] (Numpad 5) switches the editor's view
//! between perspective and orthographic (no perspective, as with a field of view of 0 from
//! infinitely far). The camera moves as before.
//!
//! - The switch keeps the vehicle the same size. Into the orthographic view, its width is the
//!   perspective view's at the depth of the brick nearest the middle of the view (`probe`; with no
//!   brick in view, of what's in its middle if nearer than [`NOTHING_DEPTH`], else that). Out of
//!   it, the camera moves to where the perspective view is as wide at the depth the view turns
//!   around (below). In between, the view narrows its field of view while backing away (a dolly
//!   zoom, [`SWITCH`] long, `blend`), or the other way back.
//! - Turning the camera turns the orthographic view around what's in its middle: the brick there
//!   when a turn starts, else the point it turned around last, which flying along the view doesn't
//!   move. Turning in place, as in perspective, would swing what's in front of the camera across
//!   the screen by its distance, which the view doesn't show. The editor's own pivot (middle button,
//!   Space) already turns around the selection and stays as it is.
//! - The zoom steps (the mouse wheel) zoom the orthographic view, by [`ZOOM_STEP`] each, gliding
//!   like the game's zoom; the perspective view keeps its own zoom for when it's back. Other
//!   features can take a zoom step first ([`listen_zoom_steps`]: the loader hooks a function only
//!   once).
//! - It shows what's in front of the camera, as the perspective view does: flying the camera into
//!   the vehicle cuts through it. How deep it shows follows its width ([`view_depth`]): its depth
//!   steps (32-bit, linear) are that depth times 2^-24, so a narrower view tells apart faces nearer
//!   together. The sky, a sphere around the view hundreds of km wide, shrinks to fit inside while
//!   the view is orthographic: no depth could reach it and still tell apart nearby bricks' faces.
//! - Temporal AA's jitter stops while the view is orthographic: it moves a perspective view by less
//!   than a pixel, but an orthographic one by its depth in cm times that, so with the game's TAA
//!   setting the view would seem to jump around every frame (`docs/re/editor-camera.md`,
//!   "Temporal AA"). Edges aren't smoothed by TAA meanwhile.
//! - The game's zoom notification says which view it is: "Perspective" and the zoom (0 to 100), or
//!   "Orthographic" and the view's width, at each zoom step and each switch (`Label`).
//! - Leaving the editor switches back to perspective at once: the game's view of the vehicle or
//!   player is perspective, and coming back the editor's view starts as the game makes it.
//!
//! In the orthographic view, [`keybinds::PAN_VIEW`] (Shift + right mouse button) held while the
//! camera is moved pans it: the mouse moves it across the view, so that what's in the view follows
//! the mouse, rather than turning it. In the perspective view it does nothing (a pan still under
//! way when the view switches to perspective finishes there, following what's as deep as the
//! vehicle, found as in a switch).
//!
//! How: the camera manager's `ApplyCameraModifiers` (`game::camera_manager`) is hooked: every frame,
//! after the view target made its view, the editor's view becomes what it should be (the camera
//! moved around what it turns around), the gizmo is sized for it (the game sizes it by the last
//! frame's view), and the sky is shrunk or put back (whatever the view target: it's back as soon as
//! the editor isn't viewed). The zoom steps are the editor input's reflected
//! `Action_ZoomInStep`/`Action_ZoomOutStep`, their thunks hooked. The overlay's
//! `OnCameraZoomRatioChanged` is hooked to label the game's zoom notification while the editor is
//! viewed (`game::zoom_notification`); the orthographic zoom steps and the switches show it
//! themselves. Panning takes the mouse's movement (`mouse_moves`) while the editor input's camera
//! flags say the camera is moved, and moves the camera by it at the next frame.

mod blend;
mod probe;

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use super::editor_frame;
use super::keybinds;
use super::mouse_moves::{self, Handling, Movement};
use crate::framework::{hook, timings};
use crate::game::camera_manager::{self, ApplyCameraModifiersFn, ViewInfo, ViewTargets};
use crate::game::console_variable::{self, ConsoleCommands, ConsoleVariables};
use crate::game::editor_camera::{self, EditorCamera, EditorCode};
use crate::game::editor_input::{self, CameraFlags, ZoomSteps};
use crate::game::editor_object::Vector;
use crate::game::line_trace::{self, LineTrace};
use crate::game::reflection::NativeThunk;
use crate::game::scene_component::{self, SceneComponent};
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;
use crate::game::world_sky::{self, WorldSky};
use crate::game::zoom_notification::{self, ZoomChangedFn, ZoomNotification};
use crate::math::{self, Quat, Vec3, add, dot, narrow, scale, sub, widen};
use crate::prelude::*;

/// How long a switch between the views lasts.
const SWITCH: Duration = Duration::from_millis(300);
/// By how much each zoom step narrows or widens the orthographic view.
const ZOOM_STEP: f32 = 1.25;
/// The narrowest and widest orthographic views, in cm.
const WIDTHS: [f32; 2] = [10.0, 200_000.0];
/// How deep the orthographic view shows things per cm of its width: its depth steps (see the
/// module's documentation) stay under half a pixel at 1920 pixels across.
const DEPTH_PER_WIDTH: f32 = 4_000.0;
/// The deepest the orthographic view shows things, and the farthest it looks for bricks, in cm.
const FAR: f32 = 1_000_000.0;
/// The share of the orthographic view's depth the sky reaches while it shows.
const SKY_SHARE: f32 = 0.9;
/// The depth a switch keeps the size at (cm) with no brick in view and nothing nearer in the middle.
const NOTHING_DEPTH: f32 = 1_000.0;
/// The nearest depth taken as something in view (cm): nearer, the trace started inside it.
const NEAREST: f32 = 1.0;
/// How long the camera must stay still for its next turn to look anew at what's in the middle.
const TURN_PAUSE: Duration = Duration::from_millis(250);
/// The screen's height over its width, and its width in pixels, when the viewport's size isn't
/// known.
const ASPECT: f32 = 9.0 / 16.0;
const PIXELS_ACROSS: f32 = 1920.0;
/// How fast the orthographic zoom glides if the game's speed isn't found (see `blend::glide`).
const GLIDE_SPEED: f32 = 10.0;
/// How many places temporal AA's jitter goes through: 0 stops it. Nothing but the renderer uses it,
/// so setting it from the console keeps the game's options working.
const TEMPORAL_SAMPLES: &str = "r.TemporalAASamples";

const _: () = assert!(WIDTHS[0] > 0.0 && WIDTHS[1] >= WIDTHS[0] && FAR > SKY_SHARE * FAR);

pub struct OrthoView;

static ORIGINAL: OnceLock<ApplyCameraModifiersFn> = OnceLock::new();
static ORIGINAL_ZOOM_IN: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_ZOOM_OUT: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_ZOOM_CHANGED: OnceLock<ZoomChangedFn> = OnceLock::new();
static ZOOM_LISTENERS: Mutex<Vec<ZoomListener>> = Mutex::new(Vec::new());
/// What the editor's code tells, found at load.
static CODE: OnceLock<EditorCode> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) the view stays the game's.
static PARTS: OnceLock<Parts> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State {
    mode: Mode::Perspective,
    key_held: false,
    width: WIDTHS[0],
    target_width: WIDTHS[0],
    pivot: NOTHING_DEPTH,
    last: None,
    sky: None,
    looked_for_sky: false,
    editor_viewed: false,
    tooltip: None,
    pan: [0.0; 2],
    pan_depth: None,
    temporal_samples: None,
});

struct Parts {
    view: ViewInfo,
    targets: ViewTargets,
    editor: EditorCamera,
    scene: SceneComponent,
    viewport: Viewport,
    /// What's in the view; without it, switches take [`NOTHING_DEPTH`].
    trace: Option<LineTrace>,
    /// Whether the editor pivots its camera itself; without it, turning stays the game's.
    turning: Option<CameraFlags>,
    /// The sky; without it, it stays as it is.
    sky: Option<WorldSky>,
    /// The zoom steps; without them, they stay the game's.
    zoom: Option<ZoomSteps>,
    /// The game's zoom notification; without it, it stays as it is.
    tooltip: Option<ZoomNotification>,
    /// Temporal AA's jitter; without it, it stays on.
    jitter: Option<Jitter>,
}

/// What stops temporal AA's jitter.
struct Jitter {
    variables: ConsoleVariables,
    commands: ConsoleCommands,
}

#[derive(Clone, Copy)]
enum Mode {
    Perspective,
    /// Switching to orthographic since `since`, what lies `depth` cm in front of the camera
    /// keeping its size.
    Entering {
        since: Instant,
        depth: f32,
    },
    Orthographic,
    /// Switching back to perspective, as [`Mode::Entering`] the other way, its width going from
    /// `from` (the orthographic view's) to the perspective view's at `depth` (the same, unless the
    /// editor's bounds stopped the camera short of where they are).
    Leaving {
        since: Instant,
        depth: f32,
        from: f32,
    },
}

struct State {
    mode: Mode,
    /// Whether the key was held at the last frame.
    key_held: bool,
    /// How wide the orthographic view is (cm), and how wide the zoom steps make it.
    width: f32,
    target_width: f32,
    /// How far in front of the camera what the orthographic view turns around is (cm; negative:
    /// behind it), along the view.
    pivot: f32,
    /// The camera at the last orthographic frame.
    last: Option<LastCamera>,
    /// The sky, while shrunk.
    sky: Option<ShrunkSky>,
    /// Whether the sky was looked for since the view became orthographic (it takes walking every
    /// object).
    looked_for_sky: bool,
    /// Whether the editor was viewed at the last frame.
    editor_viewed: bool,
    /// What to show in the zoom notification once the state is free (it calls into the game).
    tooltip: Option<Label>,
    /// How far the mouse panned the view since the last frame (pixels: right, down).
    pan: [f32; 2],
    /// While panning the perspective view: how deep what follows the mouse is (cm).
    pan_depth: Option<f32>,
    /// While temporal AA's jitter is stopped: [`TEMPORAL_SAMPLES`] before (0: already stopped).
    temporal_samples: Option<i32>,
}

#[derive(Clone, Copy)]
struct LastCamera {
    location: Vec3,
    rotation: Vector,
    /// When it last turned.
    turned: Option<Instant>,
}

struct ShrunkSky {
    component: WeakObject,
    /// Its scale before.
    scale: Vector,
    /// How far its mesh reaches, unscaled (cm).
    radius: f32,
}

// SAFETY: the sky's pointer is only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for State {}

impl Feature for OrthoView {
    fn id(&self) -> &'static str {
        "ortho_view"
    }

    fn on_load(&self) -> LoadResult {
        let _ = CODE.set(editor_camera::code()?);
        let target = camera_manager::APPLY_CAMERA_MODIFIERS.resolve()?;
        // SAFETY: `target` is the start of ABrickCameraManager::ApplyCameraModifiers, and the
        // detour has its signature.
        unsafe {
            hook::install_typed(
                camera_manager::APPLY_CAMERA_MODIFIERS.name,
                target,
                apply_camera_modifiers as ApplyCameraModifiersFn,
                &ORIGINAL,
            )
        }
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!(
                "active: {} cm to {} cm wide, from the camera to {DEPTH_PER_WIDTH} times as deep",
                WIDTHS[0],
                WIDTHS[1]
            ),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Whether a listener takes a zoom step (in, the mouse wheel up, if `true`): then the game's handler,
/// the orthographic zoom and the listeners after it don't get it.
pub type ZoomListener = fn(bool) -> bool;

/// Calls `listener` with every zoom step of the editor from now on, before anything else and after
/// the listeners before it; never if the zoom steps couldn't be hooked (logged).
pub fn listen_zoom_steps(listener: ZoomListener) {
    ZOOM_LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(listener);
}

/// Finds the parts and hooks the zoom steps and the zoom notification.
fn engine_init() -> Result<(), String> {
    const NO_TOOLTIP: &str = "the zoom notification stays the game's";
    let code = *CODE.get().ok_or("the editor's code wasn't found")?;
    let zoom_code = optional(zoom_notification::code(), NO_TOOLTIP);
    let parts = Parts {
        view: camera_manager::view_info()?,
        targets: camera_manager::view_targets()?,
        editor: editor_camera::find(code)?,
        scene: scene_component::find()?,
        viewport: viewport::find()?,
        trace: optional(line_trace::find(), "switches don't keep sizes"),
        turning: optional(editor_input::camera_flags(), "turning stays the game's"),
        sky: optional(world_sky::find(), "the sky stays as it is"),
        zoom: optional(editor_input::zoom_steps(), "zoom steps stay the game's"),
        tooltip: zoom_code.and_then(|code| optional(zoom_notification::find(code), NO_TOOLTIP)),
        jitter: optional(
            console_variable::find().and_then(|variables| {
                Ok(Jitter {
                    variables,
                    commands: console_variable::commands()?,
                })
            }),
            "with the game's TAA setting the orthographic view jumps around",
        ),
    };
    let tooltip = zoom_code.filter(|_| parts.tooltip.is_some());
    let zoom = parts
        .zoom
        .as_ref()
        .map(|zoom| (zoom.zoom_in.thunk, zoom.zoom_out.thunk));
    let pans = parts.turning.is_some();
    let _ = PARTS.set(parts);
    if pans {
        optional(mouse_moves::listen(take_pan), "no panning");
    } else {
        log!("no panning: the camera flags weren't found");
    }
    if let Some((zoom_in, zoom_out)) = zoom {
        // SAFETY: both targets are exec thunks, and the detours are NativeThunks.
        let hooked = unsafe {
            hook::install_typed(
                "UBaseInputComponent::execAction_ZoomInStep",
                zoom_in,
                zoom_in_detour as NativeThunk,
                &ORIGINAL_ZOOM_IN,
            )
            .and_then(|()| {
                hook::install_typed(
                    "UBaseInputComponent::execAction_ZoomOutStep",
                    zoom_out,
                    zoom_out_detour as NativeThunk,
                    &ORIGINAL_ZOOM_OUT,
                )
            })
        };
        optional(hooked, "zoom steps stay the game's");
    }
    if let Some(code) = tooltip {
        // SAFETY: the target is OnCameraZoomRatioChanged's start, and the detour has its
        // signature.
        let hooked = unsafe {
            hook::install_typed(
                zoom_notification::ZOOM_CHANGED.name,
                code.function,
                zoom_changed as ZoomChangedFn,
                &ORIGINAL_ZOOM_CHANGED,
            )
        };
        optional(
            hooked,
            "the perspective view's zoom notification stays the game's",
        );
    }
    Ok(())
}

/// `found`, or `None` after logging why it's missing and what that means.
fn optional<T>(found: Result<T, String>, without: &str) -> Option<T> {
    match found {
        Ok(found) => Some(found),
        Err(reason) => {
            log!("{without}: {reason}");
            None
        }
    }
}

unsafe extern "C" fn apply_camera_modifiers(manager: *mut UObjectBase, delta: f32, view: *mut u8) {
    let _span = timings::span("ortho_view::apply_camera_modifiers");
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the game's own arguments: the live camera manager and the view it's making.
    unsafe {
        timings::game(camera_manager::APPLY_CAMERA_MODIFIERS.name, || {
            original(manager, delta, view)
        });
        if let Some(parts) = PARTS.get() {
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            parts.follow(&mut state, manager, delta, view);
            let label = state.tooltip.take();
            drop(state);
            if let Some(label) = label {
                parts.show_tooltip(label);
            }
        }
    }
}

unsafe extern "C" fn zoom_changed(overlay: *mut UObjectBase, ratio: f32) {
    let _span = timings::span("ortho_view::zoom_changed");
    let original = *ORIGINAL_ZOOM_CHANGED
        .get()
        .expect("hook recorded before use");
    let labelled = PARTS
        .get()
        .and_then(|parts| Some((parts.tooltip.as_ref()?, editor_label(ratio)?)));
    // SAFETY: the game's own arguments: the live overlay and the view target's new zoom ratio.
    unsafe {
        match labelled {
            Some((tooltip, label)) => tooltip.show(overlay, ratio, &label.text(tooltip)),
            None => timings::game(zoom_notification::ZOOM_CHANGED.name, || {
                original(overlay, ratio)
            }),
        }
    }
}

/// How the zoom notification labels the view target's zoom ratio `ratio` if the editor is viewed
/// (at the last frame), else `None` (the game's own).
fn editor_label(ratio: f32) -> Option<Label> {
    // Never waits: the game doesn't show it from within `follow`, and if it did, its own is fine.
    let state = STATE.try_lock().ok()?;
    state.editor_viewed.then(|| match state.mode {
        Mode::Perspective | Mode::Leaving { .. } => Label::Perspective(ratio),
        Mode::Orthographic | Mode::Entering { .. } => Label::Orthographic(state.target_width),
    })
}

/// Takes the mouse's movement for a pan while [`keybinds::PAN_VIEW`] is held and the editor's camera
/// is moved (its button held), for the next frame to move the camera by it.
fn take_pan(movement: &Movement) -> Handling {
    let Some(flags) = PARTS.get().and_then(|parts| parts.turning.as_ref()) else {
        return Handling::Pass;
    };
    // SAFETY: the movement's input is the live editor input component handling it.
    if !keybinds::PAN_VIEW.is_held() || unsafe { !flags.moving(movement.input) } {
        return Handling::Pass;
    }
    let mut state = match STATE.try_lock() {
        Ok(state) => state,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        // Only the game thread uses the state, and not around input handlers.
        Err(TryLockError::WouldBlock) => return Handling::Pass,
    };
    if !state.editor_viewed || !matches!(state.mode, Mode::Orthographic | Mode::Entering { .. }) {
        return Handling::Pass;
    }
    let [right, down] = movement.pixels();
    state.pan[0] += right;
    state.pan[1] += down;
    Handling::Take
}

unsafe extern "C" fn zoom_in_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    let _span = timings::span("ortho_view::zoom_in_detour");
    let original = *ORIGINAL_ZOOM_IN.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { zoom_step(context, stack, result, original, true) }
}

unsafe extern "C" fn zoom_out_detour(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let _span = timings::span("ortho_view::zoom_out_detour");
    let original = *ORIGINAL_ZOOM_OUT.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { zoom_step(context, stack, result, original, false) }
}

/// Runs `original`, a zoom step's thunk (in or out) called with `context`, `stack` and `result`,
/// unless a listener takes it ([`listen_zoom_steps`]) or the editor's view is orthographic: then
/// it narrows or widens it by [`ZOOM_STEP`] instead.
///
/// # Safety
/// `context`, `stack` and `result` must be the arguments `original` was called with, on the game
/// thread.
unsafe fn zoom_step(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
    original: NativeThunk,
    zoom_in: bool,
) {
    // SAFETY: per the contract.
    unsafe {
        if let Some(parts) = PARTS.get()
            && let Some(zoom) = &parts.zoom
            && zoom.is_editor_input(context)
            && zoom.is_real(
                if zoom_in {
                    &zoom.zoom_in
                } else {
                    &zoom.zoom_out
                },
                stack,
            )
        {
            // Copied: a listener may listen too.
            let listeners = ZOOM_LISTENERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if listeners.iter().any(|listener| listener(zoom_in)) {
                zoom.handled(result);
                return;
            }
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(state.mode, Mode::Orthographic | Mode::Entering { .. }) {
                let factor = if zoom_in { 1.0 / ZOOM_STEP } else { ZOOM_STEP };
                state.target_width = (state.target_width * factor).clamp(WIDTHS[0], WIDTHS[1]);
                let label = Label::Orthographic(state.target_width);
                drop(state);
                zoom.handled(result);
                parts.show_tooltip(label);
                return;
            }
        }
        let game_name = if zoom_in {
            "UBaseInputComponent::execAction_ZoomInStep"
        } else {
            "UBaseInputComponent::execAction_ZoomOutStep"
        };
        timings::game(game_name, || original(context, stack, result));
    }
}

/// The editor's view this frame, as the game made it.
struct Frame {
    editor: *mut UObjectBase,
    view: *mut u8,
    location: Vec3,
    rotation: Vector,
    /// Which ways the view looks: forward, right and up (length 1).
    axes: [Vec3; 3],
    /// The perspective view's horizontal field of view (degrees).
    fov: f32,
    /// The screen's width and height in pixels, if known.
    screen: Option<[f32; 2]>,
}

impl Frame {
    /// The screen's height over its width, if known.
    fn aspect(&self) -> Option<f32> {
        self.screen.map(|[width, height]| height / width)
    }
}

impl Parts {
    /// Makes the editor's view what it should be this frame, and puts the sky back elsewhere.
    ///
    /// # Safety
    /// `manager` must be the live camera manager that just applied its effects to `view`, after
    /// `delta` seconds. Game thread only.
    unsafe fn follow(
        &self,
        state: &mut State,
        manager: *mut UObjectBase,
        delta: f32,
        view: *mut u8,
    ) {
        // SAFETY: per the contract.
        unsafe {
            let editor = self
                .targets
                .of(manager, view)
                .filter(|&target| self.editor.is_editor(target));
            let Some(editor) = editor else {
                state.editor_viewed = false;
                state.last = None;
                state.pan = [0.0; 2];
                state.pan_depth = None;
                if !matches!(state.mode, Mode::Perspective) {
                    // The editor was left (its view blends into the game's, or is gone).
                    state.mode = Mode::Perspective;
                    log!("perspective view: the editor isn't viewed");
                }
                self.put_sky_back(state);
                self.start_jitter(state, manager);
                return;
            };
            state.editor_viewed = true;
            let rotation = self.view.rotation(view);
            let mut frame = Frame {
                editor,
                view,
                location: widen(self.view.location(view)),
                rotation,
                axes: axes(rotation),
                fov: self.view.fov(view),
                screen: self
                    .editor
                    .player_controller(editor)
                    .and_then(|controller| self.viewport.size(controller))
                    .map(|[width, height]| [width as f32, height as f32]),
            };
            let held = keybinds::ORTHO_VIEW.is_held();
            if held && !state.key_held {
                self.switch(state, &mut frame);
            }
            state.key_held = held;

            let pan = core::mem::take(&mut state.pan);
            if pan != [0.0; 2] {
                self.pan(state, &mut frame, pan);
            }
            if !keybinds::PAN_VIEW.is_held() {
                state.pan_depth = None;
            }

            let speed = self.editor.zoom_glide_speed(editor).unwrap_or(GLIDE_SPEED);
            state.width = blend::glide(state.width, state.target_width, speed, delta);
            match state.mode {
                Mode::Perspective => self.put_sky_back(state),
                Mode::Entering { since, depth } => {
                    let progress = progress(since);
                    if progress < 1.0 {
                        self.show_dolly(&frame, state.width, depth, progress);
                        self.put_sky_back(state);
                    } else {
                        state.mode = Mode::Orthographic;
                        self.show_orthographic(state, &frame);
                    }
                }
                Mode::Orthographic => {
                    self.turn(state, &mut frame);
                    self.show_orthographic(state, &frame);
                }
                Mode::Leaving { since, depth, from } => {
                    let left = 1.0 - progress(since);
                    self.show_dolly(
                        &frame,
                        leaving_width(&frame, depth, from, left),
                        depth,
                        left,
                    );
                    self.put_sky_back(state);
                    if left <= 0.0 {
                        state.mode = Mode::Perspective;
                    }
                }
            }
            if matches!(state.mode, Mode::Orthographic) {
                self.stop_jitter(state, frame.editor);
            } else {
                state.last = None;
                self.start_jitter(state, frame.editor);
            }
        }
    }

    /// Stops temporal AA's jitter, if it isn't yet (see the module's documentation).
    ///
    /// # Safety
    /// `world` must be a live object in the view's world. Game thread only.
    unsafe fn stop_jitter(&self, state: &mut State, world: *mut UObjectBase) {
        let Some(jitter) = &self.jitter else { return };
        if state.temporal_samples.is_some() {
            return;
        }
        // SAFETY: per the contract.
        unsafe {
            let samples = jitter.variables.float(TEMPORAL_SAMPLES).unwrap_or(0.0) as i32;
            state.temporal_samples = Some(samples);
            if samples > 0 {
                jitter.commands.run(world, &format!("{TEMPORAL_SAMPLES} 0"));
            }
            // What says whether temporal AA is on at all, and temporal upsampling, which jitters
            // whatever the samples (`docs/re/editor-camera.md`, "Temporal AA"). Absent: 0.
            let variable = |name| jitter.variables.float(name);
            log!(
                "temporal AA's jitter stopped ({samples} samples before; AA method {:?}, \
                 upsampling {:?}, screen percentage {:?})",
                variable("r.DefaultFeature.AntiAliasing"),
                variable("r.TemporalAA.Upsampling"),
                variable("r.ScreenPercentage"),
            );
        }
    }

    /// Lets temporal AA's jitter go on as before, if it was stopped.
    ///
    /// # Safety
    /// As for [`Self::stop_jitter`].
    unsafe fn start_jitter(&self, state: &mut State, world: *mut UObjectBase) {
        let (Some(jitter), Some(samples)) = (&self.jitter, state.temporal_samples.take()) else {
            return;
        };
        if samples > 0 {
            // SAFETY: per the contract.
            unsafe {
                jitter
                    .commands
                    .run(world, &format!("{TEMPORAL_SAMPLES} {samples}"));
            }
        }
    }

    /// Starts switching to the other view (from where a switch on the way got to).
    ///
    /// # Safety
    /// As for [`Self::follow`], `frame` this frame's.
    unsafe fn switch(&self, state: &mut State, frame: &mut Frame) {
        let now = Instant::now();
        // SAFETY: per the contract.
        unsafe {
            match state.mode {
                Mode::Perspective => self.enter(state, frame, now),
                Mode::Leaving { since, depth, from } => {
                    let width = leaving_width(frame, depth, from, 1.0 - progress(since));
                    state.width = width;
                    state.target_width = width;
                    state.pivot = depth;
                    state.mode = Mode::Entering {
                        since: reversed(since, now),
                        depth,
                    };
                    state.tooltip = Some(Label::Orthographic(width));
                }
                Mode::Orthographic => {
                    // What's in the middle now, if a brick.
                    let pivot = self
                        .middle_brick(frame, frame.axes[0])
                        .unwrap_or(state.pivot);
                    self.leave(state, frame, now, pivot);
                }
                Mode::Entering { since, .. } => {
                    self.leave(state, frame, reversed(since, now), state.pivot)
                }
            }
        }
    }

    /// Starts switching to the orthographic view, keeping the vehicle's size.
    ///
    /// # Safety
    /// As for [`Self::switch`].
    unsafe fn enter(&self, state: &mut State, frame: &Frame, now: Instant) {
        // SAFETY: per the contract.
        let (depth, what) = unsafe { self.vehicle_depth(frame) };
        let width = blend::width_at(frame.fov, depth).clamp(WIDTHS[0], WIDTHS[1]);
        // Where the perspective view is that wide, should it be clamped.
        let depth = blend::depth_for(frame.fov, width);
        state.width = width;
        state.target_width = width;
        state.pivot = depth;
        state.mode = Mode::Entering { since: now, depth };
        state.tooltip = Some(Label::Orthographic(width));
        log!("orthographic view: {width:.0} cm wide, as {depth:.0} cm away ({what})");
    }

    /// Starts switching back to the perspective view, since `since`: the camera moves to where the
    /// perspective view is as wide as the orthographic one at `pivot` cm in front of it.
    ///
    /// # Safety
    /// As for [`Self::switch`].
    unsafe fn leave(&self, state: &mut State, frame: &mut Frame, since: Instant, pivot: f32) {
        // Where the zoom steps took it, so that the switch ends as wide.
        state.width = state.target_width;
        let width = state.width;
        let wanted = blend::depth_for(frame.fov, width);
        // SAFETY: per the contract.
        let (moved, ratio) = unsafe {
            (
                self.move_along(frame, pivot - wanted),
                self.editor.zoom_ratio(frame.editor),
            )
        };
        let depth = pivot - moved;
        state.tooltip = ratio.map(Label::Perspective);
        state.mode = if depth >= NEAREST {
            Mode::Leaving {
                since,
                depth,
                from: width,
            }
        } else {
            // Nothing to keep the size of in front of the camera.
            Mode::Perspective
        };
        state.looked_for_sky = false;
        log!("perspective view: the camera {depth:.0} cm away, for {width:.0} cm wide");
    }

    /// How far in front of the camera the vehicle is, for a switch from the perspective view: the
    /// brick nearest the middle of the view, else what's in the middle if nearer than
    /// [`NOTHING_DEPTH`], else that; and which it is.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn vehicle_depth(&self, frame: &Frame) -> (f32, &'static str) {
        let Some(trace) = &self.trace else {
            return (NOTHING_DEPTH, "no trace");
        };
        let [forward, right, up] = frame.axes;
        let aspect = frame.aspect().unwrap_or(ASPECT);
        let half_width = f64::from((frame.fov.to_radians() / 2.0).tan());
        let half_height = half_width * f64::from(aspect);
        let mut middle = None;
        for (index, [x, y]) in probe::screen_points(aspect).into_iter().enumerate() {
            let aside = add(
                scale(right, f64::from(x) * half_width),
                scale(up, f64::from(y) * half_height),
            );
            let direction = math::normalized(add(forward, aside)).unwrap_or(forward);
            let end = add(frame.location, scale(direction, f64::from(FAR)));
            // SAFETY: per the contract; the editor is a live actor in the world.
            let hit = unsafe { trace.first_hit(frame.editor, narrow(frame.location), narrow(end)) };
            let Some(hit) = hit else { continue };
            let depth = hit.distance * dot(direction, forward) as f32;
            if depth < NEAREST {
                continue;
            }
            if hit.is_brick() {
                return (depth, "a brick");
            }
            // The first point is the middle.
            if index == 0 && depth < NOTHING_DEPTH {
                middle = Some(depth);
            }
        }
        match middle {
            Some(depth) => (depth, "what's in the middle"),
            None => (NOTHING_DEPTH, "no brick in view"),
        }
    }

    /// How far in front of the camera (cm) the brick in the middle of the orthographic view is,
    /// looking along `forward`.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn middle_brick(&self, frame: &Frame, forward: Vec3) -> Option<f32> {
        let trace = self.trace.as_ref()?;
        let end = add(frame.location, scale(forward, f64::from(FAR)));
        // SAFETY: per the contract; the editor is a live actor in the world.
        let hit = unsafe { trace.first_hit(frame.editor, narrow(frame.location), narrow(end)) }?;
        hit.is_brick().then_some(hit.distance)
    }

    /// Turns the orthographic view around what's in its middle as the camera turns (see the
    /// module's documentation), unless the editor pivots it itself.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn turn(&self, state: &mut State, frame: &mut Frame) {
        let now = Instant::now();
        let mut turned = None;
        if let Some(last) = state.last {
            let turning = frame.rotation != last.rotation;
            turned = if turning { Some(now) } else { last.turned };
            // SAFETY: per the contract.
            if unsafe { !self.editor_pivots(frame.editor) } {
                let last_forward = axes(last.rotation)[0];
                // Flying along the view: what it turns around stays where it is.
                state.pivot -= dot(sub(frame.location, last.location), last_forward) as f32;
                if turning {
                    let paused = last.turned.is_none_or(|at| now - at >= TURN_PAUSE);
                    // SAFETY: per the contract.
                    let middle = paused
                        .then(|| unsafe { self.middle_brick(frame, last_forward) })
                        .flatten();
                    if let Some(depth) = middle {
                        state.pivot = depth;
                    }
                    // Around the point `pivot` along the last view, which stays in the middle (in
                    // place once flown past it: the view doesn't show what's behind the camera).
                    let radius = state.pivot.max(0.0);
                    let offset = scale(sub(last_forward, frame.axes[0]), f64::from(radius));
                    // SAFETY: per the contract.
                    unsafe { self.move_camera(frame, offset) };
                }
            }
        }
        state.last = Some(LastCamera {
            location: frame.location,
            rotation: frame.rotation,
            turned,
        });
    }

    /// Moves the camera across the view by the mouse's movement in a pan, `right` and `down`
    /// pixels on the screen, so that what's in the view follows the mouse: everything in the
    /// orthographic view; in the perspective one, what's as deep as the vehicle when the pan
    /// started.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn pan(&self, state: &mut State, frame: &mut Frame, [right, down]: [f32; 2]) {
        let across = match state.mode {
            Mode::Orthographic | Mode::Entering { .. } => state.width,
            Mode::Perspective | Mode::Leaving { .. } => {
                let depth = match state.pan_depth {
                    Some(depth) => depth,
                    None => {
                        // SAFETY: per the contract.
                        let (depth, _) = unsafe { self.vehicle_depth(frame) };
                        state.pan_depth = Some(depth);
                        depth
                    }
                };
                blend::width_at(frame.fov, depth)
            }
        };
        let per_pixel = across / frame.screen.map_or(PIXELS_ACROSS, |[width, _]| width);
        let [_, right_axis, up_axis] = frame.axes;
        // The camera moves the other way: dragging right brings the view's left side in.
        let offset = add(
            scale(right_axis, f64::from(-right * per_pixel)),
            scale(up_axis, f64::from(down * per_pixel)),
        );
        // SAFETY: per the contract.
        unsafe { self.move_camera(frame, offset) };
    }

    /// Whether the editor turns its camera around the selection itself (or whether it does can't
    /// be told).
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn editor_pivots(&self, editor: *mut UObjectBase) -> bool {
        // SAFETY: per the contract; the editor's input component is a live one.
        unsafe {
            match (&self.turning, self.editor.input(editor)) {
                (Some(flags), Some(input)) => flags.pivoting(input),
                _ => true,
            }
        }
    }

    /// Moves the editor's camera `distance` cm along its view (as far as the game lets it), and
    /// this frame's view with it: returns how far it moved along.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn move_along(&self, frame: &mut Frame, distance: f32) -> f32 {
        let forward = frame.axes[0];
        // SAFETY: per the contract.
        let moved = unsafe { self.move_camera(frame, scale(forward, f64::from(distance))) };
        dot(moved, forward) as f32
    }

    /// Moves the editor's camera by `offset` cm (as far as the game lets it), and this frame's view
    /// with it: returns how far it moved.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn move_camera(&self, frame: &mut Frame, offset: Vec3) -> Vec3 {
        // SAFETY: per the contract.
        unsafe {
            // The game keeps the camera's location in the editor's space (which is the world's
            // unless the editor is turned, see `editor_frame`).
            let wanted = editor_frame::to_editor_space(offset);
            let moved = editor_frame::to_world_space(widen(
                self.editor.move_camera(frame.editor, narrow(wanted)),
            ));
            frame.location = add(frame.location, moved);
            self.view.set_location(frame.view, narrow(frame.location));
            moved
        }
    }

    /// Shows a perspective view on the way of a switch (see [`blend::dolly`]).
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn show_dolly(&self, frame: &Frame, width: f32, depth: f32, progress: f32) {
        let (fov, back) = blend::dolly(frame.fov, width, depth, progress);
        let location = add(frame.location, scale(frame.axes[0], -f64::from(back)));
        // SAFETY: per the contract.
        unsafe {
            self.view.set_perspective(frame.view, narrow(location), fov);
            if let Some(aspect) = frame.aspect() {
                // As the game does: the view's half height at the gizmo's distance.
                let half_tan = (fov.to_radians() / 2.0).tan() * aspect;
                self.size_gizmo(frame.editor, |gizmo| {
                    math::length(sub(widen(gizmo), location)) as f32 * half_tan
                });
            }
        }
    }

    /// Shows the orthographic view.
    ///
    /// # Safety
    /// As for [`Self::follow`].
    unsafe fn show_orthographic(&self, state: &mut State, frame: &Frame) {
        let depth = view_depth(state.width, state.pivot);
        // SAFETY: per the contract.
        unsafe {
            self.view
                .set_orthographic(frame.view, state.width, [0.0, depth]);
            if let Some(aspect) = frame.aspect() {
                let half_height = state.width / 2.0 * aspect;
                self.size_gizmo(frame.editor, |_| half_height);
            }
            self.shrink_sky(state, depth);
        }
    }

    /// Shows `label` in the game's zoom notification, if found and the HUD shows.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn show_tooltip(&self, label: Label) {
        let Some(tooltip) = &self.tooltip else {
            return;
        };
        // SAFETY: per the contract; the overlay found is live.
        unsafe {
            if let Some(overlay) = tooltip.overlay() {
                tooltip.show(overlay, label.ratio(), &label.text(tooltip));
            }
        }
    }

    /// Sizes the editor's gizmo for a view `half_height(its place)` cm half high there, as the game
    /// does for its own view (see `editor_camera::CALC_CAMERA`).
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn size_gizmo(&self, editor: *mut UObjectBase, half_height: impl FnOnce(Vector) -> f32) {
        // SAFETY: per the contract.
        unsafe {
            let (Some(gizmo), Some(factor)) =
                (self.editor.gizmo(editor), self.editor.gizmo_factor(editor))
            else {
                return;
            };
            let scale = factor * half_height(self.scene.location(gizmo));
            if scale.is_finite() && scale > 0.0 {
                self.scene.set_scale(gizmo, [scale; 3]);
            }
        }
    }

    /// Shrinks the sky to fit inside the orthographic view, `depth` cm deep.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn shrink_sky(&self, state: &mut State, depth: f32) {
        // SAFETY: on the game thread; components come from the world's live objects.
        unsafe {
            if let Some(shrunk) = &state.sky
                && shrunk.component.get().is_none()
            {
                // Gone with its world: look for the new one.
                state.sky = None;
                state.looked_for_sky = false;
            }
            if state.sky.is_none() && !state.looked_for_sky {
                state.looked_for_sky = true;
                state.sky = self.find_sky();
            }
            let Some(shrunk) = &state.sky else { return };
            let Some(component) = shrunk.component.get() else {
                return;
            };
            // The view's depth follows its width, and the game may have set the scale again.
            let wanted = [SKY_SHARE * depth / shrunk.radius; 3];
            if self.scene.scale(component) != wanted {
                self.scene.set_scale(component, wanted);
            }
        }
    }

    /// The sky, as it is before it's shrunk (it takes walking every object).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn find_sky(&self) -> Option<ShrunkSky> {
        let sky = self.sky.as_ref()?;
        // SAFETY: on the game thread; the component comes from the world's live objects.
        unsafe {
            let Some(component) = sky.find_sky() else {
                log!("no sky found");
                return None;
            };
            let Some(radius) = sky.mesh_radius(component).filter(|&radius| radius > 0.0) else {
                log!("the sky's size is unknown");
                return None;
            };
            let scale = self.scene.scale(component);
            log!(
                "the sky: a mesh reaching {radius:.0} cm scaled by {:.0} ({:.0} km), shrunk to fit",
                scale[0],
                radius * scale[0] / 100_000.0
            );
            Some(ShrunkSky {
                component: WeakObject::new(component),
                scale,
                radius,
            })
        }
    }

    /// Puts the sky back as it was, if shrunk.
    fn put_sky_back(&self, state: &mut State) {
        if let Some(shrunk) = state.sky.take()
            && let Some(component) = shrunk.component.get()
        {
            // SAFETY: a live sky component; on the game thread.
            unsafe { self.scene.set_scale(component, shrunk.scale) };
        }
        if matches!(state.mode, Mode::Perspective) {
            state.looked_for_sky = false;
        }
    }
}

/// How deep the orthographic view shows things in front of the camera (cm), `width` cm wide and
/// turning around what's `pivot` cm in front of it: [`DEPTH_PER_WIDTH`] times its width, at least
/// twice as far as what it turns around, at most [`FAR`].
fn view_depth(width: f32, pivot: f32) -> f32 {
    (width * DEPTH_PER_WIDTH).max(2.0 * pivot).min(FAR)
}

/// What the game's zoom notification says about the editor's view.
#[derive(Clone, Copy)]
enum Label {
    /// The perspective view, at this zoom ratio (0 to 1).
    Perspective(f32),
    /// The orthographic view, this wide (cm).
    Orthographic(f32),
}

impl Label {
    /// The zoom ratio to give the game's notification, whose text [`Self::text`] then replaces.
    fn ratio(self) -> f32 {
        match self {
            Label::Perspective(ratio) => ratio,
            Label::Orthographic(_) => 0.0,
        }
    }

    /// E.g. "Perspective 20" or "Orthographic 2.4 m", numbers as the game writes them.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn text(self, notification: &ZoomNotification) -> String {
        // SAFETY: per the contract.
        unsafe {
            match self {
                Label::Perspective(ratio) => {
                    format!("Perspective {}", notification.ratio_text(ratio))
                }
                Label::Orthographic(width) => format!(
                    "Orthographic {}",
                    notification.distance_text(width, decimals(width))
                ),
            }
        }
    }
}

/// How many decimals show a width of `cm` with 2 or 3 digits, in m or ft.
fn decimals(cm: f32) -> i32 {
    if cm < 100.0 {
        2
    } else if cm < 1_000.0 {
        1
    } else {
        0
    }
}

/// How far a switch started at `since` is, 0 to 1 (see [`blend::progress`]).
fn progress(since: Instant) -> f32 {
    blend::progress(since.elapsed().as_secs_f32(), SWITCH.as_secs_f32())
}

/// When a switch the other way, reversing at `now` the one started at `since`, would have started
/// to be as far along.
fn reversed(since: Instant, now: Instant) -> Instant {
    let left = SWITCH.saturating_sub(now.saturating_duration_since(since));
    now.checked_sub(left).unwrap_or(now)
}

/// How wide a switch back to perspective keeps what's `depth` cm in front of the camera, `left` of
/// the way still to go (1 at its start: see [`Mode::Leaving`]).
fn leaving_width(frame: &Frame, depth: f32, from: f32, left: f32) -> f32 {
    blend::towards(blend::width_at(frame.fov, depth), from, left)
}

/// Which ways a view turned by `[pitch, yaw, roll]` degrees looks: forward, right and up.
fn axes(rotation: Vector) -> [Vec3; 3] {
    let turn = Quat::from_rotator(widen(rotation));
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| turn.rotate(axis))
}
