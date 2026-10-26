//! The Physics view mode's lift and drag (setting [`LIFT_DRAG`]): arrows for the sums of lift (cyan)
//! and drag (red) at their centers, the selected bricks' own arrows to a smaller scale, the center
//! of drag (a red marker, see `super::Kind::Drag`), and the airflow's handle ([`airflow`]): a white
//! arrow outside the vehicle, pointing at it from where the air comes, which the mouse drags, and
//! whose speed the mouse wheel steps over it. The tooltip tells the forces in newtons at that speed,
//! and the top speed the thrusters give against that drag.
//!
//! The arrows' lengths are to a scale per vehicle or selection, the biggest forces any airflow gives
//! it at full length ([`forces::Scale`]): turning the airflow, they grow and shrink with the forces.
//! The speed scales every force alike, so it leaves them be (the tooltip tells it).
//!
//! The arrows are procedural meshes (`game::procedural_mesh`, one per kind of arrow, every arrow of
//! that kind in it) attached where the game's center of mass marker is, in the editor root's
//! space, in the marker's material (unlit, drawn over the bricks) in our colors: it blends its
//! `Color` and `ContrastColor` by the vertex color, which our meshes lack, so both are ours. The coefficients are the
//! game's console variables, read when the vehicle or the selection changes; the density is the
//! map's air (`game::air_density`).

use std::sync::Once;

use brickrust::ue::coreuobject::UObjectBase;

use super::aero::Coefficients;
use super::airflow::{self, Airflow, Ball};
use super::arrows::{Arrow, Mesh};
use super::centers::Force;
use super::forces::{self, Forces, Inputs, Scale, Shows};
use super::tooltip;
use crate::framework::settings::{Alternative, Choice};
use crate::game::air_density::{self, AirDensity};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::console_variable::{self, ConsoleVariables};
use crate::game::editor_object::{LocalBounds, Transform};
use crate::game::mesh_component::MeshComponents;
use crate::game::procedural_mesh::ProceduralMeshes;
use crate::game::reflection::{self, Getter};
use crate::game::textures::{LinearColor, Textures};
use crate::game::viewport::{self, Ray, Viewport};
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat, Vec3};
use crate::platform::keyboard;
use crate::prelude::*;

/// Whether the Physics view mode shows lift and drag, and where their arrows start, a setting.
pub static LIFT_DRAG: Choice = Choice::new(
    "PhysicsLiftDragArrows",
    "Lift and drag",
    "In the Physics view mode, arrows show the lift (cyan) and drag (red) on the vehicle or selection. Drag the white arrow to set where the air comes from, scroll on it for its speed.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "OwnCenters",
            name: "Each from its center",
        },
        Alternative {
            key: "OnePoint",
            name: "Both from one point",
        },
    ],
    1,
);

/// Whether the Physics view mode shows lift and drag now ([`LIFT_DRAG`]).
pub fn shown() -> bool {
    LIFT_DRAG.get() != 0
}

/// Whether the arrows start from one point now ([`LIFT_DRAG`]).
fn together() -> bool {
    LIFT_DRAG.get() == 2
}

/// The arrows' colors (the material's `Color`, linear RGB), and the handle's under the mouse. Free
/// to change.
const LIFT_COLOR: LinearColor = [0.0, 0.85, 1.0, 1.0];
pub const DRAG_COLOR: LinearColor = [1.0, 0.02, 0.02, 1.0];
const AIRFLOW_COLOR: LinearColor = [0.9, 0.9, 0.9, 1.0];
const AIRFLOW_HOVERED_COLOR: LinearColor = [1.0, 0.85, 0.1, 1.0];

/// The speed until the mouse wheel sets one: 100 km/h, or 60 mph in imperial units (cm/s).
const METRIC_SPEED: f64 = 100.0 / KMH_PER_CM_S;
const IMPERIAL_SPEED: f64 = 60.0 / MPH_PER_CM_S;
const KMH_PER_CM_S: f64 = 0.036;
const MPH_PER_CM_S: f64 = 0.036 / 1.609_344;

/// The ball around a vehicle without boxes, and at least (cm).
const MIN_RADIUS: f64 = 60.0;

