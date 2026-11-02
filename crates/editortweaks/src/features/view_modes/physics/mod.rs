//! The game's Physics view mode, with more: next to its center of mass, the centers of lift in pitch
//! (blue) and in yaw (green) and the center of thrust (magenta), drawn like it (its mesh and material
//! in other colors, over the bricks), lift and drag ([`lift_drag`]: arrows, the center of drag in
//! red, and a handle setting where the air comes from), and the view modes' tooltip over all of
//! them, telling which it is and where, and for lift whether it's behind the center of mass (stable
//! on that axis).
//!
//! Like the game's center of mass (`ABrickEditor::UpdatePhysicsProperties`, see
//! `docs/re/physics-centers.md`), they're the selection's if anything is selected, else the whole
//! vehicle's, in the editor root's space (the bricks' places), and show only while it shows: in the
//! Physics view mode, not while moving bricks. They're worked out again when the editor changes
//! ([`super::generation`]) or the selection does ([`centers`] for how), lift and drag also when the
//! airflow turns.
//!
//! - Lift: each brick generating lift, its surface as the game's fluid dynamics get it
//!   (`game::lift_surface`), for the vehicle flying forward (its X axis) with its nose turning up
//!   and down (pitch) or left and right (yaw), each axis on its own: one can be stable and the other
//!   not, which a single center would hide.
//! - Thrust: each thruster at full throttle (`game::thruster`). Propellers and rotors are left out:
//!   their thrust depends on how fast they spin.

mod aero;
mod airflow;
mod arrows;
mod centers;
mod forces;
mod lift_drag;

pub use lift_drag::LIFT_DRAG;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, Once, OnceLock, PoisonError, TryLockError};

use brickrust::ue::coreuobject::UObjectBase;

use super::{Editor, tooltip};
use crate::features::editor_buttons::{self, Button, Click, Press};
use crate::features::mouse_moves::{self, Handling, Movement};
use crate::features::ortho_view;
use crate::game::editor_object::{self, Places, Selection, Transform, Vector};
use crate::game::lift_surface::{self, LiftSurfaces};
use crate::game::mesh_component::MeshComponents;
use crate::game::reflection::{self, BoolField, Getter};
use crate::game::textures::{LinearColor, Textures};
use crate::game::thruster::{self, Thrusters};
use crate::game::ui_library::{self, UnitConversion};
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat};
use crate::prelude::*;

/// The markers' colors (the material's `Color`, linear RGB); the center of mass is orange
/// (1, 0.096, 0). Free to change.
const PITCH_COLOR: LinearColor = [0.0, 0.25, 1.0, 1.0];
const YAW_COLOR: LinearColor = [0.0, 0.8, 0.15, 1.0];
const THRUST_COLOR: LinearColor = [1.0, 0.0, 1.0, 1.0];

/// The centers we add.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Lift(centers::Axis),
    Thrust,
    /// With [`LIFT_DRAG`] shown, for the airflow set.
    Drag,
}

impl Kind {
    const ALL: [Kind; 4] = [
        Kind::Lift(centers::Axis::Pitch),
        Kind::Lift(centers::Axis::Yaw),
        Kind::Thrust,
        Kind::Drag,
    ];

    fn color(self) -> LinearColor {
        match self {
            Kind::Lift(centers::Axis::Pitch) => PITCH_COLOR,
            Kind::Lift(centers::Axis::Yaw) => YAW_COLOR,
            Kind::Thrust => THRUST_COLOR,
            Kind::Drag => lift_drag::DRAG_COLOR,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Lift(centers::Axis::Pitch) => "Center of lift (pitch)",
            Kind::Lift(centers::Axis::Yaw) => "Center of lift (yaw)",
            Kind::Thrust => "Center of thrust",
            Kind::Drag => "Center of drag",
        }
    }
}

