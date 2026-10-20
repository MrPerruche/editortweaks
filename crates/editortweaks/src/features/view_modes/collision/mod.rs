//! The "Collision" view mode: bricks' collision shapes (their hitboxes), drawn solid with each
//! brick's own material and color, over the bricks made see-through.
//!
//! The shapes are the simple collision PhysX simulates contacts with (`game::collision`), scaled
//! like the game scales them. Each brick gets a static mesh component showing them
//! (`game::mesh_component`), attached to its own mesh component with the same scale, so the
//! brick's material renders on it as on the brick: its color, pattern, and its place while being
//! moved (all read from the brick's custom primitive data, copied over), and its selection outline
//! (custom depth, copied every frame). Brick types whose material deforms the mesh (wheels,
//! spinners, flaps) would deform the shapes too: theirs show a plain version of it
//! (`game::brick_materials`). The meshes are built at run time (`game::mesh_builder`), one per
//! brick type and, for round shapes, scale. The brick itself shows the game's hidden
//! material (dithered, as hidden bricks look): where it sticks out of its shapes, it doesn't
//! collide, and a brick showing only that has no collision at all. The shapes are drawn 1 mm
//! outward (`mesh::OUTSET`) so that faces on the brick's surface don't z-fight with it.
//!
//! Driven by `view_modes`' hooks: [`show`] after the game updates a brick in this view mode
//! (switching to it, edits, moves), [`remove`] after it updates one in any other, [`keep_only`]
//! after every change of the editor (deleted bricks), and [`after_tick`] every frame.

pub(super) mod mesh;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::brick_materials::BrickMaterials;
use crate::game::collision::{Collision, Shapes};
use crate::game::editor_object::Vector;
use crate::game::editor_view::{Bricks, EditorCode};
use crate::game::mesh_builder::MeshBuilder;
use crate::game::mesh_component::MeshComponents;
use crate::game::reflection;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// The most brick types whose shapes are logged (the first time each is shown).
const MAX_LOGGED_TYPES: usize = 64;

struct Code {
    collision: Collision,
    builder: MeshBuilder,
    components: MeshComponents,
    /// Without it, every shape shows its brick's own material, deforming or not.
    materials: Option<BrickMaterials>,
}

static CODE: OnceLock<Code> = OnceLock::new();
/// The bricks shown in this view mode, by address.
static SHOWN: Mutex<BTreeMap<usize, Shown>> = Mutex::new(BTreeMap::new());
/// The meshes built, by what they're made of (see [`fingerprint`]); the engine frees one once no
/// component shows it.
static MESHES: Mutex<Option<HashMap<u64, BuiltMesh>>> = Mutex::new(None);

/// A mesh built, held weakly.
struct BuiltMesh(WeakObject);

// SAFETY: a weak pointer to a game object, only resolved on the game thread.
unsafe impl Send for BuiltMesh {}
/// The mesh names whose shapes were logged.
static LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// A brick shown in this view mode.
struct Shown {
    brick: WeakObject,
    /// Its mesh component, which the shapes' component is attached to.
    parent: WeakObject,
    /// Its shapes' mesh component; `None` without collision.
    shapes: Option<WeakObject>,
    /// What the component shows (see [`fingerprint`]) and its scale, once made.
    made: Option<(u64, Vector)>,
    /// Its own material, as last seen (once the brick shows the hidden material, it's only here).
    material: Option<WeakObject>,
}

// SAFETY: weak pointers to game objects, only resolved on the game thread.
unsafe impl Send for Shown {}

/// Finds the game code the view mode needs (after engine init).
pub fn find() -> Result<(), String> {
    let code = Code {
        collision: Collision::find()?,
        builder: MeshBuilder::find()?,
        components: MeshComponents::find()?,
        materials: BrickMaterials::find()
            .inspect_err(|reason| log!("shapes keep their bricks' own materials: {reason}"))
            .ok(),
    };
    let _ = CODE.set(code);
    Ok(())
}

