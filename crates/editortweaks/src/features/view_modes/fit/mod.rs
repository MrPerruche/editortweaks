//! The "Reference fit" view mode: bricks colored by how far their surface is from the reference
//! models (`features::references`), smoothly, on a log scale from teal (0.01 cm) through green
//! (1 cm), yellow (10 cm) and orange (1 m) to dark red (1 km); gray while no model counts.
//!
//! Each brick's surface is the triangles of its body setup's complex collision
//! (`game::brick_model`, what the editor's clicks hit), else its simple shapes (spinners' hulls),
//! else its box. Threads of ours ([`worker`]) measure it against the references
//! ([`model_io::fit`]), subdividing where the distance changes, and put each vertex's distance in
//! its texture coordinate U. Each brick then gets a procedural mesh component of ours with those
//! triangles (`game::procedural_mesh`: the whole mesh in one call), attached to its own mesh
//! component and scaled like the collision view mode's shapes, showing a lit material whose texture
//! is the color scale: the GPU blends U across each triangle, so colors change smoothly over a face
//! without a shader of ours (the game can't compile one). The brick itself shows the game's hidden
//! material (`view_modes` hooks `GetEditorMaterialOverride` for that) until our component is
//! there; then its own mesh leaves the main pass (`MeshComponents::set_drawn`), since our component
//! lies on the same surface and the dithered copy would z-fight with it. The selection outline is
//! copied to our component every frame.
//!
//! A brick is measured again when the game updates it with another place, mesh or scale (moves,
//! resizes), and every brick when the references change (moved, rescaled, hidden with H, another
//! file). Building the meshes is spread over frames ([`BUILD_BUDGET`]).
//!
//! While bricks are dragged, the game moves their mesh components along (and our components
//! with them, attached), updating them every frame: each is measured again where its component
//! is, and keeps its colors until newer ones are measured, so nothing blinks and colors catch up
//! with the drag.
//!
//! Driven by `view_modes`' hooks like the collision view mode: [`show`] after the game updates a
//! brick in this view mode, [`remove`] after it updates one in any other, [`keep_only`] and
//! [`refresh`] after every change of the editor, and [`after_tick`] every frame.

mod worker;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use model_io::{fit, loft};

use self::worker::{Done, Job, Source, Worker};
use super::Editor;
use super::collision::mesh as shapes_mesh;
use super::tooltip::{Hover, number};
use crate::features::references::{self, looks::Looks};
use crate::game::brick_model::BrickModels;
use crate::game::collision::{Collision, Shapes};
use crate::game::editor_object::{LocalBounds, Vector};
use crate::game::editor_view::EditorCode;
use crate::game::mesh_component::MeshComponents;
use crate::game::procedural_mesh::ProceduralMeshes;
use crate::game::spinner_profiles::SpinnerProfiles;
use crate::game::textures::Textures;
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::prelude::*;

/// How long meshes may take to build per frame (at least one is built each frame).
const BUILD_BUDGET: Duration = Duration::from_millis(4);

/// How often the references are checked for changes.
const REFERENCES_POLL: Duration = Duration::from_millis(250);

/// How long changed references must stay the same before bricks are measured again (a slider
/// drag changes them every frame).
const REFERENCES_SETTLE: Duration = Duration::from_millis(300);

struct Code {
    collision: Collision,
    models: BrickModels,
    bounds: LocalBounds,
    procedural: ProceduralMeshes,
    components: MeshComponents,
    textures: Textures,
    /// Without it, spinners show their collision's coarser cross-section.
    spinners: Option<SpinnerProfiles>,
}

static CODE: OnceLock<Code> = OnceLock::new();
/// The bricks shown in this view mode, by address.
static SHOWN: Mutex<BTreeMap<usize, Shown>> = Mutex::new(BTreeMap::new());
static STATE: Mutex<State> = Mutex::new(State::new());

