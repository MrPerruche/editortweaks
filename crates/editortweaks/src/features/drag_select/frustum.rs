//! What a box drawn on the screen covers in the world: the view through it, a pyramid from the
//! camera (a [`Frustum`]), and whether it takes in bricks' boxes, points and triangles. In the
//! world's space, in double precision.

use crate::game::viewport::Ray;
use crate::math::{self, Vec3};

/// How far the view through the box reaches (cm): further than anything in the editor.
const FAR: f64 = 1.0e6;

/// A brick's box in the world: its center, its axes (length 1) and its half sizes along them.
#[derive(Clone, Copy)]
pub struct BrickBox {
    pub center: Vec3,
    pub axes: [Vec3; 3],
    pub half: Vec3,
}

impl BrickBox {
    /// How far it reaches from its center along `direction` (length 1).
    fn reach(&self, direction: Vec3) -> f64 {
        (0..3)
            .map(|axis| self.half[axis] * math::dot(direction, self.axes[axis]).abs())
            .sum()
    }
}

/// A plane, its normal (length 1) pointing inside the view.
#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    offset: f64,
}

impl Plane {
    /// The plane through `point` with `normal`, turned so that `inside` is in front of it.
    fn facing(point: Vec3, normal: Vec3, inside: Vec3) -> Option<Self> {
        let mut normal = math::normalized(normal)?;
        if math::dot(normal, math::sub(inside, point)) < 0.0 {
            normal = math::scale(normal, -1.0);
        }
        Some(Self {
            normal,
            offset: math::dot(normal, point),
        })
    }

    /// How far `point` is inside (negative: outside).
    fn depth(&self, point: Vec3) -> f64 {
        math::dot(self.normal, point) - self.offset
    }
}

/// The view through a box on the screen: four sides through the camera and the box's edges, and
/// the near side (the camera's near plane), out to [`FAR`].
pub struct Frustum {
    planes: [Plane; 5],
    /// Its corners: on the near plane, then far away.
    corners: [Vec3; 8],
    /// The directions of its edges: from the camera along the box's corners, and along the box's
    /// edges.
    edges: [Vec3; 8],
    /// The camera (the middle of the box's corners on the near plane).
    camera: Vec3,
}

impl Frustum {
    /// The view through the box whose corners show the rays `rays` (world space, in order around
    /// the box); `None` for a box without area.
    pub fn new(rays: &[Ray; 4]) -> Option<Self> {
        let near: [Vec3; 4] = rays.map(|ray| math::widen(ray.origin));
        let directions: [Vec3; 4] = rays.map(|ray| math::widen(ray.direction));
        let far: [Vec3; 4] = core::array::from_fn(|index| {
            math::add(near[index], math::scale(directions[index], FAR))
        });
        let corners: [Vec3; 8] = core::array::from_fn(|index| {
            if index < 4 {
                near[index]
            } else {
                far[index - 4]
            }
        });
        // Every point between the corners is inside.
        let inside = math::scale(
            corners.iter().fold([0.0; 3], |sum, &c| math::add(sum, c)),
            1.0 / 8.0,
        );
        let camera = math::scale(
            near.iter().fold([0.0; 3], |sum, &c| math::add(sum, c)),
            0.25,
        );
        let forward = directions
            .iter()
            .fold([0.0; 3], |sum, &d| math::add(sum, d));
        let side = |index: usize| {
            let next = (index + 1) % 4;
            let normal = math::cross(
                math::sub(far[index], near[index]),
                math::sub(far[next], near[index]),
            );
            Plane::facing(near[index], normal, inside)
        };
        let planes = [
            side(0)?,
            side(1)?,
            side(2)?,
            side(3)?,
            Plane::facing(camera, forward, inside)?,
        ];
        let edges = core::array::from_fn(|index| {
            if index < 4 {
                directions[index]
            } else {
                let index = index - 4;
                math::sub(far[(index + 1) % 4], far[index])
            }
        });
        Some(Self {
            planes,
            corners,
            edges,
            camera,
        })
    }

    /// Whether `point` is in the view.
    pub fn contains(&self, point: Vec3) -> bool {
        self.planes.iter().all(|plane| plane.depth(point) >= 0.0)
    }

    /// Whether `brick` is entirely in the view.
    pub fn encloses(&self, brick: &BrickBox) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.depth(brick.center) >= brick.reach(plane.normal))
    }

    /// Whether any part of `brick` is in the view: no plane separates them (the separating axis
    /// test between two convex shapes: their faces' normals, and the crossings of their edges).
    pub fn touches(&self, brick: &BrickBox) -> bool {
        if self
            .planes
            .iter()
            .any(|plane| plane.depth(brick.center) < -brick.reach(plane.normal))
        {
            return false;
        }
        let crossings = self
            .edges
            .iter()
            .flat_map(|&edge| brick.axes.map(|axis| math::cross(edge, axis)));
        for axis in brick.axes.into_iter().chain(crossings) {
            // Parallel edges cross nowhere: no axis.
            let Some(axis) = math::normalized(axis) else {
                continue;
            };
            let (low, high) = self
                .corners
                .iter()
                .map(|&corner| math::dot(axis, corner))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), along| {
                    (low.min(along), high.max(along))
                });
            let center = math::dot(axis, brick.center);
            let reach = brick.reach(axis);
            if center + reach < low || center - reach > high {
                return false;
            }
        }
        true
    }

    /// Whether any part of the triangle `corners` is in the view: whether anything is left of it
    /// once cut by each side of the view in turn (Sutherland-Hodgman; out to `FAR` doesn't matter).
    pub fn touches_triangle(&self, corners: [Vec3; 3]) -> bool {
        // Each side cuts at most one more corner in: 3 + 5.
        const MAX_CORNERS: usize = 8;
        let mut polygon = [[0.0; 3]; MAX_CORNERS];
        polygon[..3].copy_from_slice(&corners);
        let mut count = 3;
        for plane in &self.planes {
            let mut kept = [[0.0; 3]; MAX_CORNERS];
            let mut kept_count = 0;
            for index in 0..count {
                let (point, next) = (polygon[index], polygon[(index + 1) % count]);
                let (here, there) = (plane.depth(point), plane.depth(next));
                if here >= 0.0 {
                    kept[kept_count] = point;
                    kept_count += 1;
                }
                if (here >= 0.0) != (there >= 0.0) {
                    // Where the edge crosses the side.
                    let share = here / (here - there);
                    kept[kept_count] = math::add(point, math::scale(math::sub(next, point), share));
                    kept_count += 1;
                }
            }
            if kept_count == 0 {
                return false;
            }
            (polygon, count) = (kept, kept_count);
        }
        true
    }

    /// How far `point` is from the camera.
    pub fn distance(&self, point: Vec3) -> f64 {
        math::length(math::sub(point, self.camera))
    }
}
