//! Resizing a brick's box by what's grabbed of it (a part of its box, or of its model): where the
//! box's sides go for the mouse, freely or snapped to another brick, and the size and place giving
//! the brick that box.
//!
//! A brick's box scales with its size along each of its axes, around its origin (its mesh is
//! scaled by its size), so a size gives a box, and the brick then moves for the sides not dragged
//! to stay where they were. Everything of the brick scales the same way: along each dragged axis
//! around the opposite side (the anchor), by a ratio.

use super::geometry::{self, Bounds, Frame, Ray, Vec3};
use super::model::{BrickModel, Element};
use super::parts::Part;
use super::scene::Piece;

/// How close a dragged side may come to the opposite one (cm): boxes don't turn inside out. The
/// brick's own minimum size usually stops it earlier.
const MIN_EXTENT: f64 = 0.01;

/// The smallest cosine between the mouse ray and the plane an edge is dragged in: closer to edge-on,
/// the edge stops following (small mouse moves would throw it far).
const MIN_PLANE_COSINE: f64 = 0.08;

/// How far from the box's middle a model's vertex or edge must be along an axis to drag that axis
/// (as a share of the box's size there), and how much of a face's normal must lie along it.
const SIDE_TOLERANCE: f64 = 1e-4;
const NORMAL_TOLERANCE: f64 = 1e-6;

/// What's grabbed of a brick: the sides of its box it drags (per axis, see [`Part`]; the opposite
/// sides stay), and its shape, which follows the mouse and snaps.
#[derive(Clone, Copy, Debug)]
pub struct Grip {
    pub part: Part,
    pub shape: Shape,
}

/// The shape of what's grabbed, in the brick.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// A box's corner or a model's vertex: moves away from the anchor, its sizes in proportion.
    Point(Vec3),
    /// A box's or model's edge: moves across its length (in the plane of the two axes it drags).
    Segment(Vec3, Vec3),
    /// A box's or model's face (a point on it, its outward normal): moves along its normal, keeping
    /// its slope.
    Plane { point: Vec3, normal: Vec3 },
}

impl Grip {
    /// Grabbing a brick (with box `bounds`, model `model`) by `piece`, if that drags any side.
    pub fn of(piece: Piece, bounds: &Bounds, model: Option<&BrickModel>) -> Option<Self> {
        match piece {
            Piece::Box(part) => Some(Self::of_box(part, bounds)),
            Piece::Model(element) => Self::of_model(element, bounds, model?),
        }
    }

    fn of_box(part: Part, bounds: &Bounds) -> Self {
        let corners = part.corners(bounds);
        let shape = match part.dimensions_fixed() {
            1 => {
                let axis = part.axes().next().unwrap_or(0);
                let mut normal = [0.0; 3];
                normal[axis] = f64::from(part.sides[axis]);
                Shape::Plane {
                    point: bounds.point(part.sides),
                    normal,
                }
            }
            2 => Shape::Segment(corners[0], corners[1]),
            _ => Shape::Point(corners[0]),
        };
        Self { part, shape }
    }

    /// A model's vertex or edge drags the sides of the box's halves it's in (along axes where it's
    /// off the middle, its points all on one side); a face, the sides its normal points to.
    fn of_model(element: Element, bounds: &Bounds, model: &BrickModel) -> Option<Self> {
        let side = |axis: usize, value: f64| -> i8 {
            let middle = bounds.side(axis, 0);
            let tolerance = SIDE_TOLERANCE * bounds.extent(axis);
            if value > middle + tolerance {
                1
            } else if value < middle - tolerance {
                -1
            } else {
                0
            }
        };
        let (sides, shape) = match element {
            Element::Vertex(vertex) => {
                let point = model.point(vertex);
                let sides = core::array::from_fn(|axis| side(axis, point[axis]));
                (sides, Shape::Point(point))
            }
            Element::Edge(edge) => {
                let [start, end] = model.edge(edge);
                let sides = core::array::from_fn(|axis| {
                    let first = side(axis, start[axis]);
                    if first == side(axis, end[axis]) {
                        first
                    } else {
                        0
                    }
                });
                (sides, Shape::Segment(start, end))
            }
            Element::Face(face) => {
                let normal = model.normal(face);
                let sides = normal.map(|value| {
                    if value.abs() <= NORMAL_TOLERANCE {
                        0
                    } else {
                        value.signum() as i8
                    }
                });
                let point = model.face_edges(face).next()?[0];
                (sides, Shape::Plane { point, normal })
            }
        };
        let part = Part { sides };
        (part.dimensions_fixed() > 0).then_some(Self { part, shape })
    }
}