/// A brick shown in this view mode.
struct Shown {
    brick: WeakObject,
    /// Its mesh component, which ours is attached to.
    parent: WeakObject,
    /// Our component showing its colors, once built.
    colors: Option<WeakObject>,
    /// What it was last measured as.
    pose: Option<Pose>,
    /// Its surface, kept to measure it again when the references change.
    source: Option<Arc<Source>>,
    /// Its latest job's serial.
    serial: u64,
    /// The serial of the job whose colors our component shows: older results are thrown away.
    drawn: u64,
    /// Whether the brick's own mesh is out of the view's main pass: our component shows its
    /// surface, so the brick's hidden material would z-fight with it.
    parent_undrawn: bool,
}

// SAFETY: weak pointers to game objects, only resolved on the game thread.
unsafe impl Send for Shown {}

/// What a brick's measure depends on, besides the references.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Pose {
    /// What its surface was read from: its body setup, by address (0 for its box).
    surface: usize,
    scale: Vector,
    /// Where its mesh component is, relative to the editor's root (its spawn place, or where a
    /// move being dragged has put it).
    location: Vector,
    rotation: Vector,
}

/// What's kept between frames, besides the bricks.
struct State {
    worker: Option<Result<Worker, ()>>,
    /// The references' key last sent to the worker ([`references::fit_key`]).
    sent: Option<u64>,
    /// A newer key, and since when it's been seen.
    settling: Option<(u64, Instant)>,
    last_poll: Option<Instant>,
    serial: u64,
    /// The bricks' surfaces, by body setup (address; checked to be the same one when reused).
    sources: BTreeMap<usize, (WeakObject, Arc<Source>)>,
    /// The meshes whose bricks showed something else than their complex collision (logged once
    /// each), by name.
    fallbacks: BTreeSet<String>,
    /// The material showing the color scale, and what makes it.
    material: Option<WeakObject>,
    looks: Option<Result<Looks, ()>>,
    /// The current round of measures, if one runs.
    round: Option<Round>,
}

/// A round of measures after the references changed, for the log.
struct Round {
    since: Instant,
    built: usize,
    /// Time spent measuring, added over the worker's threads.
    measuring: Duration,
    /// Time spent on the game thread making the meshes.
    building: Duration,
}

// SAFETY: weak pointers and the game's material lookups, only used on the game thread.
unsafe impl Send for State {}

impl State {
    const fn new() -> Self {
        Self {
            worker: None,
            sent: None,
            settling: None,
            last_poll: None,
            serial: 0,
            sources: BTreeMap::new(),
            fallbacks: BTreeSet::new(),
            material: None,
            looks: None,
            round: None,
        }
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// The thread measuring bricks, started the first time; `None` if it couldn't be (logged).
    fn worker(&mut self) -> Option<&Worker> {
        let worker = self.worker.get_or_insert_with(|| {
            Worker::start().map_err(|reason| log!("Reference fit inactive: {reason}"))
        });
        worker.as_ref().ok()
    }
}

/// Finds the game code the view mode needs (after engine init).
pub fn find() -> Result<(), String> {
    let code = Code {
        collision: Collision::find()?,
        models: BrickModels::find()?,
        bounds: LocalBounds::find()?,
        procedural: ProceduralMeshes::find()?,
        components: MeshComponents::find()?,
        textures: Textures::find()?,
        spinners: SpinnerProfiles::find()
            .map_err(|reason| {
                log!("Reference fit: spinners show their collision's shape: {reason}")
            })
            .ok(),
    };
    let _ = CODE.set(code);
    Ok(())
}

/// Every frame: measures again after the references changed, builds the bricks measured, and
/// gives our components the selection outline of their bricks.
pub fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(found) = CODE.get() else { return };
    if SHOWN.lock().map_or(true, |shown| shown.is_empty()) {
        return;
    }
    check_references();
    // SAFETY: on the game thread (the editor's tick), `editor` the live editor ticking.
    unsafe {
        found.build_finished(editor);
        found.copy_outlines();
    }
}

