//! Vector math for bricks' boxes, in double precision (see [`crate::math`]): frames (a brick's own
//! axes, or the vehicle's in the world), boxes, and rays against boxes, lines and planes.
//!
//! The tool works in the vehicle's space, where bricks keep their places (their spawn location and
//! rotation): those are exact, unlike their places in the world, which the game computes in `f32`
//! far from the world's origin (the editor isn't at it), a few thousandths of a millimeter off.

use crate::game::editor_object::{Transform, Vector};
use crate::math::{self, Quat};
pub use crate::math::{Vec3, add, dot, length, normalized, scale, sub};

/// A ray: its origin and unit direction.
#[derive(Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// The point `distance` along `ray`.
pub fn along(ray: &Ray, distance: f64) -> Vec3 {
    add(ray.origin, scale(ray.direction, distance))
}

/// A place and rotation: a brick's own axes and origin in the vehicle, or the vehicle's in the
/// world.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: Vec3,
    pub rotation: Quat,
    /// The rotated axes (length 1).
    pub axes: [Vec3; 3],
}

impl Frame {
    pub fn new(origin: Vec3, rotation: Quat) -> Self {
        Self {
            origin,
            rotation,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|axis| rotation.rotate(axis)),
        }
    }

    /// Where the editor places a brick: its location and rotation (pitch, yaw, roll).
    pub fn of_place(location: Vector, rotation: Vector) -> Self {
        Self::new(
            math::widen(location),
            Quat::from_rotator(math::widen(rotation)),
        )
    }

    /// A transform's place and rotation (bricks' transforms aren't scaled).
    pub fn of_transform(transform: &Transform) -> Self {
        let [x, y, z, _] = transform.translation;
        Self::new(math::widen([x, y, z]), Quat::widen(transform.rotation))
    }

    /// The frame whose space this one is given in, from where this one is in the world:
    /// `world = parent * self`.
    pub fn parent(&self, world: &Frame) -> Frame {
        let rotation = world.rotation * self.rotation.inverse();
        Frame::new(sub(world.origin, rotation.rotate(self.origin)), rotation)
    }

    /// A direction given along the frame's axes, outside it.
    pub fn direction(&self, local: Vec3) -> Vec3 {
        let [x, y, z] = self.axes;
        add(
            add(scale(x, local[0]), scale(y, local[1])),
            scale(z, local[2]),
        )
    }

    /// A point given in the frame, outside it.
    pub fn point(&self, local: Vec3) -> Vec3 {
        add(self.origin, self.direction(local))
    }

    /// A direction outside the frame, along its axes.
    pub fn local_direction(&self, outside: Vec3) -> Vec3 {
        self.axes.map(|axis| dot(outside, axis))
    }

    /// A point outside the frame, in it.
    pub fn local(&self, outside: Vec3) -> Vec3 {
        self.local_direction(sub(outside, self.origin))
    }

    /// A ray outside the frame, in it.
    pub fn local_ray(&self, ray: &Ray) -> Ray {
        Ray {
            origin: self.local(ray.origin),
            direction: self.local_direction(ray.direction),
        }
    }
}

/// A box along a frame's axes: its smallest and largest corner.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    /// The coordinate of its side along `axis`: the smallest (`side` < 0), the largest (> 0), or
    /// the middle (0).
    pub fn side(&self, axis: usize, side: i8) -> f64 {
        match side.signum() {
            -1 => self.min[axis],
            1 => self.max[axis],
            _ => (self.min[axis] + self.max[axis]) / 2.0,
        }
    }

    /// The point on the given side along each axis (see [`Self::side`]).
    pub fn point(&self, sides: [i8; 3]) -> Vec3 {
        core::array::from_fn(|axis| self.side(axis, sides[axis]))
    }

    pub fn extent(&self, axis: usize) -> f64 {
        self.max[axis] - self.min[axis]
    }

    /// The box grown by `margin` on every side.
    pub fn grown(&self, margin: f64) -> Self {
        Self {
            min: self.min.map(|value| value - margin),
            max: self.max.map(|value| value + margin),
        }
    }
}

/// Where a ray (in the box's frame) enters the box: its distance along the ray and the face it
/// enters through (axis, and whether the largest side). `None` if it misses, or starts inside.
pub fn ray_box(ray: &Ray, bounds: &Bounds) -> Option<(f64, usize, bool)> {
    let mut enter = (f64::NEG_INFINITY, 0, false);
    let mut exit = f64::INFINITY;
    for axis in 0..3 {
        let (origin, direction) = (ray.origin[axis], ray.direction[axis]);
        if direction.abs() < 1e-12 {
            if origin < bounds.min[axis] || origin > bounds.max[axis] {
                return None;
            }
            continue;
        }
        let to_min = (bounds.min[axis] - origin) / direction;
        let to_max = (bounds.max[axis] - origin) / direction;
        // Entering through the side facing the ray.
        let (near, far, positive) = if to_min < to_max {
            (to_min, to_max, false)
        } else {
            (to_max, to_min, true)
        };
        if near > enter.0 {
            enter = (near, axis, positive);
        }
        exit = exit.min(far);
    }
    (enter.0 >= 0.0 && enter.0 <= exit).then_some(enter)
}

/// How far along the line through `point` along `direction` (length 1) its point closest to the
/// ray is. `None` if the line runs along the ray.
pub fn closest_on_line(ray: &Ray, point: Vec3, direction: Vec3) -> Option<f64> {
    let between = sub(ray.origin, point);
    let cosine = dot(ray.direction, direction);
    let denominator = 1.0 - cosine * cosine;
    if denominator < 1e-6 {
        return None;
    }
    Some((dot(direction, between) - cosine * dot(ray.direction, between)) / denominator)
}

/// The point of the segment from `start` to `end` closest to the ray.
pub fn closest_on_segment(ray: &Ray, start: Vec3, end: Vec3) -> Vec3 {
    let Some(direction) = normalized(sub(end, start)) else {
        return start;
    };
    let length = length(sub(end, start));
    let distance = closest_on_line(ray, start, direction).unwrap_or(0.0);
    add(start, scale(direction, distance.clamp(0.0, length)))
}

/// How far along the ray it meets the plane through `point` across `normal`, if in front.
/// `min_cosine` is the smallest angle's cosine between the ray and the plane accepted (rays almost
/// along the plane give unsteady points).
pub fn ray_plane(ray: &Ray, point: Vec3, normal: Vec3, min_cosine: f64) -> Option<f64> {
    let cosine = dot(ray.direction, normal);
    if cosine.abs() < min_cosine {
        return None;
    }
    let distance = dot(sub(point, ray.origin), normal) / cosine;
    (distance >= 0.0).then_some(distance)
}

/// The distance on the screen from `point` to the segment from `start` to `end` (pixels).
pub fn screen_distance(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let along = [end[0] - start[0], end[1] - start[1]];
    let to_point = [point[0] - start[0], point[1] - start[1]];
    let squared = along[0] * along[0] + along[1] * along[1];
    let share = if squared > 0.0 {
        ((to_point[0] * along[0] + to_point[1] * along[1]) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = [start[0] + along[0] * share, start[1] + along[1] * share];
    ((point[0] - closest[0]).powi(2) + (point[1] - closest[1]).powi(2)).sqrt()
}
