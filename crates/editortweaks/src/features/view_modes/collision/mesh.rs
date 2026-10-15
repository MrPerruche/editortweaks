//! Triangles of a brick's collision shapes, for a static mesh.
//!
//! Shapes are scaled the way the engine scales them for PhysX (`GetFinalScaled` of each element:
//! a sphere stays round, a capsule stays a capsule), then given in the space of a component that
//! the engine scales by the same scale: divided by it. The component thus has the brick's own
//! scale, and materials reading the object's space see what they see on the brick. The engine
//! computes the normals: flat faces get vertices of their own, round shapes share theirs.

use core::cmp::Ordering;

use crate::game::collision::{BoxShape, CapsuleShape, Shapes, SphereShape};
use crate::game::editor_object::Vector;
use crate::game::mesh_builder::Triangles;
use crate::math::{self, Quat, Vec3, hull};

/// Rings and steps around of spheres and capsules' caps: smooth enough at brick sizes.
const RINGS: u32 = 8;
const STEPS: u32 = 16;

/// UV units per centimeter: one per meter, planar on each face.
const UV_SCALE: f64 = 0.01;

/// The smallest radius and cylinder length the engine gives a scaled capsule
/// (`FKSphylElem::GetScaledRadius`, `GetScaledCylinderLength`).
const MIN_CAPSULE: f64 = 0.1;

/// What [`triangles`] made: the triangles, and the hulls it couldn't make (all points in a plane).
pub struct Built {
    pub triangles: Triangles,
    pub flat_hulls: usize,
}

/// Whether the triangles of `shapes` are the same for every scale (in the space of a component
/// scaled by it): they're only scaled along the mesh's axes, which the component does. Not so for
/// spheres and capsules (they stay round), nor for turned boxes (they stay boxes).
pub fn scales_linearly(shapes: &Shapes) -> bool {
    shapes.spheres.is_empty()
        && shapes.capsules.is_empty()
        && shapes.boxes.iter().all(|shape| shape.rotation == [0.0; 3])
}

/// The triangles of `shapes` scaled by `scale`, in the space of a component scaled by `scale`.
pub fn triangles(shapes: &Shapes, scale: Vector) -> Built {
    let mut mesh = Mesh::new(math::widen(scale));
    for shape in &shapes.boxes {
        mesh.add_box(shape);
    }
    for shape in &shapes.spheres {
        mesh.add_sphere(shape);
    }
    for shape in &shapes.capsules {
        mesh.add_capsule(shape);
    }
    let mut flat_hulls = 0;
    for points in &shapes.hulls {
        if !mesh.add_hull(points) {
            flat_hulls += 1;
        }
    }
    Built {
        triangles: mesh.triangles,
        flat_hulls,
    }
}

/// Triangles being built, in the collision's scaled space, and the scale to undo when storing.
struct Mesh {
    triangles: Triangles,
    scale: Vec3,
}

impl Mesh {
    fn new(scale: Vec3) -> Self {
        Self {
            triangles: Triangles::default(),
            scale,
        }
    }

    /// Adds a vertex given in the scaled space, with the direction its surface faces (for its UVs),
    /// and returns its index.
    fn vertex(&mut self, position: Vec3, normal: Vec3) -> u32 {
        let local: Vec3 = core::array::from_fn(|axis| position[axis] / self.scale[axis]);
        let triangles = &mut self.triangles;
        triangles.positions.push(math::narrow(local));
        let [u, v] = planar_uv(position, normal);
        triangles.uvs.push([u as f32, v as f32]);
        (triangles.positions.len() - 1) as u32
    }

    /// Adds a triangle given counterclockwise seen from its front (the math convention): the
    /// engine wants front faces clockwise, so the order is reversed.
    fn triangle(&mut self, [a, b, c]: [u32; 3]) {
        self.triangles.corners.push([a, c, b]);
    }

    /// A flat polygon (corners counterclockwise from the front), as a fan, with vertices of its
    /// own (a sharp edge all around).
    fn polygon(&mut self, corners: &[Vec3], normal: Vec3) {
        let indices: Vec<u32> = corners
            .iter()
            .map(|&corner| self.vertex(corner, normal))
            .collect();
        for pair in indices[1..].windows(2) {
            self.triangle([indices[0], pair[0], pair[1]]);
        }
    }

