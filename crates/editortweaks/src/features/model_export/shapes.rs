//! Bricks' shapes as triangles placed in the world, for the export. Like `drag_select`'s shapes
//! (what a click hits):
//!
//! - Most bricks: their mesh's complex collision triangles (`game::brick_model`, the render mesh's
//!   collision LOD; for meshes without, mods' often, and meshes with several material slots,
//!   wheels', LOD 0 read from the game's pak files, which says each triangle's slot), scaled
//!   to the brick's box like the game scales the mesh
//!   (`brick_model::scale_to_fit`). Wheels' tires and flaps are bent by their material in game: here
//!   they keep their mesh's shape, stretched to their box.
//! - Spinners: their visible cross-section (`game::spinner_profiles`) swept along their arc in as
//!   many steps as the game's mesh (`model_io::spinner`), at their size. Without it, the convex
//!   pieces the game builds along their arc for their collision (their own body setup, read
//!   through `game::collision`), their faces worked out (`math::hull`).
//! - Anything else (no readable shape, or one not fitting its box): its box.
//!
//! With [`Source::Model`] past LOD 0, static meshes show that less detailed LOD (read from the
//! game's pak files), scaled as LOD 0 is: a LOD can be a little smaller than the mesh.
//!
//! With [`Source::Collision`], every brick shows its simple collision instead, the shapes PhysX
//! simulates contacts with (`game::collision`, as the Collision view mode shows them): boxes,
//! spheres, capsules and convex pieces, scaled the way the engine scales them. A brick without
//! collision is left out.

use std::collections::HashMap;
use std::sync::Arc;

use brickrust::ue::coreuobject::UObjectBase;

use model_io::Part;
use model_io::export;
use model_io::spinner::{self as spinner_shape, Spinner};

use crate::features::view_modes::collision::mesh as collision_mesh;
use crate::game::brick_model::{self, BrickModels, Triangles};
use crate::game::collision::{Collision, Shapes as CollisionShapes};
use crate::game::editor_object::Transform;
use crate::game::spinner::Spinners;
use crate::game::spinner_profiles::SpinnerProfiles;
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat, Vec3, hull};

/// Triangles in a brick's space, unscaled, and the box around them.
pub struct Mesh {
    points: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    /// Each triangle's material slot, empty if they're all the first's (or it isn't known).
    slots: Vec<u32>,
    /// Each triangle's corners' UVs, empty if they aren't known.
    uvs: Vec<[[f32; 2]; 3]>,
    /// Smallest and largest corner.
    bounds: [Vec3; 2],
}

impl Mesh {
    /// A static mesh's triangles (its complex collision, or its render triangles with their
    /// material slots).
    fn from_triangles(triangles: &Triangles) -> Result<Self, String> {
        let points = triangles.vertices.iter().map(|&p| math::widen(p)).collect();
        Ok(Self {
            slots: triangles.slots.clone(),
            uvs: triangles.uvs.clone(),
            ..Self::new(points, triangles.triangles.clone())?
        })
    }

    /// A brick's simple collision, in its own space (`scale` is the one the game gives its body
    /// setup).
    fn from_collision(shapes: &CollisionShapes, scale: [f32; 3]) -> Result<Self, String> {
        // Built in the space of a component scaled by `scale`: scaled back.
        let built = collision_mesh::triangles(shapes, scale);
        let points = built
            .triangles
            .positions
            .iter()
            .map(|position| core::array::from_fn(|axis| f64::from(position[axis] * scale[axis])))
            .collect();
        Self::new(points, built.triangles.corners)
    }

    /// A spinner's surface, from its values and visible cross-section (`None` without one).
    fn swept(spinner: &Spinner, profile: &[[f32; 2]], quarter_turns: i32) -> Option<Self> {
        let profile = spinner_shape::profile(profile, quarter_turns)?;
        let (points, triangles) = spinner.sweep(&profile);
        Self::new(points.into_iter().map(math::widen).collect(), triangles).ok()
    }

    /// Collision made of convex pieces (a spinner's): the faces around each piece.
    fn from_pieces(shapes: &CollisionShapes) -> Result<Self, String> {
        let mut points: Vec<Vec3> = Vec::new();
        let mut triangles = Vec::new();
        for piece in &shapes.hulls {
            let corners: Vec<Vec3> = piece.iter().map(|&corner| math::widen(corner)).collect();
            // A flat piece has no faces.
            let Some(faces) = hull::convex_hull(&corners) else {
                continue;
            };
            let first = points.len();
            triangles.extend(
                faces
                    .iter()
                    .map(|face| face.map(|corner| (first + corner) as u32)),
            );
            points.extend(corners);
        }
        Self::new(points, triangles)
    }

