//! Arrows as triangles, for a procedural mesh (`game::procedural_mesh`): a round shaft and a cone
//! for its head, closed at both ends. Many go in one mesh. Plus where a ray meets one, for the
//! mouse.
//!
//! The game has no arrow to borrow: its debug arrows (`DrawDebugDirectionalArrow`) aren't in
//! shipping builds, and the editor's own (`UBrickEditorArrowComponent`) is a flat line.

use crate::math::{self, Vec3};

/// Sides around a shaft and a head.
const SIDES: usize = 12;

/// An arrow's head is this many times as long as its thickness, and its shaft this share of it
/// across.
const HEAD_LENGTH: f64 = 2.5;
const SHAFT_WIDTH: f64 = 0.4;

/// An arrow: from its tail to the tip of its head, as thick as its head's base.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrow {
    pub tail: Vec3,
    pub tip: Vec3,
    pub thickness: f64,
}

impl Arrow {
    /// Its thickness, smaller if the arrow is too short for a whole head (the head stays at most
    /// half of it).
    fn fitted_thickness(&self) -> f64 {
        let length = math::length(math::sub(self.tip, self.tail));
        self.thickness.min(length / (2.0 * HEAD_LENGTH))
    }

    /// How far along `ray` (its origin, a unit direction) it meets the arrow, give or take
    /// `slack` (cm per cm along the ray: a few pixels, for thin far ones); `None` if it misses.
    pub fn hit(&self, origin: Vec3, direction: Vec3, slack: f64) -> Option<f64> {
        let axis = math::sub(self.tip, self.tail);
        let length = math::length(axis);
        if length <= 0.0 {
            return None;
        }
        let along_arrow = math::scale(axis, 1.0 / length);
        // The closest points of the two lines.
        let gap = math::sub(origin, self.tail);
        let b = math::dot(direction, along_arrow);
        let (d, e) = (math::dot(direction, gap), math::dot(along_arrow, gap));
        let denominator = 1.0 - b * b;
        let on_arrow = if denominator > 1e-12 {
            ((e - b * d) / denominator).clamp(0.0, length)
        } else {
            0.0
        };
        let point = math::add(self.tail, math::scale(along_arrow, on_arrow));
        let along_ray = math::dot(math::sub(point, origin), direction);
        if along_ray <= 0.0 {
            return None;
        }
        let closest = math::add(origin, math::scale(direction, along_ray));
        let miss = math::length(math::sub(point, closest));
        let radius = self.fitted_thickness() / 2.0 + slack * along_ray;
        (miss <= radius).then_some(along_ray)
    }
}

/// Triangles for a procedural mesh section, in the space of the component showing them.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// Three indices into `positions`, clockwise seen from the front (Unreal's winding).
    pub corners: Vec<[u32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
}

impl Mesh {
    /// Adds `arrow` (nothing if it has no length).
    pub fn add(&mut self, arrow: &Arrow) {
        let axis = math::sub(arrow.tip, arrow.tail);
        let length = math::length(axis);
        let thickness = arrow.fitted_thickness();
        if length <= 0.0 || thickness <= 0.0 {
            return;
        }
        let forward = math::scale(axis, 1.0 / length);
        let [side, up] = across(forward);
        let ring = |center: Vec3, radius: f64| -> Vec<(Vec3, Vec3)> {
            (0..SIDES)
                .map(|index| {
                    let turn = index as f64 / SIDES as f64 * core::f64::consts::TAU;
                    let (sin, cos) = turn.sin_cos();
                    let out = math::add(math::scale(side, cos), math::scale(up, sin));
                    (math::add(center, math::scale(out, radius)), out)
                })
                .collect()
        };
        let head_base = math::sub(arrow.tip, math::scale(forward, HEAD_LENGTH * thickness));
        let (shaft, head) = (SHAFT_WIDTH * thickness / 2.0, thickness / 2.0);
        let back = math::scale(forward, -1.0);

        // The shaft's side, shaded round.
        let tail_ring = ring(arrow.tail, shaft);
        let neck_ring = ring(head_base, shaft);
        for index in 0..SIDES {
            let next = (index + 1) % SIDES;
            let [a, b] = [tail_ring[index], tail_ring[next]];
            let [c, d] = [neck_ring[index], neck_ring[next]];
            self.quad([a, b, d, c]);
        }
        // The tail's cap and the back of the head, flat.
        for index in 0..SIDES {
            let next = (index + 1) % SIDES;
            self.triangle(
                [arrow.tail, tail_ring[index].0, tail_ring[next].0],
                [back; 3],
            );
        }
        let head_ring = ring(head_base, head);
        for index in 0..SIDES {
            let next = (index + 1) % SIDES;
            let corners = [
                neck_ring[index].0,
                head_ring[index].0,
                head_ring[next].0,
                neck_ring[next].0,
            ];
            self.triangle([corners[0], corners[1], corners[2]], [back; 3]);
            self.triangle([corners[0], corners[2], corners[3]], [back; 3]);
        }
        // The head's cone: each side meets the tip with its own normal, leaning forward.
        let lean = head / (HEAD_LENGTH * thickness);
        for index in 0..SIDES {
            let next = (index + 1) % SIDES;
            let normal = |out: Vec3| {
                math::normalized(math::add(out, math::scale(forward, lean))).unwrap_or(out)
            };
            let (a, a_out) = head_ring[index];
            let (b, b_out) = head_ring[next];
            let middle = math::normalized(math::add(a_out, b_out)).unwrap_or(a_out);
            self.triangle(
                [a, b, arrow.tip],
                [normal(a_out), normal(b_out), normal(middle)],
            );
        }
    }

    /// A side of the shaft: four corners going round it (each with its outward direction).
    fn quad(&mut self, corners: [(Vec3, Vec3); 4]) {
        let [a, b, c, d] = corners;
        self.triangle([a.0, b.0, c.0], [a.1, b.1, c.1]);
        self.triangle([a.0, c.0, d.0], [a.1, c.1, d.1]);
    }

    /// A triangle facing the way its normals lean, whichever order its corners come in.
    fn triangle(&mut self, corners: [Vec3; 3], normals: [Vec3; 3]) {
        let [a, b, c] = corners;
        // Unreal's front: `(c - a) x (b - a)`.
        let front = math::cross(math::sub(c, a), math::sub(b, a));
        let facing = normals
            .iter()
            .fold([0.0; 3], |sum, &normal| math::add(sum, normal));
        let first = self.positions.len() as u32;
        for (corner, normal) in corners.into_iter().zip(normals) {
            self.positions.push(math::narrow(corner));
            self.normals.push(math::narrow(normal));
            self.uvs.push([0.0, 0.0]);
        }
        if math::dot(front, facing) >= 0.0 {
            self.corners.push([first, first + 1, first + 2]);
        } else {
            self.corners.push([first, first + 2, first + 1]);
        }
    }
}

/// Two unit directions across `forward` (unit length) and each other.
fn across(forward: Vec3) -> [Vec3; 2] {
    // Any direction not along `forward` to start from.
    let start = if forward[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let side = math::normalized(math::cross(forward, start)).unwrap_or([0.0, 1.0, 0.0]);
    [side, math::cross(side, forward)]
}