/// A brick's box being resized by what's grabbed of it, as it was when grabbed.
pub struct Resize {
    pub grip: Grip,
    /// The brick's own axes in the vehicle.
    pub frame: Frame,
    /// Its box.
    pub bounds: Bounds,
    /// Its size (`BrickSize`).
    pub size: Vec3,
    /// Where it was grabbed, in its frame.
    pub grab: Vec3,
}

/// What the dragged part snaps to: a point of another brick (see `super::snap`), and for a point on
/// a face, the face's normal (in the vehicle).
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub point: Vec3,
    pub normal: Option<Vec3>,
}

impl Resize {
    /// Where the dragged sides go to follow the mouse `ray`, in the brick's frame (only the
    /// coordinates along the dragged axes count). A face moves along its normal (a box's face along
    /// its axis), keeping its slope; else by how many axes are dragged: along one axis, across an
    /// edge's length in the plane of two, along the line from the anchor through a corner or vertex
    /// (all its sizes in proportion). `None` while the ray gives no steady answer (looking along
    /// the way it moves, or edge-on at the plane).
    pub fn follow(&self, ray: &Ray) -> Option<Vec3> {
        let grab = self.frame.point(self.grab);
        let anchor = self.anchor();
        let axes: Vec<usize> = self.grip.part.axes().collect();
        let handle = self.handle(self.grab);
        let ratios = match (self.grip.shape, axes.as_slice()) {
            (Shape::Plane { point, normal }, _) => {
                let offset = geometry::dot(normal, geometry::sub(point, anchor));
                if offset.abs() < MIN_EXTENT {
                    return None;
                }
                let moved = geometry::closest_on_line(ray, grab, self.frame.direction(normal))?;
                self.uniform((offset + moved) / offset)
            }
            (_, &[axis]) => {
                let moved = geometry::closest_on_line(ray, grab, self.frame.axes[axis])?;
                self.ratios(|axis| handle[axis] + moved, handle)
            }
            (Shape::Segment(..), &[first, second]) => {
                let along = 3 - first - second;
                let distance =
                    geometry::ray_plane(ray, grab, self.frame.axes[along], MIN_PLANE_COSINE)?;
                let point = self.frame.local(geometry::along(ray, distance));
                self.ratios(|axis| handle[axis] + point[axis] - self.grab[axis], handle)
            }
            _ => {
                let diagonal = self.restricted(geometry::sub(handle, anchor));
                let length = geometry::length(diagonal);
                let direction = geometry::normalized(self.frame.direction(diagonal))?;
                let moved = geometry::closest_on_line(ray, grab, direction)?;
                self.uniform((length + moved) / length)
            }
        };
        Some(self.sides(ratios))
    }

    /// Where the dragged sides go to meet `target` (see [`Self::follow`]). A face goes through the
    /// target's point, keeping its slope: the mouse picks where on the target. A vertex or edge
    /// onto a face keeps its sizes in proportion and stops on the face's plane (if it can reach
    /// it); else each dragged axis goes to the target's point.
    pub fn snap(&self, target: &Target) -> Vec3 {
        let point = self.frame.local(target.point);
        let anchor = self.anchor();
        let handle = self.handle(point);
        if let Shape::Plane { point: on, normal } = self.grip.shape {
            let offset = geometry::dot(normal, geometry::sub(on, anchor));
            if offset.abs() >= MIN_EXTENT {
                let reach = geometry::dot(normal, geometry::sub(point, anchor));
                return self.sides(self.uniform(reach / offset));
            }
        } else if let Some(normal) = target.normal
            && let Some(factor) = self.proportional_to_plane(handle, point, normal)
        {
            return self.sides(self.uniform(factor));
        }
        self.sides(self.ratios(|axis| point[axis], handle))
    }

    /// The point of the grabbed shape that goes to `target` (in the vehicle, see [`Self::snap`]),
    /// once the brick has `size`, in its frame as grabbed: where the grab is on a face (it moves in
    /// the face's plane, through the target), else the shape's point nearest the target. Apart
    /// from the target, the line between them shows what lines up.
    pub fn pulled(&self, size: Vec3, target: Vec3) -> Vec3 {
        let handle = match self.grip.shape {
            Shape::Plane { .. } => self.grab,
            _ => self.handle(self.frame.local(target)),
        };
        self.moved(size, handle)
    }