/// What the markers need from the game (after engine init).
struct Code {
    lift: LiftSurfaces,
    thrusters: Thrusters,
    places: Places,
    selection: Selection,
    meshes: MeshComponents,
    textures: Textures,
    /// Offset of `ABrickEditor::CenterOfMassMeshComponent`, the game's marker (null until the
    /// Physics view mode first showed it).
    com: usize,
    /// `USceneComponent::bVisible`.
    visible: BoolField,
    /// `USceneComponent* USceneComponent::GetAttachParent() const`.
    parent: Getter<*mut UObjectBase>,
    /// Offsets of `USceneComponent::RelativeScale3D` and `UStaticMeshComponent::StaticMesh`.
    scale: usize,
    mesh: usize,
    /// `EBrickEditorViewMode::Physics`.
    view: u8,
    /// The factor from cm to the distances the player sees (`ENumericValueType::DistanceAuto`):
    /// metric or imperial, as the game's settings say.
    units: UnitConversion,
    distance: u8,
    /// `None` if something lift and drag need is missing (logged): the centers still show.
    lift_drag: Option<lift_drag::Code>,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

/// The markers between frames.
struct State {
    /// The editor they're in.
    editor: Option<WeakObject>,
    markers: [Option<Marker>; 4],
    /// What the centers were worked out for: the editor's generation, its selection, and whether
    /// [`LIFT_DRAG`] was shown.
    key: Option<(u64, u64, bool)>,
    centers: Centers,
    /// Counts the times the centers were worked out, for the tooltip.
    revision: u64,
    /// Lift and drag, and the airflow (straight ahead again in another editor).
    lift_drag: lift_drag::State,
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for State {}

/// One of our markers: a mesh component, where it is and whether it shows.
struct Marker {
    component: WeakObject,
    location: Option<Vector>,
    visible: bool,
}

#[derive(Clone, Copy, Default)]
struct Centers {
    pitch: Option<Vector>,
    yaw: Option<Vector>,
    thrust: Option<Vector>,
    drag: Option<Vector>,
    /// Whether they're the selection's.
    selected: bool,
}

impl Centers {
    fn of(&self, kind: Kind) -> Option<Vector> {
        match kind {
            Kind::Lift(centers::Axis::Pitch) => self.pitch,
            Kind::Lift(centers::Axis::Yaw) => self.yaw,
            Kind::Thrust => self.thrust,
            Kind::Drag => self.drag,
        }
    }
}

static CODE: OnceLock<Option<Code>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new(0));

impl State {
    const fn new(revision: u64) -> Self {
        Self {
            editor: None,
            markers: [None, None, None, None],
            key: None,
            centers: Centers {
                pitch: None,
                yaw: None,
                thrust: None,
                drag: None,
                selected: false,
            },
            revision,
            lift_drag: lift_drag::State::new(),
        }
    }
}

/// Finds the game code the markers need, on the first tick: `None` if something's missing (logged
/// once).
fn code() -> Option<&'static Code> {
    CODE.get_or_init(|| {
        let found = (|| {
            let class =
                |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
            let scene = class("SceneComponent")?;
            let physics =
                reflection::enum_value("EBrickEditorViewMode", "EBrickEditorViewMode::Physics")?;
            let distance =
                reflection::enum_value("ENumericValueType", "ENumericValueType::DistanceAuto")?;
            Ok::<_, String>(Code {
                lift: lift_surface::find()?,
                thrusters: thruster::find()?,
                places: Places::find()?,
                selection: editor_object::selection()?,
                meshes: MeshComponents::find()?,
                textures: Textures::find()?,
                com: reflection::field_offset(
                    class("BrickEditor")?,
                    "CenterOfMassMeshComponent",
                    "ObjectProperty",
                    8,
                )?,
                // SAFETY: `find_class` returns a live class.
                visible: reflection::bool_field(unsafe { &(*scene).ustruct }, "bVisible")?,
                parent: Getter::find(scene, "GetAttachParent", "ObjectProperty")?,
                scale: reflection::field_offset(scene, "RelativeScale3D", "StructProperty", 12)?,
                mesh: reflection::field_offset(
                    class("StaticMeshComponent")?,
                    "StaticMesh",
                    "ObjectProperty",
                    8,
                )?,
                view: u8::try_from(physics).map_err(|_| "Physics view mode value too big")?,
                units: ui_library::unit_conversion()?,
                distance: u8::try_from(distance)
                    .map_err(|_| format!("ENumericValueType::DistanceAuto is {distance}"))?,
                lift_drag: lift_drag::find()
                    .map_err(|reason| log!("no lift and drag: {reason}"))
                    .ok(),
            })
        })();
        found
            .map_err(|reason| log!("no centers of lift and thrust: {reason}"))
            .ok()
    })
    .as_ref()
}

/// Takes the editor's clicks, mouse movement and zoom steps from now on, for dragging the airflow's
/// handle and stepping its speed (after engine init).
pub(super) fn listen() -> Result<(), String> {
    ortho_view::listen_zoom_steps(take_zoom_step);
    editor_buttons::listen(on_button)?;
    mouse_moves::listen(take_movement)
}

