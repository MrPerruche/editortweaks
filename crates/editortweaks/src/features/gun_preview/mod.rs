//! Shot preview: selected guns (or all of them, see [`SHOWS`]) show where their shots go, as lines
//! in the 3D view, like the editor's arrows show what other bricks do. The line is the
//! projectile's path from the muzzle (moved out by barrels) until it ends: at its range, at
//! something it can neither go through nor ricochet off, or at the first hit for explosive ammo.
//! It fades from red (full damage) to yellow (least damage) where the damage drops off with
//! distance travelled, and is drawn thicker through what it pierces. Markers facing the camera show
//! where things happen: a diamond where it enters what it goes through (cyan; magenta if that
//! breaks off one of the vehicle's bricks) and a square where it comes out, a green diamond where
//! it ricochets with the cone it leaves in (the game turns it randomly within it), and where it
//! ends an X (white where something stops it, gray at its range) or a star where it explodes. The
//! vehicle's bricks it hits are outlined by the editor's own outline, following their shape
//! (`game::outline_cases`): thick magenta if it breaks them off, thin violet if they hold. Where
//! that outline can't show (the game uses its colors while moving bricks or picking a property, a
//! drag box restyles outlines, coop shows others' selections with it, or the brick is selected and
//! keeps its selection outline) a box is drawn around the brick instead, in the same colors.
//! Tooltips ([`label`]) tell about what the mouse is on: a gun or its barrels, a path, a marker, a
//! brick hit. See [`ballistics`] for how a shot flies and why its path doesn't depend on the frame
//! rate.
//!
//! What it hits: the vehicle's own bricks (see [`HITS_VEHICLE`]), and the world around the vehicle
//! while the map shows (`features::map_in_editor`), as the shot will meet it once the vehicle is
//! spawned at that garage. The game's projectiles leave their own vehicle out
//! (`UGunBrick::GetIgnoredActors`), so the bricks are hit as a shot from another vehicle would hit
//! them: to see what it goes through and which bricks it breaks off (magenta markers: the damage
//! dealt there is at least what breaks them off, as the Strength view mode tells it). Each brick is
//! traced alone against its simple collision, as the projectile sweeps it, hidden ones left
//! out, and the guns firing and the barrels they fire through too. Off the setting, or without
//! the map, the path goes on through.
//!
//! Stacked guns: guns of one kind with the same ammo whose muzzles coincide fire one projectile
//! together, dealing their damage summed; selecting one of them shows the stack's shot. Read
//! from the game as it is (reflection), so modded guns and projectiles show as they fly, and
//! a gun whose numbers can't be read or make no sense is left out (logged once).
//!
//! Worked out again only when a gun, a barrel, the selection, the vehicle (`view_modes::
//! generation`, and the hidden bricks), the map or gravity changed; redrawn when the camera moves
//! (the markers keep their size on screen), and drawn again whenever the line batcher lost them
//! (`LineBatcher::keep`).

mod ballistics;
mod describe;
mod hover;
mod label;
mod projection;

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use self::ballistics::{Contact, Ending, Flight, Frame, Ricochet, Shot};
use super::{drag_select, map_in_editor, view_modes};
use crate::framework::settings::{Alternative, Choice, Toggle};
use crate::game::brick_connections::{self, Connections};
use crate::game::brick_editor;
use crate::game::collision::Collision;
use crate::game::component_bounds::{self, ComponentBounds};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{self, LocalBounds, Selection, Transform, WorldTransforms};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::gravity::{self, Gravity};
use crate::game::guns::{
    self, Connector, FireRate, GunInfo, Guns, ProjectileInfo, ProjectileParams,
};
use crate::game::line_batcher::{self, Line, LineBatcher};
use crate::game::line_trace::{self, ComponentTrace, ProfileTrace};
use crate::game::outline_cases::{self, CASES, Highlights, OutlineCases};
use crate::game::penetration::{self, Densities};
use crate::game::reflection;
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat, Vec3};
use crate::prelude::*;

pub struct GunPreview;

/// Which guns show their shots.
pub static SHOWS: Choice = Choice::new(
    "ShotPreview",
    "Shot preview",
    "Shows the path of shots from selected guns (and those stacked with them) or from every gun. With the map shown, shots stop at what they hit.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "Selected",
            name: "Selected guns",
        },
        Alternative {
            key: "All",
            name: "All guns",
        },
    ],
    1,
);
const SELECTED: u8 = 1;
const ALL: u8 = 2;

/// Whether shots hit the vehicle's own bricks.
pub static HITS_VEHICLE: Toggle = Toggle::new(
    "ShotPreviewHitsVehicle",
    "Shot preview hits this vehicle",
    "On: previewed shots hit this vehicle's own bricks as another vehicle's would, showing what they go through and break off (magenta). Off: they pass through, as in game.",
    true,
);

/// Whether a shot preview tooltip shows (the view modes' tooltip then doesn't).
pub fn tooltip_shows() -> bool {
    label::shows()
}

/// The colors, free to change (linear RGB): the path at full damage and at the least, what it goes
/// through, where it ricochets, a brick of the vehicle it breaks off and one it hits that holds,
/// where it stops, where it explodes, and where it runs out of range. Where its damage stops
/// dropping is marked in the least damage's color.
const FULL_DAMAGE: [f32; 3] = [1.0, 0.05, 0.02];
const LEAST_DAMAGE: [f32; 3] = [1.0, 0.75, 0.05];
const THROUGH: [f32; 3] = [0.1, 0.8, 1.0];
const RICOCHET: [f32; 3] = [0.2, 1.0, 0.3];
const BREAKS: [f32; 3] = [1.0, 0.1, 0.9];
const HOLDS: [f32; 3] = [0.55, 0.35, 1.0];
const STOP: [f32; 3] = [1.0, 1.0, 1.0];
const EXPLODES: [f32; 3] = [1.0, 0.4, 0.0];
const OUT_OF_RANGE: [f32; 3] = [0.35, 0.35, 0.35];
/// The dark halo under markers and ricochet cones, so they show on any background: its color, how
/// far it reaches past the lines on each side (a share of the distance from the camera: about a
/// pixel and a half), and how much further from the camera it's drawn (a share of that distance),
/// so the lines show in front of it.
const HALO: [f32; 3] = [0.01, 0.01, 0.01];
const HALO_WIDTH: f64 = 0.0013;
const HALO_BEHIND: f64 = 0.002;
/// How much nearer the camera each [`Layer`] of markers is drawn than the one under it (a share of
/// the distance, more than [`HALO_BEHIND`] so a marker's halo is in front of the markers under it).
const LAYER_LIFT: f64 = 0.005;
/// A marker's half size and the width of its lines, as shares of its distance from the camera
/// (about a dozen pixels and two on a 1080p screen).
const MARKER_SIZE: f64 = 0.012;
const MARKER_WIDTH: f64 = 0.0018;
/// How long a ricochet's cone is drawn, in marker sizes, and how many lines go around it.
const CONE_LENGTH: f64 = 4.0;
const CONE_LINES: usize = 12;
/// The path's width through what it pierces, as a share of its distance from the camera (a few
/// pixels); elsewhere it's a pixel wide.
const PIERCING_WIDTH: f64 = 0.005;
/// How far outside a hit brick's box its box is drawn, in cm (where the editor's outline can't
/// show it).
const OUTLINE_MARGIN: f64 = 0.5;
/// The editor outline's free cases the hit bricks get (`game::outline_cases`): the first is drawn
/// thicker than the last, so the bricks broken off stand out.
const BREAKS_CASE: usize = 0;
const HOLDS_CASE: usize = CASES - 1;
/// Path steps: about this share of the shot's range, but no shorter than [`MIN_STEP`] cm.
const STEPS: f64 = 200.0;
const MIN_STEP: f64 = 1000.0;
/// How far the camera moves (cm, or share of its distance to the vehicle) before markers are
/// redrawn.
const CAMERA_MOVE: f64 = 1.0;
/// The lines' owner in the line batcher.
const OWNER: &str = "gun_preview";
/// How much bigger than a brick's box the line must meet, in cm: the box is the engine's, rounded.
const BOX_MARGIN: f64 = 1.0;
/// How many things a trace through the world looks past at most (things gone through, the
/// editor's).
const MAX_SKIPPED: usize = 16;