/// What lift and drag need from the game (after engine init), besides the markers' own.
pub struct Code {
    procedural: ProceduralMeshes,
    bounds: LocalBounds,
    editor: BrickEditor,
    viewport: Viewport,
    /// `FTransform USceneComponent::K2_GetComponentToWorld() const`.
    to_world: Getter<Transform>,
    /// `None` if not found (logged): the default coefficients and Earth's air.
    variables: Option<ConsoleVariables>,
    density: Option<AirDensity>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

pub fn find() -> Result<Code, String> {
    let scene = reflection::find_class("SceneComponent").ok_or("class SceneComponent not found")?;
    Ok(Code {
        procedural: ProceduralMeshes::find()?,
        bounds: LocalBounds::find()?,
        editor: brick_editor::find()?,
        viewport: viewport::find()?,
        to_world: Getter::find(scene, "K2_GetComponentToWorld", "StructProperty")?,
        variables: console_variable::find()
            .map_err(|reason| log!("lift and drag use the default coefficients: {reason}"))
            .ok(),
        density: air_density::find()
            .map_err(|reason| log!("lift and drag assume Earth's air: {reason}"))
            .ok(),
    })
}

impl Code {
    /// The bounds of an editor object, for [`ball`].
    pub fn bounds(&self) -> &LocalBounds {
        &self.bounds
    }

    /// The game's coefficients now, its defaults where a variable reads 0 (not found).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn coefficients(&self) -> Coefficients {
        let mut coefficients = Coefficients::DEFAULT;
        let Some(variables) = &self.variables else {
            return coefficients;
        };
        let read = |name: &str, value: &mut f64| {
            // SAFETY: per the contract.
            if let Some(read) = unsafe { variables.float(name) } {
                *value = f64::from(read);
            }
        };
        read(
            "BR.FluidDynamics.AirLiftCoefficient",
            &mut coefficients.lift,
        );
        read(
            "BR.FluidDynamics.AirDragCoefficient",
            &mut coefficients.drag,
        );
        read(
            "BR.FluidDynamics.CrossSectionCoefficient",
            &mut coefficients.cross_section,
        );
        read(
            "BR.FluidDynamics.PeakLiftAngle",
            &mut coefficients.peak_lift_angle,
        );
        read(
            "BR.FluidDynamics.MaxLiftAngle",
            &mut coefficients.max_lift_angle,
        );
        coefficients
    }

    /// `ray` (in the world) in the space of `parent`, where our meshes are; `None` if `parent` is
    /// scaled to nothing.
    ///
    /// # Safety
    /// `parent` must be a live scene component. Game thread only.
    pub unsafe fn local_ray(&self, parent: *mut UObjectBase, ray: &Ray) -> Option<(Vec3, Vec3)> {
        // SAFETY: per the contract.
        let transform = unsafe { self.to_world.call(parent) };
        let unturn = Quat::widen(transform.rotation).inverse();
        let scale = math::widen([transform.scale[0], transform.scale[1], transform.scale[2]]);
        if scale.contains(&0.0) {
            return None;
        }
        let unscale =
            |vector: Vec3| -> Vec3 { core::array::from_fn(|axis| vector[axis] / scale[axis]) };
        let [x, y, z, _] = transform.translation;
        let origin =
            unscale(unturn.rotate(math::sub(math::widen(ray.origin), math::widen([x, y, z]))));
        let direction = math::normalized(unscale(unturn.rotate(math::widen(ray.direction))))?;
        Some((origin, direction))
    }
}

