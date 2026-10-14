//! Moving bricks by a grabbed point (while the move mode shows its gizmo, see `super`): where the
//! moved bricks go for the mouse, freely or snapped to a point of another brick, turned to match it
//! where what's grabbed and the target line up (a face onto a face, an edge onto an edge).
//!
//! A move is a rigid motion of the bricks as they were grabbed ([`Motion`]): a turn about the
//! grabbed point, then a shift taking that point where it goes. What a snap did holds once the
//! bricks are free again: they keep its turn, and follow the mouse from where it put them.
//!
//! Where the grabbed piece can match the target several ways (a face facing it or level with it, an
//! edge either way along, each squared up four ways), the smallest turn comes first and a key picks
//! the next ([`Grab::matching_turns`]).
//!
//! With mirror mode on, the moved bricks' counterparts move along, mirrored. Snapping onto one can
//! only line things up across the mirror plane: they meet it halfway ([`Grab::meet_mirrored`]).

use super::geometry::{self, Frame, Ray, Vec3};
use super::model::Element;
use super::scene::{Piece, Placed};
use crate::framework::settings::{Setting, Toggle};
use crate::math::Quat;

/// When snapping turns the moved bricks to match the target now: each case a setting
/// ([`SETTINGS`]).
pub fn matching() -> Matching {
    Matching {
        faces: FACES.get(),
        edges: EDGES.get(),
        square: SQUARE.get(),
    }
}

/// The settings of when snapping turns the moved bricks, in the menu's order.
pub static SETTINGS: [Setting; 3] = [
    Setting::Toggle(&FACES),
    Setting::Toggle(&EDGES),
    Setting::Toggle(&SQUARE),
];

static FACES: Toggle = Toggle::new(
    "MatchFaces",
    "Faces onto faces",
    "Moved bricks snapped by a face onto a face turn to lie flat on it: facing it, or facing the \
     same way.",
    true,
);
static EDGES: Toggle = Toggle::new(
    "MatchEdges",
    "Edges onto edges",
    "Moved bricks snapped by an edge onto an edge turn to run along it, either way.",
    true,
);
static SQUARE: Toggle = Toggle::new(
    "MatchSquare",
    "Square up",
    "After either, moved bricks also turn about that face's normal or that edge to line up with \
     the target brick's axes.",
    true,
);

/// The cases snapping turns the moved bricks in (see [`matching`]).
#[derive(Clone, Copy, Debug)]
pub struct Matching {
    /// A face snapped onto a face turns to lie flat on it: facing it, or facing the same way.
    pub faces: bool,
    /// An edge snapped onto an edge turns to run along it, either way.
    pub edges: bool,
    /// After either, the bricks also turn about that face's normal or that edge to line up with
    /// the target brick's axes: squared to it.
    pub square: bool,
}

/// The smallest cosine between the mouse ray and the plane the grabbed point moves in: closer to
/// edge-on, the point stops following (small mouse moves would throw it far).
const MIN_PLANE_COSINE: f64 = 0.08;

/// How far from the axis bricks turn about (the sine of their angle) a brick's axis must be to
/// square it with the target's: an axis almost along it gives no steady direction.
const MIN_SQUARE_SINE: f64 = 0.2;

/// How close two matching turns' quaternions may be (largest difference of components) to count as
/// one: squaring up by different pairs of axes often gives the same turn.
const SAME_TURN: f64 = 1e-6;

/// How a face or an edge of a brick lies, in the vehicle.
#[derive(Clone, Copy, Debug)]
pub enum Orientation {
    /// A face: its outward normal.
    Face(Vec3),
    /// An edge: its direction (either way).
    Edge(Vec3),
}