/// What the feature needs from the game (after engine init).
struct Code {
    bricks: Bricks,
    editor: EditorCode,
    selection: Selection,
    transforms: WorldTransforms,
    guns: Guns,
    gravity: Gravity,
    lines: LineBatcher,
    /// What shots hit in the world; without it, paths go to their range (logged).
    world: Option<WorldCode>,
    /// What shots hit of the vehicle; without it, they pass through it (logged).
    vehicle: Option<VehicleCode>,
    /// The editor's outline for the bricks hit; without it, they get boxes (logged).
    outline: Option<OutlineCode>,
    /// `EAmmoType` values that explode on impact, and those that steer (shown unguided).
    explosive: Vec<u8>,
    homing: Vec<u8>,
    /// `EAmmoType` names by value.
    ammo_names: Vec<(u8, String)>,
}

/// What tracing shots through the world needs.
struct WorldCode {
    trace: ProfileTrace,
    component: ComponentTrace,
    bounds: ComponentBounds,
    densities: Densities,
}

/// What tracing shots through the vehicle's own bricks needs.
struct VehicleCode {
    collision: Collision,
    component: ComponentTrace,
    bounds: ComponentBounds,
    connections: &'static Connections,
    densities: Densities,
    /// Each brick's own box and place, for outlining those hit.
    local_bounds: LocalBounds,
    transforms: WorldTransforms,
    /// `VehicleEditor`: the editor whose bricks' strength can be told.
    vehicle_editor: *const UStruct,
}

/// What outlining the bricks hit with the editor's own outline needs: its free cases, and the
/// editor's modes (they're free in its default mode only).
struct OutlineCode {
    cases: OutlineCases,
    modes: EditorModes,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

static CODE: OnceLock<Code> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());
/// Logged once each: guns left out, projectiles without a collision profile, and bricks whose
/// strength can't be told.
static SKIPPED_GUN: AtomicBool = AtomicBool::new(false);
static NO_PROFILE: AtomicBool = AtomicBool::new(false);
static NO_STRENGTH: AtomicBool = AtomicBool::new(false);