/// A zoom step (the mouse wheel) over the airflow's handle steps its speed, not the zoom.
fn take_zoom_step(zoom_in: bool) -> bool {
    let Some(code) = CODE.get().and_then(Option::as_ref) else {
        return false;
    };
    let Some(mut state) = try_state() else {
        return false;
    };
    let stepped = lift_drag::shown() && state.lift_drag.step_speed(zoom_in, code.imperial());
    if stepped {
        // The tooltip tells the speed.
        state.revision += 1;
    }
    stepped
}

/// A press of the select button on the airflow's handle starts dragging it, its release ends that.
fn on_button(press: &Press) -> bool {
    let Some(mut state) = try_state() else {
        return false;
    };
    let airflow = &mut state.lift_drag.airflow;
    match (press.button, press.click) {
        (Button::Select | Button::AddToSelection, Click::Pressed) => airflow.press(),
        (Button::Select | Button::AddToSelection, Click::Released) => airflow.release(),
        _ => false,
    }
}

/// While dragging the airflow's handle, the mouse's movement moves it, not the camera.
fn take_movement(movement: &Movement) -> Handling {
    let Some(mut state) = try_state() else {
        return Handling::Pass;
    };
    if state.lift_drag.airflow.moved(movement.pixels()) {
        Handling::Take
    } else {
        Handling::Pass
    }
}

/// The state, from an input handler: `None` if it's in use.
fn try_state() -> Option<std::sync::MutexGuard<'static, State>> {
    match STATE.try_lock() {
        Ok(state) => Some(state),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        // Only the game thread uses the state, and not around input handlers.
        Err(TryLockError::WouldBlock) => None,
    }
}

/// The editor's view mode value and our [`State::revision`], if it shows the game's Physics view
/// mode: what the tooltip depends on.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
pub(super) unsafe fn shown(editor: *mut UObjectBase) -> Option<(u8, u64)> {
    let code = CODE.get()?.as_ref()?;
    // SAFETY: per the contract.
    let view = unsafe { super::view_of(editor) }?;
    let revision = STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .revision;
    (view == code.view).then_some((view, revision))
}

/// Whether `view` is the game's Physics view mode.
pub(super) fn is_physics(view: u8) -> bool {
    CODE.get()
        .and_then(Option::as_ref)
        .is_some_and(|code| code.view == view)
}

/// Every frame: shows the centers where the game shows its center of mass, worked out again if the
/// editor or its selection changed, or hides them; and lift and drag, following the airflow's
/// handle.
pub fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(code) = code() else { return };
    let (Some(view_code), Some(views)) = (super::CODE.get(), super::VIEWS.get()) else {
        return;
    };
    let view = Editor {
        code: view_code,
        bricks: &views.bricks,
        editor,
    };
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: on the game thread (the editor's tick), `editor` the live editor ticking; the game's
    // marker is a live component if not null, and so is its parent.
    unsafe {
        if !state.editor.is_some_and(|known| known.is(editor)) {
            // Another editor: the old one's markers went with it.
            *state = State::new(state.revision + 1);
            state.editor = Some(WeakObject::new(editor));
        }
        let com = editor
            .cast::<u8>()
            .add(code.com)
            .cast::<*mut UObjectBase>()
            .read();
        if com.is_null() || !code.visible.get(com.cast()) {
            state.hide(code);
            return;
        }
        let (selected, selection) = code.selected(&view);
        let forces_on = lift_drag::shown() && code.lift_drag.is_some();
        let key = (super::generation(), selection, forces_on);
        if state.key != Some(key) {
            state.key = Some(key);
            let (found, inputs, ball) = code.centers(&view, selected);
            state.centers = found;
            if let Some(lift_drag) = &code.lift_drag {
                state.lift_drag.set_inputs(lift_drag, inputs, ball);
            }
            state.revision += 1;
        }
        let parent = code.parent.call(com);
        match &code.lift_drag {
            Some(lift_drag) if forces_on && !parent.is_null() => {
                let material = code.meshes.material(com, 0);
                let state = &mut *state;
                if state.lift_drag.tick(
                    lift_drag,
                    &code.meshes,
                    &code.textures,
                    editor,
                    parent,
                    material,
                ) {
                    state.revision += 1;
                }
                state.centers.drag = state.lift_drag.center_of_drag().map(math::narrow);
            }
            _ => {
                state.lift_drag.hide(&code.meshes);
                state.centers.drag = None;
            }
        }
        state.show(code, editor, com);
    }
}