impl Orientation {
    /// How the brick's `piece` lies, if it's a face or an edge.
    pub fn of(placed: &Placed, piece: Piece) -> Option<Self> {
        let frame = &placed.frame;
        match piece {
            Piece::Box(part) => match part.dimensions_fixed() {
                1 => Some(Self::Face(frame.direction(part.sides.map(f64::from)))),
                2 => {
                    let along = (0..3).find(|&axis| part.sides[axis] == 0)?;
                    Some(Self::Edge(frame.axes[along]))
                }
                _ => None,
            },
            Piece::Model(element) => {
                let model = placed.model.as_ref()?;
                match element {
                    Element::Face(face) => Some(Self::Face(frame.direction(model.normal(face)))),
                    Element::Edge(edge) => {
                        let [start, end] = model.edge(edge);
                        let along = geometry::normalized(geometry::sub(end, start))?;
                        Some(Self::Edge(frame.direction(along)))
                    }
                    Element::Vertex(_) => None,
                }
            }
        }
    }
}

/// The point of the brick's `piece` grabbed with the mouse on `ray`, `under` the mouse on the
/// brick: a corner or vertex itself, an edge's point nearest the ray, or the point under the mouse
/// on a face.
pub fn grabbed_point(placed: &Placed, piece: Piece, ray: &Ray, under: Vec3) -> Vec3 {
    let frame = &placed.frame;
    let ends = match piece {
        Piece::Box(part) => {
            let corners = part.corners(&placed.bounds);
            match corners[..] {
                [corner] => return frame.point(corner),
                [start, end] => [start, end],
                _ => return under,
            }
        }
        Piece::Model(element) => match (element, &placed.model) {
            (Element::Vertex(vertex), Some(model)) => return frame.point(model.point(vertex)),
            (Element::Edge(edge), Some(model)) => model.edge(edge),
            _ => return under,
        },
    };
    geometry::closest_on_segment(ray, frame.point(ends[0]), frame.point(ends[1]))
}

/// What's grabbed of the moved bricks, as they were when grabbed.
#[derive(Clone, Copy, Debug)]
pub struct Grab {
    /// The point that follows the mouse, or goes to the target (in the vehicle).
    pub point: Vec3,
    /// How the grabbed piece lies, if it's a face or an edge.
    pub orientation: Option<Orientation>,
    /// The grabbed brick's axes.
    pub axes: [Vec3; 3],
    /// Unsnapped, the point moves in the plane through it across this axis of the vehicle: the
    /// one the camera looked along the most.
    pub across: Vec3,
}

/// Where a snapped grab goes: the target point (in the vehicle), how the piece of the other brick
/// it's on lies (if it's on one face or edge), and that brick's axes.
pub struct Aim {
    pub point: Vec3,
    pub orientation: Option<Orientation>,
    pub axes: [Vec3; 3],
}

/// A rigid motion: a turn about `pivot`, then a shift taking `pivot` to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub rotation: Quat,
    pub pivot: Vec3,
    pub to: Vec3,
}

impl Motion {
    /// No motion.
    pub const NONE: Self = Self {
        rotation: Quat::IDENTITY,
        pivot: [0.0; 3],
        to: [0.0; 3],
    };

    /// Where it takes `point`.
    pub fn point(&self, point: Vec3) -> Vec3 {
        geometry::add(
            self.to,
            self.rotation.rotate(geometry::sub(point, self.pivot)),
        )
    }

    /// Where it takes a brick's `frame`.
    pub fn frame(&self, frame: &Frame) -> Frame {
        Frame::new(self.point(frame.origin), self.rotation * frame.rotation)
    }

    /// The same motion seen in a mirror across the plane where coordinate `axis` is 0: how it moves
    /// the mirror mode counterparts of the bricks it moves.
    pub fn mirrored(&self, axis: usize) -> Self {
        let flip = |mut point: Vec3| {
            point[axis] = -point[axis];
            point
        };
        // A turn seen in a mirror turns the other way about the mirrored axis: the quaternion keeps
        // its part along the mirror's normal, the other two change sign.
        let [x, y, z] = flip([self.rotation.x, self.rotation.y, self.rotation.z]).map(|part| -part);
        Self {
            rotation: Quat {
                x,
                y,
                z,
                w: self.rotation.w,
            },
            pivot: flip(self.pivot),
            to: flip(self.to),
        }
    }
}

