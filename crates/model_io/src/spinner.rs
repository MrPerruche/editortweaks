//! A spinner brick's shape and how its values change it, as the game builds it
//! (`docs/re/spinners.md`): a cross-section swept along an elliptic arc about the brick's own Z
//! axis, around its origin (the hub), from its +Y axis toward -X for a positive angle.
//!
//! A point of the spinner is given by its angle along the arc `t` (radians, 0 at the start), how
//! far it is across the ring from the arc's centre line (`across`, cm, outward positive) and its
//! height (`up`, cm). The cross-section's own coordinates `(a, b)` run from -1 to 1 across its
//! bounding square: `across = a * thickness / 2`, `up = b * height / 2`.

use core::f64::consts::{FRAC_PI_2, PI, TAU};

pub type Vec3 = [f64; 3];

/// The game's largest thickness and height for the mesh (cm): its mesh clamps past them.
pub const MAX_SIZE: f64 = 9000.0;
/// The editor panel's largest angle either way (degrees).
pub const MAX_ANGLE: f64 = 360.0;
/// The smallest angle either way the game keeps when a change is final (degrees).
pub const MIN_ANGLE: f64 = 1.0;
/// The panel's smallest radius and size (cm).
pub const MIN_LENGTH: f64 = 1.0;

/// A spinner's values (`USpinnerBrick`'s properties).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spinner {
    /// `SpinnerRadius`: the ellipse's semi-axes along X and Y, to the cross-section's middle (cm).
    pub radius: [f64; 2],
    /// `SpinnerSize`: the thickness across the ring and the height along Z (cm).
    pub size: [f64; 2],
    /// `SpinnerAngle`: how far the arc goes (degrees, signed).
    pub angle: f64,
}

/// Where a point is in a spinner (see the module's description).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Along the arc (radians), in (-pi, pi].
    pub angle: f64,
    /// Across the ring from the centre line (cm).
    pub across: f64,
    /// Along Z (cm).
    pub up: f64,
}

/// A spinner's new values, and how the brick moves to keep what stays in place: by `shift` along
/// its own axes, then turned by `turn` degrees about its own Z axis (positive: its +Y toward -X,
/// like the arc).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edit {
    pub spinner: Spinner,
    pub shift: Vec3,
    pub turn: f64,
}

/// What stays in place when a spinner's radius changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The start cap (the brick moves along its Y axis).
    Start,
    /// The hub (the brick stays).
    Hub,
}

impl Spinner {
    /// The thickness the mesh shows: at most twice the smaller radius (the inner surface at the
    /// hub) and [`MAX_SIZE`].
    pub fn thickness(&self) -> f64 {
        self.size[0]
            .min(2.0 * self.radius[0].min(self.radius[1]))
            .min(MAX_SIZE)
    }

    /// The height the mesh shows.
    pub fn height(&self) -> f64 {
        self.size[1].min(MAX_SIZE)
    }

    /// The angle in radians.
    pub fn radians(&self) -> f64 {
        self.angle.to_radians()
    }

    /// Whether the arc goes all the way round (no caps).
    pub fn is_full_turn(&self) -> bool {
        self.angle.abs() >= MAX_ANGLE - 1e-6
    }

    /// Whether its arc is a circle: only then can its start turn about the hub without the rest
    /// moving.
    pub fn is_round(&self) -> bool {
        (self.radius[0] - self.radius[1]).abs() <= 1e-6 * self.radius[0].abs().max(1.0)
    }

    /// The point at angle `t` (radians), `across` the ring and `up` (cm).
    pub fn point(&self, t: f64, across: f64, up: f64) -> Vec3 {
        let (sin, cos) = t.sin_cos();
        [
            -sin * (self.radius[0] + across),
            cos * (self.radius[1] + across),
            up,
        ]
    }

    /// The point of the cross-section `(a, b)` (from -1 to 1 across its bounding square) at angle
    /// `t` (radians).
    pub fn section_point(&self, t: f64, [a, b]: [f64; 2]) -> Vec3 {
        self.point(t, a * self.thickness() / 2.0, b * self.height() / 2.0)
    }

    /// The direction a point at angle `t` moves in as it goes across the ring (length 1).
    pub fn outward(t: f64) -> Vec3 {
        let (sin, cos) = t.sin_cos();
        [-sin, cos, 0.0]
    }

    /// Where `point` is (see [`Place`]), if it isn't at the hub.
    pub fn place(&self, point: Vec3) -> Option<Place> {
        let flat = [point[0], point[1]];
        let across = offset_to_ellipse(flat, self.radius)?;
        let semi = self.radius.map(|radius| radius + across);
        Some(Place {
            angle: ellipse_angle(flat, semi),
            across,
            up: point[2],
        })
    }

    /// `t` (radians) taken round to where it's nearest the arc's middle: a point just past the end
    /// of a 300 degree arc is at 301 degrees, not -59.
    pub fn along_arc(&self, t: f64) -> f64 {
        let middle = self.radians() / 2.0;
        t + ((middle - t) / TAU).round() * TAU
    }