/// The tooltip's line over the brick under the mouse: how far the point of it under the mouse is
/// from the nearest point of the reference models, as bricks are colored.
///
/// # Safety
/// Game thread only.
pub unsafe fn describe(editor: &Editor, hover: &Hover) -> Vec<String> {
    // SAFETY: per the contract; the editor is live.
    let Some(point) = (unsafe { hover.point(editor) }) else {
        return Vec::new();
    };
    // SAFETY: as above.
    match unsafe { references::fit_distance(editor.editor, hover.at(point.entry)) } {
        Some(distance) => vec![format!(
            "Distance to the reference: {} cm",
            number(distance)
        )],
        None => vec!["No reference model to measure against".to_owned()],
    }
}

/// Shows `brick` as this view mode does, after the game updated it: measured again if its place,
/// mesh or scale changed.
///
/// # Safety
/// `brick` must be a live brick in the editor. Game thread only.
pub unsafe fn show(code: &EditorCode, brick: *mut UObjectBase) {
    let Some(found) = CODE.get() else { return };
    // SAFETY: per the contract.
    unsafe {
        let Some(component) = found.collision.component_of(brick) else {
            remove(brick);
            return;
        };
        // Hidden with H: it shows as hidden bricks do, nothing of ours.
        if code.is_hidden(brick) {
            remove(brick);
            return;
        }
        let mut shown = match take(brick) {
            Some(shown) if shown.parent.is(component) => shown,
            stale => {
                if let Some(stale) = stale {
                    destroy(stale);
                }
                Shown {
                    // SAFETY: a live brick and its live component.
                    brick: WeakObject::new(brick),
                    parent: WeakObject::new(component),
                    colors: None,
                    pose: None,
                    source: None,
                    serial: 0,
                    drawn: 0,
                    parent_undrawn: false,
                }
            }
        };
        found.measure_if_moved(brick, component, &mut shown);
        if let Some(colors) = shown.colors.as_ref().and_then(WeakObject::get) {
            found.components.copy_custom_depth(component, colors);
        }
        put(brick, shown);
    }
}

/// Measures again the bricks whose place, mesh or scale changed, after a change of the editor.
///
/// # Safety
/// Every brick must be a live brick in the editor. Game thread only.
pub unsafe fn refresh(code: &EditorCode, editor_bricks: &[*mut UObjectBase]) {
    for &brick in editor_bricks {
        // SAFETY: per the contract.
        unsafe { show(code, brick) };
    }
}

/// Removes what this view mode added to `brick`, if anything.
///
/// # Safety
/// Game thread only.
pub unsafe fn remove(brick: *mut UObjectBase) {
    // Most calls are for bricks never shown here: one lookup.
    // SAFETY: per the contract.
    let Some(shown) = (unsafe { take(brick) }) else {
        return;
    };
    // SAFETY: per the contract.
    unsafe { destroy(shown) };
}

/// Removes what this view mode added to bricks other than `editor_bricks` (deleted ones), or to
/// every brick if empty.
///
/// # Safety
/// Game thread only.
pub unsafe fn keep_only(editor_bricks: &[*mut UObjectBase]) {
    let kept: HashSet<usize> = editor_bricks.iter().map(|&brick| brick as usize).collect();
    let removed: Vec<Shown> = match SHOWN.lock() {
        Ok(mut shown) => {
            let gone: Vec<usize> = shown
                .keys()
                .copied()
                .filter(|key| !kept.contains(key))
                .collect();
            gone.into_iter()
                .filter_map(|key| shown.remove(&key))
                .collect()
        }
        Err(_) => return,
    };
    for shown in removed {
        // SAFETY: per the contract.
        unsafe { destroy(shown) };
    }
}

/// Destroys a removed brick's component of ours, if it's still there, and gives the brick its
/// mesh back.
///
/// # Safety
/// Game thread only.
unsafe fn destroy(shown: Shown) {
    let Some(found) = CODE.get() else { return };
    if shown.parent_undrawn
        && let Some(parent) = shown.parent.get()
    {
        // SAFETY: per the contract; a live component (checked through its weak pointer).
        unsafe { found.components.set_drawn(parent, true) };
    }
    if let Some(colors) = shown.colors.and_then(|colors| colors.get()) {
        // SAFETY: per the contract; a live component we made.
        unsafe { found.components.destroy(colors) };
    }
}

