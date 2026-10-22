//! Where world points show on the screen, worked out from three rays of the camera (through the
//! view's middle, its right edge's middle and its bottom edge's middle), so placing a path's
//! hundreds of points doesn't take a call into the game each. Works for the perspective view and
//! the orthographic one (`ortho_view`: parallel rays from different origins). The tooltip checks it
//! against the game's own projection and uses the game's if they disagree.

use crate::game::viewport::Ray;
use crate::math::{self, Vec3};

/// How the camera maps world points to viewport pixels.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    origin: Vec3,
    forward: Vec3,
    right: Vec3,
    down: Vec3,
    /// The view's middle (pixels).
    middle: [f64; 2],
    /// Pixels per unit sideways and down: per tangent of the angle off the middle (perspective),
    /// or per cm (orthographic).
    scale: [f64; 2],
    orthographic: bool,
}

impl Projection {
    /// The projection of a view `size` pixels wide and high, from the rays through its middle,
    /// its right edge's middle and its bottom edge's middle; `None` if they make no sense.
    pub fn new(middle: Ray, right: Ray, bottom: Ray, size: [i32; 2]) -> Option<Self> {
        let half = [f64::from(size[0]) * 0.5, f64::from(size[1]) * 0.5];
        let origin = math::widen(middle.origin);
        let forward = math::normalized(math::widen(middle.direction))?;
        let edge = |ray: Ray, half: f64| -> Option<(Vec3, f64, bool)> {
            let direction = math::normalized(math::widen(ray.direction))?;
            let along = math::dot(direction, forward);
            let side = math::sub(direction, math::scale(forward, along));
            let tangent = math::length(side) / along;
            if along > 0.0 && tangent > 1e-6 {
                return Some((math::normalized(side)?, half / tangent, false));
            }
            // Parallel rays: the orthographic view, its edge an offset of the origin.
            let offset = math::sub(math::widen(ray.origin), origin);
            let side = math::sub(offset, math::scale(forward, math::dot(offset, forward)));
            let width = math::length(side);
            (width > 1e-6).then(|| (math::scale(side, 1.0 / width), half / width, true))
        };
        let (right_axis, scale_x, orthographic) = edge(right, half[0])?;
        let (down, scale_y, _) = edge(bottom, half[1])?;
        // The rays start on the near plane, not at the eye: back from there by the near plane's
        // distance, which the edge ray's start tells (as far to the side as the angle makes it).
        let origin = if orthographic {
            origin
        } else {
            let offset = math::sub(math::widen(right.origin), origin);
            let lateral = math::dot(offset, right_axis);
            let near = lateral * scale_x / half[0];
            math::sub(origin, math::scale(forward, near.max(0.0)))
        };
        Some(Self {
            origin,
            forward,
            right: right_axis,
            down,
            middle: half,
            scale: [scale_x, scale_y],
            orthographic,
        })
    }

    /// Where `point` shows (viewport pixels), if in front of the camera.
    pub fn project(&self, point: Vec3) -> Option<[f32; 2]> {
        let offset = math::sub(point, self.origin);
        let depth = math::dot(offset, self.forward);
        let divide = if self.orthographic {
            1.0
        } else if depth > 1e-3 {
            depth
        } else {
            return None;
        };
        let x = self.middle[0] + math::dot(offset, self.right) / divide * self.scale[0];
        let y = self.middle[1] + math::dot(offset, self.down) / divide * self.scale[1];
        (x.is_finite() && y.is_finite()).then_some([x as f32, y as f32])
    }
}