    /// Its box, as the game computes it (`CalcSpinnerMeshBounds`): from the cross-section's
    /// bounding square, whatever its shape.
    pub fn bounds(&self) -> [Vec3; 2] {
        let a = self.radians().abs();
        let half = self.thickness() / 2.0;
        let (outer_x, outer_y) = (self.radius[0] + half, self.radius[1] + half);
        let inner_y = self.radius[1] - half;
        let (sin, cos) = a.sin_cos();
        let min_x = if a < FRAC_PI_2 {
            -sin * outer_x
        } else {
            -outer_x
        };
        let max_x = if a >= 3.0 * FRAC_PI_2 {
            outer_x
        } else {
            (-sin * outer_x).max(0.0)
        };
        let min_y = if a < PI {
            (cos * outer_y).min(cos * inner_y)
        } else {
            -outer_y
        };
        let (min_x, max_x) = if self.angle < 0.0 {
            (-max_x, -min_x)
        } else {
            (min_x, max_x)
        };
        let up = self.height() / 2.0;
        [[min_x, min_y, -up], [max_x, outer_y, up]]
    }

    /// How many steps the game's visible mesh has along the arc (its first level of detail).
    pub fn steps(&self) -> usize {
        let a = self.angle.abs();
        let least = ((a / 45.0 + 0.5002).round() / 2.0).ceil().clamp(1.0, 4.0);
        let steps = if a < 4.0 {
            1.0
        } else if a < 16.0 {
            4.0
        } else {
            8.0 * ((2.0 * a / 45.0).round() / 2.0).ceil()
        };
        steps.min(32.0).max(least) as usize
    }

    /// Its surface: `profile` (the cross-section, from -1 to 1, in order round its outline) swept
    /// along the arc in [`Self::steps`] steps, closed by its caps unless it goes all the way round.
    /// Points and triangles, all facing out.
    pub fn sweep(&self, profile: &[[f64; 2]]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let steps = self.steps();
        let full = self.is_full_turn();
        let sections = if full { steps } else { steps + 1 };
        let per = profile.len();
        let mut points = Vec::with_capacity(sections * per);
        for step in 0..sections {
            let t = self.radians() * step as f64 / steps as f64;
            for &point in profile {
                points.push(self.section_point(t, point).map(|axis| axis as f32));
            }
        }
        let index =
            |section: usize, point: usize| ((section % sections) * per + point % per) as u32;
        let mut triangles = Vec::new();
        for section in 0..steps {
            for point in 0..per {
                let (a, b) = (index(section, point), index(section, point + 1));
                let (c, d) = (index(section + 1, point + 1), index(section + 1, point));
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
            }
        }
        if !full {
            for point in 1..per.saturating_sub(1) {
                triangles.push([index(0, 0), index(0, point + 1), index(0, point)]);
                triangles.push([
                    index(steps, 0),
                    index(steps, point),
                    index(steps, point + 1),
                ]);
            }
        }
        if signed_volume(&points, &triangles) < 0.0 {
            for triangle in &mut triangles {
                triangle.swap(1, 2);
            }
        }
        (points, triangles)
    }

    /// The spinner with thickness and height `size` (cm, the effective ones), keeping the sides
    /// opposite those `sides` drags (per value: -1 the inner side or the bottom, 1 the outer side
    /// or the top, 0 not dragged): both radii change by half as much as the thickness, the brick
    /// moves along its Z axis by half as much as the height.
    pub fn resized(&self, sides: [i8; 2], size: [f64; 2]) -> Edit {
        let mut spinner = *self;
        let mut shift = [0.0; 3];
        let now = [self.thickness(), self.height()];
        if sides[0] != 0 {
            let grown = size[0] - now[0];
            let moved = f64::from(sides[0]) * grown / 2.0;
            spinner.radius = self.radius.map(|radius| radius + moved);
            spinner.size[0] = size[0];
        }
        if sides[1] != 0 {
            shift[2] = f64::from(sides[1]) * (size[1] - now[1]) / 2.0;
            spinner.size[1] = size[1];
        }
        Edit {
            spinner,
            shift,
            turn: 0.0,
        }
    }

    /// The spinner with both radii grown by `grown` (cm), the thickness kept (if it still fits
    /// within them, see [`Self::thickness`]), keeping `keep` in place.
    pub fn with_radius(&self, grown: f64, keep: Keep) -> Edit {
        let mut spinner = *self;
        spinner.radius = self.radius.map(|radius| radius + grown);
        spinner.size[0] = self.thickness().min(spinner.thickness());
        let shift = match keep {
            Keep::Start => [0.0, self.radius[1] - spinner.radius[1], 0.0],
            Keep::Hub => [0.0; 3],
        };
        Edit {
            spinner,
            shift,
            turn: 0.0,
        }
    }