    /// `FKBoxElem::GetFinalScaled`: its lengths scaled along its own axes, its center scaled, its
    /// rotation kept.
    fn add_box(&mut self, shape: &BoxShape) {
        let center = scaled(self.scale, math::widen(shape.center));
        let rotation = Quat::from_rotator(math::widen(shape.rotation));
        let half: Vec3 =
            core::array::from_fn(|axis| f64::from(shape.size[axis]).abs() * self.scale[axis] / 2.0);
        let corner = |signs: [f64; 3]| {
            math::add(
                center,
                rotation.rotate(core::array::from_fn(|axis| signs[axis] * half[axis])),
            )
        };
        // Each face: its outward axis and sign, and two axes spanning it, counterclockwise.
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for sign in [1.0, -1.0] {
                let mut normal = [0.0; 3];
                normal[axis] = sign;
                let at = |su: f64, sv: f64| {
                    let mut signs = [0.0; 3];
                    signs[axis] = sign;
                    signs[u] = su;
                    signs[v] = sv * sign;
                    corner(signs)
                };
                let corners = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
                self.polygon(&corners, rotation.rotate(normal));
            }
        }
    }

    /// `FKSphereElem::GetFinalScaled`: its radius scaled by the smallest scale, its center scaled.
    fn add_sphere(&mut self, shape: &SphereShape) {
        let smallest = self.scale.iter().fold(f64::MAX, |a, &b| a.min(b.abs()));
        let radius = f64::from(shape.radius) * smallest;
        let center = scaled(self.scale, math::widen(shape.center));
        self.add_round(center, Quat::IDENTITY, radius, 0.0);
    }

    /// `FKSphylElem::GetFinalScaled`: its radius scaled by the larger of the X and Y scales, its
    /// whole length (with the caps) by the Z scale, its center scaled, its rotation kept.
    fn add_capsule(&mut self, shape: &CapsuleShape) {
        let [x, y, z] = self.scale.map(f64::abs);
        let (radius, length) = (f64::from(shape.radius), f64::from(shape.length));
        let half_length = (length + radius * 2.0) * z / 2.0;
        let radius = (radius * x.max(y)).clamp(MIN_CAPSULE, half_length.max(MIN_CAPSULE));
        let cylinder = (half_length * 2.0 - radius * 2.0).max(MIN_CAPSULE);
        let center = scaled(self.scale, math::widen(shape.center));
        let rotation = Quat::from_rotator(math::widen(shape.rotation));
        self.add_round(center, rotation, radius, cylinder);
    }

    /// A capsule along the local Z axis (a sphere if `cylinder` is 0), smooth: two half spheres
    /// `cylinder` apart, joined by their equators, every vertex shared by the triangles around it.
    fn add_round(&mut self, center: Vec3, rotation: Quat, radius: f64, cylinder: f64) {
        let mut point = |normal: Vec3, half: f64| {
            let local = math::add(
                math::scale(normal, radius),
                [0.0, 0.0, half * cylinder / 2.0],
            );
            self.vertex(
                math::add(center, rotation.rotate(local)),
                rotation.rotate(normal),
            )
        };
        let bottom = point([0.0, 0.0, -1.0], -1.0);
        // Rings between the poles, from the bottom up; the equator twice if there's a cylinder
        // (once per half).
        let mut rings: Vec<Vec<u32>> = Vec::new();
        for ring in 1..RINGS {
            let latitude = -core::f64::consts::FRAC_PI_2
                + core::f64::consts::PI * f64::from(ring) / f64::from(RINGS);
            let halves: &[f64] = match (ring * 2).cmp(&RINGS) {
                Ordering::Less => &[-1.0],
                Ordering::Greater => &[1.0],
                Ordering::Equal if cylinder > 0.0 => &[-1.0, 1.0],
                Ordering::Equal => &[0.0],
            };
            for &half in halves {
                let indices = (0..STEPS)
                    .map(|step| {
                        let longitude = core::f64::consts::TAU * f64::from(step) / f64::from(STEPS);
                        let normal = [
                            latitude.cos() * longitude.cos(),
                            latitude.cos() * longitude.sin(),
                            latitude.sin(),
                        ];
                        point(normal, half)
                    })
                    .collect();
                rings.push(indices);
            }
        }
        let top = point([0.0, 0.0, 1.0], 1.0);

        let steps = STEPS as usize;
        let next = |step: usize| (step + 1) % steps;
        // Counterclockwise seen from outside: around the Z axis, then up.
        if let (Some(first), Some(last)) = (rings.first().cloned(), rings.last().cloned()) {
            for step in 0..steps {
                self.triangle([bottom, first[next(step)], first[step]]);
                self.triangle([last[step], last[next(step)], top]);
            }
        }
        for pair in rings.windows(2) {
            let (below, above) = (&pair[0], &pair[1]);
            for step in 0..steps {
                self.triangle([below[step], below[next(step)], above[next(step)]]);
                self.triangle([below[step], above[next(step)], above[step]]);
            }
        }
    }

    /// A convex hull of `points` (unscaled, scaled here like PhysX scales the cooked hull); false
    /// if they have no volume.
    fn add_hull(&mut self, points: &[Vector]) -> bool {
        let points: Vec<Vec3> = points
            .iter()
            .map(|&point| scaled(self.scale, math::widen(point)))
            .collect();
        let Some(faces) = hull::convex_hull(&points) else {
            return false;
        };
        for [a, b, c] in faces {
            let corners = [points[a], points[b], points[c]];
            let normal = math::normalized(math::cross(
                math::sub(corners[1], corners[0]),
                math::sub(corners[2], corners[0]),
            ));
            if let Some(normal) = normal {
                self.polygon(&corners, normal);
            }
        }
        true
    }
}

