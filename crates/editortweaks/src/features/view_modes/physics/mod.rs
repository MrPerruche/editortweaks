//! The game's Physics view mode, with more: next to its center of mass, the center of lift (blue)
//! and of thrust (magenta), drawn like it (its mesh and material in other colors, over the bricks),
//! and the view modes' tooltip over all three, telling which it is and where.
//!
//! Like the game's center of mass (`ABrickEditor::UpdatePhysicsProperties`, see
//! `docs/re/physics-centers.md`), they're the selection's if anything is selected, else the whole
//! vehicle's, in the editor root's space (the bricks' places), and show only while it shows: in the
//! Physics view mode, not while moving bricks. They're worked out again when the editor changes
//! ([`super::generation`]) or the selection does ([`centers`] for how).
//!
//! - Lift: each brick generating lift, its surface as the game's fluid dynamics get it
//!   (`game::lift_surface`), for the vehicle flying forward (its X axis).
//! - Thrust: each thruster at full throttle (`game::thruster`). Propellers and rotors are left out:
//!   their thrust depends on how fast they spin.

mod centers;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use super::{Editor, tooltip};
use crate::framework::settings::Toggle;
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

/// Whether the Physics view mode shows the centers of lift and thrust.
pub static CENTERS: Toggle = Toggle::new(
    "PhysicsCenters",
    "Centers of lift and thrust",
    "In the Physics view mode, the center of lift (blue: wings and other lifting bricks, the vehicle flying forward) and the center of thrust (magenta: thrusters at full throttle) show next to the center of mass, like it: the selection's if anything is selected.",
    true,
);

/// The markers' colors (the material's `Color`, linear RGB); the center of mass is orange
/// (1, 0.096, 0). Free to change.
const LIFT_COLOR: LinearColor = [0.0, 0.25, 1.0, 1.0];
const THRUST_COLOR: LinearColor = [1.0, 0.0, 1.0, 1.0];

/// The centers we add.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Lift,
    Thrust,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Lift, Kind::Thrust];

    fn color(self) -> LinearColor {
        match self {
            Kind::Lift => LIFT_COLOR,
            Kind::Thrust => THRUST_COLOR,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Lift => "Center of lift",
            Kind::Thrust => "Center of thrust",
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
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

/// The markers between frames.
struct State {
    /// The editor they're in.
    editor: Option<WeakObject>,
    markers: [Option<Marker>; 2],
    /// What the centers were worked out for: the editor's generation, its selection, and whether
    /// [`CENTERS`] was on.
    key: Option<(u64, u64, bool)>,
    centers: Centers,
    /// Counts the times the centers were worked out, for the tooltip.
    revision: u64,
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
    lift: Option<Vector>,
    thrust: Option<Vector>,
    /// Whether they're the selection's.
    selected: bool,
}

impl Centers {
    fn of(&self, kind: Kind) -> Option<Vector> {
        match kind {
            Kind::Lift => self.lift,
            Kind::Thrust => self.thrust,
        }
    }
}

static CODE: OnceLock<Option<Code>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State {
    editor: None,
    markers: [None, None],
    key: None,
    centers: Centers {
        lift: None,
        thrust: None,
        selected: false,
    },
    revision: 0,
});

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
            })
        })();
        found
            .map_err(|reason| log!("no centers of lift and thrust: {reason}"))
            .ok()
    })
    .as_ref()
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
/// editor or its selection changed, or hides them.
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
    // marker is a live component if not null.
    unsafe {
        if !state.editor.is_some_and(|known| known.is(editor)) {
            // Another editor: the old one's markers went with it.
            *state = State {
                editor: Some(WeakObject::new(editor)),
                markers: [None, None],
                key: None,
                centers: Centers::default(),
                revision: state.revision + 1,
            };
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
        let on = CENTERS.get();
        let key = (super::generation(), selection, on);
        if state.key != Some(key) {
            state.key = Some(key);
            state.centers = if on {
                code.centers(&view, selected)
            } else {
                Centers {
                    selected,
                    ..Centers::default()
                }
            };
            state.revision += 1;
        }
        if on {
            state.show(code, editor, com);
        } else {
            state.hide(code);
        }
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

    /// The centers of `editor`'s bricks, of the selected ones only if `selected`.
    ///
    /// # Safety
    /// `editor.editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn centers(&self, editor: &Editor, selected: bool) -> Centers {
        let mut surfaces = Vec::new();
        let mut pushes = Vec::new();
        // SAFETY: per the contract; the editor's bricks are live, and their static info classes.
        unsafe {
            for brick in editor.brick_list() {
                if selected && !self.selection.is_selected(brick) {
                    continue;
                }
                let Some(place) = self.places.of(brick) else {
                    continue;
                };
                let rotation = Quat::from_rotator(math::widen(place.rotation));
                let location = math::widen(place.location);
                let [x, y, z] = place.location;
                let transform = Transform {
                    rotation: [rotation.x, rotation.y, rotation.z, rotation.w].map(|it| it as f32),
                    translation: [x, y, z, 0.0],
                    scale: [1.0, 1.0, 1.0, 0.0],
                };
                // Already in the editor root's space.
                if let Some(surface) = self.lift.of(brick, &transform) {
                    surfaces.push(centers::LiftSurface {
                        location: math::widen(surface.location),
                        normal: math::widen(surface.normal),
                        radius: f64::from(surface.radius),
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
        let found = Centers {
            lift: centers::center_of_lift(&surfaces).map(math::narrow),
            thrust: centers::center_of(&pushes).map(math::narrow),
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
        found
    }
}

impl State {
    /// Hides the markers.
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
    }

    /// Shows the markers that have a center, placed like the game's marker `com` (scale, rotation),
    /// making the missing ones.
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
}

/// The tooltip over the marker under the mouse (the game's center of mass or ours, nearest first):
/// which it is and where, in the editor root's space.
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
    // (`get_live`).
    unsafe {
        let com = editor
            .cast::<u8>()
            .add(code.com)
            .cast::<*mut UObjectBase>()
            .read();
        if com.is_null() || !code.visible.get(com.cast()) {
            return tooltip::Description::default();
        }
        let mut shown = vec![("Center of mass", com, code.meshes.relative_place(com).0)];
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let marker = state.markers[index]
                .as_ref()
                .filter(|marker| marker.visible);
            let component = marker.and_then(|marker| marker.component.get_live());
            if let (Some(component), Some(center)) = (component, state.centers.of(kind)) {
                shown.push((kind.name(), component, center));
            }
        }
        let hit = shown
            .into_iter()
            .filter_map(|(name, component, center)| {
                Some((hover.sphere_hit(component)?, name, center))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, name, [x, y, z])) = hit else {
            return tooltip::Description::default();
        };
        // Shown distances: m (0.01 per cm) or ft (0.0328), as the game shows them.
        let (cm_per_unit, unit) = if code.units.factor(code.distance) > 0.02 {
            (CM_PER_FOOT, "ft")
        } else {
            (CM_PER_METER, "m")
        };
        let mut lines: Vec<String> = [("X", x), ("Y", y), ("Z", z)]
            .iter()
            .map(|&(axis, cm)| format!("{axis}: {} {unit}", exact(cm, cm_per_unit)))
            .collect();
        if state.centers.selected {
            lines.push("Of the selection".to_owned());
        }
        tooltip::Description {
            title: name.to_owned(),
            lines,
        }
    }
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