    /// How much both radii must grow (see [`Self::with_radius`]) for the line `across` the ring
    /// (cm, kept) to go through `point` (in the brick as it is), keeping `keep` in place. `None` if
    /// no radius does it (a point by the start, with the start kept).
    pub fn radius_through(&self, point: Vec3, across: f64, keep: Keep) -> Option<f64> {
        let semi = self.radius.map(|radius| radius + across);
        match keep {
            Keep::Hub => offset_to_ellipse([point[0], point[1]], semi),
            Keep::Start => {
                // The brick moves back along Y by as much as it grows: how far outside the line
                // the point is then, less the growth. Falls as the growth grows.
                let outside = |grown: f64| {
                    offset_to_ellipse([point[0], point[1] + grown], semi).map(|s| s - grown)
                };
                let smallest = semi[0].min(semi[1]);
                let reach = point[0].hypot(point[1]) + smallest.abs();
                let (mut low, mut high) = (-smallest + 1e-6 * reach.max(1.0), 4.0 * reach + 1.0);
                if outside(low)? < 0.0 || outside(high)? > 0.0 {
                    return None;
                }
                for _ in 0..200 {
                    let middle = (low + high) / 2.0;
                    if outside(middle)? > 0.0 {
                        low = middle;
                    } else {
                        high = middle;
                    }
                }
                Some((low + high) / 2.0)
            }
        }
    }

    /// The spinner ending at `angle` (degrees), its start in place.
    pub fn with_end(&self, angle: f64) -> Edit {
        Edit {
            spinner: Spinner { angle, ..*self },
            shift: [0.0; 3],
            turn: 0.0,
        }
    }

    /// The spinner starting at `angle` (degrees, along its arc as it is), its end in place: the
    /// brick turns about the hub by `angle`. Only round spinners keep their end then (see
    /// [`Self::is_round`]).
    pub fn with_start(&self, angle: f64) -> Edit {
        Edit {
            spinner: Spinner {
                angle: self.angle - angle,
                ..*self
            },
            shift: [0.0; 3],
            turn: angle,
        }
    }
}

/// How far `point` (in the arc's plane) is outside the ellipse with semi-axes `semi`, measured the
/// way a spinner's thickness grows: the `s` putting it on the ellipse with semi-axes `semi + s`.
/// `None` at the centre, or where no such ellipse goes through it.
pub fn offset_to_ellipse(point: [f64; 2], semi: [f64; 2]) -> Option<f64> {
    let [x, y] = point;
    let reach = x.hypot(y);
    if reach < 1e-9 {
        return None;
    }
    // Where it's inside (> 1) or outside (< 1) the grown ellipse: falls as `s` grows.
    let inside = |s: f64| (x / (semi[0] + s)).powi(2) + (y / (semi[1] + s)).powi(2);
    let smallest = semi[0].min(semi[1]);
    let mut low = -smallest + 1e-9 * smallest.abs().max(1.0);
    let mut high = 2.0 * reach - smallest + 1.0;
    let nearest = inside(low);
    if nearest.is_nan() || nearest < 1.0 || inside(high) > 1.0 {
        return None;
    }
    for _ in 0..200 {
        let middle = (low + high) / 2.0;
        if inside(middle) > 1.0 {
            low = middle;
        } else {
            high = middle;
        }
        if high - low <= 1e-12 * reach.max(1.0) {
            break;
        }
    }
    Some((low + high) / 2.0)
}

/// The angle of `point` along the ellipse with semi-axes `semi` (radians, in (-pi, pi]): the `t`
/// with `point = (-sin t * semi[0], cos t * semi[1])` once on it.
pub fn ellipse_angle(point: [f64; 2], semi: [f64; 2]) -> f64 {
    (-point[0] / semi[0]).atan2(point[1] / semi[1])
}

/// `angle` (degrees) taken round to where it's nearest `previous`: an angle going on past 180
/// degrees keeps going, instead of jumping back to -180.
pub fn unwrapped(previous: f64, angle: f64) -> f64 {
    angle + ((previous - angle) / 360.0).round() * 360.0
}

/// An angle the game keeps (degrees): within [`MAX_ANGLE`] either way, and at least [`MIN_ANGLE`]
/// (an angle under it goes to the side it's on, zero to the positive one).
pub fn kept_angle(angle: f64) -> f64 {
    let angle = angle.clamp(-MAX_ANGLE, MAX_ANGLE);
    if angle.abs() >= MIN_ANGLE {
        angle
    } else if angle < 0.0 {
        -MIN_ANGLE
    } else {
        MIN_ANGLE
    }
}

/// A cross-section from the game's table (see `game/spinner_profiles.rs`), turned by
/// `quarter_turns` (see [`crate::loft::turned`]) and scaled to fill the square from -1 to 1. `None`
/// for fewer than three points or no size.
pub fn profile(points: &[[f32; 2]], quarter_turns: i32) -> Option<Vec<[f64; 2]>> {
    if points.len() < 3 {
        return None;
    }
    let turned = crate::loft::turned(points, quarter_turns);
    let reach = turned
        .iter()
        .flatten()
        .fold(0.0f64, |most, &value| most.max(f64::from(value).abs()));
    if reach < 1e-9 {
        return None;
    }
    Some(
        turned
            .iter()
            .map(|point| point.map(|value| f64::from(value) / reach))
            .collect(),
    )
}