/// Takes `brick`'s entry out (no lock is held while calling the game, which may call our hooks
/// again), if it's still the same brick.
///
/// # Safety
/// Game thread only.
unsafe fn take(brick: *mut UObjectBase) -> Option<Shown> {
    let shown = SHOWN.lock().ok()?.remove(&(brick as usize))?;
    // Another brick at the address of a destroyed one: start over.
    if !shown.brick.is(brick) || shown.brick.get().is_none() {
        // SAFETY: per the contract.
        unsafe { destroy(shown) };
        return None;
    }
    Some(shown)
}

fn put(brick: *mut UObjectBase, shown: Shown) {
    if let Ok(mut all) = SHOWN.lock() {
        all.insert(brick as usize, shown);
    }
}

/// Sends the references to the worker once they changed and settled, and every brick to measure
/// again.
fn check_references() {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    if state
        .last_poll
        .is_some_and(|last| now.duration_since(last) < REFERENCES_POLL)
    {
        return;
    }
    state.last_poll = Some(now);
    let Some(key) = references::fit_key() else {
        return;
    };
    if state.sent == Some(key) {
        state.settling = None;
        return;
    }
    // The first time at once; later, once the change settled.
    let settled = match state.settling {
        Some((settling, since)) if settling == key => {
            now.duration_since(since) >= REFERENCES_SETTLE
        }
        _ => false,
    };
    if state.sent.is_some() && !settled {
        if state.settling.is_none_or(|(settling, _)| settling != key) {
            state.settling = Some((key, now));
        }
        return;
    }
    let Some(triangles) = references::fit_triangles() else {
        return;
    };
    let count = triangles.len();
    let Some(worker) = state.worker() else { return };
    if !worker.set_references(triangles) {
        return;
    }
    log!("Reference fit: measuring against {count} reference triangles");
    state.sent = Some(key);
    state.settling = None;
    state.round = Some(Round {
        since: now,
        built: 0,
        measuring: Duration::ZERO,
        building: Duration::ZERO,
    });
    // Every brick again.
    let Ok(mut shown) = SHOWN.lock() else { return };
    for (&brick, shown) in shown.iter_mut() {
        let (Some(pose), Some(source)) = (shown.pose, shown.source.clone()) else {
            continue;
        };
        shown.serial = state.next_serial();
        let job = job(brick, shown.serial, source, pose);
        if let Some(worker) = state.worker() {
            worker.measure(job);
        }
    }
}

fn job(brick: usize, serial: u64, source: Arc<Source>, pose: Pose) -> Job {
    Job {
        brick,
        serial,
        source,
        scale: pose.scale,
        location: pose.location,
        rotation: pose.rotation,
    }
}

impl Code {
    /// Sends `brick` to the worker if its place (its mesh component's), mesh or scale changed since
    /// it was measured.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor and `component` its mesh component. Game thread
    /// only.
    unsafe fn measure_if_moved(
        &self,
        brick: *mut UObjectBase,
        component: *mut UObjectBase,
        shown: &mut Shown,
    ) {
        // SAFETY: per the contract.
        unsafe {
            // Not the spawn place: while a move is dragged, the game moves the component and
            // updates the brick there (`docs/re/collision-shapes.md`), but updates it at its
            // spawn place too.
            let (location, rotation) = self.components.relative_place(component);
            let (body_setup, mesh) = self.collision.body_setup_of(brick, component);
            let with_source = match (body_setup, mesh) {
                (Some(body_setup), Some(mesh)) => self
                    .source_of(brick, body_setup, mesh)
                    .map(|source| (body_setup, mesh, source)),
                _ => {
                    if let Some(mesh) = mesh {
                        note_fallback(mesh, "its box: no body setup");
                    }
                    None
                }
            };
            let (surface, scale, source) = match with_source {
                Some((body_setup, mesh, source)) => (
                    body_setup as usize,
                    self.collision.scale_of(brick, mesh),
                    source,
                ),
                // Its box, in its own space (already scaled).
                None => match self.bounds.of(brick) {
                    Some(bounds) => (0, [1.0; 3], Arc::new(box_source(bounds))),
                    None => return,
                },
            };
            let pose = Pose {
                surface,
                scale,
                location,
                rotation,
            };
            if shown.pose == Some(pose) && shown.source.is_some() {
                return;
            }
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            shown.pose = Some(pose);
            shown.source = Some(source.clone());
            shown.serial = state.next_serial();
            let job = job(brick as usize, shown.serial, source, pose);
            if let Some(worker) = state.worker() {
                worker.measure(job);
            }
        }
    }

