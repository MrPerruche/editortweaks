//! Resizing a brick's box by one of its parts: where the part's sides go for the mouse, freely or
//! snapped to another brick, and the size and place giving the brick that box.
//!
//! A brick's box scales with its size along each of its axes, around its origin (its mesh is
//! scaled by its size), so a size gives a box, and the brick then moves for the sides not dragged
//! to stay where they were.

use super::geometry::{self, Bounds, Frame, Ray, Vec3};
use super::parts::Part;

/// How close a dragged side may come to the opposite one (cm): boxes don't turn inside out. The
/// brick's own minimum size usually stops it earlier.
const MIN_EXTENT: f64 = 0.01;

/// The smallest cosine between the mouse ray and the plane an edge is dragged in: closer to edge-on,
/// the edge stops following (small mouse moves would throw it far).
const MIN_PLANE_COSINE: f64 = 0.08;

/// A brick's box being resized by one of its parts, as it was when grabbed.
pub struct Resize {
    pub part: Part,
    /// The brick's own axes in the vehicle.
    pub frame: Frame,
    /// Its box.
    pub bounds: Bounds,
    /// Its size (`BrickSize`).
    pub size: Vec3,
    /// Where it was grabbed, in its frame.
    pub grab: Vec3,
}

/// What the dragged part snaps to: a point of another brick's corner, edge or face (the one under
/// the mouse), and for a face, the face's normal.
pub struct Target {
    pub point: Vec3,
    pub normal: Option<Vec3>,
}

impl Resize {
    /// Where the dragged sides go to follow the mouse `ray`, in the brick's frame (only the
    /// coordinates along the part's axes count): a face along its axis, an edge across its length,
    /// a corner along the diagonal through it (all three sizes in proportion). `None` while the ray
    /// gives no steady answer (looking along the axis, or edge-on at the plane).
    pub fn follow(&self, ray: &Ray) -> Option<Vec3> {
        let handle = self.handle();
        let grab = self.frame.point(self.grab);
        let mut sides = handle;
        match self.part.dimensions_fixed() {
            1 => {
                let axis = self.part.axes().next()?;
                let moved = geometry::closest_on_line(ray, grab, self.frame.axes[axis])?;
                sides[axis] = handle[axis] + moved;
            }
            2 => {
                let along = (0..3).find(|&axis| self.part.sides[axis] == 0)?;
                let distance =
                    geometry::ray_plane(ray, grab, self.frame.axes[along], MIN_PLANE_COSINE)?;
                let point = self.frame.local(geometry::along(ray, distance));
                for axis in self.part.axes() {
                    sides[axis] = handle[axis] + point[axis] - self.grab[axis];
                }
            }
            _ => {
                let anchor = self.anchor();
                let diagonal = geometry::sub(handle, anchor);
                let length = geometry::length(diagonal);
                let direction = geometry::normalized(self.frame.direction(diagonal))?;
                let moved = geometry::closest_on_line(ray, grab, direction)?;
                let factor = (length + moved) / length;
                sides = geometry::add(anchor, geometry::scale(diagonal, factor));
            }
        }
        Some(sides)
    }

    /// Where the dragged sides go to meet `target` (see [`Self::follow`]). Onto an edge or corner
    /// each side goes to the target's point; onto a face, a dragged edge or corner keeps its sizes
    /// in proportion and stops on the face's plane (if it can reach it; else as for an edge). A
    /// dragged face goes to the target's point either way: the mouse picks where on the target.
    pub fn snap(&self, target: &Target) -> Vec3 {
        let point = self.frame.local(target.point);
        let mut sides = self.handle();
        if let Some(normal) = target.normal
            && self.part.dimensions_fixed() > 1
            && let Some(factor) = self.proportional_to_plane(point, target.point, normal)
        {
            let anchor = self.anchor();
            for axis in self.part.axes() {
                sides[axis] = anchor[axis] + (sides[axis] - anchor[axis]) * factor;
            }
            return sides;
        }
        for axis in self.part.axes() {
            sides[axis] = point[axis];
        }
        sides
    }

    /// How much the dragged part's distance to the opposite part grows (as a factor) for it to
    /// reach the plane through `on_plane` (world; `point` in the brick's frame) across `normal`.
    fn proportional_to_plane(&self, point: Vec3, on_plane: Vec3, normal: Vec3) -> Option<f64> {
        let anchor = self.anchor();
        let handle = self.handle();
        // The dragged part's point that reaches the plane: along the axes it spans (an edge's
        // length), the one nearest the target.
        let mut start = anchor;
        let mut offset: Vec3 = [0.0; 3];
        for axis in 0..3 {
            if self.part.sides[axis] == 0 {
                start[axis] = point[axis].clamp(self.bounds.min[axis], self.bounds.max[axis]);
            } else {
                offset[axis] = handle[axis] - anchor[axis];
            }
        }
        let across = geometry::dot(normal, self.frame.direction(offset));
        if across.abs() < 1e-4 {
            return None;
        }
        let factor =
            geometry::dot(normal, geometry::sub(on_plane, self.frame.point(start))) / across;
        (factor > 0.0).then_some(factor)
    }

    /// The box with the dragged sides at `sides` (see [`Self::follow`]), each kept from crossing
    /// the opposite one.
    pub fn resized(&self, sides: Vec3) -> Bounds {
        let mut bounds = self.bounds;
        for axis in self.part.axes() {
            if self.part.sides[axis] > 0 {
                bounds.max[axis] = sides[axis].max(bounds.min[axis] + MIN_EXTENT);
            } else {
                bounds.min[axis] = sides[axis].min(bounds.max[axis] - MIN_EXTENT);
            }
        }
        bounds
    }

    /// The size giving the brick a box as big as `bounds`.
    pub fn size_for(&self, bounds: &Bounds) -> Vec3 {
        core::array::from_fn(|axis| {
            let before = self.bounds.extent(axis);
            if before > 0.0 {
                self.size[axis] * bounds.extent(axis) / before
            } else {
                self.size[axis]
            }
        })
    }

    /// The box the brick gets with `size`, in its frame as grabbed, and how far its origin moves
    /// (along its axes) to keep the sides not dragged in place: the opposite side along each
    /// dragged axis, the middle along the others.
    pub fn placed(&self, size: Vec3) -> (Bounds, Vec3) {
        let mut bounds = self.bounds;
        let mut shift: Vec3 = [0.0; 3];
        for axis in 0..3 {
            let ratio = if self.size[axis] != 0.0 {
                size[axis] / self.size[axis]
            } else {
                1.0
            };
            let kept = self.bounds.side(axis, -self.part.sides[axis]);
            shift[axis] = kept - kept * ratio;
            bounds.min[axis] = self.bounds.min[axis] * ratio + shift[axis];
            bounds.max[axis] = self.bounds.max[axis] * ratio + shift[axis];
        }
        (bounds, shift)
    }

    /// The dragged part's sides, as grabbed.
    fn handle(&self) -> Vec3 {
        self.bounds.point(self.part.sides)
    }

    /// The opposite part's sides: what stays in place.
    fn anchor(&self) -> Vec3 {
        self.bounds.point(self.part.sides.map(|side| -side))
    }
}