/// A cross-section's outline as fast editing drags it: split into sides at its corners, and where
/// a curved stretch starts facing another way (up or down, outward or inward) at separators, soft
/// edges between the sides (like a box's, but not on the shape). A right triangle's slanted side
/// gets one in its middle too.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// The profile's points (from -1 to 1), with the separators that fall in the middle of a
    /// segment put in.
    pub points: Vec<[f64; 2]>,
    /// Its corners and separators (points of [`Self::points`]), in order.
    pub corners: Vec<usize>,
    pub separators: Vec<usize>,
    /// Its sides, from each corner or separator to the next.
    pub sides: Vec<Side>,
    /// The separators halving a slanted straight side (see [`Self::new`]).
    halved: Vec<usize>,
    /// Whether its points go counterclockwise (`b` up, `a` right).
    counterclockwise: bool,
}

/// A side of an [`Outline`], dragged as a whole: from its point `first`, `segments` segments on
/// (round the outline's end to its start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Side {
    pub first: usize,
    pub segments: usize,
}

/// How far an outline turns at a point (degrees) to be a corner whatever its neighbours do.
const SHARP_CORNER: f64 = 45.0;
/// How close to a diagonal a segment may face (as a difference of its normal's components) to face
/// both ways: the game's points are `f32`.
const DIAGONAL_TOLERANCE: f64 = 1e-4;
/// How close (as a cosine) a side's segments must all face the way the side does to be straight.
const STRAIGHT_COSINE: f64 = 1.0 - 1e-6;
/// How far from an axis a straight side between corners must face (the smaller component of its
/// normal) to be halved.
const SLANTED: f64 = 0.2;