/// `point` scaled per axis by `scale`.
fn scaled(scale: Vec3, point: Vec3) -> Vec3 {
    core::array::from_fn(|axis| point[axis] * scale[axis])
}

/// The two axes a face with `normal` is projected onto for its UVs: the ones it faces least.
fn projection_axes(normal: Vec3) -> (usize, usize) {
    let facing = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .unwrap_or(2);
    ((facing + 1) % 3, (facing + 2) % 3)
}

/// UVs projected from the axis the face faces most.
fn planar_uv(position: Vec3, normal: Vec3) -> [f64; 2] {
    let (u, v) = projection_axes(normal);
    [position[u] * UV_SCALE, position[v] * UV_SCALE]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> Shapes {
        Shapes {
            boxes: vec![BoxShape {
                center: [0.0; 3],
                rotation: [0.0; 3],
                size: [2.0, 2.0, 2.0],
            }],
            ..Shapes::default()
        }
    }

    /// The volume enclosed by triangles (in their own space), positive if their fronts face out
    /// the way the engine wants (clockwise).
    fn volume(triangles: &Triangles) -> f64 {
        triangles
            .corners
            .iter()
            .map(|corners| {
                let [a, b, c] = corners.map(|i| math::widen(triangles.positions[i as usize]));
                // Reversed: the engine's front faces are clockwise.
                math::dot(a, math::cross(c, b)) / 6.0
            })
            .sum()
    }

    /// Whether the triangles close up: each edge used twice, once each way.
    fn is_closed(triangles: &Triangles) -> bool {
        let edges: Vec<(u32, u32)> = triangles
            .corners
            .iter()
            .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
            .collect();
        let unique: std::collections::HashSet<_> = edges.iter().copied().collect();
        unique.len() == edges.len() && edges.iter().all(|&(a, b)| unique.contains(&(b, a)))
    }

    #[test]
    fn box_encloses_its_volume_facing_out() {
        let built = triangles(&unit_box(), [1.0; 3]);
        assert_eq!(built.triangles.corners.len(), 12);
        assert!((volume(&built.triangles) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn scale_is_undone_in_the_component_space() {
        // Scaled by the component, the box is 2 x 4 x 6: here it stays 2 x 2 x 2.
        let built = triangles(&unit_box(), [1.0, 2.0, 3.0]);
        assert!((volume(&built.triangles) - 8.0).abs() < 1e-5);
        assert!(scales_linearly(&unit_box()));
    }

    #[test]
    fn sphere_stays_round_when_scaled_unevenly() {
        let shapes = Shapes {
            spheres: vec![SphereShape {
                center: [0.0; 3],
                radius: 10.0,
            }],
            ..Shapes::default()
        };
        assert!(!scales_linearly(&shapes));
        let scale = [2.0, 1.0, 1.0];
        let built = triangles(&shapes, scale);
        // Scaled back up by the component, every vertex is 10 (the smallest scale) from the
        // center.
        for vertex in &built.triangles.positions {
            let world: Vec3 = core::array::from_fn(|axis| f64::from(vertex[axis] * scale[axis]));
            assert!((math::length(world) - 10.0).abs() < 1e-3);
        }
        assert!(volume(&built.triangles) > 0.0);
        // Smooth: one vertex per point, shared all around.
        assert!(is_closed(&built.triangles));
        assert_eq!(
            built.triangles.positions.len(),
            2 + (RINGS as usize - 1) * STEPS as usize
        );
    }

    #[test]
    fn capsule_closes_up_around_its_volume() {
        let shapes = Shapes {
            capsules: vec![CapsuleShape {
                center: [0.0; 3],
                rotation: [0.0; 3],
                radius: 1.0,
                length: 4.0,
            }],
            ..Shapes::default()
        };
        let built = triangles(&shapes, [1.0; 3]);
        assert!(is_closed(&built.triangles));
        let exact = core::f64::consts::PI * (4.0 + 4.0 / 3.0);
        let found = volume(&built.triangles);
        assert!(found < exact && found > exact * 0.9, "{found} vs {exact}");
    }

    #[test]
    fn hull_matches_its_box() {
        let mut points = Vec::new();
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    points.push([x, y, z]);
                }
            }
        }
        let shapes = Shapes {
            hulls: vec![points, vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]],
            ..Shapes::default()
        };
        let built = triangles(&shapes, [1.0; 3]);
        assert_eq!(built.flat_hulls, 1);
        assert!((volume(&built.triangles) - 8.0).abs() < 1e-5);
    }
}