    fn new(points: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Result<Self, String> {
        let first = *points.first().ok_or("no points")?;
        if triangles.is_empty() {
            return Err("no triangles".into());
        }
        let bounds = points.iter().fold([first, first], |[low, high], point| {
            [
                core::array::from_fn(|axis| low[axis].min(point[axis])),
                core::array::from_fn(|axis| high[axis].max(point[axis])),
            ]
        });
        Ok(Self {
            points,
            triangles,
            slots: Vec::new(),
            uvs: Vec::new(),
            bounds,
        })
    }
}

/// What bricks are exported as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// What the game draws, at LOD `level` (0 the most detailed) where meshes have it.
    Model { level: usize },
    /// Their simple collision.
    Collision,
}

/// How a brick shows a [`Mesh`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// Scaled to its box (a static mesh's triangles).
    Scaled,
    /// As it is (a spinner's pieces, made at its size).
    AsIs,
}

/// Which way a brick was exported, for the summary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Mesh,
    /// A spinner's cross-section swept along its arc.
    Swept,
    /// A spinner's collision pieces.
    Pieces,
    Box,
    /// Its simple collision.
    Collision,
    /// It has no collision: left out.
    Nothing,
}

/// What's known of each triangle of a placed shape, in the order of its part's triangles (empty
/// where it isn't known).
#[derive(Default)]
pub struct Faces {
    /// Its material slot (empty if they're all the first's).
    pub slots: Vec<u32>,
    /// Its corners' UVs, in the order of the part's corners.
    pub uvs: Vec<[[f32; 2]; 3]>,
}

/// Spinners' values and cross-sections.
pub type SpinnerLooks = (Spinners, SpinnerProfiles);

/// Reads bricks' shapes, each mesh once per export.
pub struct Shapes<'a> {
    source: Source,
    models: Option<&'a BrickModels>,
    collision: Option<&'a Collision>,
    spinners: Option<&'a SpinnerLooks>,
    /// Meshes read so far, by what they're read from (a static mesh, a spinner's body setup).
    read: HashMap<usize, (WeakObject, Result<Arc<Mesh>, String>)>,
}

impl<'a> Shapes<'a> {
    pub fn new(
        source: Source,
        models: Option<&'a BrickModels>,
        collision: Option<&'a Collision>,
        spinners: Option<&'a SpinnerLooks>,
    ) -> Self {
        Self {
            source,
            models,
            collision,
            spinners,
            read: HashMap::new(),
        }
    }