    /// How much the dragged sides' distance to the anchor grows (as a factor) for `handle` (in the
    /// brick's frame) to reach the plane through `on_plane` (in the brick's frame) across `normal`
    /// (in the vehicle).
    fn proportional_to_plane(&self, handle: Vec3, on_plane: Vec3, normal: Vec3) -> Option<f64> {
        let normal = self.frame.local_direction(normal);
        let anchor = self.anchor();
        let offset = self.restricted(geometry::sub(handle, anchor));
        // The handle's point once scaled to nothing around the anchor.
        let start = geometry::sub(handle, offset);
        let across = geometry::dot(normal, offset);
        if across.abs() < 1e-4 {
            return None;
        }
        let factor = geometry::dot(normal, geometry::sub(on_plane, start)) / across;
        (factor > 0.0).then_some(factor)
    }

    /// The box with the dragged sides at `sides` (see [`Self::follow`]), each kept from crossing
    /// the opposite one.
    pub fn resized(&self, sides: Vec3) -> Bounds {
        let mut bounds = self.bounds;
        for axis in self.grip.part.axes() {
            if self.grip.part.sides[axis] > 0 {
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
        let shift: Vec3 = core::array::from_fn(|axis| {
            let kept = self.bounds.side(axis, -self.grip.part.sides[axis]);
            kept - kept * self.ratio(size, axis)
        });
        let bounds = Bounds {
            min: self.moved_by(size, shift, self.bounds.min),
            max: self.moved_by(size, shift, self.bounds.max),
        };
        (bounds, shift)
    }

    /// Where a point of the brick as grabbed (in its frame) is once it has `size`.
    pub fn moved(&self, size: Vec3, point: Vec3) -> Vec3 {
        let (_, shift) = self.placed(size);
        self.moved_by(size, shift, point)
    }

    /// `point` scaled for `size`, then shifted by `shift` (see [`Self::placed`]).
    fn moved_by(&self, size: Vec3, shift: Vec3, point: Vec3) -> Vec3 {
        core::array::from_fn(|axis| point[axis] * self.ratio(size, axis) + shift[axis])
    }

    fn ratio(&self, size: Vec3, axis: usize) -> f64 {
        if self.size[axis] != 0.0 {
            size[axis] / self.size[axis]
        } else {
            1.0
        }
    }

    /// The grabbed shape's point nearest `point` (in the brick's frame): what follows the mouse or
    /// goes to a target.
    fn handle(&self, point: Vec3) -> Vec3 {
        match self.grip.shape {
            Shape::Point(vertex) => vertex,
            Shape::Segment(start, end) => closest_on_segment(start, end, point),
            Shape::Plane { point: on, normal } => {
                let off = geometry::dot(normal, geometry::sub(point, on));
                geometry::sub(point, geometry::scale(normal, off))
            }
        }
    }

    /// Per dragged axis, the ratio taking `handle` to `wanted(axis)` around the anchor; 1 along
    /// the others, and where `handle` is at the anchor.
    fn ratios(&self, wanted: impl Fn(usize) -> f64, handle: Vec3) -> Vec3 {
        let anchor = self.anchor();
        let mut ratios = [1.0; 3];
        for axis in self.grip.part.axes() {
            let from = handle[axis] - anchor[axis];
            if from.abs() >= MIN_EXTENT {
                ratios[axis] = (wanted(axis) - anchor[axis]) / from;
            }
        }
        ratios
    }

    /// `factor` along every dragged axis, 1 along the others.
    fn uniform(&self, factor: f64) -> Vec3 {
        let mut ratios = [1.0; 3];
        for axis in self.grip.part.axes() {
            ratios[axis] = factor;
        }
        ratios
    }

    /// The dragged sides, scaled around the anchor by `ratios`.
    fn sides(&self, ratios: Vec3) -> Vec3 {
        let anchor = self.anchor();
        let sides = self.bounds.point(self.grip.part.sides);
        core::array::from_fn(|axis| anchor[axis] + (sides[axis] - anchor[axis]) * ratios[axis])
    }

    /// `vector` along the dragged axes only.
    fn restricted(&self, vector: Vec3) -> Vec3 {
        core::array::from_fn(|axis| {
            if self.grip.part.sides[axis] != 0 {
                vector[axis]
            } else {
                0.0
            }
        })
    }

    /// The opposite sides: what stays in place (the middle along axes not dragged).
    fn anchor(&self) -> Vec3 {
        self.bounds.point(self.grip.part.sides.map(|side| -side))
    }
}

/// The point of the segment from `start` to `end` closest to `point`.
fn closest_on_segment(start: Vec3, end: Vec3, point: Vec3) -> Vec3 {
    let along = geometry::sub(end, start);
    let squared = geometry::dot(along, along);
    if squared <= 0.0 {
        return start;
    }
    let share = (geometry::dot(geometry::sub(point, start), along) / squared).clamp(0.0, 1.0);
    geometry::add(start, geometry::scale(along, share))
}