impl Code {
    /// Whether anything in the editor is selected, and a hash of what is.
    ///
    /// # Safety
    /// `editor.editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn selected(&self, editor: &Editor) -> (bool, u64) {
        let mut hasher = DefaultHasher::new();
        let mut any = false;
        // SAFETY: per the contract; the editor's objects are live.
        unsafe {
            for &object in editor.objects() {
                if !object.is_null() && self.selection.is_selected(object) {
                    (object as usize).hash(&mut hasher);
                    any = true;
                }
            }
        }
        (any, hasher.finish())
    }

    /// The centers of `editor`'s bricks, of the selected ones only if `selected` (the center of drag
    /// comes from [`lift_drag`]); what lift and drag are worked out from, and the ball around every
    /// brick.
    ///
    /// # Safety
    /// `editor.editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn centers(
        &self,
        editor: &Editor,
        selected: bool,
    ) -> (Centers, forces::Inputs, Option<airflow::Ball>) {
        let mut surfaces = Vec::new();
        let mut pushes = Vec::new();
        let mut boxes = Vec::new();
        // SAFETY: per the contract; the editor's bricks are live, and their static info classes.
        unsafe {
            for brick in editor.brick_list() {
                let Some(place) = self.places.of(brick) else {
                    continue;
                };
                let rotation = Quat::from_rotator(math::widen(place.rotation));
                let location = math::widen(place.location);
                if let Some(bounds) = &self.lift_drag.as_ref().map(lift_drag::Code::bounds)
                    && let Some(local) = bounds.of(brick)
                {
                    boxes.push((local.map(math::widen), location, rotation));
                }
                if selected && !self.selection.is_selected(brick) {
                    continue;
                }
                let [x, y, z] = place.location;
                let transform = Transform {
                    rotation: [rotation.x, rotation.y, rotation.z, rotation.w].map(|it| it as f32),
                    translation: [x, y, z, 0.0],
                    scale: [1.0, 1.0, 1.0, 0.0],
                };
                // Already in the editor root's space.
                if let Some(surface) = self.lift.of(brick, &transform) {
                    surfaces.push(aero::Surface {
                        location: math::widen(surface.location),
                        normal: math::widen(surface.normal),
                        radius: f64::from(surface.radius),
                        cross_section: f64::from(surface.area),
                    });
                }
                if let Some(push) = self.thrusters.push(brick, place.class) {
                    pushes.push(centers::Force {
                        location: math::add(location, rotation.rotate(math::widen(push.location))),
                        force: math::scale(
                            rotation.rotate(math::widen(push.direction)),
                            f64::from(push.force),
                        ),
                    });
                }
            }
        }
        let coefficients = aero::Coefficients::DEFAULT;
        let center_of_lift =
            |axis| centers::center_of_lift(&surfaces, axis, &coefficients).map(math::narrow);
        let found = Centers {
            pitch: center_of_lift(centers::Axis::Pitch),
            yaw: center_of_lift(centers::Axis::Yaw),
            thrust: centers::center_of(&pushes).map(math::narrow),
            drag: None,
            selected,
        };
        static LOGGED: Once = Once::new();
        LOGGED.call_once(|| {
            log!(
                "centers of lift and thrust: {} lift surfaces, {} thrusters{}",
                surfaces.len(),
                pushes.len(),
                if selected { " (selected)" } else { "" }
            )
        });
        let inputs = forces::Inputs {
            // Bricks show their own arrows when they're selected.
            surfaces: surfaces
                .into_iter()
                .map(|surface| (surface, selected))
                .collect(),
            thrust: pushes,
        };
        (found, inputs, lift_drag::ball(&boxes))
    }
}

impl State {
    /// Hides the markers, and lift and drag.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn hide(&mut self, code: &Code) {
        for marker in self.markers.iter_mut().flatten() {
            if let Some(component) = marker.component.get_live().filter(|_| marker.visible) {
                // SAFETY: per the contract; a live component of ours.
                unsafe { code.meshes.set_visible(component, false) };
            }
            marker.visible = false;
        }
        // SAFETY: per the contract.
        unsafe { self.lift_drag.hide(&code.meshes) };
    }

    /// Shows the markers that have a center, placed like the game's marker `com` (scale, rotation),
    /// making the missing ones, and hides the others.
    ///
    /// # Safety
    /// `editor` must be the live editor, `com` its live marker. Game thread only.
    unsafe fn show(&mut self, code: &Code, editor: *mut UObjectBase, com: *mut UObjectBase) {
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let center = self.centers.of(kind);
            // SAFETY: per the contract.
            unsafe {
                let alive = self.markers[index]
                    .as_ref()
                    .is_some_and(|marker| marker.component.get_live().is_some());
                if !alive {
                    if center.is_none() {
                        continue;
                    }
                    self.markers[index] = code.marker(editor, com, kind);
                }
                let Some(marker) = &mut self.markers[index] else {
                    continue;
                };
                let Some(component) = marker.component.get_live() else {
                    continue;
                };
                if let Some(center) = center.filter(|&center| marker.location != Some(center)) {
                    code.place(component, com, center);
                    marker.location = Some(center);
                }
                if marker.visible != center.is_some() {
                    marker.visible = center.is_some();
                    code.meshes.set_visible(component, marker.visible);
                }
            }
        }
    }
}