    /// `brick`'s shape at `place` (its world transform), its box (in itself) going from corner `a`
    /// to corner `b` (inside out along the axes it's mirrored along): triangles facing out, how it
    /// was found (no triangles for [`Kind::Nothing`]), and what's known of each triangle.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn placed(
        &mut self,
        brick: *mut UObjectBase,
        place: &Transform,
        [a, b]: [Vec3; 2],
    ) -> (Part, Kind, Faces) {
        let mirrored: [bool; 3] = core::array::from_fn(|axis| a[axis] > b[axis]);
        if self.source == Source::Collision {
            // SAFETY: per the contract.
            return match unsafe { self.collision_of(brick) } {
                Some(mesh) => {
                    // In the brick's space at its size: only turned inside out where mirrored.
                    let scale = mirrored.map(|mirrored| if mirrored { -1.0 } else { 1.0 });
                    (
                        Self::part(&mesh, scale, place),
                        Kind::Collision,
                        Faces::default(),
                    )
                }
                None => (Part::default(), Kind::Nothing, Faces::default()),
            };
        }
        let exact: [Vec3; 2] = [
            core::array::from_fn(|axis| a[axis].min(b[axis])),
            core::array::from_fn(|axis| a[axis].max(b[axis])),
        ];
        // SAFETY: per the contract.
        let shape = unsafe { self.shape(brick) };
        let fitted = shape.and_then(|(mesh, fit, kind)| {
            let scale = match fit {
                Fit::Scaled => brick_model::scale_to_fit(mesh.bounds, mirrored, exact)?,
                Fit::AsIs => [1.0; 3],
            };
            Some((mesh, scale, kind))
        });
        match fitted {
            Some((mesh, scale, kind)) => {
                let part = Self::part(&mesh, scale, place);
                // Placing turns every triangle around or none (swapping their last two corners).
                let turned = part.corners.first() != mesh.triangles.first();
                let mut uvs = mesh.uvs.clone();
                if turned {
                    for corners in &mut uvs {
                        corners.swap(1, 2);
                    }
                }
                let faces = Faces {
                    slots: mesh.slots.clone(),
                    uvs,
                };
                (part, kind, faces)
            }
            None => {
                let mut part = export::box_part(math::narrow(exact[0]), math::narrow(exact[1]));
                Self::place(&mut part, place);
                (part, Kind::Box, Faces::default())
            }
        }
    }

    /// `mesh` scaled by `scale` (in the brick's space), placed at `place`.
    fn part(mesh: &Mesh, scale: Vec3, place: &Transform) -> Part {
        let mut part = Part {
            positions: mesh
                .points
                .iter()
                .map(|point| math::narrow(core::array::from_fn(|axis| point[axis] * scale[axis])))
                .collect(),
            corners: mesh.triangles.clone(),
            ..Part::default()
        };
        Self::place(&mut part, place);
        part
    }

    /// Moves `part` from its brick's space to the world, at `place`, facing out.
    fn place(part: &mut Part, place: &Transform) {
        let [x, y, z, _] = place.translation;
        let location = math::widen([x, y, z]);
        let rotation = Quat::widen(place.rotation);
        for position in &mut part.positions {
            let world = math::add(location, rotation.rotate(math::widen(*position)));
            *position = math::narrow(world);
        }
        // Mirrored bricks and some sources (PhysX, hulls) wind the other way.
        export::face_outward(part);
    }

    /// `brick`'s simple collision in its own space at its size, `None` if it has none (or it can't
    /// be read).
    ///
    /// # Safety
    /// As for [`Self::placed`].
    unsafe fn collision_of(&self, brick: *mut UObjectBase) -> Option<Mesh> {
        let collision = self.collision?;
        // SAFETY: per the contract; body setups and meshes are live if not null.
        unsafe {
            let component = collision.component_of(brick)?;
            let (body_setup, mesh) = collision.body_setup_of(brick, component);
            let scale = match mesh {
                Some(mesh) => collision.scale_of(brick, mesh),
                None => [1.0; 3],
            };
            // Scalable bricks of a kind share a body setup at different sizes: built every time.
            let shapes = collision.shapes(body_setup?).ok()?;
            Mesh::from_collision(&shapes, scale).ok()
        }
    }

    /// What `brick` shows and how: a spinner's swept cross-section or its own collision pieces,
    /// else its mesh's complex collision. `None` to show its box.
    ///
    /// # Safety
    /// As for [`Self::placed`].
    unsafe fn shape(&mut self, brick: *mut UObjectBase) -> Option<(Arc<Mesh>, Fit, Kind)> {
        // SAFETY: per the contract; body setups and meshes are live if not null.
        unsafe {
            if let Some(swept) = self.swept(brick) {
                return Some((Arc::new(swept), Fit::AsIs, Kind::Swept));
            }
            if let Some(collision) = self.collision
                && collision.has_own_body_setup(brick)
            {
                let body_setup = collision
                    .component_of(brick)
                    .and_then(|component| collision.body_setup_of(brick, component).0)?;
                return self
                    .get(body_setup, || {
                        Mesh::from_pieces(&collision.shapes(body_setup)?)
                    })
                    .ok()
                    .map(|pieces| (pieces, Fit::AsIs, Kind::Pieces));
            }
            let models = self.models?;
            let mesh = models.mesh_of(brick)?;
            let level = match self.source {
                Source::Model { level } => level,
                Source::Collision => 0,
            };
            self.get(mesh, || {
                let full = Mesh::from_triangles(&models.triangles_by_slot(mesh)?)?;
                if level == 0 {
                    return Ok(full);
                }
                // Scaled as LOD 0; LOD 0 itself where the one wanted can't be read.
                match models.level(mesh, level) {
                    Ok((_, coarse)) => Ok(Mesh {
                        bounds: full.bounds,
                        ..Mesh::from_triangles(&coarse)?
                    }),
                    Err(_) => Ok(full),
                }
            })
            .ok()
            .map(|mesh| (mesh, Fit::Scaled, Kind::Mesh))
        }
    }

    /// `brick`'s surface if it's a spinner whose cross-section reads (each spinner differs, so it
    /// isn't kept).
    ///
    /// # Safety
    /// As for [`Self::placed`].
    unsafe fn swept(&self, brick: *mut UObjectBase) -> Option<Mesh> {
        let (spinners, profiles) = self.spinners?;
        // SAFETY: per the contract.
        unsafe {
            if !spinners.is_spinner(brick) {
                return None;
            }
            let values = spinners.read(brick);
            let profiles = profiles.of(brick)?;
            let spinner = Spinner {
                radius: values.radius.map(f64::from),
                size: values.size.map(f64::from),
                angle: f64::from(values.angle),
            };
            Mesh::swept(&spinner, &profiles.visible, profiles.quarter_turns)
        }
    }

    /// The mesh read from `source`, with `read` the first time.
    ///
    /// # Safety
    /// `source` must be a live object. Game thread only.
    unsafe fn get(
        &mut self,
        source: *mut UObjectBase,
        read: impl FnOnce() -> Result<Mesh, String>,
    ) -> Result<Arc<Mesh>, String> {
        let key = source as usize;
        if let Some((known, mesh)) = self.read.get(&key)
            && known.get() == Some(source)
        {
            return mesh.clone();
        }
        let mesh = read().map(Arc::new);
        // SAFETY: per the contract.
        let weak = unsafe { WeakObject::new(source) };
        self.read.insert(key, (weak, mesh.clone()));
        mesh
    }
}
