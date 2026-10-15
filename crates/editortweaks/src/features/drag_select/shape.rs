//! Bricks' shapes, for picking: what a click's trace hits (see `docs/re/selection.md`), placed in
//! the world like the game places it.
//!
//! - Most bricks: their mesh's complex collision triangles (read through `game::brick_model`),
//!   scaled to the brick's box like the game scales the mesh (see `brick_model::scale_to_fit`).
//! - Spinners: the convex pieces the game builds along their arc, at their size (their own body
//!   setup, read through `game::collision`); the pieces' faces are worked out here (`math::hull`).
//!
//! A brick whose shape can't be read, or doesn't fit its box (one showing other meshes too), is
//! picked by its box. The box comes first anyway: a brick the view misses or takes in whole needs
//! no triangle.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;

use brickrust::ue::coreuobject::UObjectBase;

use super::frustum::{BrickBox, Frustum};
use crate::game::brick_model::{self, Triangles};
use crate::game::collision::Shapes;
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat, Vec3, hull};

/// How far a spinner's pieces may stick out of its box (worked out on its own, from its arc): this
/// share of the box's size, plus [`LOOSE_MARGIN`]. Further, something is off, and the spinner is
/// picked by its box.
const LOOSE_SHARE: f64 = 0.1;
/// Centimeters.
const LOOSE_MARGIN: f64 = 1.0;

/// Triangles, in a brick's space, and the box around them.
pub struct Mesh {
    points: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    /// Smallest and largest corner.
    bounds: [Vec3; 2],
}

impl Mesh {
    /// A static mesh's complex collision (unscaled).
    pub fn from_triangles(triangles: &Triangles) -> Result<Self, String> {
        let points = triangles.vertices.iter().map(|&p| math::widen(p)).collect();
        Self::new(points, triangles.triangles.clone())
    }

    /// Collision made of convex pieces (a spinner's): the faces around each piece.
    pub fn from_pieces(shapes: &Shapes) -> Result<Self, String> {
        if !shapes.boxes.is_empty() || !shapes.spheres.is_empty() || !shapes.capsules.is_empty() {
            return Err("collision with other shapes than convex pieces".into());
        }
        let mut points: Vec<Vec3> = Vec::new();
        let mut triangles = Vec::new();
        for piece in &shapes.hulls {
            let corners: Vec<Vec3> = piece.iter().map(|&corner| math::widen(corner)).collect();
            // A flat piece (all its points in a plane) has no inside: left out, as the collision
            // view leaves it out.
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
            bounds,
        })
    }

    /// Its smallest and largest corner.
    pub fn bounds(&self) -> [Vec3; 2] {
        self.bounds
    }
}

/// Meshes read so far, by what they're read from (a static mesh, a spinner's body setup): each is
/// read once (they stay while bricks use them; one that's gone is read again).
#[derive(Default)]
pub struct Meshes(HashMap<usize, Read>);

struct Read {
    source: WeakObject,
    /// Its mesh, or why there's none.
    mesh: Result<Arc<Mesh>, String>,
}

impl Meshes {
    /// The mesh read from `source`, with `read` the first time.
    ///
    /// # Safety
    /// `source` must be a live object. Game thread only.
    pub unsafe fn get(
        &mut self,
        source: *mut UObjectBase,
        read: impl FnOnce() -> Result<Mesh, String>,
    ) -> Result<Arc<Mesh>, String> {
        let key = source as usize;
        if let Some(known) = self.0.get(&key)
            && known.source.get() == Some(source)
        {
            return known.mesh.clone();
        }
        let mesh = read().map(Arc::new);
        self.0.insert(
            key,
            Read {
                // SAFETY: per the contract.
                source: unsafe { WeakObject::new(source) },
                mesh: mesh.clone(),
            },
        );
        mesh
    }
}

/// How a brick shows its mesh.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Scaled to its box, as the game scales a brick's mesh (a static mesh's triangles).
    Scaled,
    /// As it is (a spinner's pieces, made at its size).
    AsIs,
}

/// Where a brick is in the world: its location and rotation (not scaled).
#[derive(Clone, Copy)]
pub struct Place {
    pub location: Vec3,
    pub rotation: Quat,
}

impl Place {
    /// `point` (in the brick) in the world.
    fn point(&self, point: Vec3) -> Vec3 {
        math::add(self.location, self.rotation.rotate(point))
    }

    /// The box from `low` to `high` (in the brick) in the world.
    fn bounds(&self, [low, high]: [Vec3; 2]) -> BrickBox {
        BrickBox {
            center: self.point(middle([low, high])),
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|axis| self.rotation.rotate(axis)),
            half: core::array::from_fn(|axis| (high[axis] - low[axis]) / 2.0),
        }
    }
}