    /// The surface `brick` colliding with `body_setup` shows (read once per body setup): its
    /// complex collision, else its simple shapes (spinners: swept, see [`Self::swept`]), else
    /// `None` for its box. Each fallback is logged once per mesh name.
    ///
    /// # Safety
    /// `brick` must be a live brick, `body_setup` its live body setup and `mesh` its live static
    /// mesh. Game thread only.
    unsafe fn source_of(
        &self,
        brick: *mut UObjectBase,
        body_setup: *mut UObjectBase,
        mesh: *mut UObjectBase,
    ) -> Option<Arc<Source>> {
        let key = body_setup as usize;
        {
            let state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((weak, source)) = state.sources.get(&key)
                && weak.is(body_setup)
                && weak.get().is_some()
            {
                return Some(source.clone());
            }
        }
        // SAFETY: per the contract.
        let source = match unsafe { self.models.triangles_of_body_setup(body_setup) } {
            Ok(triangles) => Source {
                exact: true,
                ..oriented(triangles.vertices, triangles.triangles)
            },
            // SAFETY: per the contract.
            Err(reason) => match unsafe { self.collision.shapes(body_setup) } {
                // SAFETY: per the contract.
                Ok(shapes) if !shapes.is_empty() => unsafe { self.swept(brick, mesh, &shapes) }
                    .unwrap_or_else(|| {
                        // SAFETY: per the contract.
                        unsafe { note_fallback(mesh, &format!("its collision shapes ({reason})")) };
                        // Unscaled: the brick's scale is applied when it's measured, as to the rest.
                        let built = shapes_mesh::triangles(&shapes, [1.0; 3]).triangles;
                        oriented(built.positions, built.corners)
                    }),
                _ => {
                    // SAFETY: per the contract.
                    unsafe {
                        note_fallback(mesh, &format!("its box: {reason}, no collision shapes"))
                    };
                    return None;
                }
            },
        };
        let source = Arc::new(source);
        // SAFETY: a live body setup.
        let weak = unsafe { WeakObject::new(body_setup) };
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        state.sources.insert(key, (weak, source.clone()));
        Some(source)
    }

    /// A spinner's surface: its collision is a chain of convex pieces along its arc, each from one
    /// cross-section to the next, with a coarser cross-section than the visible mesh's. The visible
    /// one is swept along the same arc ([`loft::reswept`]); failing that, the collision's own
    /// sections are joined up ([`loft::chained`]). `None` for other simple shapes.
    ///
    /// # Safety
    /// `brick` must be a live brick and `mesh` its live static mesh. Game thread only.
    unsafe fn swept(
        &self,
        brick: *mut UObjectBase,
        mesh: *mut UObjectBase,
        shapes: &Shapes,
    ) -> Option<Source> {
        let only_hulls =
            shapes.boxes.is_empty() && shapes.spheres.is_empty() && shapes.capsules.is_empty();
        if !only_hulls {
            return None;
        }
        // SAFETY: per the contract.
        let profiles = self
            .spinners
            .as_ref()
            .and_then(|spinners| unsafe { spinners.of(brick) });
        let visible = profiles.and_then(|profiles| {
            let swept = loft::reswept(
                &shapes.hulls,
                &profiles.collision,
                &profiles.visible,
                profiles.quarter_turns,
            );
            if swept.is_none() {
                // SAFETY: per the contract.
                unsafe { note_fallback(mesh, "its collision's cross-section (no arc found)") };
            }
            swept
        });
        let swept = visible.or_else(|| loft::chained(&shapes.hulls))?;
        Some(oriented(swept.positions, swept.corners))
    }

