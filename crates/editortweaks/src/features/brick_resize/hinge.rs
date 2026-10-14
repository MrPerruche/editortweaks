//! Moving bricks about a line (see [`super::line_tool`] for placing it): while a line is placed,
//! a drag of a moved brick's part turns the moved bricks about the line, the grabbed point going
//! round it as near the mouse as it can ([`Hinge::angle_for`]), or slides them along it
//! ([`Hinge::slide_for`]), by the game's angle and grid steps while its snapping is on. Snapped,
//! they turn to where the grabbed piece matches the target best ([`Hinge::matching_angles`]), or
//! slide to where the grabbed point is level with it ([`Hinge::slide_to`]).
//!
//! Turning about a line and sliding along it leave the line where it is, and one doesn't change the
//! other: the bricks' motion is both ([`Hinge::motion`]), each as it was last set.

use core::f64::consts::PI;

use super::geometry::{self, Ray, Vec3};
use super::motion::{self, Aim, Grab, Matching, Motion, Orientation};
use crate::math::Quat;

/// How many angles round the line are tried for the one whose point shows nearest the mouse, before
/// refining it.
const SAMPLES: usize = 720;

/// How much further from the mouse than the nearest (pixels) another place of the grabbed point
/// round the line may show and still win, if it's nearer where it was: seen edge-on, the circle it
/// goes round shows its near and far side over each other.
const TIE_PIXELS: f64 = 3.0;

/// How near the line (cm) the grabbed point may be and still turn about it: on the line, the bricks
/// slide along it instead.
const MIN_RADIUS: f64 = 1e-6;

/// How far from the line a direction must be (the sine of their angle) to turn onto another: one
/// along the line stays along it however the bricks turn.
const MIN_FLAT: f64 = 1e-6;

/// Two matching angles closer than this (degrees) are one.
const SAME_ANGLE: f64 = 1e-9;

/// A line through the vehicle: a point on it and its direction (length 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub point: Vec3,
    pub direction: Vec3,
}

impl Line {
    /// The line through `start` and `end`, if they're apart.
    pub fn through(start: Vec3, end: Vec3) -> Option<Self> {
        Some(Self {
            point: start,
            direction: geometry::normalized(geometry::sub(end, start))?,
        })
    }

    /// `vector` without its part along the line.
    fn flat(&self, vector: Vec3) -> Vec3 {
        let along = geometry::dot(vector, self.direction);
        geometry::sub(vector, geometry::scale(self.direction, along))
    }

    /// The signed angle (degrees) turning direction `from` onto `onto` about the line, both seen
    /// across it; `None` if either runs along it.
    fn angle_between(&self, from: Vec3, onto: Vec3) -> Option<f64> {
        let from = self.flat(from);
        let onto = self.flat(onto);
        if geometry::length(from) < MIN_FLAT || geometry::length(onto) < MIN_FLAT {
            return None;
        }
        let sine = geometry::dot(geometry::cross(from, onto), self.direction);
        Some(sine.atan2(geometry::dot(from, onto)).to_degrees())
    }
}

/// A drag of the moved bricks about a line: how far they're turned about it and slid along it.
#[derive(Clone, Copy, Debug)]
pub struct Hinge {
    pub line: Line,
    /// The grabbed point, as grabbed.
    point: Vec3,
    /// The turn (degrees, right-handed about the line's direction; counted on past a full turn
    /// while following the mouse round) and the slide (cm along the line's direction).
    pub turned: f64,
    pub slid: f64,
}

impl Hinge {
    pub fn new(line: Line, point: Vec3) -> Self {
        Self {
            line,
            point,
            turned: 0.0,
            slid: 0.0,
        }
    }

    /// The bricks' motion: turned and slid.
    pub fn motion(&self) -> Motion {
        let line = &self.line;
        Motion {
            rotation: Quat::from_axis_angle(line.direction, self.turned),
            pivot: line.point,
            to: geometry::add(line.point, geometry::scale(line.direction, self.slid)),
        }
    }

    /// How far the grabbed point is from the line.
    fn radius(&self) -> f64 {
        geometry::length(self.line.flat(geometry::sub(self.point, self.line.point)))
    }

    /// Whether the grabbed point is on the line: turning doesn't move it, the bricks slide instead.
    pub fn on_line(&self) -> bool {
        self.radius() < MIN_RADIUS
    }

    /// Where the grabbed point goes turned by `degrees` (and slid as now).
    pub fn around(&self, degrees: f64) -> Vec3 {
        Self {
            turned: degrees,
            ..*self
        }
        .motion()
        .point(self.point)
    }