/// A brick as a box picks it.
pub struct Shape {
    /// A box in the world holding all of it: its box, grown to hold its mesh.
    bounds: BrickBox,
    /// Its center: its box's.
    center: Vec3,
    /// Its mesh in the world, if it has one that fits its box.
    model: Option<Model>,
}

/// A mesh as a brick shows it.
struct Model {
    mesh: Arc<Mesh>,
    fit: Fit,
    place: Place,
    /// Along the brick's axes.
    scale: Vec3,
    /// The mesh's points in the world, worked out the first time they're needed.
    points: OnceCell<Vec<Vec3>>,
}

impl Model {
    fn points(&self) -> &[Vec3] {
        self.points.get_or_init(|| {
            self.mesh
                .points
                .iter()
                .map(|&point| {
                    let scaled = core::array::from_fn(|axis| point[axis] * self.scale[axis]);
                    self.place.point(scaled)
                })
                .collect()
        })
    }

    /// The box around it, in the brick.
    fn bounds(&self) -> [Vec3; 2] {
        let [low, high] = self.mesh.bounds;
        let ends = |axis: usize| {
            let (a, b) = (low[axis] * self.scale[axis], high[axis] * self.scale[axis]);
            (a.min(b), a.max(b))
        };
        [
            core::array::from_fn(|axis| ends(axis).0),
            core::array::from_fn(|axis| ends(axis).1),
        ]
    }
}

impl Shape {
    /// A brick at `place` whose box (in itself) goes from corner `a` to corner `b` (inside out
    /// along the axes it's mirrored along), showing `mesh` if it fits that box.
    pub fn new(place: Place, [a, b]: [Vec3; 2], mesh: Option<(Arc<Mesh>, Fit)>) -> Self {
        let mirrored = core::array::from_fn(|axis| a[axis] > b[axis]);
        let exact = [
            core::array::from_fn(|axis| a[axis].min(b[axis])),
            core::array::from_fn(|axis| a[axis].max(b[axis])),
        ];
        let model = mesh.and_then(|(mesh, fit)| {
            let scale = match fit {
                Fit::Scaled => brick_model::scale_to_fit(mesh.bounds, mirrored, exact)?,
                Fit::AsIs => holds_loosely(exact, mesh.bounds).then_some([1.0; 3])?,
            };
            Some(Model {
                mesh,
                fit,
                place,
                scale,
                points: OnceCell::new(),
            })
        });
        let outer = match &model {
            Some(model) => {
                let [low, high] = model.bounds();
                [
                    core::array::from_fn(|axis| exact[0][axis].min(low[axis])),
                    core::array::from_fn(|axis| exact[1][axis].max(high[axis])),
                ]
            }
            None => exact,
        };
        Self {
            bounds: place.bounds(outer),
            center: place.point(middle(exact)),
            model,
        }
    }

    /// How it shows its mesh, if it has one (else it's picked by its box).
    pub fn fit(&self) -> Option<Fit> {
        self.model.as_ref().map(|model| model.fit)
    }

    /// Whether any part of it is in `view`.
    pub fn touched_by(&self, view: &Frustum) -> bool {
        if !view.touches(&self.bounds) {
            return false;
        }
        let Some(model) = &self.model else {
            return true;
        };
        if view.encloses(&self.bounds) {
            return true;
        }
        let points = model.points();
        points.iter().any(|&point| view.contains(point))
            || model
                .mesh
                .triangles
                .iter()
                .any(|triangle| view.touches_triangle(triangle.map(|index| points[index as usize])))
    }

    /// Whether all of it is in `view`.
    pub fn inside(&self, view: &Frustum) -> bool {
        if view.encloses(&self.bounds) {
            return true;
        }
        match &self.model {
            // The view is convex: holding every corner, it holds every triangle.
            Some(model) => {
                view.touches(&self.bounds)
                    && model.points().iter().all(|&point| view.contains(point))
            }
            None => false,
        }
    }

    /// Its center: its box's.
    pub fn center(&self) -> Vec3 {
        self.center
    }
}

/// The point halfway between `low` and `high`.
fn middle([low, high]: [Vec3; 2]) -> Vec3 {
    core::array::from_fn(|axis| (low[axis] + high[axis]) / 2.0)
}