impl Code {
    /// A new marker of `kind` like the game's marker `com`: its mesh, its material in our color,
    /// attached to what it's attached to; `None` if the engine made none (logged once).
    ///
    /// # Safety
    /// `editor` must be the live editor, `com` its live marker. Game thread only.
    unsafe fn marker(
        &self,
        editor: *mut UObjectBase,
        com: *mut UObjectBase,
        kind: Kind,
    ) -> Option<Marker> {
        static FAILED: Once = Once::new();
        // SAFETY: per the contract; the parent, mesh and material are live objects if not null.
        let component = unsafe {
            let parent = self.parent.call(com);
            let mesh = com
                .cast::<u8>()
                .add(self.mesh)
                .cast::<*mut UObjectBase>()
                .read();
            let material = self.meshes.material(com, 0);
            let scale = com
                .cast::<u8>()
                .add(self.scale)
                .cast::<Vector>()
                .read_unaligned();
            let made = [parent, mesh, material]
                .iter()
                .all(|object| !object.is_null())
                .then(|| self.meshes.create(parent, mesh, scale))
                .flatten();
            if let Some(component) = made {
                let instance = self.textures.instance(editor, material);
                if !instance.is_null() {
                    self.textures.set_color(instance, "Color", kind.color());
                    self.meshes.set_material(component, 0, instance);
                }
                self.meshes.set_visible(component, false);
            }
            made
        };
        let Some(component) = component else {
            FAILED.call_once(|| log!("couldn't make a marker like the center of mass"));
            return None;
        };
        Some(Marker {
            // SAFETY: just made, live.
            component: unsafe { WeakObject::new(component) },
            location: None,
            visible: false,
        })
    }

    /// Puts `marker` at `center` (in the editor root's space), turned and scaled like `com`.
    ///
    /// # Safety
    /// Both must be live scene components. Game thread only.
    unsafe fn place(&self, marker: *mut UObjectBase, com: *mut UObjectBase, center: Vector) {
        // SAFETY: per the contract; the scale's offset comes from reflection.
        unsafe {
            let (_, rotation) = self.meshes.relative_place(com);
            let rotation = Quat::from_rotator(math::widen(rotation));
            let [x, y, z] = center;
            let [sx, sy, sz] = com
                .cast::<u8>()
                .add(self.scale)
                .cast::<Vector>()
                .read_unaligned();
            self.meshes.set_transform(
                marker,
                Transform {
                    rotation: [rotation.x, rotation.y, rotation.z, rotation.w].map(|it| it as f32),
                    translation: [x, y, z, 0.0],
                    scale: [sx, sy, sz, 0.0],
                },
            );
        }
    }

    /// Whether the game shows distances in imperial units (ft) rather than metric ones (m).
    fn imperial(&self) -> bool {
        self.units.factor(self.distance) > 0.02
    }
}