impl Outline {
    /// `profile`'s (closed) outline, with corners where it turns at least `degrees` (see
    /// [`corners`]). A separator goes where a stretch between corners stops facing mostly along one
    /// axis and starts facing along the other, its outward normal crossing a diagonal: at a point
    /// whose segments face either side of it, or in the middle of a segment facing along it. A
    /// straight side stays whole, slanted or not, except a slanted one between sides facing along
    /// different axes (a right triangle's): halved, each half dragging along one axis.
    pub fn new(profile: &[[f64; 2]], degrees: f64) -> Self {
        let count = profile.len();
        let counterclockwise = signed_area(profile) >= 0.0;
        let at_corner = corners(profile, degrees);
        let segment = |index: usize| [profile[index % count], profile[(index + 1) % count]];
        let facing = |index: usize| facing_axis(outward(segment(index), counterclockwise)?);
        // How far the segment from `index` faces from the axis it faces most.
        let slant = |index: usize| {
            outward(segment(index), counterclockwise)
                .map(|normal| normal[0].abs().min(normal[1].abs()))
        };
        let mut outline = Self {
            points: Vec::with_capacity(count),
            corners: Vec::new(),
            separators: Vec::new(),
            sides: Vec::new(),
            halved: Vec::new(),
            counterclockwise,
        };
        for (index, &point) in profile.iter().enumerate() {
            let before = facing(index + count - 1);
            let after = facing(index);
            if at_corner.contains(&index) {
                outline.corners.push(outline.points.len());
            } else if before.is_some() && after.is_some() && before != after {
                outline.separators.push(outline.points.len());
            }
            outline.points.push(point);
            // A segment facing along a diagonal between segments facing either side of it.
            let [start, end] = segment(index);
            let (previous, next) = (facing(index + count - 1), facing(index + 1));
            let ends_free =
                !at_corner.contains(&index) && !at_corner.contains(&((index + 1) % count));
            let diagonal = outward([start, end], counterclockwise).is_some() && after.is_none();
            let curve_turns =
                diagonal && ends_free && previous.is_some() && next.is_some() && previous != next;
            // A slanted side from corner to corner between sides along different axes.
            let ends_corners =
                at_corner.contains(&index) && at_corner.contains(&((index + 1) % count));
            let along_axis =
                |index: usize| slant(index).is_some_and(|slant| slant <= DIAGONAL_TOLERANCE);
            let slanted = slant(index).is_some_and(|slant| slant >= SLANTED);
            let between_axes =
                along_axis(index + count - 1) && along_axis(index + 1) && previous != next;
            let halves = slanted && ends_corners && between_axes;
            if curve_turns || halves {
                if halves {
                    outline.halved.push(outline.points.len());
                }
                outline.separators.push(outline.points.len());
                outline
                    .points
                    .push([(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0]);
            }
        }
        let mut starts: Vec<usize> = outline
            .corners
            .iter()
            .chain(&outline.separators)
            .copied()
            .collect();
        starts.sort_unstable();
        outline.sides = sides_from(outline.points.len(), &starts);
        outline
    }

    /// The way `side` drags (from -1 to 1, length 1, outward): a straight side (its segments all
    /// facing one way) along its normal, slanted or not, but half a halved one along the axis it
    /// lies toward (the upper half up); a curved one along the axis it faces most (it faces mostly
    /// along it all along, see [`Self::new`]). `None` for a side of no length.
    pub fn facing(&self, side: Side) -> Option<[f64; 2]> {
        let count = self.points.len();
        let point = |index: usize| self.points[(side.first + index) % count];
        // Its segments' normals weighted by their lengths add up to the normal of its chord.
        let chord = [point(0), point(side.segments)];
        let normal = outward(chord, self.counterclockwise)?;
        let straight = (0..side.segments).all(|index| {
            outward([point(index), point(index + 1)], self.counterclockwise)
                .is_none_or(|along| along[0] * normal[0] + along[1] * normal[1] >= STRAIGHT_COSINE)
        });
        let end = (side.first + side.segments) % count;
        let middle = [side.first, end]
            .into_iter()
            .find(|end| self.halved.contains(end));
        let axis = match (straight, middle) {
            (true, None) => return Some(normal),
            (true, Some(middle)) => {
                // From the middle of the slanted side to the half's other end: along one axis the
                // way the side faces, along the other against it.
                let far = self.points[if middle == side.first {
                    end
                } else {
                    side.first
                }];
                let middle = self.points[middle];
                (0..2).find(|&axis| normal[axis] * (far[axis] - middle[axis]) > 0.0)?
            }
            (false, _) => usize::from(normal[1].abs() > normal[0].abs()),
        };
        let mut way = [0.0; 2];
        way[axis] = normal[axis].signum();
        Some(way)
    }
}

/// The corners of `profile` (closed): where its outline turns at least [`SHARP_CORNER`], or at
/// least `degrees` and more than twice as much as at a point next to it. A curve turns about as
/// much at each of its points; and the game's profiles fill the square, so a half circle's tip,
/// stretched, turns more than its points elsewhere.
pub fn corners(profile: &[[f64; 2]], degrees: f64) -> Vec<usize> {
    let count = profile.len();
    let turn = |index: usize| {
        let before = profile[(index + count - 1) % count];
        let after = profile[(index + 1) % count];
        turn_degrees(before, profile[index], after)
    };
    let turns: Vec<f64> = (0..count).map(turn).collect();
    (0..count)
        .filter(|&index| {
            let here = turns[index];
            let calmer = turns[(index + count - 1) % count].min(turns[(index + 1) % count]);
            here >= SHARP_CORNER || (here >= degrees && here > 2.0 * calmer)
        })
        .collect()
}

/// How far an outline turns at `here` (degrees).
fn turn_degrees(before: [f64; 2], here: [f64; 2], after: [f64; 2]) -> f64 {
    let into = [here[0] - before[0], here[1] - before[1]];
    let out = [after[0] - here[0], after[1] - here[1]];
    let cross = into[0] * out[1] - into[1] * out[0];
    let dot = into[0] * out[0] + into[1] * out[1];
    cross.atan2(dot).abs().to_degrees()
}

/// Twice the area `profile` (closed) holds: positive when it goes counterclockwise.
fn signed_area(profile: &[[f64; 2]]) -> f64 {
    (0..profile.len())
        .map(|index| {
            let [a, b] = [profile[index], profile[(index + 1) % profile.len()]];
            a[0] * b[1] - a[1] * b[0]
        })
        .sum()
}

/// The outward normal (length 1) of the segment from `start` to `end` of an outline going
/// counterclockwise or not. `None` for a segment of no length.
fn outward([start, end]: [[f64; 2]; 2], counterclockwise: bool) -> Option<[f64; 2]> {
    let along = [end[0] - start[0], end[1] - start[1]];
    let length = along[0].hypot(along[1]);
    if length < 1e-12 {
        return None;
    }
    let normal = [along[1] / length, -along[0] / length];
    Some(if counterclockwise {
        normal
    } else {
        normal.map(|value| -value)
    })
}

/// The axis `normal` lies mostly along and which way along it, `None` along a diagonal.
fn facing_axis(normal: [f64; 2]) -> Option<(usize, bool)> {
    let gap = normal[0].abs() - normal[1].abs();
    if gap.abs() <= DIAGONAL_TOLERANCE {
        return None;
    }
    let axis = usize::from(gap < 0.0);
    Some((axis, normal[axis] > 0.0))
}

/// The sides of an outline of `count` points from its sides' first points `starts` (in order):
/// each up to the next, the whole outline as one with none.
fn sides_from(count: usize, starts: &[usize]) -> Vec<Side> {
    if starts.is_empty() {
        return vec![Side {
            first: 0,
            segments: count,
        }];
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &first)| {
            let next = starts[(index + 1) % starts.len()];
            // All the way round to the next start, the whole outline if it's the only one.
            let segments = (next + count - first - 1) % count + 1;
            Side { first, segments }
        })
        .collect()
}