/// Whether the box `inner` (smallest and largest corner) is about within `outer`: by
/// [`LOOSE_SHARE`] of its size and [`LOOSE_MARGIN`].
fn holds_loosely(outer: [Vec3; 2], inner: [Vec3; 2]) -> bool {
    (0..3).all(|axis| {
        let margin = (outer[1][axis] - outer[0][axis]) * LOOSE_SHARE + LOOSE_MARGIN;
        inner[0][axis] >= outer[0][axis] - margin && inner[1][axis] <= outer[1][axis] + margin
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::viewport::Ray;

    fn place() -> Place {
        Place {
            location: [0.0; 3],
            rotation: Quat::IDENTITY,
        }
    }

    /// A 10 cm cube from the origin, as a convex piece.
    fn cube() -> Shapes {
        let corners = (0..8)
            .map(|index| core::array::from_fn(|axis| ((index >> axis) & 1) as f32 * 10.0))
            .collect();
        Shapes {
            hulls: vec![corners],
            ..Shapes::default()
        }
    }

    #[test]
    fn pieces_make_closed_faces() {
        let mesh = Mesh::from_pieces(&cube()).unwrap();
        assert_eq!(mesh.points.len(), 8);
        // Two triangles per side.
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.bounds, [[0.0; 3], [10.0; 3]]);
    }

    #[test]
    fn flat_pieces_are_left_out() {
        let mut shapes = cube();
        shapes.hulls.push(vec![
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ]);
        assert_eq!(Mesh::from_pieces(&shapes).unwrap().points.len(), 8);
        let flat = Shapes {
            hulls: vec![vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]],
            ..Shapes::default()
        };
        assert!(Mesh::from_pieces(&flat).is_err());
    }

    #[test]
    fn pieces_keep_their_size_within_the_box() {
        let mesh = Arc::new(Mesh::from_pieces(&cube()).unwrap());
        // Its box a little smaller than the pieces (worked out on its own): still taken.
        let shape = Shape::new(
            place(),
            [[0.5; 3], [9.5; 3]],
            Some((mesh.clone(), Fit::AsIs)),
        );
        assert_eq!(shape.fit(), Some(Fit::AsIs));
        let model = shape.model.as_ref().unwrap();
        assert_eq!(model.scale, [1.0; 3]);
        // The box picking starts with holds all of it.
        assert_eq!(shape.bounds.half, [5.0; 3]);
        assert_eq!(shape.center(), [5.0; 3]);
        // A box far smaller: something's off, picked by its box.
        let shape = Shape::new(place(), [[0.0; 3], [2.0; 3]], Some((mesh, Fit::AsIs)));
        assert_eq!(shape.fit(), None);
    }

    /// A spinner's pieces as the game builds them: a 10 cm square cross-section swept along half a
    /// ring (80 to 90 cm from its middle) in 8 steps, each piece from one step to the next.
    fn half_ring() -> Shapes {
        const STEPS: usize = 8;
        let corner = |step: usize, radius: f32, z: f32| {
            let angle = core::f32::consts::PI * step as f32 / STEPS as f32;
            [radius * angle.cos(), radius * angle.sin(), z]
        };
        let hulls = (0..STEPS)
            .map(|step| {
                [step, step + 1]
                    .into_iter()
                    .flat_map(|at| {
                        [(80.0, -5.0), (90.0, -5.0), (80.0, 5.0), (90.0, 5.0)]
                            .map(|(radius, z)| corner(at, radius, z))
                    })
                    .collect()
            })
            .collect();
        Shapes {
            hulls,
            ..Shapes::default()
        }
    }

    /// The view down through the square `half` cm around `middle` (x, y), from high above.
    fn view_down([x, y]: [f32; 2], half: f32) -> Frustum {
        let rays = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].map(|[dx, dy]| Ray {
            origin: [x + dx * half, y + dy * half, 1000.0],
            direction: [0.0, 0.0, -1.0],
        });
        Frustum::new(&rays).unwrap()
    }

    #[test]
    fn spinners_are_picked_by_their_pieces() {
        let mesh = Arc::new(Mesh::from_pieces(&half_ring()).unwrap());
        // Its box as the game works it out, from the arc.
        let shape = Shape::new(
            place(),
            [[-90.0, 0.0, -5.0], [90.0, 90.0, 5.0]],
            Some((mesh, Fit::AsIs)),
        );
        assert_eq!(shape.fit(), Some(Fit::AsIs));
        // In the ring's hole: in its box, but not on it.
        assert!(!shape.touched_by(&view_down([0.0, 30.0], 10.0)));
        // On the ring.
        assert!(shape.touched_by(&view_down([0.0, 85.0], 2.0)));
        assert!(shape.touched_by(&view_down([60.0, 60.0], 2.0)));
        // Around all of it, around a part.
        assert!(shape.inside(&view_down([0.0, 45.0], 100.0)));
        assert!(!shape.inside(&view_down([0.0, 45.0], 60.0)));
    }
}