    /// Builds the meshes of the bricks the worker measured, for up to [`BUILD_BUDGET`].
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn build_finished(&self, editor: *mut UObjectBase) {
        let started = Instant::now();
        // SAFETY: per the contract.
        let material = unsafe { self.material(editor) };
        loop {
            let done = {
                let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
                let Some(done) = state.worker().and_then(Worker::finished) else {
                    break;
                };
                done
            };
            let brick = done.brick as *mut UObjectBase;
            // SAFETY: per the contract; the entry's weak pointers say whether it's still live.
            let Some(mut shown) = (unsafe { take(brick) }) else {
                continue;
            };
            let latest = shown.serial == done.serial;
            let building = Instant::now();
            // Not only the latest: while a brick is dragged, it has moved on by the time its
            // measure is done, and each one done brings its colors closer.
            if done.serial > shown.drawn {
                shown.drawn = done.serial;
                // SAFETY: per the contract.
                unsafe { self.show_colors(&mut shown, &done, material) };
            }
            put(brick, shown);
            if latest {
                self.count_built(done.took, building.elapsed());
            }
            if started.elapsed() >= BUILD_BUDGET {
                break;
            }
        }
    }

    /// Builds `shown`'s mesh from what the worker measured, and shows it.
    ///
    /// # Safety
    /// `material` must be null or a live material. Game thread only.
    unsafe fn show_colors(&self, shown: &mut Shown, done: &Done, material: *mut UObjectBase) {
        let (Some(parent), Some(pose)) = (shown.parent.get(), shown.pose) else {
            return;
        };
        // SAFETY: per the contract; components are live (checked through their weak pointers),
        // and ours is a procedural mesh component (made below).
        unsafe {
            let colors = match shown.colors.as_ref().and_then(WeakObject::get) {
                Some(colors) => {
                    self.components.set_scale(colors, pose.scale);
                    colors
                }
                None => {
                    let class = self.procedural.class();
                    let Some(colors) = self.components.create_of(class, parent, pose.scale) else {
                        return;
                    };
                    shown.colors = Some(WeakObject::new(colors));
                    colors
                }
            };
            let sampled = &done.sampled;
            self.procedural.set_section(
                colors,
                &sampled.positions,
                &sampled.corners,
                &sampled.normals,
                &sampled.uvs,
            );
            if !material.is_null() {
                self.components.set_material(colors, 0, material);
            }
            self.components.copy_custom_depth(parent, colors);
            // Our component is the brick's surface: the brick's own mesh underneath, in the hidden
            // material, is not needed and z-fights with it. The outline comes from our component.
            // The brick keeps its mesh until ours is there, and for good when ours only stands in
            // for its surface (a spinner's is swept again from its collision, which can be off).
            let undrawn = shown.source.as_ref().is_some_and(|source| source.exact);
            if undrawn != shown.parent_undrawn && self.components.set_drawn(parent, !undrawn) {
                shown.parent_undrawn = undrawn;
            }
        }
    }

    /// Counts a brick built in the current round, and logs the round once it's over.
    fn count_built(&self, measuring: Duration, building: Duration) {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        let threads = state
            .worker
            .as_ref()
            .and_then(|worker| worker.as_ref().ok())
            .map_or(0, Worker::threads);
        let Some(round) = &mut state.round else {
            return;
        };
        round.built += 1;
        round.measuring += measuring;
        round.building += building;
        let all = SHOWN.lock().map_or(0, |shown| shown.len());
        if round.built >= all {
            let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
            log!(
                "Reference fit: {} bricks shown in {:.0} ms (measuring {:.0} ms over {threads} \
                 threads, building {:.0} ms)",
                round.built,
                ms(round.since.elapsed()),
                ms(round.measuring),
                ms(round.building)
            );
            state.round = None;
        }
    }