/// What the tooltip can be over.
enum Hovered {
    /// The game's center of mass, or one of our markers.
    Marker(&'static str, Vector, Option<Kind>),
    /// Lift and drag's arrows and handle.
    LiftDrag(lift_drag::Hit),
}

/// The tooltip over what's under the mouse, nearest first: the game's center of mass or one of our
/// markers (which it is and where, in the editor root's space, and for a center of lift how far
/// behind the center of mass it is), or lift and drag's arrows or handle.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
pub(super) unsafe fn describe(
    editor: *mut UObjectBase,
    hover: &tooltip::Hover,
) -> tooltip::Description {
    let Some(code) = CODE.get().and_then(Option::as_ref) else {
        return tooltip::Description::default();
    };
    let state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: per the contract; the game's marker is a live component if not null, ours are live
    // (`get_live`), and so is the marker's parent if not null.
    unsafe {
        let com = editor
            .cast::<u8>()
            .add(code.com)
            .cast::<*mut UObjectBase>()
            .read();
        if com.is_null() || !code.visible.get(com.cast()) {
            return tooltip::Description::default();
        }
        let mass = code.meshes.relative_place(com).0;
        let mut shown = vec![("Center of mass", com, mass, None)];
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let marker = state.markers[index]
                .as_ref()
                .filter(|marker| marker.visible);
            let component = marker.and_then(|marker| marker.component.get_live());
            if let (Some(component), Some(center)) = (component, state.centers.of(kind)) {
                shown.push((kind.name(), component, center, Some(kind)));
            }
        }
        let markers = shown
            .into_iter()
            .filter_map(|(name, component, center, kind)| {
                let along = hover.sphere_hit(component)?;
                Some((f64::from(along), Hovered::Marker(name, center, kind)))
            });
        let parent = code.parent.call(com);
        let arrows = code
            .lift_drag
            .as_ref()
            .filter(|_| lift_drag::shown() && !parent.is_null())
            .and_then(|lift_drag| lift_drag.local_ray(parent, &hover.ray))
            .and_then(|(origin, direction)| state.lift_drag.hit(origin, direction))
            .map(|(along, hit)| (along, Hovered::LiftDrag(hit)));
        let hit = markers
            .chain(arrows)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, hovered)| hovered);
        let (name, [x, y, z], kind) = match hit {
            None => return tooltip::Description::default(),
            Some(Hovered::LiftDrag(hit)) => {
                return state
                    .lift_drag
                    .describe(hit, code.imperial(), state.centers.selected);
            }
            Some(Hovered::Marker(name, center, kind)) => (name, center, kind),
        };
        // Shown distances: m (0.01 per cm) or ft (0.0328), as the game shows them.
        let (cm_per_unit, unit) = if code.imperial() {
            (CM_PER_FOOT, "ft")
        } else {
            (CM_PER_METER, "m")
        };
        let mut lines: Vec<String> = [("X", x), ("Y", y), ("Z", z)]
            .iter()
            .map(|&(axis, cm)| format!("{axis}: {} {unit}", exact(cm, cm_per_unit)))
            .collect();
        match kind {
            Some(Kind::Lift(axis)) => {
                let behind = f64::from(mass[0] - x) / cm_per_unit;
                lines.push(stability(axis, behind, unit));
            }
            Some(Kind::Drag) => {
                lines.push(state.lift_drag.drag_line(code.imperial()));
            }
            _ => {}
        }
        if state.centers.selected {
            lines.push("Of the selection".to_owned());
        }
        tooltip::Description {
            title: name.to_owned(),
            lines,
        }
    }
}

/// How a center of lift `behind` (in `unit`s) the center of mass makes the vehicle behave on
/// `axis`: behind, lift turns the nose back into the airflow; ahead, further away from it.
fn stability(axis: centers::Axis, behind: f64, unit: &str) -> String {
    let axis = match axis {
        centers::Axis::Pitch => "pitch",
        centers::Axis::Yaw => "yaw",
    };
    // Closer than this is on the center of mass, as far as the shown decimals tell.
    if behind.abs() < 0.005 {
        return format!("On the center of mass: neutral in {axis}");
    }
    let (distance, side, verdict) = if behind > 0.0 {
        (behind, "behind", "stable")
    } else {
        (-behind, "ahead of", "unstable")
    };
    format!("{distance:.2} {unit} {side} the center of mass.")
}

const CM_PER_METER: f64 = 100.0;
const CM_PER_FOOT: f64 = 30.48;

/// The most decimals [`exact`] tries before giving every digit.
const MAX_DECIMALS: usize = 12;

/// `cm` in a unit of `cm_per_unit` cm, exactly: the shortest decimal number that is the same
/// stored value (`f32` cm) again when turned back, so nothing is rounded away and no digits are
/// made up. E.g. 12.5 cm is "0.125" m, 30.48 cm "1" ft.
fn exact(cm: f32, cm_per_unit: f64) -> String {
    let value = f64::from(cm) / cm_per_unit;
    (0..=MAX_DECIMALS)
        .map(|decimals| format!("{value:.decimals$}"))
        .find(|text| {
            text.parse::<f64>()
                .is_ok_and(|back| (back * cm_per_unit) as f32 == cm)
        })
        // "-0" for a hair below zero.
        .map(|text| if cm == 0.0 { "0".to_owned() } else { text })
        .unwrap_or_else(|| value.to_string())
}