/// The preview between frames.
struct State {
    editor: Option<WeakObject>,
    /// What the shots were worked out for.
    key: Option<u64>,
    shots: Vec<Preview>,
    /// The vehicle's bricks as measured, and the [`Found::vehicle`] they were measured for.
    vehicle: Option<(u64, Vehicle)>,
    /// The camera location the lines were drawn for, if drawn.
    drawn_for: Option<Vec3>,
    label: label::State,
    /// The vehicle's bricks the shots hit.
    hits: Vec<Hit>,
    /// The editor's outline in our colors, once used.
    highlights: Option<Highlights>,
    /// The bricks hit that the editor's outline shows (the others get boxes), as last drawn.
    outlined: Vec<usize>,
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for State {}

impl State {
    const fn new() -> Self {
        Self {
            editor: None,
            key: None,
            shots: Vec::new(),
            vehicle: None,
            drawn_for: None,
            label: label::State::new(),
            hits: Vec::new(),
            highlights: None,
            outlined: Vec::new(),
        }
    }
}

/// A brick of the vehicle the shots hit: its address, the brick, whether a shot breaks it off,
/// and the component the editor's outline shows of it.
struct Hit {
    brick: usize,
    object: WeakObject,
    breaks: bool,
    component: Option<WeakObject>,
}

/// The vehicle's bricks shots can hit.
struct Vehicle {
    targets: Vec<Target>,
    /// The box around them all.
    bounds: Option<[Vec3; 2]>,
    /// The damage each brick (by address) needs to break off; empty if that can't be told.
    break_off: HashMap<usize, f32>,
}

/// A brick shots can hit: its component, the box around it, its material's density, and its own
/// box where it is (to outline it), if it has one.
struct Target {
    brick: usize,
    component: WeakObject,
    bounds: [Vec3; 2],
    density: f64,
    outline: Option<Outline>,
}

/// A brick's own box where it is: the box in its space, and its place (not scaled).
#[derive(Clone, Copy)]
struct Outline {
    bounds: [Vec3; 2],
    location: Vec3,
    rotation: Quat,
}

/// One shot shown: a gun's, or a stack's.
pub struct Preview {
    pub flight: Flight,
    /// For each of the flight's impacts: whether it breaks off the vehicle's brick it hits.
    pub breaks: Vec<bool>,
    /// The params it flies with (barrels counted) and one projectile's damage at the end.
    pub params: ProjectileParams,
    pub info: GunInfo,
    pub projectile: ProjectileInfo,
    /// Guns stacked (one projectile each, merged).
    pub stack: usize,
    /// The length of barrel in front of its muzzle (cm).
    pub barrel_length: f64,
    pub spread_radius: f64,
    pub ammo: String,
    pub explosive: bool,
    pub homing: bool,
    /// How fast its guns fire.
    pub fire: FireRate,
    /// The stack's guns, and the barrels they fire through (addresses).
    pub guns: Vec<usize>,
    pub barrels: Vec<usize>,
}

impl Preview {
    /// Where its damage stops dropping (`DropoffEnd`), if it drops and the shot gets there.
    pub fn weakest(&self) -> Option<ballistics::Point> {
        let params = &self.params;
        let end = params.dropoff_end;
        let drops = params.damage_at(end) < params.damage_at(0.0) * 0.999;
        let reached = self.flight.points.last()?.range;
        (drops && end > 0.0 && end < reached).then(|| self.flight.point_at(end))
    }
}

impl Feature for GunPreview {
    fn id(&self) -> &'static str {
        "gun_preview"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match find() {
            Ok(code) => {
                if code.world.is_none() {
                    log!("shots won't stop at what they hit in the world");
                }
                let _ = CODE.set(code);
                label::find();
                log!("active");
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn find() -> Result<Code, String> {
    let world = (|| {
        Ok::<_, String>(WorldCode {
            trace: line_trace::profile_trace()?,
            component: line_trace::component_trace()?,
            bounds: component_bounds::find()?,
            densities: penetration::find()?,
        })
    })()
    .map_err(|reason| log!("no tracing through the world: {reason}"))
    .ok();
    let vehicle = (|| {
        Ok::<_, String>(VehicleCode {
            collision: Collision::find()?,
            component: line_trace::component_trace()?,
            bounds: component_bounds::find()?,
            connections: brick_connections::find()?,
            densities: penetration::find()?,
            local_bounds: LocalBounds::find()?,
            transforms: WorldTransforms::find()?,
            vehicle_editor: reflection::find_class("VehicleEditor")
                .ok_or("class VehicleEditor not found")?
                .cast(),
        })
    })()
    .map_err(|reason| log!("shots pass through the vehicle: {reason}"))
    .ok();
    let outline = (|| {
        Ok::<_, String>(OutlineCode {
            cases: outline_cases::find()?,
            modes: editor_mode::find()?,
        })
    })()
    .map_err(|reason| log!("bricks hit get boxes, not the editor's outline: {reason}"))
    .ok();
    let ammo = |name: &str| {
        reflection::enum_value("EAmmoType", &format!("EAmmoType::{name}"))
            .ok()
            .and_then(|value| u8::try_from(value).ok())
    };
    let names = [
        "Default",
        "Incendiary",
        "HighExplosive",
        "TargetSeeking",
        "Guided",
        "Flare",
    ];
    let ammo_names = names
        .iter()
        .filter_map(|&name| Some((ammo(name)?, name.to_owned())))
        .collect();
    Ok(Code {
        bricks: editor_view::bricks()?,
        editor: editor_view::editor_code()?,
        selection: editor_object::selection()?,
        transforms: WorldTransforms::find()?,
        guns: guns::find()?,
        gravity: gravity::find()?,
        lines: line_batcher::find()?,
        world,
        vehicle,
        outline,
        explosive: ["Incendiary", "HighExplosive", "TargetSeeking", "Guided"]
            .iter()
            .filter_map(|name| ammo(name))
            .collect(),
        homing: ["TargetSeeking", "Guided"]
            .iter()
            .filter_map(|name| ammo(name))
            .collect(),
        ammo_names,
    })
}

/// A gun brick in the editor, as this frame finds it.
struct GunBrick {
    object: *mut UObjectBase,
    info: *const u8,
    ammo: u8,
    selected: bool,
    /// Its own muzzle connector (the first) in the world, and that connector's kind.
    muzzle: Option<(Frame, u8)>,
}

/// A barrel brick, its connectors in the world with their kinds, and whether it's hidden.
struct BarrelBrick {
    object: *mut UObjectBase,
    connectors: Vec<(Frame, u8)>,
    hidden: bool,
}

/// What this frame finds in the editor.
struct Found {
    guns: Vec<GunBrick>,
    barrels: Vec<BarrelBrick>,
    /// The bricks shown (not hidden).
    bricks: Vec<*mut UObjectBase>,
    /// What the vehicle's bricks as shots meet them depend on, hashed: which bricks show, the
    /// editor's changes, and where the editor is (showing the map moves it into the map).
    vehicle: u64,
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(code) = CODE.get() else { return };
    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let state = &mut *guard;
    if !state.editor.as_ref().is_some_and(|known| known.is(editor)) {
        // A new editor: the old one's lines and outlines went with its world.
        let mut highlights = state.highlights.take();
        if let Some(highlights) = &mut highlights {
            highlights.forget();
        }
        *state = State {
            // SAFETY: the live editor, ticking.
            editor: Some(unsafe { WeakObject::new(editor) }),
            label: core::mem::replace(&mut state.label, label::State::new()),
            highlights,
            ..State::new()
        };
    }
    // SAFETY: the live editor, on the game thread.
    unsafe { tick(code, editor, state) };
}

/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn tick(code: &Code, editor: *mut UObjectBase, state: &mut State) {
    let shows = SHOWS.get();
    if shows != SELECTED && shows != ALL {
        // SAFETY: per the contract.
        unsafe { clear(code, state) };
        return;
    }
    // SAFETY: per the contract.
    let found = unsafe { find_bricks(code, editor) };
    let wanted: Vec<usize> = (0..found.guns.len())
        .filter(|&index| shows == ALL || found.guns[index].selected)
        .collect();
    if wanted.is_empty() {
        // SAFETY: per the contract.
        unsafe { clear(code, state) };
        return;
    }
    let map = map_in_editor::shows_map() && code.world.is_some();
    let hits_vehicle = HITS_VEHICLE.get() && code.vehicle.is_some();
    // SAFETY: per the contract.
    let gravity = unsafe { code.gravity.z() };
    let key = key(&found, &wanted, map, hits_vehicle, gravity);
    if state.key != Some(key) {
        state.key = Some(key);
        let vehicle = match &code.vehicle {
            Some(vehicle_code) if hits_vehicle => {
                if state
                    .vehicle
                    .as_ref()
                    .is_none_or(|(measured, _)| *measured != found.vehicle)
                {
                    // SAFETY: per the contract; the bricks are the editor's, live.
                    let measured = unsafe { measure(vehicle_code, editor, &found.bricks) };
                    state.vehicle = Some((found.vehicle, measured));
                }
                state
                    .vehicle
                    .as_ref()
                    .map(|(_, vehicle)| (vehicle_code, vehicle))
            }
            _ => None,
        };
        // SAFETY: per the contract.
        state.shots = unsafe { work_out(code, editor, &found, &wanted, vehicle, map, gravity) };
        // SAFETY: per the contract; the bricks are the editor's, live.
        state.hits = unsafe { hits(code, &state.shots, &found.bricks) };
        state.drawn_for = None;
    }
    // SAFETY: per the contract.
    let outlined = unsafe { outline_hits(code, editor, state) };
    if outlined != state.outlined {
        // Boxes for the others.
        state.outlined = outlined;
        state.drawn_for = None;
    }
    // SAFETY: per the contract.
    let camera = unsafe { label::camera(editor) };
    let moved = match (state.drawn_for, camera) {
        (Some(drawn), Some(camera)) => {
            let to_vehicle = state
                .shots
                .first()
                .and_then(|shot| shot.flight.points.first())
                .map_or(1.0, |start| math::length(math::sub(start.at, camera)));
            math::length(math::sub(drawn, camera)) > CAMERA_MOVE.max(to_vehicle * 0.01)
        }
        (None, _) => true,
        (Some(_), None) => false,
    };
    if moved {
        let vehicle = state.vehicle.as_ref().map(|(_, vehicle)| vehicle);
        let boxed: HashMap<usize, bool> = state
            .hits
            .iter()
            .filter(|hit| !state.outlined.contains(&hit.brick))
            .map(|hit| (hit.brick, hit.breaks))
            .collect();
        // SAFETY: per the contract.
        unsafe { draw(code, &state.shots, vehicle, &boxed, camera) };
        state.drawn_for = Some(camera.unwrap_or_default());
    } else {
        // SAFETY: per the contract; the batcher is the current world's, or null.
        unsafe {
            let batcher = code.lines.batcher();
            if !batcher.is_null() {
                code.lines.keep(batcher);
            }
        }
    }
    if view_modes::shot_preview_shows() {
        let vehicle = state.vehicle.as_ref().map(|(_, vehicle)| vehicle);
        let hidden = |object: *mut UObjectBase| {
            // SAFETY: one of the editor's objects, live.
            unsafe { code.editor.is_hidden(object) }
        };
        // SAFETY: per the contract.
        unsafe {
            let key = state.key.unwrap_or_default();
            state
                .label
                .update(editor, &state.shots, key, vehicle, hidden);
        }
    } else {
        // SAFETY: per the contract.
        unsafe { state.label.hide() };
    }
}

/// Takes the lines and tooltip away, if shown.
///
/// # Safety
/// Game thread only.
unsafe fn clear(code: &Code, state: &mut State) {
    state.key = None;
    state.shots.clear();
    state.hits.clear();
    state.outlined.clear();
    if let (Some(outline), Some(highlights)) = (&code.outline, &mut state.highlights) {
        // SAFETY: per the contract.
        unsafe { highlights.hide(&outline.cases) };
    }
    if state.drawn_for.take().is_some() {
        // SAFETY: per the contract; the batcher is the current world's, or null.
        unsafe {
            let batcher = code.lines.batcher();
            if !batcher.is_null() {
                code.lines.replace(batcher, OWNER, &[]);
            }
        }
    }
    // SAFETY: per the contract.
    unsafe { state.label.hide() };
}

/// The editor's visible bricks, and among them its guns and barrels (hidden barrels too: they cut
/// off the barrels beyond them).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn find_bricks(code: &Code, editor: *mut UObjectBase) -> Found {
    let mut found = Found {
        guns: Vec::new(),
        barrels: Vec::new(),
        bricks: Vec::new(),
        vehicle: 0,
    };
    let mut vehicle = DefaultHasher::new();
    view_modes::generation().hash(&mut vehicle);
    // SAFETY: per the contract; the editor's objects are live (or null), and copied out before
    // calling into the game.
    unsafe {
        let objects = code.bricks.editor_objects(editor).to_vec();
        for object in objects {
            if object.is_null() || !code.bricks.is_brick(object) {
                continue;
            }
            let hidden = code.editor.is_hidden(object);
            if !hidden {
                if found.bricks.is_empty() {
                    // The bricks move together when the editor moves: the first one tells.
                    let placed = code.transforms.of(object);
                    for value in placed.translation.iter().chain(&placed.rotation) {
                        value.to_bits().hash(&mut vehicle);
                    }
                }
                found.bricks.push(object);
                (object as usize).hash(&mut vehicle);
            }
            let is_gun = code.guns.is_gun(object);
            // Hidden guns don't fire; hidden barrels cut off those beyond them.
            let wanted = if is_gun {
                !hidden
            } else {
                code.guns.is_barrel(object)
            };
            if !wanted {
                continue;
            }
            let info = code.editor.static_info(object);
            let transform = code.transforms.of(object);
            let connectors = code.guns.connectors(info);
            let in_world = |connector: &Connector| (place(&transform, connector), connector.kind);
            if is_gun {
                found.guns.push(GunBrick {
                    object,
                    info,
                    ammo: code.guns.ammo_type(object),
                    selected: code.selection.is_selected(object),
                    muzzle: connectors.first().map(in_world),
                });
            } else {
                found.barrels.push(BarrelBrick {
                    object,
                    connectors: connectors.iter().map(in_world).collect(),
                    hidden,
                });
            }
        }
    }
    found.vehicle = vehicle.finish();
    found
}

/// A connector of a brick at `transform`, in the world.
fn place(transform: &Transform, connector: &Connector) -> Frame {
    let rotation = Quat::widen(transform.rotation);
    let [x, y, z, _] = transform.translation;
    let [sx, sy, sz, _] = transform.scale;
    let scaled = [
        connector.origin[0] * f64::from(sx),
        connector.origin[1] * f64::from(sy),
        connector.origin[2] * f64::from(sz),
    ];
    let axis = rotation.rotate(connector.axis);
    Frame {
        origin: math::add(math::widen([x, y, z]), rotation.rotate(scaled)),
        axis: math::normalized(axis).unwrap_or(axis),
    }
}

/// What the shots depend on, hashed.
fn key(found: &Found, wanted: &[usize], map: bool, hits_vehicle: bool, gravity: f64) -> u64 {
    let mut hasher = DefaultHasher::new();
    let frame = |hasher: &mut DefaultHasher, frame: &Frame| {
        for value in frame.origin.iter().chain(&frame.axis) {
            value.to_bits().hash(hasher);
        }
    };
    for gun in &found.guns {
        (gun.object as usize, gun.info as usize, gun.ammo).hash(&mut hasher);
        if let Some((muzzle, kind)) = &gun.muzzle {
            frame(&mut hasher, muzzle);
            kind.hash(&mut hasher);
        }
    }
    wanted.hash(&mut hasher);
    for barrel in &found.barrels {
        barrel.hidden.hash(&mut hasher);
        for (connector, kind) in &barrel.connectors {
            frame(&mut hasher, connector);
            kind.hash(&mut hasher);
        }
    }
    hits_vehicle.then_some(found.vehicle).hash(&mut hasher);
    (map, gravity.to_bits(), SHOWS.get()).hash(&mut hasher);
    hasher.finish()
}

/// The vehicle's `bricks` as shots meet them.
///
/// # Safety
/// `editor` must be the live editor, `bricks` its bricks. Game thread only.
unsafe fn measure(
    code: &VehicleCode,
    editor: *mut UObjectBase,
    bricks: &[*mut UObjectBase],
) -> Vehicle {
    // SAFETY: per the contract; the components are the bricks' own, live.
    unsafe {
        let targets: Vec<Target> = bricks
            .iter()
            .filter_map(|&brick| {
                let component = code.collision.component_of(brick)?;
                let [low, high] = code.bounds.of(component);
                let material = code.connections.physical_material(brick);
                let outline = code.local_bounds.of(brick).map(|bounds| {
                    let transform = code.transforms.of(brick);
                    let [x, y, z, _] = transform.translation;
                    Outline {
                        bounds: bounds.map(math::widen),
                        location: math::widen([x, y, z]),
                        rotation: Quat::widen(transform.rotation),
                    }
                });
                Some(Target {
                    brick: brick as usize,
                    component: WeakObject::new(component),
                    bounds: [math::widen(low), math::widen(high)],
                    density: f64::from(code.densities.density(material.cast_const())),
                    outline,
                })
            })
            .collect();
        let bounds = targets.iter().map(|target| target.bounds).reduce(
            |[low, high], [other_low, other_high]| {
                [
                    core::array::from_fn(|axis| low[axis].min(other_low[axis])),
                    core::array::from_fn(|axis| high[axis].max(other_high[axis])),
                ]
            },
        );
        let break_off = if reflection::is_a(editor, code.vehicle_editor) {
            view_modes::break_off_damage(editor, bricks)
                .map_err(|reason| {
                    if !NO_STRENGTH.swap(true, Ordering::Relaxed) {
                        log!("the bricks shots break off aren't told: {reason}");
                    }
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        Vehicle {
            targets,
            bounds,
            break_off,
        }
    }
}

/// Works out the shots of the `wanted` guns (indices in `found.guns`), one per stack, hitting
/// `vehicle`'s bricks if given.
///
/// # Safety
/// `editor` must be the live editor, `found` what this frame found in it. Game thread only.
unsafe fn work_out(
    code: &Code,
    editor: *mut UObjectBase,
    found: &Found,
    wanted: &[usize],
    vehicle: Option<(&VehicleCode, &Vehicle)>,
    map: bool,
    gravity: f64,
) -> Vec<Preview> {
    let guns = &found.guns;
    let mut shown: HashSet<usize> = HashSet::new();
    let mut shots = Vec::new();
    for &index in wanted {
        if shown.contains(&index) {
            continue;
        }
        let Some(gun) = guns.get(index) else { continue };
        // SAFETY: the static info is the live gun's.
        let infos = unsafe {
            code.guns
                .gun_info(gun.info)
                .zip(code.guns.projectile_info(gun.info))
        };
        let (Some((info, projectile)), Some((own_muzzle, kind))) = (infos, gun.muzzle) else {
            if !SKIPPED_GUN.swap(true, Ordering::Relaxed) {
                log!(
                    "a gun's shots can't be worked out (its numbers, projectile or muzzle can't be read): left out"
                );
            }
            continue;
        };
        // The guns stacked with it: one projectile together.
        let stack: Vec<usize> = (0..guns.len())
            .filter(|&other| {
                let other_gun = &guns[other];
                other_gun.info == gun.info
                    && other_gun.ammo == gun.ammo
                    && other_gun
                        .muzzle
                        .is_some_and(|(frame, _)| ballistics::coincide(frame, own_muzzle))
            })
            .collect();
        shown.extend(&stack);
        let in_line = |barrel: &BarrelBrick| {
            barrel.connectors.iter().any(|&(frame, barrel_kind)| {
                barrel_kind == kind && ballistics::in_line(own_muzzle, frame).is_some()
            })
        };
        let connectors = |hidden: bool| {
            found
                .barrels
                .iter()
                .filter(move |barrel| barrel.hidden == hidden)
                .flat_map(|barrel| &barrel.connectors)
                .filter(|(_, barrel_kind)| *barrel_kind == kind)
                .map(|&(frame, _)| frame)
        };
        let muzzle = ballistics::muzzle(own_muzzle, connectors(false), connectors(true));
        // What fires it, which it doesn't hit: the stack's guns and the barrels in front of them.
        let stack_guns: Vec<usize> = stack
            .iter()
            .map(|&other| guns[other].object as usize)
            .collect();
        let barrels: Vec<usize> = found
            .barrels
            .iter()
            .filter(|barrel| !barrel.hidden && in_line(barrel))
            .map(|barrel| barrel.object as usize)
            .collect();
        let firing: Vec<usize> = stack_guns.iter().chain(&barrels).copied().collect();
        let share = ballistics::barrel_share(&info, muzzle.barrel_length);
        let (params, spread_radius) = ballistics::scaled(&info, share);
        let mut speed = params.initial_speed;
        if projectile.max_speed > 0.0 {
            speed = speed.min(projectile.max_speed);
        }
        let explosive = code.explosive.contains(&gun.ammo);
        let shot = Shot {
            start: muzzle.frame.origin,
            velocity: math::scale(muzzle.frame.axis, speed),
            gravity: gravity * projectile.gravity_scale,
            params,
            stack: stack.len().max(1) as f64,
            penetration_coefficient: projectile.penetration_coefficient,
            explodes: explosive,
            bounces: projectile.bounces,
            bounce_stop_speed: projectile.bounce_stop_speed,
        };
        let step = (params.max_range / STEPS).max(MIN_STEP);
        let profile = projectile.profile;
        if map && profile.is_none() && !NO_PROFILE.swap(true, Ordering::Relaxed) {
            log!(
                "a projectile has no collision profile: its shots aren't traced through the world"
            );
        }
        let world = code.world.as_ref().filter(|_| map).zip(profile);
        let flight = ballistics::fly(&shot, step, |from, to, skip| {
            // SAFETY: per the contract; the vehicle was measured for the editor as it is.
            let own = vehicle.and_then(|(vehicle_code, vehicle)| unsafe {
                vehicle_contact(vehicle_code, vehicle, from, to, skip, &firing)
            });
            // SAFETY: per the contract; the editor is a live actor in the world traced.
            let around = world.and_then(|(world, profile)| unsafe {
                contact(world, editor, from, to, profile, skip)
            });
            match (own, around) {
                (Some(own), Some(around)) if around.distance < own.distance => Some(around),
                (own, around) => own.or(around),
            }
        });
        let breaks = flight
            .impacts
            .iter()
            .map(|impact| {
                let needed =
                    vehicle.and_then(|(_, vehicle)| vehicle.break_off.get(&impact.brick?).copied());
                needed.is_some_and(|needed| impact.damage >= f64::from(needed))
            })
            .collect();
        shots.push(Preview {
            flight,
            breaks,
            params,
            info,
            projectile,
            stack: stack.len().max(1),
            barrel_length: muzzle.barrel_length,
            spread_radius,
            ammo: code
                .ammo_names
                .iter()
                .find(|(value, _)| *value == gun.ammo)
                .map_or_else(|| format!("ammo {}", gun.ammo), |(_, name)| name.clone()),
            explosive,
            homing: code.homing.contains(&gun.ammo),
            // SAFETY: the static info is the live gun's.
            fire: unsafe { code.guns.fire_rate(gun.info) },
            guns: stack_guns,
            barrels,
        });
    }
    shots
}

/// What the line from `from` to `to` first meets of the vehicle's bricks, leaving out those gone
/// through (`skip`, by component) and those `firing` (by address); and how thick it is along the
/// line. Each brick whose box the line meets is traced alone: where the line enters it, and back
/// from beyond it for where it last leaves it (none found: too thick, as the game takes it). Against its simple collision, as the game's
/// projectile sweeps it and measures it (not the mesh the editor's clicks and the view modes'
/// tooltip trace: its bevelled edges and rounded faces turn the surface's normal, so ricochets).
///
/// # Safety
/// `vehicle` must have been measured for the editor as it is. Game thread only.
unsafe fn vehicle_contact(
    code: &VehicleCode,
    vehicle: &Vehicle,
    from: Vec3,
    to: Vec3,
    skip: &[usize],
    firing: &[usize],
) -> Option<Contact> {
    if !vehicle.bounds.is_some_and(|bounds| meets(from, to, bounds)) {
        return None;
    }
    let length = math::length(math::sub(to, from));
    let direction = math::normalized(math::sub(to, from))?;
    let mut nearest: Option<(f64, &Target, *mut UObjectBase, Vec3)> = None;
    for target in &vehicle.targets {
        if firing.contains(&target.brick) || !meets(from, to, target.bounds) {
            continue;
        }
        let Some(component) = target.component.get_live() else {
            continue;
        };
        if skip.contains(&(component as usize)) {
            continue;
        }
        // SAFETY: a live component (just checked).
        let entry = unsafe {
            code.component
                .surface_hit(component, math::narrow(from), math::narrow(to), false)
        };
        let Some(entry) = entry else { continue };
        let distance =
            math::dot(math::sub(math::widen(entry.location), from), direction).clamp(0.0, length);
        if nearest.is_none_or(|(best, ..)| distance < best) {
            nearest = Some((distance, target, component, math::widen(entry.normal)));
        }
    }
    let (distance, target, component, normal) = nearest?;
    let entry = math::add(from, math::scale(direction, distance));
    let [low, high] = target.bounds;
    let across = math::length(math::sub(high, low));
    let beyond = math::add(entry, math::scale(direction, across + 1.0));
    // SAFETY: a live component (checked above, nothing ran since).
    let exit = unsafe {
        code.component
            .first_hit_with(component, math::narrow(beyond), math::narrow(entry), false)
    };
    Some(Contact {
        distance,
        normal,
        what: component as usize,
        brick: Some(target.brick),
        density: target.density,
        thickness: exit.map_or(f64::INFINITY, |exit| {
            math::dot(math::sub(math::widen(exit), entry), direction).max(0.0)
        }),
    })
}

/// Whether the segment from `from` to `to` meets the box `[low, high]`, grown by [`BOX_MARGIN`].
fn meets(from: Vec3, to: Vec3, [low, high]: [Vec3; 2]) -> bool {
    let (mut near, mut far) = (0.0_f64, 1.0_f64);
    for axis in 0..3 {
        let (low, high) = (low[axis] - BOX_MARGIN, high[axis] + BOX_MARGIN);
        let (origin, direction) = (from[axis], to[axis] - from[axis]);
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

/// What the line from `from` to `to` first meets in the world, as a projectile with the collision
/// profile `profile` meets it (the editor and its bricks left out, and what was gone through:
/// `skip`, by component), and how thick it is along the line: traced back against it alone from
/// beyond it, as the game measures it (no far side found: too thick, as the game takes it; a
/// landscape has none). What has no component is passed through, as the game passes through what
/// has no body.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn contact(
    world: &WorldCode,
    editor: *mut UObjectBase,
    from: Vec3,
    to: Vec3,
    profile: [u8; 8],
    skip: &[usize],
) -> Option<Contact> {
    let direction = math::normalized(math::sub(to, from))?;
    let length = math::length(math::sub(to, from));
    let mut start = 0.0;
    for _ in 0..MAX_SKIPPED {
        // SAFETY: per the contract; the component and material are live (just resolved).
        unsafe {
            let hit = world.trace.first_hit(
                editor,
                math::narrow(math::add(from, math::scale(direction, start))),
                math::narrow(to),
                profile,
            )?;
            let distance = start + f64::from(hit.distance);
            let passed = hit.component.is_some_and(|component| {
                skip.contains(&(component as usize)) || line_trace::is_editor_component(component)
            });
            if passed {
                // On past it (the trace starts outside it again soon enough: 1 cm on).
                start = distance + 1.0;
                if start >= length {
                    return None;
                }
                continue;
            }
            let entry = math::add(from, math::scale(direction, distance));
            let thickness = hit.component.map_or(0.0, |component| {
                let [low, high] = world.bounds.of(component);
                let across = math::length(math::sub(math::widen(high), math::widen(low)));
                let beyond = math::add(entry, math::scale(direction, across + 1.0));
                world
                    .component
                    .first_hit_with(component, math::narrow(beyond), math::narrow(entry), false)
                    .map_or(f64::INFINITY, |exit| {
                        math::dot(math::sub(math::widen(exit), entry), direction).max(0.0)
                    })
            });
            return Some(Contact {
                distance,
                normal: math::widen(hit.normal),
                what: hit.component.map_or(0, |component| component as usize),
                brick: None,
                density: f64::from(
                    world.densities.density(
                        hit.material
                            .map_or(core::ptr::null(), |material| material.cast_const()),
                    ),
                ),
                thickness,
            });
        }
    }
    None
}

/// The bricks of the vehicle (among `bricks`, those shown) that `shots` hit, in `bricks`' order:
/// whether any shot breaks them off.
///
/// # Safety
/// `bricks` must be live editor objects. Game thread only.
unsafe fn hits(code: &Code, shots: &[Preview], bricks: &[*mut UObjectBase]) -> Vec<Hit> {
    let mut breaks: HashMap<usize, bool> = HashMap::new();
    for shot in shots {
        for (impact, &broken) in shot.flight.impacts.iter().zip(&shot.breaks) {
            if let Some(brick) = impact.brick {
                *breaks.entry(brick).or_default() |= broken;
            }
        }
    }
    bricks
        .iter()
        .filter_map(|&object| {
            let breaks = *breaks.get(&(object as usize))?;
            // SAFETY: per the contract; the component is the live brick's.
            unsafe {
                let component = code
                    .outline
                    .as_ref()
                    .and_then(|outline| outline.cases.outlines().component_of(object));
                Some(Hit {
                    brick: object as usize,
                    object: WeakObject::new(object),
                    breaks,
                    component: component.map(|component| WeakObject::new(component)),
                })
            }
        })
        .collect()
}

/// Outlines the bricks hit with the editor's own outline, in our colors, where it can show them;
/// returns those it shows (the others get boxes). It can't while the editor isn't in its default
/// mode or a drag box restyles outlines, nor on selected bricks (they keep their selection
/// outline); and gives way when the game or coop uses its free cases (`game::outline_cases`).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn outline_hits(code: &Code, editor: *mut UObjectBase, state: &mut State) -> Vec<usize> {
    let Some(outline) = &code.outline else {
        return Vec::new();
    };
    let highlights = state
        .highlights
        .get_or_insert_with(|| Highlights::new(OWNER, 0));
    // SAFETY: per the contract.
    let free = unsafe { outline.modes.is_editing(editor) } && !drag_select::dragging();
    let mut wanted = Vec::new();
    let mut outlined = Vec::new();
    for hit in state.hits.iter().filter(|_| free) {
        let (Some(object), Some(component)) = (
            hit.object.get_live(),
            hit.component.and_then(|component| component.get_live()),
        ) else {
            continue;
        };
        // SAFETY: a live editor object (just checked).
        if unsafe { code.selection.is_selected(object) } {
            continue;
        }
        let case = if hit.breaks { BREAKS_CASE } else { HOLDS_CASE };
        wanted.push((component, case));
        outlined.push(hit.brick);
    }
    let mut colors = [None; CASES];
    colors[BREAKS_CASE] = Some(BREAKS.map(srgb));
    colors[HOLDS_CASE] = Some(HOLDS.map(srgb));
    // SAFETY: per the contract; the components are live (just checked, nothing ran since).
    let shown = unsafe { highlights.show(&outline.cases, editor, &wanted, colors) };
    if shown { outlined } else { Vec::new() }
}

/// A linear color channel in sRGB (the editor's outline is drawn after tone mapping).
fn srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Draws the shots, their markers and widths sized for a camera at `camera`, and boxes around the
/// bricks of `vehicle` in `boxed` (by address: whether they break off).
///
/// # Safety
/// Game thread only.
unsafe fn draw(
    code: &Code,
    shots: &[Preview],
    vehicle: Option<&Vehicle>,
    boxed: &HashMap<usize, bool>,
    camera: Option<Vec3>,
) {
    let mut canvas = Canvas::default();
    let lines = &mut canvas.lines;
    let width = |at: Vec3| {
        camera.map_or(2.0, |camera| {
            math::length(math::sub(at, camera)) * PIERCING_WIDTH
        })
    };
    for shot in shots {
        let params = &shot.params;
        let color = |range: f64| {
            let span = params.max_damage - params.min_damage;
            let share = if span.abs() > 1e-9 {
                ((params.max_damage - params.damage_at(range)) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            blend(FULL_DAMAGE, LEAST_DAMAGE, share as f32)
        };
        for pair in shot.flight.points.windows(2) {
            push_line(lines, pair[0].at, pair[1].at, color(pair[1].range), 0.0);
        }
        for impact in &shot.flight.impacts {
            if impact.through {
                let middle = math::scale(math::add(impact.at, impact.exit), 0.5);
                push_line(lines, impact.at, impact.exit, THROUGH, width(middle));
            }
        }
    }
    // Markers over the paths, each with its halo, by layer.
    for shot in shots {
        for (impact, &breaks) in shot.flight.impacts.iter().zip(&shot.breaks) {
            if impact.comes_out {
                Marker::new(impact.exit, camera, Layer::Through).draw(
                    &mut canvas,
                    Shape::Square,
                    0.6,
                    THROUGH,
                );
            }
            let at = |layer| Marker::new(impact.at, camera, layer);
            if let Some(ricochet) = impact.ricochet {
                let marker = at(Layer::Hit);
                marker.draw(&mut canvas, Shape::Diamond, 1.0, RICOCHET);
                marker.cone(&mut canvas, &ricochet, RICOCHET);
            } else if breaks {
                at(Layer::Hit).draw(&mut canvas, Shape::Diamond, 1.0, BREAKS);
            } else if impact.through {
                at(Layer::Through).draw(&mut canvas, Shape::Diamond, 0.8, THROUGH);
            }
        }
        if let Some(end) = shot.flight.points.last().map(|point| point.at) {
            let (shape, color) = match shot.flight.ending {
                Ending::Range | Ending::Unfinished => (Shape::Cross, OUT_OF_RANGE),
                Ending::Stopped => (Shape::Cross, STOP),
                Ending::Exploded => (Shape::Star, EXPLODES),
            };
            Marker::new(end, camera, Layer::End).draw(&mut canvas, shape, 1.0, color);
        }
        if let Some(weakest) = shot.weakest() {
            Marker::new(weakest.at, camera, Layer::DropOff).bar(
                &mut canvas,
                weakest.velocity,
                LEAST_DAMAGE,
            );
        }
    }
    for target in vehicle.map_or(&[][..], |vehicle| &vehicle.targets) {
        let color = match boxed.get(&target.brick) {
            Some(true) => BREAKS,
            Some(false) => HOLDS,
            None => continue,
        };
        if let Some(outline) = &target.outline {
            outline_box(&mut canvas.lines, outline, color);
        }
    }
    // Paths and outlines first, then each layer of markers over the last: its halos, then itself.
    let mut lines = canvas.lines;
    for layer in &mut canvas.layers {
        lines.append(&mut layer.halos);
        lines.append(&mut layer.lines);
    }
    // SAFETY: per the contract; the batcher is the current world's, or null.
    unsafe {
        let batcher = code.lines.batcher();
        if !batcher.is_null() {
            code.lines.replace(batcher, OWNER, &lines);
        }
    }
}

/// The lines drawn: paths and outlines, then markers by [`Layer`].
#[derive(Default)]
struct Canvas {
    lines: Vec<Line>,
    layers: [Strokes; Layer::COUNT],
}

/// A layer of markers: their lines, and the dark halos drawn under them.
#[derive(Default)]
struct Strokes {
    halos: Vec<Line>,
    lines: Vec<Line>,
}

/// How much a marker matters: each layer is drawn over those before it, so where a shot ends shows
/// over where it hit things, which shows over what it goes through.
#[derive(Clone, Copy)]
enum Layer {
    /// Where it goes into and comes out of what it pierces.
    Through,
    /// Where it ricochets or breaks a brick off.
    Hit,
    /// Where its damage stops dropping.
    DropOff,
    /// Where it ends: stops, explodes or runs out of range.
    End,
}

impl Layer {
    const COUNT: usize = 4;
}

/// A marker's outline.
#[derive(Clone, Copy)]
enum Shape {
    Diamond,
    Square,
    /// An X.
    Cross,
    /// An X over a +.
    Star,
}

/// Where a marker goes, facing the camera: its middle, the screen's right and up there (length 1),
/// the way from the camera to it (length 1), its distance from the camera and its size (cm), and
/// its layer.
struct Marker {
    at: Vec3,
    right: Vec3,
    up: Vec3,
    toward: Vec3,
    distance: f64,
    size: f64,
    layer: Layer,
}

impl Marker {
    /// A marker at `at` in `layer`, facing a camera at `camera` (seen from the side without one).
    fn new(at: Vec3, camera: Option<Vec3>, layer: Layer) -> Self {
        let (toward, distance) = camera
            .and_then(|camera| {
                let away = math::sub(at, camera);
                Some((math::normalized(away)?, math::length(away)))
            })
            .unwrap_or(([1.0, 0.0, 0.0], 2_000.0));
        let world_up = if toward[2].abs() > 0.99 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let right = math::normalized(math::cross(world_up, toward)).unwrap_or([0.0, 1.0, 0.0]);
        let up = math::cross(toward, right);
        Self {
            at,
            right,
            up,
            toward,
            distance,
            size: distance * MARKER_SIZE,
            layer,
        }
    }

    /// A line `width` cm wide (0: a pixel) near the marker, over a dark halo: wider, a bit longer
    /// at both ends, and a bit further from the camera (where it looks the same), so the line shows
    /// in front of it whether or not the engine tests the depth of lines on top. Both are drawn
    /// [`LAYER_LIFT`] nearer the camera per layer, for the same reason.
    fn stroke(&self, canvas: &mut Canvas, start: Vec3, end: Vec3, color: [f32; 3], width: f64) {
        let reach = self.distance * HALO_WIDTH;
        let along = math::normalized(math::sub(end, start)).unwrap_or_default();
        let layer = self.layer as usize;
        let lift = math::scale(self.toward, -self.distance * LAYER_LIFT * layer as f64);
        let (start, end) = (math::add(start, lift), math::add(end, lift));
        let behind = math::scale(self.toward, self.distance * HALO_BEHIND);
        let moved = |point: Vec3, outward: f64| {
            math::add(
                math::add(point, math::scale(along, outward * reach)),
                behind,
            )
        };
        let strokes = &mut canvas.layers[layer];
        push_line(
            &mut strokes.halos,
            moved(start, -1.0),
            moved(end, 1.0),
            HALO,
            width + 2.0 * reach,
        );
        push_line(&mut strokes.lines, start, end, color, width);
    }

    /// The point `x` and `y` marker sizes off its middle, on the screen.
    fn point(&self, x: f64, y: f64) -> Vec3 {
        math::add(
            self.at,
            math::add(
                math::scale(self.right, x * self.size),
                math::scale(self.up, y * self.size),
            ),
        )
    }

    /// Draws `shape`, `scale` times the marker size.
    fn draw(&self, canvas: &mut Canvas, shape: Shape, scale: f64, color: [f32; 3]) {
        const DIAMOND: [[f64; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
        const SQUARE: [[f64; 2]; 4] = [[0.7, 0.7], [-0.7, 0.7], [-0.7, -0.7], [0.7, -0.7]];
        const X: [[[f64; 2]; 2]; 2] = [[[0.7, 0.7], [-0.7, -0.7]], [[0.7, -0.7], [-0.7, 0.7]]];
        const PLUS: [[[f64; 2]; 2]; 2] = [[[1.0, 0.0], [-1.0, 0.0]], [[0.0, 1.0], [0.0, -1.0]]];
        let width = self.size / MARKER_SIZE * MARKER_WIDTH;
        let mut stroke = |[[x1, y1], [x2, y2]]: [[f64; 2]; 2]| {
            let start = self.point(x1 * scale, y1 * scale);
            let end = self.point(x2 * scale, y2 * scale);
            self.stroke(canvas, start, end, color, width);
        };
        let outline = |corners: [[f64; 2]; 4]| {
            (0..corners.len()).map(move |index| [corners[index], corners[(index + 1) % 4]])
        };
        match shape {
            Shape::Diamond => outline(DIAMOND).for_each(&mut stroke),
            Shape::Square => outline(SQUARE).for_each(&mut stroke),
            Shape::Cross => X.into_iter().for_each(&mut stroke),
            Shape::Star => X.into_iter().chain(PLUS).for_each(&mut stroke),
        }
    }

    /// Draws a bar across a path going `direction` here (as seen on the screen), like a mark on a
    /// ruler.
    fn bar(&self, canvas: &mut Canvas, direction: Vec3, color: [f32; 3]) {
        let along = [
            math::dot(direction, self.right),
            math::dot(direction, self.up),
        ];
        let length = along[0].hypot(along[1]);
        // Seen end on, any way across will do.
        let [x, y] = if length > 1e-9 {
            [-along[1] / length, along[0] / length]
        } else {
            [1.0, 0.0]
        };
        let width = self.size / MARKER_SIZE * MARKER_WIDTH;
        let (start, end) = (self.point(x, y), self.point(-x, -y));
        self.stroke(canvas, start, end, color, width);
    }

    /// Draws the cone a ricochet leaves in, as far as it points away from the surface (the rest
    /// would meet it again), and its middle line.
    fn cone(&self, canvas: &mut Canvas, ricochet: &Ricochet, color: [f32; 3]) {
        let length = self.size * CONE_LENGTH;
        let axis = ricochet.direction;
        let tip = |direction: Vec3| math::add(self.at, math::scale(direction, length));
        self.stroke(canvas, self.at, tip(axis), color, 0.0);
        let half_angle = ricochet.scatter.to_radians();
        // Two directions across the axis.
        let helper = if axis[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let Some(across) = math::normalized(math::cross(axis, helper)) else {
            return;
        };
        if half_angle < 1e-3 {
            return;
        }
        let other = math::cross(axis, across);
        let edge = |index: usize| {
            let turn = index as f64 / CONE_LINES as f64 * core::f64::consts::TAU;
            let side = math::add(
                math::scale(across, turn.cos()),
                math::scale(other, turn.sin()),
            );
            math::add(
                math::scale(axis, half_angle.cos()),
                math::scale(side, half_angle.sin()),
            )
        };
        let leaves = |direction: Vec3| math::dot(direction, ricochet.normal) > 0.0;
        for index in 0..CONE_LINES {
            let (this, next) = (edge(index), edge(index + 1));
            if leaves(this) {
                self.stroke(canvas, self.at, tip(this), color, 0.0);
                if leaves(next) {
                    self.stroke(canvas, tip(this), tip(next), color, 0.0);
                }
            }
        }
    }
}

/// The twelve edges of a brick's box, [`OUTLINE_MARGIN`] outside it.
fn outline_box(lines: &mut Vec<Line>, outline: &Outline, color: [f32; 3]) {
    let [low, high] = outline.bounds;
    let corner = |index: usize| {
        let local: Vec3 = core::array::from_fn(|axis| {
            if index >> axis & 1 == 0 {
                low[axis] - OUTLINE_MARGIN
            } else {
                high[axis] + OUTLINE_MARGIN
            }
        });
        math::add(outline.location, outline.rotation.rotate(local))
    };
    for index in 0..8 {
        for axis in 0..3 {
            // Each edge once: from the corner on its low side.
            if index >> axis & 1 == 0 {
                push_line(lines, corner(index), corner(index | 1 << axis), color, 0.0);
            }
        }
    }
}

/// A line `width` cm wide (0: a pixel).
fn push_line(lines: &mut Vec<Line>, start: Vec3, end: Vec3, color: [f32; 3], width: f64) {
    if start.iter().chain(&end).all(|value| value.is_finite()) && width.is_finite() {
        lines.push(Line {
            start: math::narrow(start),
            end: math::narrow(end),
            color,
            thickness: width as f32,
            on_top: true,
        });
    }
}

/// Between two linear colors.
fn blend(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    core::array::from_fn(|channel| from[channel] + (to[channel] - from[channel]) * share)
}