/// The ball around `boxes` (each a brick's box in its own space, and where that is: its location
/// and rotation in the editor root's space).
pub fn ball(boxes: &[([Vec3; 2], Vec3, Quat)]) -> Option<Ball> {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for &([min, max], location, rotation) in boxes {
        for corner in 0..8 {
            let local = core::array::from_fn(|axis| {
                if corner >> axis & 1 == 0 {
                    min[axis]
                } else {
                    max[axis]
                }
            });
            let point = math::add(location, rotation.rotate(local));
            for axis in 0..3 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
    }
    if !low.iter().chain(&high).all(|value| value.is_finite()) {
        return None;
    }
    let center = math::scale(math::add(low, high), 0.5);
    let radius = (math::length(math::sub(high, low)) / 2.0).max(MIN_RADIUS);
    Some(Ball { center, radius })
}

/// Our components, by kind.
const LIFT: usize = 0;
const DRAG: usize = 1;
const HANDLE: usize = 2;

/// Lift and drag between frames.
pub struct State {
    pub airflow: Airflow,
    inputs: Inputs,
    /// `None` without bricks: nothing shows.
    ball: Option<Ball>,
    coefficients: Coefficients,
    /// The map's air (kg/cm³), as of the last [`State::set_inputs`].
    density: f64,
    /// Whether the forces must be worked out again (the inputs changed).
    dirty: bool,
    /// [`together`] as the arrows were last made.
    together: bool,
    forces: Forces,
    /// The arrows' scales for the inputs, the same whatever the airflow.
    scale: Scale,
    arrows: Vec<(Shows, Arrow)>,
    /// Our meshes ([`LIFT`], [`DRAG`], [`HANDLE`]) and whether each shows.
    components: [Option<WeakObject>; 3],
    visible: [bool; 3],
    /// The materials of the lift, drag, handle and hovered handle arrows.
    materials: Option<[WeakObject; 4]>,
    /// Whether the handle shows hovered.
    hovered: bool,
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for State {}

impl State {
    pub const fn new() -> Self {
        Self {
            airflow: Airflow::new(),
            inputs: Inputs {
                surfaces: Vec::new(),
                thrust: Vec::new(),
            },
            ball: None,
            coefficients: Coefficients::DEFAULT,
            density: air_density::DEFAULT_DENSITY,
            dirty: false,
            together: false,
            forces: Forces {
                bricks: Vec::new(),
                lift: None,
                drag: None,
                thrust: 0.0,
            },
            scale: Scale {
                brick: 0.0,
                sum: 0.0,
            },
            arrows: Vec::new(),
            components: [None, None, None],
            visible: [false; 3],
            materials: None,
            hovered: false,
        }
    }

    /// New bricks to work out the forces of, within `ball`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_inputs(&mut self, code: &Code, inputs: Inputs, ball: Option<Ball>) {
        self.inputs = inputs;
        self.ball = ball;
        // SAFETY: per the contract.
        unsafe {
            self.coefficients = code.coefficients();
            self.density = code
                .density
                .as_ref()
                .and_then(|density| density.current())
                .unwrap_or(air_density::DEFAULT_DENSITY);
        }
        self.scale = Scale::of(&self.inputs, &self.coefficients);
        self.dirty = true;
        static LOGGED: Once = Once::new();
        let (coefficients, density) = (self.coefficients, self.density);
        LOGGED.call_once(|| log!("lift and drag: {coefficients:?}, air {density} kg/cm³"));
    }

    /// Where the drag acts, if there's any.
    pub fn center_of_drag(&self) -> Option<Vec3> {
        // Without surfaces the forces aren't worked out again: the old ones are stale.
        if self.inputs.surfaces.is_empty() {
            return None;
        }
        self.forces.drag.map(|drag| drag.location)
    }

    /// A step of the mouse wheel: steps the airflow's speed if the mouse is on its handle (see
    /// [`Airflow::step_speed`]), in metric or `imperial` units. Whether it did.
    pub fn step_speed(&mut self, faster: bool, imperial: bool) -> bool {
        let (default, _, per_cm_s) = units(imperial);
        self.airflow.step_speed(faster, per_cm_s, default)
    }

    /// The airflow's speed (cm/s), its unit and how many of those a cm/s is.
    fn speed(&self, imperial: bool) -> (f64, &'static str, f64) {
        let (default, unit, per_cm_s) = units(imperial);
        (self.airflow.speed(default), unit, per_cm_s)
    }

    /// A force of `per_unit` (see [`forces::newtons`]) at the airflow's speed, e.g. "12 N at 100
    /// km/h".
    fn newtons(&self, per_unit: f64, imperial: bool) -> String {
        let (speed, unit, per_cm_s) = self.speed(imperial);
        format!(
            "{} N at {} {unit}",
            tooltip::number(forces::newtons(per_unit, self.density, speed)),
            (speed * per_cm_s).round()
        )
    }

    /// Every frame while shown: follows the mouse with the handle, works the forces out again if the
    /// airflow or the bricks changed, and shows them in meshes attached to `parent` in
    /// `material`'s look. Whether what the tooltip tells changed.
    ///
    /// # Safety
    /// `editor` must be the live editor, `parent` a live scene component, `material` null or a live
    /// material. Game thread only.
    pub unsafe fn tick(
        &mut self,
        code: &Code,
        meshes: &MeshComponents,
        textures: &Textures,
        editor: *mut UObjectBase,
        parent: *mut UObjectBase,
        material: *mut UObjectBase,
    ) -> bool {
        // Without lift surfaces there's no lift or drag at all (the game has no other air forces).
        let Some(ball) = self.ball.filter(|_| !self.inputs.surfaces.is_empty()) else {
            // SAFETY: per the contract.
            unsafe { self.hide(meshes) };
            return false;
        };
        // SAFETY: per the contract; the controller is live if not null.
        let turned = unsafe {
            let controller = code.editor.player_controller(editor);
            let (mouse, ray) = if controller.is_null() {
                (None, None)
            } else {
                let mouse = self.airflow.pointer(code.viewport.mouse(controller));
                let ray = mouse
                    .and_then(|mouse| code.viewport.ray(controller, mouse))
                    .and_then(|ray| code.local_ray(parent, &ray));
                (mouse, ray)
            };
            let held = keyboard::is_mouse_button_down();
            self.airflow.update(ray, mouse, &ball, held)
        };
        let changed = turned || self.dirty || together() != self.together;
        // SAFETY: per the contract.
        unsafe {
            let Some(materials) = self.materials(textures, editor, material) else {
                return false;
            };
            if changed {
                self.dirty = false;
                self.together = together();
                self.forces =
                    forces::forces(&self.inputs, self.airflow.direction(), &self.coefficients);
                self.arrows = forces::arrows(&self.forces, &self.scale, &ball, self.together);
                for (slot, lift) in [(LIFT, true), (DRAG, false)] {
                    let mut mesh = Mesh::default();
                    for (shows, arrow) in &self.arrows {
                        if shows.is_lift() == lift {
                            mesh.add(arrow);
                        }
                    }
                    self.show(code, meshes, parent, slot, &mesh, materials[slot]);
                }
                let mut mesh = Mesh::default();
                mesh.add(&self.airflow.handle(&ball));
                let material = materials[if self.hovered { 3 } else { HANDLE }];
                self.show(code, meshes, parent, HANDLE, &mesh, material);
            }
            let hovered = self.airflow.hovered();
            if hovered != self.hovered
                && let Some(handle) = self.components[HANDLE]
                    .as_ref()
                    .and_then(WeakObject::get_live)
            {
                self.hovered = hovered;
                meshes.set_material(handle, 0, materials[if hovered { 3 } else { HANDLE }]);
            }
        }
        changed
    }

    /// Hides our meshes and lets go of the handle.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self, meshes: &MeshComponents) {
        self.airflow.leave();
        for slot in [LIFT, DRAG, HANDLE] {
            // SAFETY: per the contract; a live component of ours.
            unsafe { self.set_visible(meshes, slot, false) };
        }
        // Shown again worked out anew: the bricks may have changed meanwhile.
        self.dirty = true;
    }

    /// Shows `mesh` in our component `slot` (made if missing, hidden if `mesh` is empty) in
    /// `material`.
    ///
    /// # Safety
    /// As for [`Self::tick`], `material` a live material.
    unsafe fn show(
        &mut self,
        code: &Code,
        meshes: &MeshComponents,
        parent: *mut UObjectBase,
        slot: usize,
        mesh: &Mesh,
        material: *mut UObjectBase,
    ) {
        // SAFETY: per the contract; our components are live (checked through their weak pointers)
        // procedural mesh components.
        unsafe {
            if mesh.corners.is_empty() {
                self.set_visible(meshes, slot, false);
                return;
            }
            let component = match self.components[slot]
                .as_ref()
                .and_then(WeakObject::get_live)
            {
                Some(component) => component,
                None => {
                    let made = meshes.create_of(code.procedural.class(), parent, [1.0; 3]);
                    let Some(component) = made else {
                        static FAILED: Once = Once::new();
                        FAILED.call_once(|| log!("couldn't make the lift and drag arrows"));
                        return;
                    };
                    self.components[slot] = Some(WeakObject::new(component));
                    self.visible[slot] = true;
                    component
                }
            };
            code.procedural.set_section(
                component,
                &mesh.positions,
                &mesh.corners,
                &mesh.normals,
                &mesh.uvs,
            );
            meshes.set_material(component, 0, material);
            self.set_visible(meshes, slot, true);
        }
    }

    /// # Safety
    /// Game thread only.
    unsafe fn set_visible(&mut self, meshes: &MeshComponents, slot: usize, visible: bool) {
        let Some(component) = self.components[slot]
            .as_ref()
            .and_then(WeakObject::get_live)
        else {
            self.visible[slot] = false;
            return;
        };
        if self.visible[slot] != visible {
            self.visible[slot] = visible;
            // SAFETY: per the contract; a live component of ours.
            unsafe { meshes.set_visible(component, visible) };
        }
    }

    /// The arrows' materials: instances of `material` (the center of mass marker's) in our colors,
    /// made once; `None` if they can't be.
    ///
    /// # Safety
    /// As for [`Self::tick`].
    unsafe fn materials(
        &mut self,
        textures: &Textures,
        editor: *mut UObjectBase,
        material: *mut UObjectBase,
    ) -> Option<[*mut UObjectBase; 4]> {
        if let Some(materials) = &self.materials {
            let live = materials.each_ref().map(WeakObject::get_live);
            if live.iter().all(Option::is_some) {
                return Some(live.map(|material| material.unwrap_or(core::ptr::null_mut())));
            }
        }
        if material.is_null() {
            return None;
        }
        let colors = [LIFT_COLOR, DRAG_COLOR, AIRFLOW_COLOR, AIRFLOW_HOVERED_COLOR];
        // SAFETY: per the contract; the instances are live once made.
        let made = unsafe {
            colors.map(|color| {
                let instance = textures.instance(editor, material);
                if !instance.is_null() {
                    textures.set_color(instance, "Color", color);
                    textures.set_color(instance, "ContrastColor", color);
                }
                instance
            })
        };
        if made.iter().any(|instance| instance.is_null()) {
            static FAILED: Once = Once::new();
            FAILED.call_once(|| log!("couldn't make the lift and drag arrows' materials"));
            return None;
        }
        // SAFETY: just made, live.
        self.materials = Some(made.map(|instance| unsafe { WeakObject::new(instance) }));
        Some(made)
    }

    /// What's under the ray from `origin` along `direction` (unit length, in the editor root's
    /// space): the handle or one of the arrows shown, and how far along the ray; the nearest.
    pub fn hit(&self, origin: Vec3, direction: Vec3) -> Option<(f64, Hit)> {
        let ball = self.ball?;
        let handle = self
            .airflow
            .handle(&ball)
            .hit(origin, direction, airflow::SLACK)
            .map(|along| (along, Hit::Airflow));
        let arrows = self.arrows.iter().filter_map(|(shows, arrow)| {
            let along = arrow.hit(origin, direction, airflow::SLACK)?;
            Some((along, Hit::Arrow(*shows)))
        });
        handle
            .into_iter()
            .chain(arrows)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The tooltip over `hit`, telling forces in metric or `imperial` units, of the selection if
    /// `selected`.
    pub fn describe(&self, hit: Hit, imperial: bool, selected: bool) -> tooltip::Description {
        let size = |force: Option<Force>| force.map_or(0.0, |force| math::length(force.force));
        let of = if selected { " of the selection" } else { "" };
        let (title, mut lines) = match hit {
            Hit::Airflow => {
                let (speed, unit, per_cm_s) = self.speed(imperial);
                let lines = vec![
                    format!(
                        "{} {unit}, {}",
                        (speed * per_cm_s).round(),
                        self.airflow.from()
                    ),
                    "Drag to turn, scroll for speed".to_owned(),
                ];
                ("Airflow".to_owned(), lines)
            }
            Hit::Arrow(Shows::Lift) => (
                format!("Lift{of}"),
                vec![self.newtons(size(self.forces.lift), imperial)],
            ),
            Hit::Arrow(Shows::Drag) => {
                let drag = size(self.forces.drag);
                let mut lines = vec![self.newtons(drag, imperial)];
                if let Some(top) = forces::top_speed(self.forces.thrust, drag, self.density) {
                    let (_, unit, per_cm_s) = self.speed(imperial);
                    lines.push(format!(
                        "Top speed on thrusters: {} {unit}",
                        tooltip::number((top * per_cm_s).round())
                    ));
                }
                (format!("Drag{of}"), lines)
            }
            Hit::Arrow(Shows::BrickLift(index) | Shows::BrickDrag(index)) => {
                let lift = matches!(hit, Hit::Arrow(Shows::BrickLift(_)));
                let push = |index: usize| {
                    self.forces.bricks.get(index).map_or(0.0, |(_, push)| {
                        math::length(if lift { push.lift } else { push.drag })
                    })
                };
                let all: f64 = (0..self.forces.bricks.len()).map(push).sum();
                let own = push(index);
                let mut lines = vec![self.newtons(own, imperial)];
                if all > 0.0 {
                    lines.push(format!("{:.0}% of the selection", own / all * 100.0));
                }
                let what = if lift { "Brick lift" } else { "Brick drag" };
                (what.to_owned(), lines)
            }
        };
        if self.density <= 0.0 {
            lines.push("No air on this map".to_owned());
        }
        tooltip::Description { title, lines }
    }

    /// The tooltip line for the center of drag: its drag in newtons.
    pub fn drag_line(&self, imperial: bool) -> String {
        let drag = self
            .forces
            .drag
            .map_or(0.0, |drag| math::length(drag.force));
        format!("Drag: {}", self.newtons(drag, imperial))
    }
}

/// The default speed (cm/s) in metric or `imperial` units, the unit and how many of those a cm/s
/// is.
fn units(imperial: bool) -> (f64, &'static str, f64) {
    if imperial {
        (IMPERIAL_SPEED, "mph", MPH_PER_CM_S)
    } else {
        (METRIC_SPEED, "km/h", KMH_PER_CM_S)
    }
}

/// What the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Airflow,
    Arrow(Shows),
}