    /// The turn (degrees) taking the grabbed point round the line to where it shows nearest the
    /// mouse `ray`, `pixel` the angle a pixel spans as seen from the camera (radians). Of the
    /// places showing about as near (see [`TIE_PIXELS`]), the one nearest the turn now, counted on
    /// from it (past a full turn). `None` with the point on the line.
    pub fn angle_for(&self, ray: &Ray, pixel: f64) -> Option<f64> {
        if self.on_line() {
            return None;
        }
        // How far from the ray (as an angle seen from the camera) the point shows, turned by
        // `degrees`; half a turn when it's behind the camera.
        let off = |degrees: f64| {
            let to = geometry::sub(self.around(degrees), ray.origin);
            let depth = geometry::dot(to, ray.direction);
            if depth <= 0.0 {
                return PI;
            }
            geometry::length(geometry::cross(to, ray.direction)).atan2(depth)
        };
        let step = 360.0 / SAMPLES as f64;
        let offs: Vec<f64> = (0..SAMPLES).map(|index| off(index as f64 * step)).collect();
        let nearest = offs.iter().copied().fold(f64::INFINITY, f64::min);
        // Each place nearer the ray than its neighbors, and about as near as the nearest.
        let previous = |index: usize| offs[(index + SAMPLES - 1) % SAMPLES];
        let next = |index: usize| offs[(index + 1) % SAMPLES];
        let chosen = (0..SAMPLES)
            .filter(|&index| {
                let here = offs[index];
                here <= previous(index)
                    && here <= next(index)
                    && here <= nearest + TIE_PIXELS * pixel
            })
            .map(|index| index as f64 * step)
            .min_by(|&a, &b| {
                let apart = |degrees: f64| wrapped(degrees - self.turned).abs();
                apart(a).total_cmp(&apart(b))
            })?;
        let refined = refine(off, chosen - step, chosen + step);
        Some(self.turned + wrapped(refined - self.turned))
    }

    /// The slide (cm) taking the grabbed point along the line to where it shows nearest the mouse
    /// `ray`; `None` while the line runs along the ray.
    pub fn slide_for(&self, ray: &Ray) -> Option<f64> {
        let unslid = Self { slid: 0.0, ..*self };
        let from = unslid.around(self.turned);
        geometry::closest_on_line(ray, from, self.line.direction)
    }

    /// The slide (cm) taking the grabbed point level with `target` along the line.
    pub fn slide_to(&self, target: Vec3) -> f64 {
        geometry::dot(geometry::sub(target, self.point), self.line.direction)
    }

    /// The turns (degrees) matching what's grabbed to the target as well as turning about the line
    /// can, nearest the turn `near` first (counted on from it; the mouse's, so that the order stays
    /// while the bricks take one or another): a face's normal onto the target face's
    /// or its opposite (facing it, or level with it), an edge onto the target edge either way, as
    /// seen across the line (where `matching` has that case on). When that can't change how they
    /// line up (one runs along the line), squared to the target brick's axes instead (if `matching`
    /// squares). Else (and for other pieces) the grabbed point turned round to the target's side of
    /// the line. None if nothing can be matched.
    pub fn matching_angles(
        &self,
        grab: &Grab,
        aim: &Aim,
        matching: &Matching,
        near: f64,
    ) -> Vec<f64> {
        let lined = |from: Vec3, onto: Vec3| -> Vec<f64> {
            [onto, geometry::scale(onto, -1.0)]
                .into_iter()
                .filter_map(|onto| self.line.angle_between(from, onto))
                .collect()
        };
        let mut angles = match (grab.orientation, aim.orientation) {
            (Some(Orientation::Face(from)), Some(Orientation::Face(onto))) if matching.faces => {
                Some(lined(from, onto))
            }
            (Some(Orientation::Edge(from)), Some(Orientation::Edge(onto))) if matching.edges => {
                Some(lined(from, onto))
            }
            _ => None,
        };
        if let Some(found) = &mut angles
            && found.is_empty()
            && matching.square
        {
            *found = motion::squaring_angles(self.line.direction, &grab.axes, &aim.axes);
        }
        let mut angles = angles.unwrap_or_default();
        if angles.is_empty() {
            let from = geometry::sub(self.point, self.line.point);
            let onto = geometry::sub(aim.point, self.line.point);
            angles.extend(self.line.angle_between(from, onto));
        }
        let mut angles: Vec<f64> = angles
            .into_iter()
            .map(|degrees| near + wrapped(degrees - near))
            .collect();
        angles.sort_by(|a, b| (a - near).abs().total_cmp(&(b - near).abs()));
        angles.dedup_by(|a, b| (*a - *b).abs() < SAME_ANGLE);
        angles
    }
}

/// `value` by whole `step`s (the nearest, from half a step below, like the game's moves); as it is
/// if `step` is 0 (the game's snapping off).
pub fn stepped(value: f64, step: f64) -> f64 {
    if step > 0.0 {
        ((value + step / 2.0) / step).floor() * step
    } else {
        value
    }
}

/// `degrees` as an angle from -180 to 180.
fn wrapped(degrees: f64) -> f64 {
    degrees - 360.0 * (degrees / 360.0).round()
}

/// Where between `low` and `high` `off` is smallest, by narrowing down (it has one low there).
fn refine(off: impl Fn(f64) -> f64, mut low: f64, mut high: f64) -> f64 {
    for _ in 0..40 {
        let third = (high - low) / 3.0;
        if off(low + third) <= off(high - third) {
            high -= third;
        } else {
            low += third;
        }
    }
    (low + high) / 2.0
}