impl Grab {
    /// The plane the point moves in unsnapped, for a camera looking along `direction`: across the
    /// vehicle's axis it's most along.
    pub fn across_for(direction: Vec3) -> Vec3 {
        let axis = (0..3)
            .max_by(|&a, &b| direction[a].abs().total_cmp(&direction[b].abs()))
            .unwrap_or(2);
        let mut across = [0.0; 3];
        across[axis] = 1.0;
        across
    }

    /// Following the mouse `ray` unsnapped, from `kept` (the motion the last snap gave, or none):
    /// the point moves in its plane (see [`Self::across`]) through where `kept` put it, by whole
    /// `grid` steps from there along the vehicle's axes if `grid` > 0 (like the game's free moves:
    /// the nearest step, from half a step below), and the bricks keep `kept`'s turn. `None` while
    /// the ray gives no steady point (edge-on at the plane, or away from it).
    pub fn follow(&self, ray: &Ray, grid: f64, kept: &Motion) -> Option<Motion> {
        let from = kept.point(self.point);
        let distance = geometry::ray_plane(ray, from, self.across, MIN_PLANE_COSINE)?;
        let mut shift = geometry::sub(geometry::along(ray, distance), from);
        if grid > 0.0 {
            shift = shift.map(|offset| ((offset + grid / 2.0) / grid).floor() * grid);
        }
        Some(Motion {
            rotation: kept.rotation,
            pivot: self.point,
            to: geometry::add(from, shift),
        })
    }

    /// Snapped to `aim`: the point goes onto the aim's point, the bricks turned about it to match
    /// the target where `matching` says (see [`Matching`]): by the `choice`th of the matching turns
    /// (0 the smallest, counting round past the last, see [`Self::matching_turns`]). Else they keep
    /// `kept`'s turn (the motion the last snap gave, or none).
    pub fn snap(&self, aim: &Aim, matching: &Matching, kept: &Motion, choice: usize) -> Motion {
        let turns = self.matching_turns(aim, matching);
        let rotation = match turns.len() {
            0 => kept.rotation,
            count => turns[choice % count],
        };
        Motion {
            rotation,
            pivot: self.point,
            to: aim.point,
        }
    }

    /// Snapped to `target` on a mirror mode counterpart of the moved bricks (where it was when
    /// grabbed), with the mirror plane where coordinate `axis` is 0. The counterpart moves too,
    /// mirrored, so no motion brings the two points together in the plane's directions: their
    /// offset there stays. Across it they meet halfway, both the same distance from the plane;
    /// the rest is `free`'s (the motion following the mouse), turn included.
    pub fn meet_mirrored(&self, target: Vec3, axis: usize, free: Motion) -> Motion {
        // The moved bricks' point the target mirrors, and where it goes from the grabbed one.
        let mut source = target;
        source[axis] = -source[axis];
        let offset = free.rotation.rotate(geometry::sub(source, self.point));
        // The grabbed point at `to`, the source at `to + offset`, its mirror image at the same
        // distance from the plane as `to`: `to = -(to + offset)` along the axis.
        let mut to = free.to;
        to[axis] = -offset[axis] / 2.0;
        Motion { to, ..free }
    }