/// Gives the shapes the selection outline of their bricks, every frame: the game sets it after
/// updating a brick (when [`show`] runs), and when the selection or the hovered brick changes.
pub fn after_tick(_editor: *mut UObjectBase, _delta: f32) {
    let Some(found) = CODE.get() else { return };
    let pairs: Vec<(*mut UObjectBase, *mut UObjectBase)> = match SHOWN.lock() {
        Ok(shown) => shown
            .values()
            .filter_map(|shown| Some((shown.parent.get()?, shown.shapes.as_ref()?.get()?)))
            .collect(),
        Err(_) => return,
    };
    for (parent, shapes) in pairs {
        // SAFETY: both are live components (just checked through their weak pointers), on the
        // game thread (the editor's tick).
        unsafe { found.components.copy_custom_depth(parent, shapes) };
    }
}

/// Shows `brick` as this view mode does, after the game updated it (its materials and custom
/// primitive data are its own, or the hidden material if hidden).
///
/// # Safety
/// `brick` must be a live brick in the editor. Game thread only.
pub unsafe fn show(code: &EditorCode, bricks: &Bricks, brick: *mut UObjectBase) {
    let Some(found) = CODE.get() else { return };
    // SAFETY: per the contract.
    unsafe {
        let Some(component) = found.collision.component_of(brick) else {
            remove(brick);
            return;
        };
        let mut shown = match take(brick) {
            Some(shown) if shown.parent.is(component) => shown,
            stale => {
                // Another component shows the brick now: start over.
                if let Some(stale) = stale {
                    destroy(stale);
                }
                Shown {
                    // SAFETY: a live brick and its live component.
                    brick: WeakObject::new(brick),
                    parent: WeakObject::new(component),
                    shapes: None,
                    made: None,
                    material: None,
                }
            }
        };
        found.update_shapes(brick, component, &mut shown);
        found.update_look(code, bricks, brick, component, &mut shown);
        put(brick, shown);
    }
}

/// Updates the shapes of the bricks shown in this view mode whose shapes changed (e.g. resized),
/// after a change of the editor.
///
/// # Safety
/// Every brick must be a live brick in the editor. Game thread only.
pub unsafe fn refresh(code: &EditorCode, bricks: &Bricks, editor_bricks: &[*mut UObjectBase]) {
    let Some(found) = CODE.get() else { return };
    // SAFETY: per the contract.
    unsafe {
        for &brick in editor_bricks {
            match take(brick) {
                Some(mut shown) => {
                    if let Some(component) = found.collision.component_of(brick)
                        && found.update_shapes(brick, component, &mut shown)
                    {
                        found.update_look(code, bricks, brick, component, &mut shown);
                    }
                    put(brick, shown);
                }
                // A brick the game didn't update yet (e.g. just made): shown in full.
                None => show(code, bricks, brick),
            }
        }
    }
}