/// The segment of `profile`'s outline (closed, from the point of that index) nearest `point`.
pub fn nearest_segment(profile: &[[f64; 2]], point: [f64; 2]) -> usize {
    let distance = |[start, end]: [[f64; 2]; 2]| {
        let along = [end[0] - start[0], end[1] - start[1]];
        let squared = along[0] * along[0] + along[1] * along[1];
        let share = if squared > 0.0 {
            (((point[0] - start[0]) * along[0] + (point[1] - start[1]) * along[1]) / squared)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        (start[0] + along[0] * share - point[0]).hypot(start[1] + along[1] * share - point[1])
    };
    let segment = |index: usize| [profile[index], profile[(index + 1) % profile.len()]];
    (0..profile.len())
        .min_by(|&a, &b| distance(segment(a)).total_cmp(&distance(segment(b))))
        .unwrap_or(0)
}

/// Six times the volume a closed surface holds: positive when its triangles face out.
fn signed_volume(points: &[[f32; 3]], triangles: &[[u32; 3]]) -> f64 {
    triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|index| points[index as usize].map(f64::from));
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f64; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

    fn spinner(radius: [f64; 2], size: [f64; 2], angle: f64) -> Spinner {
        Spinner {
            radius,
            size,
            angle,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    /// The box of the points of the cross-section's square swept finely along the arc.
    fn swept_box(spinner: &Spinner) -> [Vec3; 2] {
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for step in 0..=20_000 {
            let t = spinner.radians() * f64::from(step) / 20_000.0;
            for corner in SQUARE {
                let point = spinner.section_point(t, corner);
                for axis in 0..3 {
                    bounds[0][axis] = bounds[0][axis].min(point[axis]);
                    bounds[1][axis] = bounds[1][axis].max(point[axis]);
                }
            }
        }
        bounds
    }

    #[test]
    fn the_box_is_the_swept_box() {
        for &angle in &[
            1.0, 45.0, 90.0, 135.0, 200.0, 280.0, 360.0, -60.0, -190.0, -300.0,
        ] {
            let spinner = spinner([80.0, 50.0], [30.0, 20.0], angle);
            let [min, max] = spinner.bounds();
            let [swept_min, swept_max] = swept_box(&spinner);
            for axis in 0..3 {
                assert!(
                    (min[axis] - swept_min[axis]).abs() < 1e-3,
                    "{angle}: {min:?}"
                );
                assert!(
                    (max[axis] - swept_max[axis]).abs() < 1e-3,
                    "{angle}: {max:?}"
                );
            }
        }
    }

    #[test]
    fn thickness_stops_at_the_hub_and_sizes_at_the_mesh_limit() {
        assert_eq!(
            spinner([30.0, 20.0], [100.0, 20000.0], 90.0).thickness(),
            40.0
        );
        assert_eq!(
            spinner([30.0, 20.0], [100.0, 20000.0], 90.0).height(),
            MAX_SIZE
        );
        assert_eq!(spinner([1e5, 1e5], [2e4, 5.0], 90.0).thickness(), MAX_SIZE);
    }

    #[test]
    fn places_are_found_back() {
        let spinner = spinner([120.0, 70.0], [40.0, 10.0], 270.0);
        for &(t, across, up) in &[
            (0.0, 0.0, 0.0),
            (0.7, 15.0, -3.0),
            (2.5, -19.0, 5.0),
            (-1.2, 20.0, 1.0),
        ] {
            let place = spinner.place(spinner.point(t, across, up)).unwrap();
            assert!(close(place.angle, t), "{place:?}");
            assert!(close(place.across, across), "{place:?}");
            assert!(close(place.up, up), "{place:?}");
        }
        assert_eq!(spinner.place([0.0, 0.0, 4.0]), None);
    }

    #[test]
    fn angles_go_round_to_the_arc() {
        let long = spinner([50.0, 50.0], [10.0, 10.0], 300.0);
        assert!(close(
            long.along_arc((-59f64).to_radians()),
            301f64.to_radians()
        ));
        // In the gap, nearer the start than the end: before the start.
        assert!(close(
            long.along_arc((-20f64).to_radians()),
            (-20f64).to_radians()
        ));
        assert!(close(
            long.along_arc((-70f64).to_radians()),
            290f64.to_radians()
        ));
        let negative = spinner([50.0, 50.0], [10.0, 10.0], -300.0);
        assert!(close(
            negative.along_arc(59f64.to_radians()),
            (-301f64).to_radians()
        ));
    }

    #[test]
    fn angles_unwrap_and_skip_the_smallest() {
        assert_eq!(unwrapped(170.0, -170.0), 190.0);
        assert_eq!(unwrapped(-350.0, 5.0), -355.0);
        assert_eq!(unwrapped(10.0, 20.0), 20.0);
        assert_eq!(kept_angle(0.4), 1.0);
        assert_eq!(kept_angle(-0.4), -1.0);
        assert_eq!(kept_angle(0.0), 1.0);
        assert_eq!(kept_angle(400.0), 360.0);
        assert_eq!(kept_angle(-12.5), -12.5);
    }

    #[test]
    fn steps_follow_the_game_mesh() {
        let steps = |angle| spinner([50.0, 50.0], [10.0, 10.0], angle).steps();
        assert_eq!(steps(2.0), 1);
        assert_eq!(steps(10.0), 4);
        assert_eq!(steps(45.0), 8);
        assert_eq!(steps(90.0), 16);
        assert_eq!(steps(-90.0), 16);
        assert_eq!(steps(360.0), 32);
    }

    /// Whether every edge is used once each way: a closed surface, all facing the same way.
    fn is_closed(triangles: &[[u32; 3]]) -> bool {
        let mut edges = std::collections::HashMap::new();
        for &[a, b, c] in triangles {
            for edge in [(a, b), (b, c), (c, a)] {
                *edges.entry(edge).or_insert(0) += 1;
            }
        }
        edges
            .iter()
            .all(|(&(from, to), &count)| count == 1 && edges.get(&(to, from)) == Some(&1))
    }

    #[test]
    fn the_sweep_is_closed_and_faces_out() {
        for &angle in &[90.0, -90.0, 360.0] {
            for profile in [SQUARE.to_vec(), SQUARE.iter().rev().copied().collect()] {
                let spinner = spinner([60.0, 60.0], [20.0, 10.0], angle);
                let (points, triangles) = spinner.sweep(&profile);
                assert!(is_closed(&triangles), "{angle}");
                assert!(signed_volume(&points, &triangles) > 0.0, "{angle}");
            }
        }
        let (points, _) = spinner([60.0, 60.0], [20.0, 10.0], 360.0).sweep(&SQUARE);
        // A full turn: the last section is the first one.
        assert_eq!(points.len(), 32 * 4);
    }

    #[test]
    fn offsets_put_points_on_grown_ellipses() {
        let semi = [80.0, 30.0];
        for &(t, s) in &[(0.3, 5.0), (2.0, -20.0), (-1.0, 100.0)] {
            let point = [
                -(t as f64).sin() * (semi[0] + s),
                (t as f64).cos() * (semi[1] + s),
            ];
            assert!(close(offset_to_ellipse(point, semi).unwrap(), s));
            assert!(close(ellipse_angle(point, semi.map(|e| e + s)), t));
        }
        assert_eq!(offset_to_ellipse([0.0, 0.0], semi), None);
    }

    #[test]
    fn profiles_fill_the_square_and_turn() {
        let profile = profile(&[[0.0, -0.5], [0.5, 0.5], [-0.5, 0.5]], 1).unwrap();
        assert_eq!(profile, vec![[1.0, 0.0], [-1.0, 1.0], [-1.0, -1.0]]);
    }

    #[test]
    fn resizing_keeps_the_opposite_side() {
        let start = spinner([50.0, 40.0], [10.0, 20.0], 90.0);
        let outer = start.resized([1, 0], [16.0, 20.0]);
        assert_eq!(outer.spinner.radius, [53.0, 43.0]);
        let inner_before = start.radius[1] - start.thickness() / 2.0;
        let inner_after = outer.spinner.radius[1] - outer.spinner.thickness() / 2.0;
        assert!(close(inner_before, inner_after));
        let inner = start.resized([-1, 0], [16.0, 20.0]);
        assert_eq!(inner.spinner.radius, [47.0, 37.0]);
        let top = start.resized([0, 1], [10.0, 30.0]);
        assert_eq!(top.shift, [0.0, 0.0, 5.0]);
        assert_eq!(top.spinner.size, [10.0, 30.0]);
        let bottom = start.resized([0, -1], [10.0, 30.0]);
        assert_eq!(bottom.shift, [0.0, 0.0, -5.0]);
    }

    #[test]
    fn a_radius_keeps_the_start_or_the_hub() {
        let start = spinner([50.0, 40.0], [10.0, 20.0], 90.0);
        let kept = start.with_radius(10.0, Keep::Start);
        assert_eq!(kept.spinner.radius, [60.0, 50.0]);
        // The start's centre, (0, Ry, 0), stays: the brick moves back along Y.
        assert_eq!(kept.shift, [0.0, -10.0, 0.0]);
        assert_eq!(start.with_radius(10.0, Keep::Hub).shift, [0.0; 3]);
        // Down to the thickness: it shrinks along.
        let small = start.with_radius(-37.0, Keep::Hub);
        assert_eq!(small.spinner.size[0], 6.0);
    }

    #[test]
    fn a_radius_line_goes_through_a_point() {
        let start = spinner([50.0, 40.0], [10.0, 20.0], 180.0);
        let point = [-70.0, -10.0, 3.0];
        for keep in [Keep::Hub, Keep::Start] {
            let grown = start.radius_through(point, 2.0, keep).unwrap();
            let edit = start.with_radius(grown, keep);
            // In the brick once moved, the point is on its line 2 cm out.
            let moved = [point[0] - edit.shift[0], point[1] - edit.shift[1], point[2]];
            let place = edit.spinner.place(moved).unwrap();
            assert!(close(place.across, 2.0), "{keep:?}: {place:?}");
        }
        // Right by the start, with the start kept: no radius gets there.
        assert_eq!(
            start.radius_through([0.0, 60.0, 0.0], 0.0, Keep::Start),
            None
        );
    }

    #[test]
    fn a_start_turns_the_brick_and_keeps_the_end() {
        let start = spinner([50.0, 50.0], [10.0, 20.0], 90.0);
        let edit = start.with_start(30.0);
        assert_eq!(edit.spinner.angle, 60.0);
        assert_eq!(edit.turn, 30.0);
        // The end, turned with the brick, is where it was.
        let end = edit.spinner.point(edit.spinner.radians(), 0.0, 0.0);
        let (sin, cos) = 30f64.to_radians().sin_cos();
        let turned = [end[0] * cos - end[1] * sin, end[0] * sin + end[1] * cos];
        let before = start.point(start.radians(), 0.0, 0.0);
        assert!(close(turned[0], before[0]) && close(turned[1], before[1]));
    }

    /// A circle's outline of `count` points, counterclockwise from `start` (radians).
    fn circle(count: usize, start: f64) -> Vec<[f64; 2]> {
        (0..count)
            .map(|index| {
                let t = start + core::f64::consts::TAU * index as f64 / count as f64;
                [t.cos(), t.sin()]
            })
            .collect()
    }

    /// The ways `outline`'s sides drag, in order, rounded to a hundredth.
    fn facings(outline: &Outline) -> Vec<[f64; 2]> {
        outline
            .sides
            .iter()
            .map(|&side| {
                let way = outline.facing(side).unwrap();
                way.map(|value| (value * 100.0).round() / 100.0 + 0.0)
            })
            .collect()
    }

    #[test]
    fn a_square_has_four_corners_and_sides() {
        let outline = Outline::new(&SQUARE, 20.0);
        assert_eq!(outline.corners, [0, 1, 2, 3]);
        assert!(outline.separators.is_empty());
        assert!(outline.sides.iter().all(|side| side.segments == 1));
        assert_eq!(
            facings(&outline),
            [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]]
        );
        // The outer side (u = 1) is the segment from (1, -1) to (1, 1).
        assert_eq!(nearest_segment(&outline.points, [1.2, 0.3]), 1);
        // Clockwise, still outward: from (-1, 1) along the top first.
        let reversed: Vec<[f64; 2]> = SQUARE.iter().rev().copied().collect();
        let outline = Outline::new(&reversed, 20.0);
        assert_eq!(
            facings(&outline),
            [[0.0, 1.0], [1.0, 0.0], [0.0, -1.0], [-1.0, 0.0]]
        );
    }

    #[test]
    fn a_right_triangle_halves_its_slanted_side() {
        let triangle = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]];
        let outline = Outline::new(&triangle, 20.0);
        assert_eq!(outline.corners, [0, 1, 2]);
        assert_eq!(outline.separators, [3]);
        assert_eq!(outline.points[3], [0.0, 0.0]);
        // Bottom, out, then the slanted side's upper half up and its lower half in.
        assert_eq!(
            facings(&outline),
            [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]]
        );
        // Clockwise, the same halves.
        let reversed: Vec<[f64; 2]> = triangle.iter().rev().copied().collect();
        let outline = Outline::new(&reversed, 20.0);
        assert_eq!(outline.separators, [3]);
        assert_eq!(
            facings(&outline),
            [[1.0, 0.0], [0.0, -1.0], [-1.0, 0.0], [0.0, 1.0]]
        );
    }

    #[test]
    fn other_slanted_sides_stay_whole() {
        let triangle = [[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]];
        let outline = Outline::new(&triangle, 20.0);
        assert!(outline.separators.is_empty());
        assert_eq!(facings(&outline)[1], [0.89, 0.45]);
        let diamond = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
        let outline = Outline::new(&diamond, 20.0);
        assert_eq!(outline.corners.len(), 4);
        assert!(outline.separators.is_empty());
    }

    #[test]
    fn a_circle_has_four_sides_facing_the_axes() {
        for start in [0.0, 0.1, -0.05] {
            let outline = Outline::new(&circle(32, start), 20.0);
            assert!(outline.corners.is_empty());
            assert_eq!(outline.separators.len(), 4);
            assert_eq!(outline.points.len(), 32);
            let mut ways = facings(&outline);
            ways.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(
                ways,
                [[-1.0, 0.0], [0.0, -1.0], [0.0, 1.0], [1.0, 0.0]],
                "{start}"
            );
            // Each separator is about halfway between two axes.
            for &separator in &outline.separators {
                let [a, b] = outline.points[separator];
                assert!((a.abs() - b.abs()).abs() < 0.2, "{start}: {a}, {b}");
            }
        }
    }

    #[test]
    fn a_separator_goes_in_the_middle_of_a_diagonal_segment() {
        // An octagon's segments face the axes and the diagonals; turning 45 degrees, its points
        // are corners, so it's drawn finer: 16 points, from 11.25 degrees.
        let outline = Outline::new(&circle(16, core::f64::consts::PI / 16.0), 20.0);
        assert!(outline.corners.is_empty());
        assert_eq!(outline.separators.len(), 4);
        assert_eq!(outline.points.len(), 20);
        for &separator in &outline.separators {
            let [a, b] = outline.points[separator];
            assert!((a.abs() - b.abs()).abs() < 1e-9, "{a}, {b}");
        }
    }

    #[test]
    fn a_half_circle_faces_down_out_up_and_in() {
        // The diameter on u = -1, the half circle bulging toward +u.
        let mut profile: Vec<[f64; 2]> = (0..=16)
            .map(|index| {
                let t =
                    -core::f64::consts::FRAC_PI_2 + core::f64::consts::PI * f64::from(index) / 16.0;
                [-1.0 + 2.0 * t.cos(), t.sin()]
            })
            .collect();
        profile.dedup();
        let outline = Outline::new(&profile, 20.0);
        assert_eq!(outline.corners, [0, 16]);
        assert_eq!(outline.separators, [6, 10]);
        assert_eq!(
            facings(&outline),
            [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]]
        );
    }
}