    /// The turns matching what's grabbed to the target, smallest first: a face's normal onto the
    /// target face's or its opposite (facing it, or level with it), an edge onto the target edge
    /// either way, each then squared to the target brick's axes every way it can be. None if they
    /// don't match up (or that case is off).
    pub fn matching_turns(&self, aim: &Aim, matching: &Matching) -> Vec<Quat> {
        let (from, onto) = match (self.orientation, aim.orientation) {
            (Some(Orientation::Face(from)), Some(Orientation::Face(onto))) if matching.faces => {
                (from, onto)
            }
            (Some(Orientation::Edge(from)), Some(Orientation::Edge(onto))) if matching.edges => {
                (from, onto)
            }
            _ => return Vec::new(),
        };
        let mut turns = Vec::new();
        for onto in [onto, geometry::scale(onto, -1.0)] {
            let lined = shortest_turn(from, onto, &self.axes);
            let axes = self.axes.map(|axis| lined.rotate(axis));
            let squarings = if matching.square {
                squaring_angles(onto, &axes, &aim.axes)
            } else {
                Vec::new()
            };
            if squarings.is_empty() {
                turns.push(lined);
            }
            for degrees in squarings {
                turns.push(Quat::from_axis_angle(onto, degrees) * lined);
            }
        }
        // Turns as large (like a quarter turn either way) keep their order: facing the same way
        // first.
        turns.sort_by_key(|&turn| (angle(turn) * 1e4).round() as i64);
        let mut distinct: Vec<Quat> = Vec::with_capacity(turns.len());
        for turn in turns {
            if distinct.iter().all(|seen| seen.distance(turn) > SAME_TURN) {
                distinct.push(turn);
            }
        }
        distinct
    }
}

/// How far a turn turns (degrees, 0 to 180).
fn angle(turn: Quat) -> f64 {
    2.0 * turn.w.abs().min(1.0).acos().to_degrees()
}

/// The smallest turn taking direction `from` onto `onto` (both length 1). Opposite ones take half
/// a turn about the one of `axes` (length 1) most across them.
fn shortest_turn(from: Vec3, onto: Vec3, axes: &[Vec3; 3]) -> Quat {
    let axis = geometry::cross(from, onto);
    let sine = geometry::length(axis);
    let cosine = geometry::dot(from, onto);
    if sine >= 1e-12 {
        let degrees = sine.atan2(cosine).to_degrees();
        return Quat::from_axis_angle(geometry::scale(axis, 1.0 / sine), degrees);
    }
    if cosine > 0.0 {
        return Quat::IDENTITY;
    }
    let along = |axis: &Vec3| geometry::dot(*axis, from).abs();
    let across = axes
        .iter()
        .min_by(|a, b| along(a).total_cmp(&along(b)))
        .copied()
        .unwrap_or([0.0, 0.0, 1.0]);
    // Exactly across `from`, in case that axis isn't.
    let across = geometry::sub(across, geometry::scale(from, geometry::dot(across, from)));
    match geometry::normalized(across) {
        Some(across) => Quat::from_axis_angle(across, 180.0),
        None => Quat::IDENTITY,
    }
}

/// The turns about `around` (length 1, degrees) lining one of `axes` up with one of `target`'s or
/// its opposite, seen across `around`; none if no pair can be. May repeat.
pub fn squaring_angles(around: Vec3, axes: &[Vec3; 3], target: &[Vec3; 3]) -> Vec<f64> {
    // An axis as seen across `around` (length 1), if it isn't almost along it.
    let flat = |axis: Vec3| {
        let flat = geometry::sub(axis, geometry::scale(around, geometry::dot(axis, around)));
        (geometry::length(flat) >= MIN_SQUARE_SINE)
            .then(|| geometry::normalized(flat))
            .flatten()
    };
    let mut angles = Vec::new();
    for from in axes.iter().filter_map(|&axis| flat(axis)) {
        for onto in target.iter().filter_map(|&axis| flat(axis)) {
            let sine = geometry::dot(geometry::cross(from, onto), around);
            let degrees = sine.atan2(geometry::dot(from, onto)).to_degrees();
            // Lining up with the opposite direction does as well: half a turn further.
            let opposite = if degrees > 0.0 {
                degrees - 180.0
            } else {
                degrees + 180.0
            };
            angles.extend([degrees, opposite]);
        }
    }
    angles
}