/// Removes what this view mode added to `brick`, if anything (its own look is the game's again
/// after the update that called this).
///
/// # Safety
/// Game thread only.
pub unsafe fn remove(brick: *mut UObjectBase) {
    // Most calls are for bricks never shown here (every update of every brick, in the editor or
    // not): one lookup.
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

/// Destroys a removed brick's shapes component, if it's still there.
///
/// # Safety
/// Game thread only.
unsafe fn destroy(shown: Shown) {
    let (Some(found), Some(shapes)) = (CODE.get(), shown.shapes.and_then(|shapes| shapes.get()))
    else {
        return;
    };
    // SAFETY: per the contract; a live component we made.
    unsafe { found.components.destroy(shapes) };
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

impl Code {
    /// Makes or changes `brick`'s shapes component if its shapes or scale changed. Returns
    /// whether it made a new one (which shows its mesh's material until [`Code::update_look`]).
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor and `component` its mesh component. Game thread
    /// only.
    unsafe fn update_shapes(
        &self,
        brick: *mut UObjectBase,
        component: *mut UObjectBase,
        shown: &mut Shown,
    ) -> bool {
        // SAFETY: per the contract; components are live (checked through their weak pointers).
        unsafe {
            let (body_setup, static_mesh) = self.collision.body_setup_of(brick, component);
            let scale = match static_mesh {
                Some(static_mesh) => self.collision.scale_of(brick, static_mesh),
                None => [1.0; 3],
            };
            log_type(static_mesh, "setup", || {
                let class =
                    (*reflection::struct_name(reflection::class_of(brick.cast()))).to_string();
                let setup = body_setup.map_or("none".to_string(), |setup| {
                    let owner = (*setup).outer_private;
                    let owner = if owner.is_null() {
                        "null".to_string()
                    } else {
                        (*owner).name_private.to_string()
                    };
                    format!("{} in {owner}", (*setup).name_private)
                });
                let bounds = static_mesh.map_or(String::new(), |mesh| {
                    self.collision.bounds_report(brick, mesh)
                });
                format!("a {class}, body setup {setup}, scale {scale:?}, {bounds}")
            });
            let shapes = match body_setup.map(|body_setup| self.collision.shapes(body_setup)) {
                Some(Ok(shapes)) => shapes,
                Some(Err(reason)) => {
                    log_type(static_mesh, "unreadable", || {
                        format!("collision not readable: {reason}")
                    });
                    Shapes::default()
                }
                None => Shapes::default(),
            };
            // Shapes scaling with their component share one mesh for every scale.
            let mesh_scale = if mesh::scales_linearly(&shapes) {
                [1.0; 3]
            } else {
                scale
            };
            let key = fingerprint(&shapes, mesh_scale);
            let current = shown.shapes.as_ref().and_then(WeakObject::get);
            if shown.made == Some((key, scale)) && (current.is_some() || shapes.is_empty()) {
                return false;
            }
            if shapes.is_empty() {
                log_type(static_mesh, "none", || "no collision".into());
                if let Some(current) = current {
                    self.components.destroy(current);
                }
                shown.shapes = None;
                shown.made = Some((key, scale));
                return false;
            }
            let material = self.components.material(component, 0);
            let Some(mesh) = self.mesh(
                key,
                &shapes,
                scale,
                mesh_scale,
                component,
                material,
                static_mesh,
            ) else {
                log_type(static_mesh, "unbuilt", || "no mesh built".into());
                return false;
            };
            let (shapes_component, created) = match current {
                Some(current) => {
                    if shown.made.is_none_or(|(made, _)| made != key) {
                        self.components.set_mesh(current, mesh);
                    }
                    self.components.set_scale(current, scale);
                    (current, false)
                }
                None => match self.components.create(component, mesh, scale) {
                    Some(created) => (created, true),
                    None => {
                        log_type(static_mesh, "uncreated", || "no mesh component made".into());
                        shown.shapes = None;
                        return false;
                    }
                },
            };
            shown.shapes = Some(WeakObject::new(shapes_component));
            shown.made = Some((key, scale));
            created
        }
    }

    /// The mesh made of `shapes` scaled by `mesh_scale` (built before, or now), known as `key`;
    /// `scale` is the brick's (for the log).
    ///
    /// # Safety
    /// `component` must be a live component, `material` null or a live material, `static_mesh`
    /// the brick's live mesh. Game thread only.
    #[allow(clippy::too_many_arguments)]
    unsafe fn mesh(
        &self,
        key: u64,
        shapes: &Shapes,
        scale: Vector,
        mesh_scale: Vector,
        component: *mut UObjectBase,
        material: *mut UObjectBase,
        static_mesh: Option<*mut UObjectBase>,
    ) -> Option<*mut UObjectBase> {
        let cached = MESHES
            .lock()
            .ok()?
            .get_or_insert_with(HashMap::new)
            .get(&key)
            .and_then(|built| built.0.get());
        if cached.is_some() {
            return cached;
        }
        let built = mesh::drawn(shapes, mesh_scale);
        log_type(static_mesh, "shapes", || {
            describe(shapes, scale, built.flat_hulls)
        });
        // SAFETY: per the contract; the package holding the brick's component lives as long as
        // the component.
        let mesh = unsafe {
            self.builder
                .build(outermost(component), &built.triangles, material)?
        };
        // SAFETY: a live mesh, just made.
        let weak = unsafe { WeakObject::new(mesh) };
        MESHES
            .lock()
            .ok()?
            .get_or_insert_with(HashMap::new)
            .insert(key, BuiltMesh(weak));
        Some(mesh)
    }

    /// Gives `brick`'s shapes component the brick's material (or its plain version, see
    /// [`BrickMaterials::plain_version`]) and custom primitive data, and the brick the hidden
    /// material.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor and `component` its mesh component. Game thread
    /// only.
    unsafe fn update_look(
        &self,
        code: &EditorCode,
        bricks: &Bricks,
        brick: *mut UObjectBase,
        component: *mut UObjectBase,
        shown: &mut Shown,
    ) {
        // SAFETY: per the contract; materials are live objects if not null.
        unsafe {
            let components = &self.components;
            let hidden = bricks.hidden_material(code, brick);
            let current = components.material(component, 0);
            // The brick's own material, unless it's hidden (H: the game gives it the hidden
            // material, and so do we) or showing ours already (a move only updates its data).
            let material = if code.is_hidden(brick) {
                hidden
            } else if !current.is_null() && current != hidden {
                shown.material = Some(WeakObject::new(current));
                for slot in 0..components.material_count(component) {
                    if !hidden.is_null() {
                        components.set_material(component, slot, hidden);
                    }
                }
                current
            } else {
                shown
                    .material
                    .as_ref()
                    .and_then(WeakObject::get)
                    .unwrap_or(current)
            };
            let Some(shapes) = shown.shapes.as_ref().and_then(WeakObject::get) else {
                return;
            };
            let look = match &self.materials {
                // Hidden: what hidden plain bricks show (the brick type's own hidden material may
                // deform the mesh, like its other materials).
                Some(materials) if material == hidden => {
                    let plain = materials.plain_hidden();
                    if plain.is_null() { material } else { plain }
                }
                Some(materials) if !material.is_null() => {
                    materials.plain_version(brick, material, component)
                }
                _ => material,
            };
            if !look.is_null() && components.material(shapes, 0) != look {
                components.set_material(shapes, 0, look);
            }
            components.copy_custom_data(component, shapes);
            components.copy_custom_depth(component, shapes);
            let static_mesh = self.collision.body_setup_of(brick, component).1;
            log_type(static_mesh, "look", || {
                let name = |object: *mut UObjectBase| {
                    if object.is_null() {
                        "null".to_string()
                    } else {
                        (*object).name_private.to_string()
                    }
                };
                let base = |material: *mut UObjectBase| match &self.materials {
                    Some(materials) if !material.is_null() => name(materials.base_of(material)),
                    _ => "?".to_string(),
                };
                let class =
                    (*reflection::struct_name(reflection::class_of(brick.cast()))).to_string();
                let first = |values: Vec<f32>| {
                    format!(
                        "{} floats {:?}",
                        values.len(),
                        &values[..values.len().min(32)]
                    )
                };
                format!(
                    "look of a {class}: brick material {} of {} (hidden {}), shapes material {} of {}, brick data {}, shapes data {}",
                    name(material),
                    base(material),
                    name(hidden),
                    name(components.material(shapes, 0)),
                    base(components.material(shapes, 0)),
                    first(components.custom_values(component)),
                    first(components.custom_values(shapes)),
                )
            });
        }
    }
}

/// The package `object` is in (its outermost outer).
///
/// # Safety
/// `object` must be a live object.
unsafe fn outermost(object: *mut UObjectBase) -> *mut UObjectBase {
    let mut current = object;
    // SAFETY: per the contract; a live object's outers are live.
    unsafe {
        while !(*current).outer_private.is_null() {
            current = (*current).outer_private.cast_mut();
        }
    }
    current
}

/// What a mesh of shapes is made from: the shapes and the scale they're built with, hashed.
fn fingerprint(shapes: &Shapes, scale: Vector) -> u64 {
    fn floats(hasher: &mut DefaultHasher, values: &[f32]) {
        for value in values {
            value.to_bits().hash(hasher);
        }
    }
    let mut hasher = DefaultHasher::new();
    floats(&mut hasher, &scale);
    for shape in &shapes.boxes {
        floats(&mut hasher, &shape.center);
        floats(&mut hasher, &shape.rotation);
        floats(&mut hasher, &shape.size);
    }
    for shape in &shapes.spheres {
        floats(&mut hasher, &shape.center);
        floats(&mut hasher, &[shape.radius]);
    }
    for shape in &shapes.capsules {
        floats(&mut hasher, &shape.center);
        floats(&mut hasher, &shape.rotation);
        floats(&mut hasher, &[shape.radius, shape.length]);
    }
    for points in &shapes.hulls {
        points.len().hash(&mut hasher);
        for point in points {
            floats(&mut hasher, point);
        }
    }
    hasher.finish()
}

/// A brick type's shapes, for the log.
fn describe(shapes: &Shapes, scale: Vector, flat_hulls: usize) -> String {
    let mut parts = Vec::new();
    for (count, what) in [
        (shapes.boxes.len(), "box(es)"),
        (shapes.spheres.len(), "sphere(s)"),
        (shapes.capsules.len(), "capsule(s)"),
        (shapes.hulls.len(), "convex hull(s)"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {what}"));
        }
    }
    if !shapes.hulls.is_empty() {
        let points: usize = shapes.hulls.iter().map(Vec::len).sum();
        parts.push(format!("{points} hull points"));
    }
    if let Some(points) = shapes.hulls.first() {
        let extent = |axis: usize| {
            let (low, high) = points
                .iter()
                .fold((f32::MAX, f32::MIN), |(low, high), point| {
                    (low.min(point[axis]), high.max(point[axis]))
                });
            [low, high]
        };
        parts.push(format!(
            "first hull spans {:?}",
            [extent(0), extent(1), extent(2)]
        ));
    }
    if flat_hulls > 0 {
        parts.push(format!("{flat_hulls} flat hull(s) not shown"));
    }
    if let Some(sphere) = shapes.spheres.first() {
        let smallest = scale.iter().fold(f32::MAX, |a, &b| a.min(b.abs()));
        parts.push(format!(
            "first sphere's radius {} cm, {} cm scaled",
            sphere.radius,
            sphere.radius * smallest
        ));
    }
    if shapes.ignored > 0 {
        parts.push(format!("{} tapered capsule(s) ignored", shapes.ignored));
    }
    format!("{} (first one scaled {scale:?})", parts.join(", "))
}

/// Logs `what` about the brick type showing `static_mesh`, the first time for each `tag` only.
fn log_type(static_mesh: Option<*mut UObjectBase>, tag: &str, what: impl FnOnce() -> String) {
    // SAFETY: a live static mesh (the caller's brick shows it); its name is a valid name.
    let name = static_mesh.map_or_else(
        || "(no mesh)".to_string(),
        |mesh| unsafe { (*mesh).name_private.to_string() },
    );
    let Ok(mut logged) = LOGGED.lock() else {
        return;
    };
    let logged = logged.get_or_insert_with(HashSet::new);
    if logged.len() >= 4 * MAX_LOGGED_TYPES || !logged.insert(format!("{name}/{tag}")) {
        return;
    }
    log!("{name}: {}", what());
}