    /// The material showing the color scale (made once, and again if the engine freed it); null
    /// if it can't be had (logged once).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn material(&self, editor: *mut UObjectBase) -> *mut UObjectBase {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(material) = state.material.as_ref().and_then(WeakObject::get_live) {
            return material;
        }
        let looks = state.looks.get_or_insert_with(|| {
            Looks::find().map_err(|reason| log!("Reference fit shows no colors: {reason}"))
        });
        let Ok(looks) = looks else {
            return core::ptr::null_mut();
        };
        let Ok(png) = fit::gradient().to_png() else {
            return core::ptr::null_mut();
        };
        // SAFETY: per the contract; the editor lives in the editor's world.
        let material = unsafe {
            let texture = self.textures.import_buffer(editor, &png);
            if texture.is_null() {
                return core::ptr::null_mut();
            }
            looks.textured(&self.textures, editor, texture)
        };
        if !material.is_null() {
            // SAFETY: just made, live.
            state.material = Some(unsafe { WeakObject::new(material) });
        }
        material
    }

    /// Gives our components the selection outline of their bricks: the game sets it after
    /// updating a brick (when [`show`] runs), and when the selection or the hovered brick changes.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn copy_outlines(&self) {
        let pairs: Vec<(*mut UObjectBase, *mut UObjectBase)> = match SHOWN.lock() {
            Ok(shown) => shown
                .values()
                .filter_map(|shown| Some((shown.parent.get()?, shown.colors.as_ref()?.get()?)))
                .collect(),
            Err(_) => return,
        };
        for (parent, colors) in pairs {
            // SAFETY: both are live components (just checked through their weak pointers).
            unsafe { self.components.copy_custom_depth(parent, colors) };
        }
    }
}

/// Logs, once per mesh name, that bricks showing `mesh` are measured as `what` rather than their
/// complex collision.
///
/// # Safety
/// `mesh` must be a live object. Game thread only.
unsafe fn note_fallback(mesh: *mut UObjectBase, what: &str) {
    // SAFETY: per the contract.
    let name = unsafe { (*mesh).name_private.to_string() };
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if state.fallbacks.insert(name.clone()) {
        log!("Reference fit: {name} shows {what}");
    }
}

/// A mesh's triangles, wound the way the engine wants: PhysX's are counterclockwise seen from
/// outside (the math convention), which the engine takes for their back. Told by the volume they
/// enclose, so meshes the other way round stay as they are.
fn oriented(positions: Vec<[f32; 3]>, mut corners: Vec<[u32; 3]>) -> Source {
    corners.retain(|triangle| triangle.iter().all(|&i| (i as usize) < positions.len()));
    let volume: f64 = corners
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|i| math::widen(positions[i as usize]));
            math::dot(a, math::cross(b, c))
        })
        .sum();
    if volume > 0.0 {
        for triangle in &mut corners {
            triangle.swap(1, 2);
        }
    }
    Source {
        positions,
        corners,
        exact: false,
    }
}

/// A brick's box (`[min, max]`) as a surface, for bricks without complex collision.
fn box_source([min, max]: [Vector; 2]) -> Source {
    let positions: Vec<[f32; 3]> = (0..8)
        .map(|corner| {
            core::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    min[axis]
                } else {
                    max[axis]
                }
            })
        })
        .collect();
    // Each face's corners in order around it; `oriented` turns them the engine's way.
    const FACES: [[u32; 4]; 6] = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let corners = FACES
        .iter()
        .flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
        .collect();
    oriented(positions, corners)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The volume the engine sees enclosed: front faces clockwise (see the collision view mode's
    /// mesh tests).
    fn engine_volume(source: &Source) -> f64 {
        source
            .corners
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.map(|i| math::widen(source.positions[i as usize]));
                math::dot(a, math::cross(c, b)) / 6.0
            })
            .sum()
    }

    #[test]
    fn box_faces_out_for_the_engine() {
        let source = box_source([[-1.0; 3], [1.0; 3]]);
        assert_eq!(source.corners.len(), 12);
        assert!((engine_volume(&source) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn either_winding_ends_up_facing_out() {
        let source = box_source([[0.0; 3], [2.0, 3.0, 4.0]]);
        let flipped: Vec<[u32; 3]> = source.corners.iter().map(|&[a, b, c]| [a, c, b]).collect();
        let again = oriented(source.positions.clone(), flipped);
        assert!((engine_volume(&again) - 24.0).abs() < 1e-4);
    }
}
